# tagteam M5a — Settings, Logging, Displaced and Completions Implementation Plan

**Status:** Approved

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The admin commands that need nothing from M3b or M4b:
- **`HOME` must be usable.** tagteam refuses to run on an unset, empty or relative `HOME` instead of putting state under `/` or the working directory (§5).
- **One settings registry.** Every §6.4 key is declared once. Reads, `config list`, `config get`, `config set`, `config unset` and completions all use it, and the reader learns the keys M2b skipped.
- **`tagteam config list|get|set|unset|path`.** It edits `config.toml` with `toml_edit`, keeps comments, writes through a symlink and holds the settings lock (§6.4).
- **Logging to a file.** A private, rotating `tagteam.log` that several processes share. It is opened only when an event reaches it, and it never holds an email, token or passphrase (§14.2).
- **`tagteam displaced [--purge ID...]`.** It lists displaced credentials from rows and files, and deletes them under the displaced lock (§6.3).
- **`tagteam completions bash|zsh|fish`** (§13.7).

M5b (doctor, purge, export and import, `cargo xtask compat`) builds on these.

**Architecture:**
- **`tagteam-provider`:** `Env::from_process` validates `HOME` (Task 1). The atomic writer logs a temp file it could not remove (Task 7), and the `security` driver tells a timeout from a spawn failure (Task 7).
- **`tagteam-engine`:**
  - `settings` holds the key registry, the forgiving reader, the per-key inspection and `resolve`, which maps a key's two spellings to one entry (Task 2).
  - A new `config` module does the strict writes, as `Engine` methods (Task 4).
  - The store logs every event row at INFO, and so do `Vault::store`, the rescue writer, `displace` and the recovery decision. A switch's rollback and every refresh outcome are logged too, and `statusline` stays at DEBUG (Task 7).
  - `displace` writes under the displaced lock, new `Engine` methods list and purge entries, and a displaced row names only the live login's own secret (Task 8).
- **`tagteam` (CLI):**
  - The `config`, `displaced` and `completions` commands, rendered in new modules (Tasks 3, 4, 9, 10).
  - Without `--provider`, every command acts on `default_provider`, read once per command through `app::command_settings` (Task 3).
  - Logging moves out of `app::run` to the process boundary. There, `logging` builds the stderr layer and the file layer over `logfile::LogFile` (Tasks 5, 6).

**Tech Stack:** Rust (edition 2024), `toml_edit` 0.22 (already a dependency), `tracing` and `tracing-subscriber` (`env-filter`, `fmt`, `registry`), `clap` 4 (now with its `string` feature) and the new `clap_complete` 4, rusqlite (bundled), serde_json, libc. Tests use tempfile, assert_cmd, `FakeKeychain`, `FileKeychain`, `MockServer` and the existing `tests/common` fixtures.

**Spec:** `docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md`, signed off at `d14a860`: the M5 amendments (`86882e3`) and `d14a860`, which lets the displaced lock nest under CC's storage-write lock and a log line try the rotation lock under any lock (§4.3, §9.1). Read these before starting any task:
- §4.3 (the lock order and its outsiders), §4.4 (logging), §5, §6.1 (`displaced`, `events`), §6.3, §6.4;
- §9.1 (the storage-write lock), §9.4 steps 2 and 7, §9.5, §9.6, §13.1, §13.2, §13.7, §14, §14.1, §14.2, §15.1, §15.2, §15.3;
- Appendix B 35, 36, 68 and 69.

Section numbers below refer to that spec.

## Execution notes

- **Execution waits for M4a to merge into `main`.** M3a (PR #4) and M3b (PR #5) have merged (`main` at `3e5ba2b`), and `m5-admin` is rebased onto it. M5a's `cli.rs`, `app.rs`, `lib.rs`, `settings.rs` and `env.rs` changes would otherwise churn against M4a, which is in flight. The first execution step rebases `m5-admin` onto the `main` M4a has merged into. The M5 spec commit `86882e3` is also on `m4-run-sessions` as `089b873`, so whichever lands second drops as already applied; `d14a860` stays. It then re-syncs this plan with the merged code: the names, line numbers and interfaces under "M3a, M3b and M4a interfaces this plan builds on". The re-sync is recorded under "Execution rulings" below before Task 1 starts.
- When execution starts, set this plan's `**Status:**` to `In progress` in one commit. The spec stays `In progress` until M5b.
- **Run this plan in the `m5-admin` worktree** (`~/Code/tagteam-m5-admin`, Michael's `wt` worktree).
- Feature flags used by tests (unchanged): `tagteam-provider/file-keychain`, `tagteam-provider/mock-server`, `tagteam-engine/test-hooks`, `tagteam-cc/test-hooks`, and `tagteam/test-support`, which enables all of them. Binary tests (`crates/tagteam/tests/*_cli.rs`, `logging.rs`, `completions.rs`) are gated `#![cfg(feature = "test-support")]` like `tests/cli.rs`, and run with `cargo test -p tagteam --features test-support`.
- Clippy must pass both with `--features tagteam/test-support` and with no features.
- Every task runs `cargo fmt --all` before `cargo fmt --all --check`.
- Tests never reach the real network, the real HOME or the login keychain (§15.1). The binary tests run through `common::std_cmd`, which clears the environment, so a test that wants `TAGTEAM_LOG` sets it explicitly.

### M3a, M3b and M4a interfaces this plan builds on

The task text below is written against `main` at `3c1f458`, except where a task names an M3a or M4a interface explicitly: Task 4 throughout, and Task 8's lock call. The M3a and M3b halves of the re-sync can be checked against `main` (`3e5ba2b`) already; M4a's waits for its merge. Each task keeps its own Re-sync note with its delta in full. This list gathers those notes by upstream task, so that the re-sync can be checked off in one pass:
- **M3a Task 1:** `Env.cancel: Cancel` and `FlockGuard::lock(path, timeout, cancel: &Cancel)`.
  - Task 1: `Env::from_process`'s `Ok(Self { … })` keeps `cancel: Cancel::new(),`.
  - Task 4: the settings lock waits on `&self.env.cancel`.
  - Task 8: the displaced lock waits on a fresh `Cancel::new()` that nothing sets, because it is taken inside critical spans (§14.1, Decision 11).
- **M3a Task 2:** `EngineError::Interrupted(i32)`, `EngineError::signal()`, `Engine::cancel()` and `LockError::Interrupted`.
  - Task 2: `lock_kind` gains `LockError::Interrupted { .. } => "interrupted"`, so a settings-lock wait a signal ends has the `interrupted` kind. `the_settings_lock_has_the_engine_lock_s_kinds` lists it.
  - Task 4: `signal()` gains the `Settings(SettingsError::Lock(e))` arm and its unit test, so such a wait exits 128 + n. `a_signal_ends_the_settings_lock_wait_as_interrupted` pins it end to end.
  - Tasks 2 and 8: the new variants sit beside M3a's and M4a's. `Settings` goes before `Io`, `NoSuchDisplaced` after `NoSuchAccount`, and their kind arms likewise.
  - Task 7: the recovery loop's `warn!` keeps its fields; Task 7's line is inside `recover_one`.
  - Task 7 Part D: Step 14's edit to `refresh_active`'s self-heal call is in its loop, which becomes `run_active_refresh`'s body. An interruption that `refresh_active` returns is logged as `outcome="error"` with its kind.
- **M3a Task 6:**
  - `main_with_args` installs the signal handlers at the process boundary, and `app::run` becomes `run_command` returning `Ended`. The order is the parse, the statusline drain, `Context::from_process` (Task 1's `HOME` check, whose `Err` arm returns before any logging or handler), `logging::init` (Task 6), `signals::install`, `TtyPrompter::new(cancel)`, then the command.
  - Task 6: the `init_logging` call it deletes is in `run_command`. Its unlocked `std::io::stderr()` replaces M3a's locked one on the `(out, err)` line. Its `hooks.rs` change is additive to M3a's pause point.
  - The exhaustive `command_name` gains `Config`, `Displaced` and `Completions` arms (Tasks 3, 9 and 10).
  - Task 9: a prompt is a cancellation point, so `App::displaced` calls `self.after_prompt()?` after its confirmation, and `KIND_NEEDS_CONFIRMATION` goes next to `KIND_INTERRUPTED`. `TtyPrompter::new(cancel)` is the prompter it asks through the existing `Prompter` trait.
  - Task 10: the `completions` branch goes into `run_command` beside `statusline`'s, returning `Ended::Code(…)`.
  - `KIND_INTERRUPTED` and the late-signal notice apply to the new commands unchanged.
- **M3a Tasks 9 and 11:**
  - Task 8's deltas to `switch.rs` (`transact`, `save_unheld`, `before_fallback`), `recover.rs` (`finish_forward`) and `active.rs` (`publish`) re-apply onto their versions, anchored on text they keep. Task 7's `switch.rs` edit is anchored by its message.
  - After M3a Task 11, a Keychain fallback's `before_fallback` displaces under CC's storage-write lock, and `displace` takes the displaced lock there. §4.3 and §9.1 allow that nesting (`d14a860`).
  - Task 7 Part D replaces `Rollback::fail` whole, anchored on `fn fail(mut self, cause: EngineError) -> EngineError {`. Task 11 changes what the rollback's undos do (they take CC's storage-write lock), not `fail`, `struct Rollback` or its literal in `transact`.
  - Task 9 edits a comment inside `refresh_stored`, which Part D's wrapper turns into `run_gate`'s body. Part D's anchor is the doc comment and signature, which Task 9 keeps.
- **M3a Task 10:** `Command::Switch` gains `--strategy` and `--model`. The new variants follow `Command::Statusline` in task order: Task 3's `Config`, Task 9's `Displaced`, then Task 10's `Completions`, the last.
- **M3b (merged, `3e5ba2b`):** `settings.rs` already reads every `autoswitch.*` key into `Settings`. It exports `DEFAULT_THRESHOLD`, `DEFAULT_INTERVAL_SECONDS`, `DEFAULT_COOLDOWN_SECONDS`, `DEFAULT_HYSTERESIS_PCT`, `DEFAULT_UNHEALTHY_TICKS`, the ranges `THRESHOLD_RANGE`, `INTERVAL_SECONDS_RANGE`, `COOLDOWN_SECONDS_RANGE`, `HYSTERESIS_PCT_RANGE` and `UNHEALTHY_TICKS_RANGE`, `parse_bool` and `Settings::mtime`. `auto` clamps its flags to the same ranges, and `crates/tagteam/src/auto.rs` and its tests pin the warning phrases.
  - Task 2 is written against `settings.rs` before M3b. At the re-sync it folds M3b's reader into the registry and keeps M3b's names, types and tests:
    - `interval_seconds` and `cooldown_seconds` are `i64`;
    - the strategy enum is M3b's `tagteam_core::autoswitch::Strategy`, with its own `as_str` and `parse`, which `auto`'s `AutoConfig.strategy` uses. Task 2's `AutoStrategy` enum and its `impl` are dropped rather than renamed, every use becomes `Strategy`, and Task 2's tests import it from `tagteam_core::autoswitch`, since `settings` imports it privately;
    - the registry's `Float` and `Int` bounds come from the `*_RANGE` constants, and its defaults from the `DEFAULT_*` constants, never restated;
    - `Key::expect` returns M3b's phrases for the `autoswitch` keys, such as "must be a whole number of seconds from 15 to 3600";
    - a boolean in the file reads through M3b's `parse_bool_item`, and `Key::parse_arg` through `parse_bool` (Decision 16).
  - `default_provider` and `inspect` remain Task 2's own: M3b reads neither.
  - Tasks 7 and 8: M3b threads an event source through recovery (`guard_or_refuse_as`, `settle_or_refuse_as`). Task 7's line inside `recover_one` and Task 8's `recover.rs` delta re-apply onto it, anchored on text M3b keeps.
  - The engine-lock record M5b's doctor reads is `{"pid":<u32>,"start":<u64>}\n`, replacing the whole file under the lock (`crates/tagteam-engine/src/auto.rs`).
- **M4a Task 7:** `Env.vars`, `Provider::share_policy(env)`, `Capabilities.sessions` and the providers' known-private lists.
  - Task 1: `Env::from_process` keeps `vars: BTreeMap::new(),`, and the `Env` unit test's `Env::from_process().vars` becomes `Env::from_process().unwrap().vars`.
  - Task 4: `config set run.share_extra` checks its names against `share_policy`, through M4a Task 13's matcher. A provider's entry is checked against that provider; the global one against every provider with `sessions`.
- **M4a Task 8:** `Context::from_process`, `build_engine`, `run` and `run_statusline` are restructured (`build_registry`, `locate`, `run_shell`), and `Env::inside_run_shell` is gone.
  - Task 1: `Context::from_process` returns `Result<Self, EnvError>`, takes `env: Env::from_process()?,` and ends `Ok(ctx)` after `capture_vars`, which so happens only in the `Ok` case.
  - Task 3: `build_engine`'s last parameter becomes `flag: Option<&ProviderId>`, `command_settings` replaces its `Settings::load`, and `run` drops `resolved`.
  - Task 4: `config_writes_inside_a_run_shell` uses `Fx::{write_marker, shell_env, engine_located}`, `Engine::run_shell()` and `RunShell`.
  - Task 10: the `completions` branch moves to just after the `RunShell::Unreadable` refusal, since §12.8 refuses every command but `statusline` under an unreadable marker. Its unit test compares `build_registry(&ctx).all()`'s ids with `COMPLETED_PROVIDERS` in both directions.
  - `config` and `displaced` change no account, so a readable marker refuses neither (§6.4).
- **M4a Task 4:** `ActiveOutcome::Replaced`. Task 7 Part D's `log_active` gains a DEBUG arm for it, since nothing is sent or written (§7.5 step 2).
- **M4a Tasks 9 and 10:** `Owned(Session)` from `owner_of`, and Task 10 replaces the whole of `refresh_stored` with the same doc comment and signature. Task 7 Part D's anchor holds, and their body becomes `run_gate`'s. `log_gate`'s arms already cover `Owned(Session)`, `Conflict` and `Transient { kind: "profile-unreadable" }`.
- **M4a Task 13:**
  - Task 2 is written against `settings.rs` and `tests/settings.rs` as 13a leaves them. It keeps 13a's `Settings.share_extra` and `is_share_name` (now `pub`), and the registry's `ShareNames` kind replaces `Reader::share_extra`. M4a's `share_extra` tests pass unchanged.
  - Task 4: `profiles.rs`'s `is_private`, the matcher link sync uses, becomes `pub(crate)`.
- **M4a Task 14:** `common::cc_profile`, which Task 3's `config_answers_inside_a_run_shell` uses. `account_view_with` gains `in_session: bool`: Task 7 Part D's `status_bar` goes after it, and `account_view` passes `false` for it.
- **M4a Task 15:** `statusline::engine` takes `flag: Option<&str>` and returns the provider. Its provider order (M4a's Decision 13) ends in `command_settings`'s default, `default_provider` (Task 3). Both of its `Engine::statusline` calls to `account_view_with` pass `status_bar = true` (Task 7 Part D).
- **M4a Tasks 1–3 and 5:**
  - `begin_replacement_with` and `finish_replacement`: M5a uses neither, and §12.5's `replacement-unreadable` is M5b's.
  - `commit_switch` gains `epoch`; Task 7's log line still follows its commit.
  - `settle_outgoing`'s facts, and the step-7 loop over `doomed`: Task 8's anchors survive both.
  - `views.rs` gains a second `id = %row.id`, in `in_session`; Task 7 renames it too.

## Milestones

| Milestone | Scope |
|---|---|
| M1, M2a, M2b | Implemented (`main` at `3c1f458`) |
| M3a | Signals, cancellation and usage strategies: implemented (`main` at `7ee733b`) |
| M3b | Auto-switch: implemented (`main` at `3e5ba2b`) |
| M4a, M4b | Sessions foundation; `tagteam run` (`m4-run-sessions`) |
| **M5a (this plan)** | `HOME` validation; the settings registry and every §6.4 key; `config list\|get\|set\|unset\|path`; logging to a file; `displaced` listing and purge; `completions` |
| M5b | `doctor`, `purge`, export and import, `cargo xtask compat`, §12.5's `replacement-unreadable`, the `unquarantine` event on every quarantine clear (§7.4), release |

**Deliberately absent from M5a, and why that is safe:**
- **No `doctor`.** Task 2's `settings::inspect` reports unknown keys and invalid values in the shape `doctor` (M5b) will read. Until then `config list` shows them.
- **No change to the auto-switch keys' behaviour.** M3b (merged) already reads `autoswitch.interval_seconds`, `cooldown_seconds`, `hysteresis_pct`, `strategy`, `include_api_key_accounts` and `unhealthy_ticks` into `Settings`, and `auto` uses them. Task 2 folds M3b's reader into the registry and keeps its behaviour and tests ("M3b" under the interfaces below).
- **No §7.4 `unquarantine` event for the switch's outgoing capture.** That is M5b's, next to `import`, the other path that clears a quarantine.
- **`replacement-unreadable`** (§12.5) needs M4a's `finish_replacement`, and is M5b's.
- **Engine errors are not yet logged by `kind()` alone.** Several `EngineError` variants carry a label, often an email, in their `Display`: `UnreadableAccount`, `NeedsRelogin`, `RescuePending`, `OwnerMismatch`, `IdentityConflict`, `NeedsConfirmation`, `Ambiguous` and `NoSuchAccount`. Task 7's audit traced every `{e}` a log line formats today, and none can be one of them, so no line leaks a label now. Nothing stops a later line from formatting one, so M5b's review takes the rule that a new log line formats an engine error by `kind()`, or only on a path that cannot carry a label.
- **The other discarded errors.** §14 says contained errors are logged, never discarded. Task 7 logs the two M1's review left (L323, L349). The remaining `let _ =` discards stay for M5b's sweep, which decides each between WARN and silence. One example is the directory `fsync` after a published atomic write, which some filesystems refuse on every call.

## Decisions

Rulings made while planning. Each names what it would cost if wrong.

1. **The registry is a `const` table in `tagteam-engine::settings`.** `KEYS: &[Key]` lists every §6.4 key with its kind, its per-provider flag and its "must be …" phrase. `Settings` keeps typed fields; the reader fills them through the registry, and `Settings::value(&Key)` reads one back as a `Value` for display. Cost if wrong: a table moved to another crate.
2. **`default_provider` lives in the top-level table, is read globally and is checked by the CLI.** The reader checks only that it is a well-formed provider id. The CLI checks it against the registry: a provider this build lacks warns and falls back to `claude-code`. `config set default_provider` refuses an unregistered id. Cost if wrong: one check moved into the engine.
3. **Strict writes are `Engine` methods in a new `tagteam-engine::config` module.** The engine already holds the `Env`, the registry (for `default_provider` and the known-private check of `run.share_extra`) and the cancel token the settings lock waits on. Reads stay free functions in `settings`, which never need an engine. Cost if wrong: a module boundary.
4. **The kinds of a settings error.** `SettingsError`, carried as `EngineError::Settings` (Task 2), has these kinds:
   - `invalid-input`: an unknown key; a key that a provider table cannot override; a `provider.<id>.` prefix and a `--provider` that name different providers; a value of the wrong type or out of range; and a `default_provider` value naming a provider this build does not register (`SettingsError::UnknownProvider`);
   - `settings-unreadable`: a corrupt file;
   - the lock's own kind for the settings lock: `lock-timeout`, `lock`, or `interrupted` after M3a;
   - `io`: I/O.

   A `provider.<id>.` prefix or `--provider` naming a provider this build does not register is not a settings error. It fails with `EngineError::UnknownProvider`, kind `unknown-provider`, in `config get`, `set` and `unset` alike. The two spellings name the same entry (§6.4), and `--provider` has always failed that way, before any command runs. So `config get`, `set` and `unset` check a prefix's provider with `Engine::provider`, as `run` checks the flag. Cost if wrong: renamed stable strings, before any release.
5. **Logging is a process concern.** `main_with_args` sets the subscriber up once; `app::run` no longer does, so an in-process test installs nothing global. The subscriber is installed with `tracing::subscriber::set_global_default`, never `try_init`, so the `log` crate's records (ureq, rustls) are not bridged into the file. Cost if wrong: a dependency's records missing from `--debug`.
6. **The file filter defaults to the tagteam crates.** By default it is `warn,tagteam=info,tagteam_core=info,tagteam_provider=info,tagteam_cc=info,tagteam_engine=info`. With `--debug`, `info` becomes `debug`. `TAGTEAM_LOG` replaces the whole directive. The stderr layer keeps M2's behaviour: ERROR, or DEBUG with `--debug`. Cost if wrong: one directive string.
7. **The log file is a `MakeWriter` with one `write` per event.** Each event's line is buffered and handed to the file as a single `write(2)` on an `O_APPEND` descriptor. The file is opened on the first event. Rotation is tried under `tagteam.log.lock` (a try-only `flock`), and an inode check before each write reopens a rotated file. Any failure disables the file for the rest of the process. Cost if wrong: a writer type.
8. **The line format is tagteam's own `FormatEvent`.** A line is `2026-10-01T18:42:33.581Z 4242 INFO tagteam_engine::switch: message key=value …`. Every occurrence of the home directory's path followed by `/` is written `~/`. Cost if wrong: a formatter.
9. **A panic is logged with target `tagteam::panic`,** which the stderr layer filters out, since the default hook already prints the panic there. Cost if wrong: a duplicated stderr line.
10. **INFO comes from a few central sites, not from every caller.** These are:
    - the store's event insert (every `events` row: switches, adds, removes, quarantines);
    - `Vault::store` (every vault generation written: refreshes, captures, replacements);
    - the rescue writer and `displace`;
    - the purge of a displaced entry, one line per entry;
    - the recovery decision;
    - a switch's rollback, in `Rollback::fail`: WARN, or ERROR when something was not put back;
    - every refresh outcome, once per call, from wrappers around the gate (`refresh_stored`) and
      the active-token refresh (`refresh_active`): DEBUG when nothing was sent or changed;
    - settings writes.

    A site that holds the settings lock or the displaced lock logs once the lock is released, so nothing is taken under either (§4.3). §4.3 lets a log line try the rotation lock under any lock, so this is tidiness, not correctness. Cost if wrong: an INFO line added at a missed site.
11. **The displaced lock is waited for up to 5 s on a token nothing sets.** `displace` runs inside the switch's and the gate's critical spans (§14.1), where nothing may be cancelled. `displaced --purge` waits on the same token: one deletion is milliseconds. Cost if wrong: a cancellation point added to the purge.
12. **A displaced ID is validated before any path is built from it.** It must match `^[0-9]{1,19}-[0-9a-f]{12}-[a-z0-9]{6}$`. `--purge` refuses any other ID as unknown, and the listing ignores files whose names do not match. Cost if wrong: none; it is a path-traversal defence (B.34's spirit).
13. **A displaced row is attributed only to the live login's own secret** (§6.3, L467: "its row carries an identity only when the bytes themselves name one"). `save_unheld` gains `own_secret: Option<&[u8]>`, and `switch::attributed` names the live identity only for bytes equal to it, so a stray secret on the other auth axis gets no identity. In a switch, the own secret is `own_secret`: the live secret on the outgoing account's own axis, or the credential entry's when no stored account is live, since an unmanaged login is named by `oauthAccount`, the entry's axis. It never falls back to the other axis. Step 6's `from_secret` does: with the outgoing axis empty it takes the entry's secret, which is what the journal's `from_fp` needs (§9.6) and stays as it is. As an own secret, though, that fallback would file a stray OAuth login under an API-key account whose key is gone. The rule holds wherever a live secret is displaced:
    - §9.4 step 7's `save_unheld`, in the switch's step-7 loop and in a Keychain fallback's `before_fallback`, with `own_secret`;
    - §9.4 step 2's direct branch, which displaces the live secret on both axes. `own_secret` is therefore computed before step 2;
    - recovery's forward finish (§9.6, `recover.rs`), which passes `None` for both and so attributes nothing. Every entry it clears sits on the other auth axis, and §9.6 never takes the live identity as evidence of whose an entry is.

    Cost if wrong: two arguments at three call sites.
14. **Completions are static.** The `config` key argument uses a value parser that accepts any string but advertises the registry's keys, so completion offers them while an unknown key still reaches the engine's `invalid-input` (§6.4) rather than clap's usage error. Cost if wrong: a parser type.
15. **`Env::from_process` returns `Result<Env, EnvError>`.** `Context::from_process` passes the error up, and `main_with_args` reports it before any engine exists: kind `env`, exit 1, or nothing at all from the status bar's own `statusline` call, which exits 0 (§5). Cost if wrong: one signature.
16. **Rulings taken inside the tasks**, one line each; the task named records the reasoning:
    - `EnvError::kind()` is the only source of `env`; the CLI keeps no `KIND_ENV` (Task 1).
    - "Not absolute" is `Path::is_absolute`. `/` and a trailing slash pass, nothing is canonicalized, and only `HOME` is checked (Task 1).
    - Only the status bar's own call (`statusline` without `--print-config` or `--json`) answers an unusable `HOME` with silence (Task 1).
    - `EngineError::Settings` and its kinds are Task 2's; Task 4 adds only the settings arm of M3a's `signal()` (Tasks 2, 4).
    - A boolean in the file reads as M3b's `parse_bool_item` reads it: a TOML boolean, an integer `1` or `0`, or one of the six words as a string; the command line takes the six words through `parse_bool` (Task 2, after the M3b re-sync).
    - `keys_this_milestone_does_not_read_are_ignored_without_a_warning` becomes `an_unknown_key_is_ignored_without_a_warning`, since the keys it listed are read now (Task 2).
    - An unknown key is listed by `config list`, never warned about, sorted by dotted name (Task 2).
    - A float prints in Rust's shortest form (`80`), which `config set` takes back unchanged (Task 2).
    - A provider this build lacks, named by a `provider.<id>.` prefix or by `--provider`, is `unknown-provider` in `get`, `set` and `unset` alike (Tasks 3, 4; Decision 4).
    - `ConfigKeyParser` needs clap's `string` feature, for possible values built at run time from `KEYS` (Task 3).
    - An unregistered `default_provider` warns on every command and falls back to claude-code, and `statusline` drops the warning (Task 3).
    - `config list`'s text form is a `KEY  SOURCE  VALUE` table in registry order, with an empty value shown as `''` (Task 3).
    - `config list` and `get` print the warnings of the read they show, less those `run` already printed (Task 3).
    - `unset` refuses an unknown key or entry as strictly as `set`, and never checks the value it removes (Task 4).
    - A `set` or `unset` that changes nothing writes and creates nothing, not even the settings lock's file (Task 4).
    - The settings INFO line names the key alone, never the value, and is logged after the lock is released (Task 4).
    - A table that `unset` empties is removed unless it holds a comment; new tables keep the file's style (Task 4).
    - Log-capturing tests take turns behind a mutex. A capture whose call site other tests' threads also fire fires it once first, because `tracing` caches a call site's interest across threads (Tasks 4, 6, 7).
    - "Exceeds 1 MiB" is `len > limit`, checked before each write. A short write disables the file (Task 5).
    - `Io.err` is an unlocked `std::io::stderr()`, which fixes a hang when a collector thread logs to stderr (Task 6).
    - A panic's line logs its message only when it is a literal; the default hook still prints it whole on stderr (Task 6).
    - `TAGTEAM_TEST_FAIL_AT=panic:<name>` panics the real binary at a hook point, under `test-hooks` only (Task 6).
    - A line naming two accounts writes `from_account=` and `to_account=`, each ending in §14.2's `account=<id>` (Task 7).
    - An events row's `detail` is never logged: it is free-form JSON (Task 7).
    - A rolled-back switch's line names `from_account`, `to_account` and the cause's `kind()`, never its text; `Drop`'s lines for a panic's rollback stay as they are (Task 7).
    - A refresh outcome's level follows its variant, not the path that returned it: INFO once a request was sent or state changed, DEBUG otherwise, so no `return` is edited (Task 7).
    - A gate or §7.5 refresh that ends in an error logs `outcome="error"` at INFO, with the error's `kind()` (Task 7).
    - `account_view_with` takes `status_bar: bool`, which logs a failed usage read at DEBUG; an account command's result keeps the WARN (Task 7).
    - `Engine::known_displaced` lets `displaced --purge` refuse an unknown ID before it asks for confirmation (Tasks 8, 9).
    - A purge deletes the path itself, never a symlink's target, and verifies it with `lstat` (Task 8).
    - A switch's own secret is the live secret on the outgoing account's own axis, or the credential entry's with no stored account live. Step 6's `from_secret` keeps its fallback to the entry for the journal alone (Task 8; Decision 13).
    - `displaced`'s WHEN is UTC ISO 8601 to the second, the JSON's `at`, not `list`'s relative age (Task 9).
    - `unrecorded` and `file missing` trail a row as a note; IDENTITY keeps showing the identity (Task 9).
    - `--yes` without `--purge` is a usage error the app raises, so the message survives argument scrubbing (Task 9).
    - `displaced --purge --json` prints `{schemaVersion, ok, deleted: [id]}`, a shape §6.3 leaves open (Task 9).
    - The redaction pin gains a displaced entry that names a stranger and the organization, listed and purged (Task 9).
    - Completion tests derive their expectations from the command definitions rather than snapshot `clap_complete`'s bytes with `insta`: the reading of §15.2 for completions (Task 10).
    - fish gets one `complete` line per positional argument with fixed values, which `clap_complete` does not emit: the reading of §13.7's "settings keys … complete". A line applies only while the words before the cursor, less options and option values, are exactly its subcommands and then the positionals before it, which one helper, `__fish_tagteam_words`, lists (Task 10).
    - `--provider` takes `ProviderParser`: any string, with this build's providers advertised for completion (Task 10).
    - `completions` refuses under an unreadable run-shell marker like every command but `statusline` (§12.8) (Task 10).

## Global Constraints

Every task's requirements include these. Values are copied from the spec.

- Platforms: macOS and Linux only (§1.2). Rust edition 2024, MSRV ≥ 1.85 (§16).
- Directories are created lazily with mode 0700, so a command that changes nothing creates nothing (§5). Every tagteam file is created 0600 (§5, §6.4).
- `--json` prints exactly one JSON object on stdout; warnings and notices go to stderr (§13.2, B.36). Errors print `{"schemaVersion":1,"error":{"type":<kind>,"message":…}}` with a non-zero exit. Exit codes: `0` OK · `1` error · `2` usage error (§13.1).
- **Settings** (§6.4):
  - Reads are forgiving: a corrupt file or an invalid value falls back to the default with a warning naming the path and the key.
  - `set` and `unset` are strict: they refuse a corrupt file, and refuse an unknown key, a key that a provider table cannot override, a wrong type or an out-of-range value with `invalid-input`. They never clamp.
  - Booleans parse only `true/false/1/0/yes/no`. Lists are comma-separated, each item trimmed, an empty item refused, `''` an empty list. `all` in `autoswitch.models` stands alone.
- **The settings lock** is `$XDG_DATA_HOME/tagteam/locks/config.lock`: a standalone `flock`, outside the lock order, waited for up to 5 s, with each wait a cancellation point (§4.3, §6.4).
- **The displaced lock** is `$XDG_DATA_HOME/tagteam/locks/displaced.lock`: a leaf `flock` held only around one entry's file and row, which may be taken under any other lock, CC's storage-write lock included, and under which nothing is taken (§4.3, §6.3, §9.1).
- **The log** is `$XDG_STATE_HOME/tagteam/tagteam.log`: mode 0600 in a 0700 directory, rotated at 1 MiB to `.1` and `.2`, under a try-only `flock` on `tagteam.log.lock` (§14.2).
  - Nothing waits on the log, and logging never fails a command.
  - No log line, at any level, holds an email, label or organization name, a token, key or credential (or any part of one), a passphrase, an export's contents, or a request's `Authorization` header or body (§14.2, B.69).
  - An account is named `account=<id> position=<n>`; a fingerprint appears as its first 12 hex digits at most.
- Secrets never reach error output (B.36, M1's argument scrubbing in `lib.rs`).
- `HOME` unset, empty or not absolute: exit 1 with kind `env`; `statusline` prints nothing (§5).
- Tests never reach the real network, the real HOME or the login keychain (§15.1).

## Review Focus

The five input classes or failure modes most likely to bite a user that no task's main tests already exercise. Each has a test in its owning task:

1. **A `config.toml` that is a symlink, as chezmoi or a dotfiles repo leaves it.** `config set` writes through the link to its target, keeping every comment. The link stays a link, and the target keeps its mode. (Task 4)
2. **An empty-list override, and lists typed the way people type them.** `config set provider.claude-code.autoswitch.models ''` writes `[]`, which beats a non-empty global list. `"Fable, opus"` is two trimmed names. `"Fable,,Opus"` refuses, and `"all,Fable"` refuses. (Tasks 2, 4)
3. **`displaced --purge ../../tagteam.db`, or any ID that is not a displaced ID.** It refuses as unknown before any path is built, and deletes nothing, even when other IDs on the same command line are valid. (Tasks 8, 9)
4. **Several tagteam processes logging at once across a rotation:** `auto`, `statusline` and `list` together. No line is interleaved or cut, and `statusline` never creates the log, even when it cannot read the live account's usage. (Tasks 5, 6, 7)
5. **`TAGTEAM_LOG=trace` on commands that send tokens.** A switch, a usage fetch through the mock server and an `add-token` leave no token, key, email or organization name in the log. `ureq`'s own `log` records are not bridged in. (Task 7)

---

## File Structure

| File | Task | Responsibility |
|---|---|---|
| `Cargo.toml` | 3, 10 | clap's `string` feature; `clap_complete` |
| `Cargo.lock` | 7, 10 | `tracing` in `tagteam-provider`'s dependencies, `rusqlite` in `tagteam`'s (7); `clap_complete` (10) |
| `crates/tagteam-provider/Cargo.toml` | 7 | `tracing`; dev `tracing-subscriber` |
| `crates/tagteam-provider/src/env.rs` | 1 | `Env::from_process` validates `HOME`; `EnvError`, `home_from` |
| `crates/tagteam-provider/src/lib.rs` | 1 | re-exports `EnvError` |
| `crates/tagteam-provider/src/atomic.rs`, `security.rs` | 7 | a temp file left behind is logged (L323); a timeout is told from a spawn failure (L349) |
| `crates/tagteam-engine/src/settings.rs` | 2 | the key registry, the forgiving reader, `inspect`, `resolve`, `SettingsError` |
| `crates/tagteam-engine/src/config.rs` (new) | 4 | `Engine::config_set` / `config_unset`: strict writes under the settings lock |
| `crates/tagteam-engine/src/lib.rs` | 4, 8 | `pub mod config`; `displace` becomes `pub mod` |
| `crates/tagteam-engine/src/error.rs` | 2, 4, 8 | `EngineError::Settings` and its kinds, `lock_kind` (2); the settings arm of M3a's `signal()` (4, Re-sync); `EngineError::NoSuchDisplaced` (8) |
| `crates/tagteam-engine/src/profiles.rs` | 4 | M4a Task 13's `is_private`, made `pub(crate)` |
| `crates/tagteam-engine/src/displace.rs` | 7, 8 | INFO per displacement (7); IDs, the listing, the writer and the purge under the displaced lock (8) |
| `crates/tagteam-engine/src/store/mod.rs` | 7, 8 | INFO per event row, `log_event` (7); `Store::displaced_rows`, `Store::delete_displaced` (8) |
| `crates/tagteam-engine/src/switch.rs` | 7, 8 | `account=` on the capture's WARN, a rollback's WARN or ERROR in `Rollback::fail` (7); `attributed`, `save_unheld`'s `own_secret` (8, L467) |
| `crates/tagteam-engine/src/recover.rs` | 7, 8 | INFO at the recovery decision (7); forward recovery attributes nothing (8) |
| `crates/tagteam-engine/src/active.rs` | 7, 8 | `account=` on `publish`'s WARNs; `refresh_active` wraps `run_active_refresh` and logs its outcome (`log_active`) (7); `save_unheld`'s new argument (8) |
| `crates/tagteam-engine/src/vault.rs`, `rescue.rs` | 7 | INFO at a written generation and at a rescue |
| `crates/tagteam-engine/src/refresh.rs` | 7 | `refresh_stored` wraps `run_gate` and logs its outcome (`log_gate`) |
| `crates/tagteam-engine/src/views.rs`, `lifecycle.rs` | 7 | `account=` for `id=`; `account_view_with`'s `status_bar`, so `statusline` logs a failed usage read at DEBUG (`views.rs`) |
| `crates/tagteam-engine/src/hooks.rs` | 6 | `TAGTEAM_TEST_FAIL_AT=panic:<name>` |
| `crates/tagteam/Cargo.toml` | 7, 10 | dev `rusqlite` (7); `clap_complete` (10) |
| `crates/tagteam/src/lib.rs` | 1, 3, 5, 6, 9 | process boundary: `HOME` failure reporting, logging setup, an unlocked stderr; the module list |
| `crates/tagteam/src/app.rs` | 1, 3, 4, 6, 7, 9, 10 | `Context::from_process` returns `Result`; `command_settings` and `default_provider`; dispatch of `config` and `displaced`; `completions`' fast path; `init_logging` removed; `account=` in `or_inactive` |
| `crates/tagteam/src/cli.rs` | 3, 4, 9, 10 | `Command::Config`, `ConfigAction`, `ConfigKeyParser`, `COMPLETED_PROVIDERS`; `Command::Displaced`; `Command::Completions`, `CompletionShell`, `ProviderParser` |
| `crates/tagteam/src/statusline.rs` | 3 | `engine(ctx, flag)` resolves `default_provider` |
| `crates/tagteam/src/render.rs` | 9 | `width` and `pad` become `pub(crate)` |
| `crates/tagteam/src/config_cmd.rs` (new) | 3, 4 | human and JSON rendering of `config` results |
| `crates/tagteam/src/displaced_cmd.rs` (new) | 9 | human and JSON rendering of `displaced` |
| `crates/tagteam/src/logfile.rs` (new) | 5 | `LogFile`: the lazy, private, rotating, shared log writer |
| `crates/tagteam/src/logging.rs` (new) | 6 | the subscriber: filters, layers, line format, panic hook |
| Tests (new) | 1, 4, 6–10 | `crates/tagteam/tests/{home_cli,config_cli,logging,displaced_cli,completions}.rs`; `crates/tagteam-engine/tests/{config,displaced,log_lines}.rs` |
| Tests (extended) | 2–4, 7–10 | `crates/tagteam-engine/tests/{settings,store}.rs`; `crates/tagteam/tests/{app,cli}.rs`; unit tests in `env.rs`, `atomic.rs`, `security.rs` (`tagteam-provider`), `error.rs`, `displace.rs` (`tagteam-engine`), `app.rs`, `statusline.rs`, `logfile.rs`, `logging.rs`, `displaced_cmd.rs` (`tagteam`) |

## Task Overview

| # | Task | Depends on |
|---|---|---|
| 1 | `HOME` must be usable | — |
| 2 | The settings registry and the reader for every key | — (M4a Task 13a's `settings.rs`) |
| 3 | `config list`, `get` and `path`; `default_provider` | 2 |
| 4 | `config set` and `unset` | 2, 3 |
| 5 | The log file | — |
| 6 | The logging setup | 1, 5 |
| 7 | What the log records, and the redaction pin | 3, 4 (its pin runs `config`), 6 |
| 8 | Displaced entries in the engine | 7 (keeps `displace`'s INFO line; inserts after `log_event`) |
| 9 | `tagteam displaced` | 3 (anchors), 7 (the pin), 8 |
| 10 | `tagteam completions` | 3, 7 (the pin), 9 (anchors) |
| 11 | Final verification | all |

## Execution rulings

Recorded at execution time: the re-sync against merged M3a and M4a, then rulings taken during the run.

---

### Task 1: `HOME` must be usable

> **Re-sync (M3a, M4a).** This task is written against `main` at `3c1f458`. Apply it to the merged
> code as follows:
> - **`Env::from_process`'s literal.** It is `Ok(Self { … })` here. Keep M3a Task 1's
>   `cancel: Cancel::new(),` and M4a Task 7's `vars: BTreeMap::new(),` inside it, where those
>   tasks put them.
> - **M4a Task 7's test.** The `Env` unit test's last line,
>   `assert!(Env::from_process().vars.is_empty());`, becomes
>   `assert!(Env::from_process().unwrap().vars.is_empty());`. `HOME` is always set under test,
>   because `Env::for_test` requires it.
> - **M4a Task 8's `Context::from_process`.** It builds `let mut ctx = Self { env:
>   Env::from_process(), … }`, then calls `ctx.env.capture_vars(&names)` and returns `ctx`.
>   After this task, `env: Env::from_process()?,` replaces the first, the function returns
>   `Result<Self, EnvError>`, and it ends with `Ok(ctx)` after the capture. The capture now
>   happens only in the `Ok` case.
> - **M3a Task 6's `main_with_args`.** It has `let ctx = app::Context::from_process();`, then
>   `signals::install(&ctx.env.cancel)`, then `prompt::TtyPrompter::new(ctx.env.cancel.clone())`.
>   The `match` below replaces only the first of these lines. The order is fixed: the parse, the
>   statusline drain, this `match`, logging (Task 6), then the signal handlers. A process that
>   refuses on `HOME` therefore installs no handlers and opens no log.

§5: "**`HOME` must be usable.** Every default path derives from it, so tagteam refuses to run,
with exit 1 and kind `env`, when `HOME` is unset, empty or not absolute; a fallback would put
state under `/` or the working directory. `statusline` prints nothing instead." Today
`Env::from_process` falls back to `/` when `HOME` is unset. It takes an empty or relative value
as given, which resolves every path against the working directory. This task adds
`home_from` and `EnvError` to `tagteam-provider`, and `Env::from_process` now returns
`Result<Env, EnvError>` (Decision 15). `Context::from_process` passes the error up. Before any
engine exists, `main_with_args` reports it:
- on stderr as `tagteam: <message>`, or as the error envelope with type `env` under `--json`,
  exit 1;
- or not at all for the status bar's line, which exits 0.

**Readings of the spec this task commits to:**
- **"Not absolute" is `Path::is_absolute`,** a value starting with `/`. `/` itself passes, and
  so does a trailing slash. The value is not canonicalized or checked for existence. A `HOME`
  that names no directory still fails later, at the first write, as `io`, just as it does today.
- **Only `HOME` is checked.** A relative `XDG_*` value is already ignored, with the
  `HOME`-based default used instead (`Env::from_process`'s `abs`). That stays.
- **"`statusline` prints nothing" means the status bar's own call:** `statusline` without
  `--print-config` and without `--json`, the one invocation that drains stdin. `--print-config`
  prints text for a person and fails with `env` like any command. So does `statusline --json`,
  whose usage refusal (`run_statusline`) comes after the context: the environment is checked
  first. One predicate, `status_bar_line`, now decides both the drain and the silence.
- **The parse comes first.** `--help`, `--version` and usage errors (exit 2) behave as before
  under a bad `HOME`.
- **The `HOME` check runs before the root refusal** (`root_guard`, inside `app::run`). Both are
  refusals, so a root user with a bad `HOME` sees `env`.
- **A relative value is named in the message** through `Debug`, so a control character in it is
  escaped. A non-UTF-8 value is shown lossily.

**Files:**
- Modify: `crates/tagteam-provider/src/env.rs`:
  - imports, lines 1–2;
  - new `EnvError` and `home_from` before `pub struct Env`, line 4;
  - `Env::from_process`, lines 20–38;
  - the test module, lines 110–162.
- Modify: `crates/tagteam-provider/src/lib.rs` (the `env` re-export, line 25)
- Modify: `crates/tagteam/src/app.rs`:
  - imports, line 24;
  - `Context::from_process`, lines 120–133.
- Modify: `crates/tagteam/src/lib.rs`:
  - new `status_bar_line` before `main_with_args`, line 45;
  - `main_with_args`, lines 74–100.
- Test: `crates/tagteam-provider/src/env.rs` (unit), `crates/tagteam/tests/home_cli.rs` (new)

**Interfaces:**
- Consumes: `app::error_json(kind: &str, message: &str) -> Value`, `app::EXIT_ERROR`,
  `common::std_cmd(root: &Path) -> std::process::Command` (it sets `HOME`, `USER`, `PATH` and the
  test overrides on a cleared environment)
- Produces:
  - `tagteam_provider::EnvError` (re-exported at the crate root): `#[derive(Debug, Clone,
    PartialEq, Eq, thiserror::Error)] pub enum EnvError { HomeUnset, HomeEmpty,
    HomeRelative(String) }`, plus `pub fn kind(&self) -> &'static str`, always `"env"`
  - `tagteam_provider::env::home_from(value: Option<OsString>) -> Result<PathBuf, EnvError>`
  - `Env::from_process() -> Result<Env, EnvError>`
  - `tagteam::app::Context::from_process() -> Result<Context, EnvError>`
  - `fn status_bar_line(cli: &cli::Cli) -> bool`, private to `crates/tagteam/src/lib.rs`
  - The order in `main_with_args`: the parse, the drain, `Context::from_process`, then
    `app::run`. Task 6 inserts logging between the last two.

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-provider/src/env.rs`'s test module, after the test
`a_run_shell_is_detected_from_claude_config_dir` (line 161; the test is gone after M4a Task 8,
so after the module's last test), add:

```rust
    #[test]
    fn home_must_be_set_non_empty_and_absolute() {
        // §5: every default path derives from HOME, so nothing may fall back to `/` or the
        // working directory.
        assert_eq!(home_from(None), Err(EnvError::HomeUnset));
        assert_eq!(home_from(Some(OsString::new())), Err(EnvError::HomeEmpty));
        for relative in ["rel", "./home/u", "~/u", " /home/u"] {
            assert_eq!(
                home_from(Some(relative.into())),
                Err(EnvError::HomeRelative(relative.into())),
                "{relative:?}"
            );
        }
        assert_eq!(
            home_from(Some("/home/u".into())),
            Ok(PathBuf::from("/home/u"))
        );
        assert_eq!(
            home_from(Some("/home/u/".into())),
            Ok(PathBuf::from("/home/u/")),
            "a trailing slash is fine"
        );
        assert_eq!(home_from(Some("/".into())), Ok(PathBuf::from("/")));
    }

    #[test]
    fn each_refusal_says_what_to_do_and_is_kind_env() {
        let cases = [
            (
                EnvError::HomeUnset,
                "HOME is not set; set it to the absolute path of your home directory",
            ),
            (
                EnvError::HomeEmpty,
                "HOME is empty; set it to the absolute path of your home directory",
            ),
            (
                EnvError::HomeRelative("rel\x1b[2J".into()),
                "HOME is \"rel\\u{1b}[2J\", which is not an absolute path; set it to the absolute path of your home directory",
            ),
        ];
        for (e, message) in cases {
            assert_eq!(e.to_string(), message);
            assert_eq!(e.kind(), "env", "{e:?}");
        }
    }
```

Create `crates/tagteam/tests/home_cli.rs`:

```rust
//! §5: on an unset, empty or relative `HOME`, tagteam refuses to run before anything is
//! created, and the status bar's line prints nothing instead. Needs `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::fs;
use std::path::Path;
use std::process::Output;

use common::std_cmd;
use serde_json::{Value, json};

/// What a relative `HOME` is set to.
const RELATIVE: &str = "rel";
/// Where the old fallback for an unset `HOME` put tagteam's state.
const UNDER_ROOT: [&str; 3] = [
    "/.local/share/tagteam",
    "/.config/tagteam",
    "/.local/state/tagteam",
];
/// Commands that read or write state under `HOME`. On a fresh machine `add-token` creates the
/// store and the vault, so a fallback would show in the working directory.
const COMMANDS: [&[&str]; 5] = [
    &["list"],
    &["status"],
    &["switch"],
    &["history"],
    &["add-token", "sk-ant-api03-key"],
];

#[derive(Clone, Copy, Debug)]
enum Home {
    Unset,
    Empty,
    Relative,
}

impl Home {
    const ALL: [Home; 3] = [Home::Unset, Home::Empty, Home::Relative];

    /// The refusal, verbatim: its wording tells the user what to do.
    fn message(self) -> &'static str {
        match self {
            Home::Unset => "HOME is not set; set it to the absolute path of your home directory",
            Home::Empty => "HOME is empty; set it to the absolute path of your home directory",
            Home::Relative => {
                "HOME is \"rel\", which is not an absolute path; set it to the absolute path of your home directory"
            }
        }
    }

    /// Where the old fallback read `~/.claude.json`, relative to the working directory. `None`
    /// for an unset `HOME`, whose fallback was `/`.
    fn claude_json(self) -> Option<&'static str> {
        match self {
            Home::Unset => None,
            Home::Empty => Some(".claude.json"),
            Home::Relative => Some("rel/.claude.json"),
        }
    }
}

/// `tagteam <args>` under `home`, run from the working directory `cwd`.
fn run(root: &Path, cwd: &Path, home: Home, args: &[&str]) -> Output {
    let mut c = std_cmd(root);
    match home {
        Home::Unset => c.env_remove("HOME"),
        Home::Empty => c.env("HOME", ""),
        Home::Relative => c.env("HOME", RELATIVE),
    };
    c.current_dir(cwd).args(args).output().unwrap()
}

fn under_root() -> Vec<bool> {
    UNDER_ROOT.iter().map(|p| Path::new(p).exists()).collect()
}

fn is_empty(dir: &Path) -> bool {
    fs::read_dir(dir).unwrap().next().is_none()
}

#[test]
fn an_unusable_home_refuses_every_command_and_creates_nothing() {
    let root = tempfile::tempdir().unwrap();
    let before = under_root();
    for home in Home::ALL {
        for args in COMMANDS {
            let cwd = tempfile::tempdir().unwrap();
            let out = run(root.path(), cwd.path(), home, args);
            assert_eq!(out.status.code(), Some(1), "{home:?} {args:?}");
            assert_eq!(
                String::from_utf8_lossy(&out.stdout),
                "",
                "{home:?} {args:?}"
            );
            assert_eq!(
                String::from_utf8_lossy(&out.stderr),
                format!("tagteam: {}\n", home.message()),
                "{home:?} {args:?}"
            );
            assert!(
                is_empty(cwd.path()),
                "{home:?} {args:?}: something was created in the working directory"
            );
        }
        // The parse comes first: a usage error and the version are as before.
        let cwd = tempfile::tempdir().unwrap();
        assert_eq!(
            run(root.path(), cwd.path(), home, &["frobnicate"])
                .status
                .code(),
            Some(2)
        );
        assert_eq!(
            run(root.path(), cwd.path(), home, &["--version"])
                .status
                .code(),
            Some(0)
        );
    }
    assert_eq!(under_root(), before, "something was created under /");
    assert!(
        !root.path().join("keychain").exists(),
        "a vault item was written"
    );
}

#[test]
fn under_json_the_refusal_is_one_env_error() {
    let root = tempfile::tempdir().unwrap();
    for home in Home::ALL {
        for args in [
            &["list", "--json"][..],
            &["--json", "add-token", "sk-ant-api03-key"][..],
            // `--json` is no status bar: it gets the error object, not silence.
            &["statusline", "--json"][..],
        ] {
            let cwd = tempfile::tempdir().unwrap();
            let out = run(root.path(), cwd.path(), home, args);
            assert_eq!(out.status.code(), Some(1), "{home:?} {args:?}");
            assert_eq!(
                serde_json::from_slice::<Value>(&out.stdout).unwrap(),
                json!({"schemaVersion": 1, "error": {"type": "env", "message": home.message()}}),
                "{home:?} {args:?}"
            );
            assert_eq!(
                String::from_utf8_lossy(&out.stderr),
                "",
                "{home:?} {args:?}"
            );
            assert!(is_empty(cwd.path()), "{home:?} {args:?}");
        }
    }
}

#[test]
fn the_status_bar_line_prints_nothing_and_print_config_is_refused() {
    let root = tempfile::tempdir().unwrap();
    for home in Home::ALL {
        let cwd = tempfile::tempdir().unwrap();
        // A login where the old fallback would have found it, so the old code printed a line.
        if let Some(path) = home.claude_json() {
            let path = cwd.path().join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(
                &path,
                r#"{"oauthAccount":{"emailAddress":"stranger@x.co","organizationUuid":"","accountUuid":"uuid-s"}}"#,
            )
            .unwrap();
        }
        let out = run(root.path(), cwd.path(), home, &["statusline"]);
        assert_eq!(
            (
                out.status.code(),
                out.stdout.as_slice(),
                out.stderr.as_slice()
            ),
            (Some(0), &b""[..], &b""[..]),
            "{home:?}"
        );
        let out = run(
            root.path(),
            cwd.path(),
            home,
            &["statusline", "--print-config"],
        );
        assert_eq!(out.status.code(), Some(1), "{home:?}");
        assert_eq!(out.stdout, b"", "{home:?}");
        assert_eq!(
            String::from_utf8_lossy(&out.stderr),
            format!("tagteam: {}\n", home.message()),
            "{home:?}"
        );
    }
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-provider --lib env::tests`
Expected: compile errors: `cannot find function `home_from` in this scope` and
`failed to resolve: use of undeclared type `EnvError``.

Run: `cargo test -p tagteam --features test-support --test home_cli`
Expected: all three tests FAIL. Each fails on its first assertion, because `Home::Unset` comes
first and the old fallback treats `/` as `HOME`:
- `an_unusable_home_refuses_every_command_and_creates_nothing`:
  `assertion `left == right` failed: Unset ["list"]`, `left: Some(0)`, `right: Some(1)`. `list`
  finds no store under `/.local/share/tagteam` and prints "No accounts yet".
- `under_json_the_refusal_is_one_env_error`: the same, for `["list", "--json"]`.
- `the_status_bar_line_prints_nothing_and_print_config_is_refused`: the unset `statusline`
  already prints nothing, because nobody is logged in under `/`. The test then fails on
  `statusline --print-config` with `Unset`, `left: Some(0)`, `right: Some(1)`. With `Empty` and
  `Relative`, the old code would also have printed `stranger@x.co`.

- [ ] **Step 3: Implement**

**`crates/tagteam-provider/src/env.rs`.** Replace the imports (lines 1–2):

```rust
use std::ffi::OsString;
use std::path::PathBuf;
```

with:

```rust
use std::ffi::OsString;
use std::path::PathBuf;

/// Why the process's environment cannot place tagteam's files (§5). Each message says what to
/// do.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EnvError {
    #[error("HOME is not set; set it to the absolute path of your home directory")]
    HomeUnset,
    #[error("HOME is empty; set it to the absolute path of your home directory")]
    HomeEmpty,
    #[error(
        "HOME is {0:?}, which is not an absolute path; set it to the absolute path of your home directory"
    )]
    HomeRelative(String),
}

impl EnvError {
    /// Stable `error.type` for `--json` output (§5, §14).
    pub fn kind(&self) -> &'static str {
        "env"
    }
}

/// `HOME` as tagteam may use it (§5): set, non-empty and absolute. Every default path derives
/// from it, so a fallback would put state under `/` or the working directory.
pub fn home_from(value: Option<OsString>) -> Result<PathBuf, EnvError> {
    let value = value.ok_or(EnvError::HomeUnset)?;
    if value.is_empty() {
        return Err(EnvError::HomeEmpty);
    }
    let home = PathBuf::from(value);
    if !home.is_absolute() {
        return Err(EnvError::HomeRelative(home.to_string_lossy().into_owned()));
    }
    Ok(home)
}
```

Replace `Env::from_process` (lines 20–38):

```rust
    pub fn from_process() -> Self {
        let abs = |k: &str| {
            std::env::var_os(k)
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
        };
        Self {
            home: std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| "/".into()),
```

with:

```rust
    /// The process's environment. Fails when `HOME` cannot place tagteam's files (§5).
    pub fn from_process() -> Result<Self, EnvError> {
        let abs = |k: &str| {
            std::env::var_os(k)
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
        };
        Ok(Self {
            home: home_from(std::env::var_os("HOME"))?,
```

and, at the end of the same function, replace:

```rust
            forbidden_root: None,
        }
    }
```

with:

```rust
            forbidden_root: None,
        })
    }
```

That is the first `forbidden_root: None,` in the file. `for_test` sets `forbidden_root:
Some(real_home)`, so the match is unique. On the merged code, M3a's `cancel` line and M4a's
`vars` line stay between `claude_securestorage_config_dir` and `forbidden_root`.

**`crates/tagteam-provider/src/lib.rs`.** Replace `pub use env::Env;` with:

```rust
pub use env::{Env, EnvError};
```

**`crates/tagteam/src/app.rs`.** Replace the import line
`use tagteam_provider::{Clock, Env, Keychain, LockState, SystemClock};` with:

```rust
use tagteam_provider::{Clock, Env, EnvError, Keychain, LockState, SystemClock};
```

Replace `Context::from_process` (lines 120–133):

```rust
impl Context {
    pub fn from_process() -> Self {
        let o = test_overrides(&|k| std::env::var_os(k));
        Self {
            env: Env::from_process(),
            keychain: o.keychain.unwrap_or_else(|| Arc::new(SecurityCli::new())),
            platform: o.platform.unwrap_or_else(Platform::current),
            api_base: o.api_base,
            stdout_terminal: std::io::stdout().is_terminal(),
            no_color_env: env_flag(NO_COLOR),
            force_color_env: env_flag(FORCE_COLOR),
        }
    }
}
```

with:

```rust
impl Context {
    /// The binary's context, read from the process. Fails when `HOME` cannot place tagteam's
    /// files (§5), before anything else is read.
    pub fn from_process() -> Result<Self, EnvError> {
        let env = Env::from_process()?;
        let o = test_overrides(&|k| std::env::var_os(k));
        Ok(Self {
            env,
            keychain: o.keychain.unwrap_or_else(|| Arc::new(SecurityCli::new())),
            platform: o.platform.unwrap_or_else(Platform::current),
            api_base: o.api_base,
            stdout_terminal: std::io::stdout().is_terminal(),
            no_color_env: env_flag(NO_COLOR),
            force_color_env: env_flag(FORCE_COLOR),
        })
    }
}
```

**`crates/tagteam/src/lib.rs`.** Before `/// Runs the CLI and returns the process exit code.` (line 45), insert:

```rust
/// The status bar's own call: `statusline` printing its line (§13.5). Only this call drains
/// stdin, and only this call answers an unusable `HOME` with silence (§5), because a status bar
/// has nowhere to show an error. `--print-config` and the refused `--json` print text that a
/// person reads.
fn status_bar_line(cli: &cli::Cli) -> bool {
    matches!(
        cli.command,
        Some(cli::Command::Statusline {
            print_config: false
        })
    ) && !cli.json
}

```

In `main_with_args`, replace everything from the drain's `if` to the end of the function (lines
79–99):

```rust
    if matches!(
        cli.command,
        Some(cli::Command::Statusline {
            print_config: false
        })
    ) && !cli.json
    {
        let stdin = std::io::stdin();
        statusline::drain(stdin.lock(), stdin.is_terminal());
    }
    let mut prompter = prompt::TtyPrompter;
    let (mut out, mut err) = (std::io::stdout().lock(), std::io::stderr().lock());
    app::run(
        cli,
        app::Context::from_process(),
        &mut app::Io {
            out: &mut out,
            err: &mut err,
            prompter: &mut prompter,
        },
    )
```

with:

```rust
    if status_bar_line(&cli) {
        let stdin = std::io::stdin();
        statusline::drain(stdin.lock(), stdin.is_terminal());
    }
    // §5: every default path derives from `HOME`, so an unusable one refuses the command here,
    // before any engine exists, and nothing is created under `/` or the working directory.
    let ctx = match app::Context::from_process() {
        Ok(ctx) => ctx,
        Err(_) if status_bar_line(&cli) => return 0,
        Err(e) => {
            if cli.json {
                let error = app::error_json(e.kind(), &e.to_string());
                let _ = writeln!(std::io::stdout(), "{error}");
            } else {
                let _ = writeln!(std::io::stderr(), "tagteam: {e}");
            }
            return app::EXIT_ERROR;
        }
    };
    let mut prompter = prompt::TtyPrompter;
    let (mut out, mut err) = (std::io::stdout().lock(), std::io::stderr().lock());
    app::run(
        cli,
        ctx,
        &mut app::Io {
            out: &mut out,
            err: &mut err,
            prompter: &mut prompter,
        },
    )
```

The drain's comment above the `if` (lines 74–78) stays as it is.

- [ ] **Step 4: Run the tests and see them pass**

Run: `cargo test -p tagteam-provider --lib env::tests`. Expected: PASS (seven tests).

Run: `cargo test -p tagteam --features test-support --test home_cli`. Expected: PASS (three
tests).

Run: `cargo test -p tagteam-provider` and `cargo test -p tagteam --features test-support`.
Expected: PASS. Every other binary test sets `HOME` through `common::std_cmd`, and the
in-process tests build their `Context` by hand.

- [ ] **Step 5: Format, lint, commit**

```
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p tagteam-provider && cargo test -p tagteam --features test-support
git add crates/tagteam-provider/src/env.rs crates/tagteam-provider/src/lib.rs \
  crates/tagteam/src/app.rs crates/tagteam/src/lib.rs crates/tagteam/tests/home_cli.rs
git commit -m "Refuse to run when HOME is unset, empty or relative"
```

---

### Task 2: The settings registry and the reader for every key

> **Re-sync.**
> - **M4a Task 13a.** Written against `settings.rs` and `tests/settings.rs` as 13a leaves them. Step 3 replaces the whole of `settings.rs`. The new file keeps, word for word, 13a's `Settings.share_extra` and its doc, `is_share_name` (same body, now `pub`, since `config set` validates with it), and both `run.share_extra` warning texts; `Reader::share_extra` becomes the registry's `ShareNames` kind. M4a's `profiles.rs` (`use crate::settings::is_share_name;`) and its `sharing()` fixture (`..Settings::default()`) compile unchanged. If 13a landed differently from its plan, carry its differences into the new file.
> - **M3a Task 2.** `kind()` then holds `EngineError::Lock(LockError::Interrupted { .. }) => "interrupted"`. Step 8 replaces all three `EngineError::Lock` arms with the one `lock_kind` arm, so add `LockError::Interrupted { .. } => "interrupted",` to `lock_kind` (its exhaustive `match` will not compile until it is there). Add `LockError::Interrupted { path: PathBuf::from("x"), signal: 2 }` to both lists in `the_settings_lock_has_the_engine_lock_s_kinds`. `EngineError::signal()` is Task 4's to extend (its Re-sync note).
> - **M3a and M4a error variants** (`Interrupted`, `ProfileSplit`, `RunShellUnreadable`, …) stay where they are: `Settings` goes immediately before the `Io` variant, and its kind arms immediately before `EngineError::Io(_) => "io"`.

§6.4: "**One registry.** Every key in the table below is declared once, with its type, default, valid values and whether a provider table may override it. Reads, `set` and `unset`, `config list`, `doctor` (§13.6) and completions (§13.7) all use that declaration, so none of them accepts a key or value that another rejects." "**Reads are forgiving.** A corrupt file or an invalid value falls back to the default, with a warning. An unknown key is ignored; `config list` and `doctor` report it." "**Values on the command line.** Numbers are written as typed. **Booleans** parse only `true/false/1/0/yes/no` and are written as TOML booleans. **Lists** are comma-separated; each item is trimmed, and an empty item is refused. An empty argument (`''`) writes an empty list, which is how a provider table overrides a non-empty global list. `all` in `autoswitch.models` must stand alone." "`provider.<id>.<key>` and `<key> --provider <id>` name the same entry in every `config` command." This task declares every §6.4 key once, in `KEYS`, with its kind, its "must be …" phrase and its per-provider flag. One per-key reader serves both `Settings::load` and the new `inspect`, which reports each key's value, default and source and the keys the file holds that the registry does not know (Decision 1). `Key::parse_arg` is the strict command-line parser that Task 4's `set` uses, and `resolve` maps a key's two spellings to one entry. `SettingsError` reaches the CLI as `EngineError::Settings`, with Decision 4's kinds. The reader learns `default_provider` and the six `autoswitch` keys M2b skipped; nothing in M5a acts on the six yet (M3b's `auto` is their first reader).

**Readings of the spec this task commits to:**
- **A boolean in the file is a TOML boolean.** §6.4 writes booleans "as TOML booleans"; the six words are the command line's spelling. A string (`"yes"`, `"true"`) or an integer in the file warns and falls back like any invalid value. *Superseded at the re-sync by M3b's merged reader:* a TOML boolean, an integer `1` or `0`, or one of the six words as a string all read; `a_boolean_key_reads_a_toml_boolean_and_nothing_else` takes M3b's cases ("M3b" under the interfaces list).
- **A float key reads a TOML integer** (`hysteresis_pct = 5` is 5.0), as `autoswitch.threshold` always has; an integer key refuses a float (`60.0`), as `usage.history_retention_days` always has.
- **`default_provider` is read from the top level only,** whichever provider reads the file. The reader checks only that it is a well-formed provider id: 1 to 64 lowercase ASCII letters, digits and dashes, with no dash at either end. Whether this build has the provider is the CLI's check (Decision 2, Task 3). A `default_provider` inside a provider table is an unknown key.
- **A key's source is the table its value came from.** An invalid provider value that falls through to a valid global value is `global`; nothing valid anywhere is `default`.
- **Unknown keys are reported by dotted name, sorted.** Sorting gives one order however the file interleaves its tables. Under `provider.<id>`, for any id, a key is known exactly when a provider table may override it, so `provider.claude-code.ui.color` is unknown, as the spec's own example has it. Inside an unknown table every key is named (`extra.anything`), and an unknown entry with no key in it is named itself. A known table name holding something that is not a table (`autoswitch = 5`) is the reader's warning, not an unknown key. An unknown key never warns.
- **A number on the command line** is whatever Rust's `f64` or `i64` parser takes (so `1e1` and `+5` too), then must be finite and in range. A float key typed as `80` is the value 80.0.
- **A list on the command line:** `''` is the empty list, but `' '` is one empty item and is refused. Repeats collapse there as in the file: model names ignoring case, entry names exactly.
- **`run.share_extra` on the command line also refuses `.tagteam-*`,** which §6.4 lists among the things an entry name must not be. In the file, 13a's rule stands: such a name is read, and the link sync drops it with a warning. The known-private check needs the provider's share policy, so it is Task 4's.
- **`config get` prints a float in Rust's shortest form** (`80` for 80.0), which `config set` takes back unchanged. JSON shows `80.0`.
- **`resolve` refuses `--provider` with a key no provider table can hold** (`ui.color --provider claude-code`), for reads too, because both spellings name the entry `provider.claude-code.ui.color`, which `set` refuses.
- **A key's default is `Settings::default()` read back through `Settings::value`,** so the defaults stay declared once.
- **Warnings now come in registry order.** Every existing test counts warnings or looks one up, so none depends on the old order.

**Files:**
- **2a:**
  - Modify: `crates/tagteam-engine/src/settings.rs` (the whole file, as M4a Task 13a leaves it; 311 lines on 3c1f458)
  - Test: `crates/tagteam-engine/tests/settings.rs` (imports :1-7; `the_defaults_are_the_specs_table` :28-39; `every_key_is_read_from_a_full_file` :61-90; `keys_this_milestone_does_not_read_are_ignored_without_a_warning` :100-107; new tests at the end, after 13a's)
- **2b:**
  - Modify: `crates/tagteam-engine/src/settings.rs` (the `tagteam_provider` import; new `Resolved`, `resolve`, `SettingsError` after `inspect`)
  - Modify: `crates/tagteam-engine/src/error.rs` (imports :4-7; the `Settings` variant before `Io`, :105-106; `kind()` :111-144; new `lock_kind`; the pin test :153-277 and one new test)
  - Test: `crates/tagteam-engine/tests/settings.rs` (imports; new tests at the end)

**Interfaces:**
- Consumes:
  - Existing: `Env::config_dir`, `tagteam_provider::LockError`, `toml_edit::{DocumentMut, Item, TableLike}`, `Settings::load`, `is_statusline_placeholder`, `STATUSLINE_PLACEHOLDERS`, `STATUSLINE_MODEL_PREFIX`, the `Reader`'s `table` and `warn`.
  - M4a Task 13a: `Settings.share_extra`, `is_share_name`, its `share_extra` tests.
- Produces (in `tagteam_engine::settings`):
  - `pub enum KeyKind { Float { min: f64, max: f64 }, Int { min: i64, max: i64 }, Bool, Choice(&'static [&'static str]), Models, Format, ShareNames, Provider }` (`Debug, Clone, Copy, PartialEq`)
  - `pub struct Key { pub name: &'static str, pub kind: KeyKind, pub per_provider: bool }` (`Debug, Clone, Copy, PartialEq`), with `pub fn table(&self) -> Option<&'static str>`, `pub fn leaf(&self) -> &'static str`, `pub fn expect(&self) -> String`, `pub fn parse_item(&self, item: &toml_edit::Item, warn: &mut dyn FnMut(String)) -> Option<Value>` and `pub fn parse_arg(&self, raw: &str) -> Result<Value, String>`
  - `pub const KEYS: &[Key]` (§6.4's table order) and `pub fn key(name: &str) -> Option<&'static Key>`
  - `pub enum Value { Float(f64), Int(i64), Bool(bool), Str(String), List(Vec<String>) }` (`Debug, Clone, PartialEq`), with `pub fn to_item(&self) -> toml_edit::Item`, `pub fn display(&self) -> String` and `pub fn to_json(&self) -> serde_json::Value`
  - `pub enum Source { Default, Global, Provider }` (`Debug, Clone, Copy, PartialEq, Eq`), with `pub fn as_str(self) -> &'static str`
  - `pub struct KeyState { pub key: &'static Key, pub value: Value, pub default: Value, pub source: Source }` and `pub struct Inspection { pub path: PathBuf, pub exists: bool, pub keys: Vec<KeyState>, pub unknown: Vec<String>, pub warnings: Vec<String> }` (both `Debug, Clone, PartialEq`)
  - `pub fn inspect(env: &Env, provider: &ProviderId) -> Inspection` and `pub fn config_path(env: &Env) -> PathBuf`
  - `pub enum AutoStrategy { Best, ConsumeFirst }` and `ColorMode`, each with `pub fn as_str(self) -> &'static str`
  - `pub fn is_share_name(name: &str) -> bool` (13a's, made `pub`)
  - `Settings` gains `default_provider: ProviderId`, `interval_seconds: u32`, `cooldown_seconds: u32`, `hysteresis_pct: f64`, `strategy: AutoStrategy`, `include_api_key_accounts: bool`, `unhealthy_ticks: u32`, and `pub fn value(&self, key: &Key) -> Value`; `pub const DEFAULT_INTERVAL_SECONDS: u32`, `DEFAULT_COOLDOWN_SECONDS: u32`, `DEFAULT_HYSTERESIS_PCT: f64`, `DEFAULT_UNHEALTHY_TICKS: u32`
  - 2b: `pub struct Resolved { pub key: &'static Key, pub provider: Option<ProviderId> }` (`Debug, Clone, PartialEq`); `pub fn resolve(name: &str, flag: Option<&ProviderId>) -> Result<Resolved, SettingsError>`; `pub enum SettingsError { UnknownKey(String), NotPerProvider(String), ProviderMismatch { prefix: String, flag: String }, Invalid { key: String, reason: String }, UnknownProvider(String), Corrupt { path: PathBuf, detail: String }, Lock(#[from] LockError), Io(#[from] std::io::Error) }`
  - 2b, in `tagteam_engine::error`: `EngineError::Settings(#[from] SettingsError)`, with kinds `invalid-input` (UnknownKey, NotPerProvider, ProviderMismatch, Invalid, UnknownProvider), `settings-unreadable` (Corrupt), the engine lock's kind (Lock) and `io` (Io).

#### 2a: The registry and the reader

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-engine/tests/settings.rs`, replace the imports (today :1-7) with:

```rust
use std::fs;

use serde_json::json;
use tagteam_core::ProviderId;
use tagteam_engine::settings::{
    self, AutoStrategy, ColorMode, Inspection, KEYS, Key, KeyKind, KeyState,
    STATUSLINE_PLACEHOLDERS, Settings, Source, Value, inspect, is_statusline_placeholder,
};
use tagteam_provider::Env;
```

Replace `the_defaults_are_the_specs_table` (as 13a leaves it, with its `share_extra` assertion) with:

```rust
#[test]
fn the_defaults_are_the_specs_table() {
    let d = Settings::default();
    assert_eq!(d.default_provider, ProviderId::new("claude-code"));
    assert_eq!(d.threshold, 90.0);
    assert_eq!(d.interval_seconds, 60);
    assert_eq!(d.cooldown_seconds, 300);
    assert_eq!(d.hysteresis_pct, 10.0);
    assert_eq!(d.strategy, AutoStrategy::Best);
    assert!(!d.include_api_key_accounts);
    assert_eq!(d.unhealthy_ticks, 3);
    assert_eq!(d.models, Vec::<String>::new());
    assert_eq!(d.history_retention_days, 180);
    assert_eq!(
        d.statusline_format,
        "{account} · 5h {5h}% · 7d {7d}%{stale}"
    );
    assert_eq!(d.color, ColorMode::Auto);
    assert!(d.share_extra.is_empty());
}
```

Replace `every_key_is_read_from_a_full_file` (13a's version, ending in `share_extra: vec!["hook-data".into()],`). Its full `Settings` literal stops compiling once the struct gains fields, and it now reads every key:

```rust
#[test]
fn every_key_is_read_from_a_full_file() {
    let (settings, warnings) = load(
        r#"
default_provider = "fake-agent"

[autoswitch]
threshold = 75.5
interval_seconds = 120
cooldown_seconds = 0
hysteresis_pct = 12.5
strategy = "consume-first"
include_api_key_accounts = true
unhealthy_ticks = 5
models = ["Fable", "Opus"]

[usage]
history_retention_days = 30

[statusline]
format = "{5h}%"

[ui]
color = "never"

[run]
share_extra = ["hook-data"]
"#,
    );
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(
        settings,
        Settings {
            default_provider: ProviderId::new("fake-agent"),
            threshold: 75.5,
            interval_seconds: 120,
            cooldown_seconds: 0,
            hysteresis_pct: 12.5,
            strategy: AutoStrategy::ConsumeFirst,
            include_api_key_accounts: true,
            unhealthy_ticks: 5,
            models: models(&["Fable", "Opus"]),
            history_retention_days: 30,
            statusline_format: "{5h}%".to_owned(),
            color: ColorMode::Never,
            share_extra: vec!["hook-data".into()],
        }
    );
}
```

Replace `keys_this_milestone_does_not_read_are_ignored_without_a_warning` (today :100-107) with the test below. It asserted that `default_provider`, `autoswitch.interval_seconds` and `autoswitch.strategy` are ignored, and reading them is this task's point. The rule it still pins, that an unknown key is ignored without a warning, stays, and the three keys join `every_key_is_read_from_a_full_file`:

```rust
#[test]
fn an_unknown_key_is_ignored_without_a_warning() {
    let (settings, warnings) =
        load("colour = \"never\"\n[autoswitch]\nfuture = true\n[elsewhere]\nx = 1\n");
    assert_eq!(settings, Settings::default());
    assert!(warnings.is_empty(), "{warnings:?}");
}
```

Append at the end of the file, after 13a's `a_share_extra_that_is_neither_a_name_nor_a_list_warns_and_the_next_table_applies`:

```rust
/// `inspect` of `text` as the `config.toml` of a fresh environment.
fn inspect_as(text: &str, provider: &str) -> Inspection {
    let dir = tempfile::tempdir().unwrap();
    let env = Env::for_test(dir.path());
    fs::create_dir_all(env.config_dir()).unwrap();
    fs::write(settings::config_path(&env), text).unwrap();
    inspect(&env, &ProviderId::new(provider))
}

/// The registry's key `name`.
fn reg(name: &str) -> &'static Key {
    settings::key(name).unwrap_or_else(|| panic!("no registry key {name}"))
}

/// `name`'s line in `inspection`.
fn state<'a>(inspection: &'a Inspection, name: &str) -> &'a KeyState {
    inspection
        .keys
        .iter()
        .find(|s| s.key.name == name)
        .unwrap_or_else(|| panic!("{name} is not inspected"))
}

fn list(names: &[&str]) -> Value {
    Value::List(models(names))
}

#[test]
fn the_registry_is_section_six_four_s_table_in_order() {
    let rows: Vec<(&str, KeyKind, bool)> = KEYS
        .iter()
        .map(|k| (k.name, k.kind, k.per_provider))
        .collect();
    assert_eq!(
        rows,
        vec![
            ("default_provider", KeyKind::Provider, false),
            (
                "autoswitch.threshold",
                KeyKind::Float {
                    min: 50.0,
                    max: 99.9
                },
                true
            ),
            (
                "autoswitch.interval_seconds",
                KeyKind::Int { min: 15, max: 3600 },
                true
            ),
            (
                "autoswitch.cooldown_seconds",
                KeyKind::Int {
                    min: 0,
                    max: 86_400
                },
                true
            ),
            (
                "autoswitch.hysteresis_pct",
                KeyKind::Float {
                    min: 0.0,
                    max: 50.0
                },
                true
            ),
            (
                "autoswitch.strategy",
                KeyKind::Choice(&["best", "consume-first"]),
                true
            ),
            ("autoswitch.include_api_key_accounts", KeyKind::Bool, true),
            (
                "autoswitch.unhealthy_ticks",
                KeyKind::Int { min: 1, max: 100 },
                true
            ),
            ("autoswitch.models", KeyKind::Models, true),
            (
                "usage.history_retention_days",
                KeyKind::Int { min: 1, max: 3650 },
                false
            ),
            ("statusline.format", KeyKind::Format, true),
            ("run.share_extra", KeyKind::ShareNames, true),
            (
                "ui.color",
                KeyKind::Choice(&["auto", "always", "never"]),
                false
            ),
        ]
    );
}

#[test]
fn a_key_is_found_by_its_dotted_name_and_splits_into_table_and_leaf() {
    for k in KEYS {
        assert_eq!(settings::key(k.name), Some(k));
    }
    assert_eq!(reg("autoswitch.threshold").table(), Some("autoswitch"));
    assert_eq!(reg("autoswitch.threshold").leaf(), "threshold");
    assert_eq!(reg("run.share_extra").table(), Some("run"));
    assert_eq!(reg("default_provider").table(), None);
    assert_eq!(reg("default_provider").leaf(), "default_provider");
    for name in [
        "",
        "threshold",
        "autoswitch",
        "Autoswitch.threshold",
        "autoswitch.threshold ",
        "provider.claude-code.autoswitch.threshold",
    ] {
        assert_eq!(settings::key(name), None, "{name:?}");
    }
}

#[test]
fn every_key_reads_back_its_default_from_the_settings() {
    let d = Settings::default();
    let defaults: Vec<(&str, Value)> = KEYS.iter().map(|k| (k.name, d.value(k))).collect();
    assert_eq!(
        defaults,
        vec![
            ("default_provider", Value::Str("claude-code".into())),
            ("autoswitch.threshold", Value::Float(90.0)),
            ("autoswitch.interval_seconds", Value::Int(60)),
            ("autoswitch.cooldown_seconds", Value::Int(300)),
            ("autoswitch.hysteresis_pct", Value::Float(10.0)),
            ("autoswitch.strategy", Value::Str("best".into())),
            ("autoswitch.include_api_key_accounts", Value::Bool(false)),
            ("autoswitch.unhealthy_ticks", Value::Int(3)),
            ("autoswitch.models", list(&[])),
            ("usage.history_retention_days", Value::Int(180)),
            (
                "statusline.format",
                Value::Str("{account} · 5h {5h}% · 7d {7d}%{stale}".into())
            ),
            ("run.share_extra", list(&[])),
            ("ui.color", Value::Str("auto".into())),
        ]
    );
}

#[test]
fn each_key_says_what_a_valid_value_is() {
    let expect = |name: &str| reg(name).expect();
    assert_eq!(
        expect("default_provider"),
        "must be a provider id of lowercase letters, digits and dashes, such as \"claude-code\""
    );
    assert_eq!(
        expect("autoswitch.threshold"),
        "must be a number from 50 to 99.9"
    );
    assert_eq!(
        expect("autoswitch.interval_seconds"),
        "must be a whole number of seconds from 15 to 3600"
    );
    assert_eq!(
        expect("autoswitch.cooldown_seconds"),
        "must be a whole number of seconds from 0 to 86400"
    );
    assert_eq!(
        expect("autoswitch.hysteresis_pct"),
        "must be a number from 0 to 50"
    );
    assert_eq!(
        expect("autoswitch.strategy"),
        "must be \"best\" or \"consume-first\""
    );
    assert_eq!(
        expect("autoswitch.include_api_key_accounts"),
        "must be true or false"
    );
    assert_eq!(
        expect("autoswitch.unhealthy_ticks"),
        "must be a whole number from 1 to 100"
    );
    assert_eq!(
        expect("autoswitch.models"),
        "must be a model name, a list of model names, or [\"all\"] alone"
    );
    assert_eq!(
        expect("usage.history_retention_days"),
        "must be a whole number of days from 1 to 3650"
    );
    assert!(
        expect("statusline.format")
            .starts_with("must be a non-empty string using only the placeholders {account}, ")
    );
    assert_eq!(
        expect("run.share_extra"),
        "must be an entry name or a list of entry names"
    );
    assert_eq!(
        expect("ui.color"),
        "must be \"auto\", \"always\" or \"never\""
    );
}

/// The numbers at a numeric kind's bounds, and just outside them, as TOML.
fn bounds(kind: KeyKind) -> (Vec<String>, Vec<String>) {
    match kind {
        KeyKind::Float { min, max } => (
            vec![format!("{min:?}"), format!("{max:?}")],
            vec![format!("{:?}", min - 0.1), format!("{:?}", max + 0.1)],
        ),
        KeyKind::Int { min, max } => (
            vec![min.to_string(), max.to_string()],
            vec![(min - 1).to_string(), (max + 1).to_string()],
        ),
        _ => (Vec::new(), Vec::new()),
    }
}

#[test]
fn every_numeric_key_reads_its_bounds_and_defaults_just_outside_them() {
    // §15.2: every registry key's bounds are defaulted by reads. The command line agrees.
    let mut numeric = 0;
    for key in KEYS {
        let (inside, outside) = bounds(key.kind);
        if inside.is_empty() {
            continue;
        }
        numeric += 1;
        let text = |v: &str| format!("[{}]\n{} = {v}\n", key.table().unwrap(), key.leaf());
        for v in &inside {
            let i = inspect_as(&text(v), PROVIDER);
            let s = state(&i, key.name);
            assert_eq!(
                (&s.value, s.source),
                (&key.parse_arg(v).unwrap(), Source::Global),
                "{}: {v}",
                key.name
            );
            assert!(i.warnings.is_empty(), "{}: {v}: {:?}", key.name, i.warnings);
        }
        for v in &outside {
            let i = inspect_as(&text(v), PROVIDER);
            let s = state(&i, key.name);
            assert_eq!(
                (&s.value, s.source),
                (&s.default, Source::Default),
                "{}: {v}",
                key.name
            );
            assert_eq!(i.warnings.len(), 1, "{}: {v}: {:?}", key.name, i.warnings);
            assert!(
                i.warnings[0].contains(&format!("`{}` {} (ignored)", key.name, key.expect())),
                "{:?}",
                i.warnings
            );
            assert_eq!(key.parse_arg(v), Err(key.expect()), "{}: {v}", key.name);
        }
    }
    assert_eq!(
        numeric, 6,
        "threshold, three seconds and ticks, hysteresis, retention"
    );
}

#[test]
fn the_auto_switch_keys_are_read_from_the_provider_s_table_first() {
    let text = "[autoswitch]\ninterval_seconds = 120\ncooldown_seconds = 600\nhysteresis_pct = 5\n\
                strategy = \"best\"\ninclude_api_key_accounts = false\nunhealthy_ticks = 2\n\n\
                [provider.claude-code.autoswitch]\ninterval_seconds = 30\nstrategy = \"consume-first\"\n\
                include_api_key_accounts = true\n";
    let (s, warnings) = load(text);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(
        (
            s.interval_seconds,
            s.cooldown_seconds,
            s.hysteresis_pct,
            s.strategy,
            s.include_api_key_accounts,
            s.unhealthy_ticks
        ),
        (30, 600, 5.0, AutoStrategy::ConsumeFirst, true, 2),
        "a whole number reads as a float key's value"
    );
    let (s, _) = load_as(text, "fake-agent");
    assert_eq!(
        (s.interval_seconds, s.strategy, s.include_api_key_accounts),
        (120, AutoStrategy::Best, false),
        "another provider's table does not apply"
    );
}

#[test]
fn a_boolean_key_reads_a_toml_boolean_and_nothing_else() {
    for (text, want) in [("true", true), ("false", false)] {
        let (s, warnings) = load(&format!(
            "[autoswitch]\ninclude_api_key_accounts = {text}\n"
        ));
        assert_eq!(s.include_api_key_accounts, want);
        assert!(warnings.is_empty(), "{warnings:?}");
    }
    // Each would read as false if it were taken; the global `true` applies instead.
    for text in ["\"no\"", "\"false\"", "0", "\"\"", "[false]"] {
        let (s, warnings) = load(&format!(
            "[autoswitch]\ninclude_api_key_accounts = true\n\
             [provider.claude-code.autoswitch]\ninclude_api_key_accounts = {text}\n"
        ));
        assert!(s.include_api_key_accounts, "{text}");
        assert_eq!(warnings.len(), 1, "{text}: {warnings:?}");
        assert!(
            warnings[0].contains(
                "`provider.claude-code.autoswitch.include_api_key_accounts` must be true or false"
            ),
            "{warnings:?}"
        );
    }
}

#[test]
fn the_strategy_and_the_colour_read_their_values_exactly() {
    for text in ["\"Best\"", "\"consume_first\"", "\"next-available\"", "1"] {
        let (s, warnings) = load(&format!(
            "[autoswitch]\nstrategy = \"consume-first\"\n\
             [provider.claude-code.autoswitch]\nstrategy = {text}\n"
        ));
        assert_eq!(s.strategy, AutoStrategy::ConsumeFirst, "{text}");
        assert_eq!(warnings.len(), 1, "{text}: {warnings:?}");
        assert!(
            warnings[0].contains(
                "`provider.claude-code.autoswitch.strategy` must be \"best\" or \"consume-first\""
            ),
            "{warnings:?}"
        );
    }
    let i = inspect_as("[provider.claude-code.ui]\ncolor = \"never\"\n", PROVIDER);
    assert_eq!(
        state(&i, "ui.color").source,
        Source::Default,
        "ui.color has no provider table"
    );
}

#[test]
fn default_provider_is_read_from_the_top_level_alone() {
    for reader in [PROVIDER, "fake-agent"] {
        let (s, warnings) = load_as("default_provider = \"fake-agent\"\n", reader);
        assert_eq!(
            s.default_provider,
            ProviderId::new("fake-agent"),
            "{reader}"
        );
        assert!(warnings.is_empty(), "{warnings:?}");
    }
    let i = inspect_as(
        "[provider.claude-code]\ndefault_provider = \"fake-agent\"\n",
        PROVIDER,
    );
    let s = state(&i, "default_provider");
    assert_eq!(
        (&s.value, s.source),
        (&Value::Str("claude-code".into()), Source::Default)
    );
    assert_eq!(i.unknown, ["provider.claude-code.default_provider"]);
}

#[test]
fn unknown_keys_are_sorted_by_dotted_name_however_the_tables_interleave() {
    let i = inspect_as(
        "[provider.claude-code.autoswitch]\nzeta = 1\n\
         [ui]\nalpha = 2\n\
         [provider.claude-code.statusline]\nbeta = 3\n",
        PROVIDER,
    );
    assert_eq!(
        i.unknown,
        [
            "provider.claude-code.autoswitch.zeta",
            "provider.claude-code.statusline.beta",
            "ui.alpha",
        ]
    );
}

#[test]
fn a_malformed_default_provider_falls_back_to_claude_code_with_a_warning() {
    for text in [
        "\"Claude Code\"",
        "\"claude_code\"",
        "\"\"",
        "\"-x\"",
        "5",
        "[\"claude-code\"]",
    ] {
        let (s, warnings) = load(&format!("default_provider = {text}\n"));
        assert_eq!(s.default_provider, ProviderId::new("claude-code"), "{text}");
        assert_eq!(warnings.len(), 1, "{text}: {warnings:?}");
        assert!(
            warnings[0].contains("`default_provider` must be a provider id"),
            "{warnings:?}"
        );
    }
}

#[test]
fn an_empty_provider_list_overrides_a_non_empty_global_one() {
    // Review Focus 2, as a read: `[]` in the provider's table is a value, and it wins.
    let i = inspect_as(
        "[autoswitch]\nmodels = [\"Opus\"]\n[run]\nshare_extra = [\"hook-data\"]\n\n\
         [provider.claude-code.autoswitch]\nmodels = []\n[provider.claude-code.run]\nshare_extra = []\n",
        PROVIDER,
    );
    assert!(i.warnings.is_empty(), "{:?}", i.warnings);
    for name in ["autoswitch.models", "run.share_extra"] {
        let s = state(&i, name);
        assert_eq!(
            (&s.value, s.source),
            (&list(&[]), Source::Provider),
            "{name}"
        );
    }
}

#[test]
fn inspect_reports_each_key_s_value_default_and_source() {
    let text = "[autoswitch]\nthreshold = 80\nmodels = [\"Opus\"]\n\n\
                [provider.claude-code.autoswitch]\nthreshold = 70\n";
    let i = inspect_as(text, PROVIDER);
    assert!(i.exists);
    assert_eq!(
        i.keys.iter().map(|s| s.key.name).collect::<Vec<_>>(),
        KEYS.iter().map(|k| k.name).collect::<Vec<_>>(),
        "every key, in the registry's order"
    );
    let threshold = state(&i, "autoswitch.threshold");
    assert_eq!(
        (&threshold.value, &threshold.default, threshold.source),
        (&Value::Float(70.0), &Value::Float(90.0), Source::Provider)
    );
    let models = state(&i, "autoswitch.models");
    assert_eq!(
        (&models.value, models.source),
        (&list(&["Opus"]), Source::Global)
    );
    let color = state(&i, "ui.color");
    assert_eq!(
        (&color.value, color.source),
        (&Value::Str("auto".into()), Source::Default)
    );
    let other = inspect_as(text, "fake-agent");
    let threshold = state(&other, "autoswitch.threshold");
    assert_eq!(
        (&threshold.value, threshold.source),
        (&Value::Float(80.0), Source::Global)
    );
}

#[test]
fn inspect_and_load_read_every_key_alike() {
    let dir = tempfile::tempdir().unwrap();
    let env = Env::for_test(dir.path());
    fs::create_dir_all(env.config_dir()).unwrap();
    fs::write(
        settings::config_path(&env),
        "default_provider = \"fake-agent\"\n[autoswitch]\nthreshold = 120\ninterval_seconds = 45\n\
         models = \"Fable\"\n[provider.claude-code.autoswitch]\nstrategy = \"consume-first\"\n\
         [provider.claude-code.statusline]\nformat = \"{7d}\"\n[ui]\ncolor = \"never\"\n",
    )
    .unwrap();
    let provider = ProviderId::new(PROVIDER);
    let (loaded, warnings) = Settings::load(&env, &provider);
    let i = inspect(&env, &provider);
    assert_eq!(i.warnings, warnings);
    for s in &i.keys {
        assert_eq!(s.value, loaded.value(s.key), "{}", s.key.name);
    }
}

#[test]
fn inspect_lists_the_keys_the_registry_does_not_know_sorted() {
    let i = inspect_as(
        r#"
colour = "never"
default_provider = "claude-code"

[autoswitch]
threshold = 80
thresold = 80

[usage]
retention = 30

[extra]
anything = 1

[ui]
color = "never"
theme = "dark"

[provider.claude-code]
default_provider = "fake-agent"

[provider.claude-code.autoswitch]
models = ["Fable"]
future = 1

[provider.claude-code.ui]
color = "always"

[provider.fake-agent.statusline]
format = "{5h}"

[provider.fake-agent.run]
share_extra = ["x"]

[provider.fake-agent.usage]
history_retention_days = 3
"#,
        PROVIDER,
    );
    assert_eq!(
        i.unknown,
        [
            "autoswitch.thresold",
            "colour",
            "extra.anything",
            "provider.claude-code.autoswitch.future",
            "provider.claude-code.default_provider",
            "provider.claude-code.ui.color",
            "provider.fake-agent.usage.history_retention_days",
            "ui.theme",
            "usage.retention",
        ]
    );
    assert!(
        i.warnings.is_empty(),
        "an unknown key is no warning: {:?}",
        i.warnings
    );
    assert_eq!(
        state(&i, "ui.color").value,
        Value::Str("never".into()),
        "the provider's ui table is not read"
    );
}

#[test]
fn a_known_table_of_the_wrong_type_warns_and_is_not_unknown() {
    let i = inspect_as(
        "autoswitch = 5\n[[usage]]\nhistory_retention_days = 3\n",
        PROVIDER,
    );
    assert!(i.unknown.is_empty(), "{:?}", i.unknown);
    assert_eq!(i.warnings.len(), 2, "{:?}", i.warnings);
}

#[test]
fn inspecting_a_missing_file_gives_every_default_and_creates_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let env = Env::for_test(dir.path());
    let i = inspect(&env, &ProviderId::new(PROVIDER));
    assert_eq!(i.path, env.config_dir().join("config.toml"));
    assert_eq!(settings::config_path(&env), i.path);
    assert!(!i.exists);
    assert!(
        i.keys
            .iter()
            .all(|s| s.source == Source::Default && s.value == s.default)
    );
    assert!(i.unknown.is_empty() && i.warnings.is_empty());
    assert!(!env.config_dir().exists(), "inspecting creates nothing");
}

#[test]
fn inspecting_a_corrupt_file_gives_every_default_and_one_warning() {
    let i = inspect_as("[autoswitch\nthreshold = = 3\n", PROVIDER);
    assert!(i.exists);
    assert!(i.keys.iter().all(|s| s.source == Source::Default));
    assert!(i.unknown.is_empty());
    assert_eq!(i.warnings.len(), 1, "{:?}", i.warnings);
    assert!(i.warnings[0].contains("not valid TOML"), "{:?}", i.warnings);
}

#[test]
fn a_boolean_on_the_command_line_is_one_of_six_words() {
    let key = reg("autoswitch.include_api_key_accounts");
    for (raw, want) in [
        ("true", true),
        ("1", true),
        ("yes", true),
        ("false", false),
        ("0", false),
        ("no", false),
    ] {
        assert_eq!(key.parse_arg(raw), Ok(Value::Bool(want)), "{raw}");
    }
    for raw in ["True", "YES", "on", "off", "y", "", " yes", "2"] {
        assert_eq!(
            key.parse_arg(raw),
            Err("must be true, false, 1, 0, yes or no".to_owned()),
            "{raw:?}"
        );
    }
}

#[test]
fn a_number_on_the_command_line_is_taken_as_typed_and_never_clamped() {
    let threshold = reg("autoswitch.threshold");
    assert_eq!(threshold.parse_arg("80"), Ok(Value::Float(80.0)));
    assert_eq!(threshold.parse_arg("99.9"), Ok(Value::Float(99.9)));
    for raw in [
        "100", "49.9", "99.95", "nan", "inf", "-inf", "", "80%", " 80", "eighty",
    ] {
        assert_eq!(
            threshold.parse_arg(raw),
            Err("must be a number from 50 to 99.9".to_owned()),
            "{raw:?}"
        );
    }
    let interval = reg("autoswitch.interval_seconds");
    assert_eq!(interval.parse_arg("60"), Ok(Value::Int(60)));
    for raw in [
        "14",
        "3601",
        "60.0",
        "1e2",
        "",
        "-60",
        "9223372036854775808",
    ] {
        assert_eq!(
            interval.parse_arg(raw),
            Err("must be a whole number of seconds from 15 to 3600".to_owned()),
            "{raw:?}"
        );
    }
}

#[test]
fn a_list_on_the_command_line_is_split_on_commas_and_each_item_trimmed() {
    // Review Focus 2: lists typed the way people type them.
    let models = reg("autoswitch.models");
    assert_eq!(
        models.parse_arg("Fable, opus"),
        Ok(list(&["Fable", "opus"]))
    );
    assert_eq!(models.parse_arg(" Fable "), Ok(list(&["Fable"])));
    assert_eq!(models.parse_arg(""), Ok(list(&[])), "'' is the empty list");
    assert_eq!(models.parse_arg("all"), Ok(list(&["all"])));
    assert_eq!(
        models.parse_arg("Fable,fable,FABLE"),
        Ok(list(&["Fable"])),
        "repeats collapse, ignoring case"
    );
    for raw in ["Fable,,Opus", "Fable,", ",Fable", " ", " , "] {
        let reason = models.parse_arg(raw).unwrap_err();
        assert!(
            reason.starts_with("must be model names separated by commas, with no empty item"),
            "{raw:?}: {reason}"
        );
    }
    for raw in ["all,Fable", "Fable,ALL", " all , Opus"] {
        assert_eq!(
            models.parse_arg(raw),
            Err("must be \"all\" alone, or model names without \"all\"".to_owned()),
            "{raw:?}"
        );
    }
}

#[test]
fn share_extra_on_the_command_line_takes_entry_names_only() {
    let share = reg("run.share_extra");
    assert_eq!(
        share.parse_arg("hook-data, .my-tool,hooks.json"),
        Ok(list(&["hook-data", ".my-tool", "hooks.json"]))
    );
    assert_eq!(
        share.parse_arg("hook-data,hook-data"),
        Ok(list(&["hook-data"]))
    );
    assert_eq!(share.parse_arg(""), Ok(list(&[])));
    for raw in ["a/b", ".", "..", ".tagteam-links.json", "ok,../x"] {
        let reason = share.parse_arg(raw).unwrap_err();
        assert!(
            reason.starts_with("must be entry names of the source home"),
            "{raw:?}: {reason}"
        );
    }
    assert!(
        share
            .parse_arg("a,,b")
            .unwrap_err()
            .starts_with("must be entry names separated by commas")
    );
}

#[test]
fn a_choice_a_format_and_a_provider_on_the_command_line_are_taken_exactly() {
    let strategy = reg("autoswitch.strategy");
    assert_eq!(
        strategy.parse_arg("consume-first"),
        Ok(Value::Str("consume-first".into()))
    );
    for raw in ["Best", "best ", "next-available", ""] {
        assert_eq!(strategy.parse_arg(raw), Err(strategy.expect()), "{raw:?}");
    }
    let color = reg("ui.color");
    assert_eq!(color.parse_arg("never"), Ok(Value::Str("never".into())));
    assert_eq!(color.parse_arg("Never"), Err(color.expect()));
    let format = reg("statusline.format");
    assert_eq!(
        format.parse_arg("{5h}% {model:Fable}"),
        Ok(Value::Str("{5h}% {model:Fable}".into()))
    );
    for raw in ["", "  ", "{nope}", "{5h", "{model: Fable}"] {
        assert_eq!(format.parse_arg(raw), Err(format.expect()), "{raw:?}");
    }
    let provider = reg("default_provider");
    for raw in ["claude-code", "fake-agent", "x1"] {
        assert_eq!(provider.parse_arg(raw), Ok(Value::Str(raw.into())));
    }
    for raw in [
        "",
        "Claude-Code",
        "claude code",
        "claude_code",
        "-x",
        "x-",
        "provider.x",
    ] {
        assert_eq!(provider.parse_arg(raw), Err(provider.expect()), "{raw:?}");
    }
}

#[test]
fn every_refusal_says_what_the_value_must_be() {
    for key in KEYS {
        for raw in [
            "", "nope", "a,,b", "all,x", "{x}", "-1", "1e9", "a/b", "Yes",
        ] {
            if let Err(reason) = key.parse_arg(raw) {
                assert!(
                    reason.starts_with("must be "),
                    "{}: {raw:?}: {reason}",
                    key.name
                );
            }
        }
    }
}

#[test]
fn a_value_survives_the_command_line_and_the_file_unchanged() {
    // What `config get` prints, `config set` takes back; what a write stores, a read reads back.
    let samples = [
        ("default_provider", "fake-agent"),
        ("autoswitch.threshold", "75.5"),
        ("autoswitch.interval_seconds", "120"),
        ("autoswitch.cooldown_seconds", "0"),
        ("autoswitch.hysteresis_pct", "12.25"),
        ("autoswitch.strategy", "consume-first"),
        ("autoswitch.include_api_key_accounts", "yes"),
        ("autoswitch.unhealthy_ticks", "100"),
        ("autoswitch.models", "Fable, Opus"),
        ("usage.history_retention_days", "3650"),
        ("statusline.format", "{account} {5h}%"),
        ("run.share_extra", ""),
        ("ui.color", "always"),
    ];
    assert_eq!(
        samples.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
        KEYS.iter().map(|k| k.name).collect::<Vec<_>>(),
        "one sample per key"
    );
    for (name, raw) in samples {
        let key = reg(name);
        let value = key.parse_arg(raw).unwrap();
        assert_eq!(key.parse_arg(&value.display()), Ok(value.clone()), "{name}");
        let mut details = Vec::new();
        assert_eq!(
            key.parse_item(&value.to_item(), &mut |d| details.push(d)),
            Some(value.clone()),
            "{name}"
        );
        assert!(details.is_empty(), "{name}: {details:?}");
    }
}

#[test]
fn a_value_is_written_as_its_toml_type_and_shown_as_its_json_type() {
    assert_eq!(Value::Float(80.0).to_item().as_float(), Some(80.0));
    assert_eq!(Value::Int(60).to_item().as_integer(), Some(60));
    assert_eq!(Value::Bool(true).to_item().as_bool(), Some(true));
    assert_eq!(Value::Str("best".into()).to_item().as_str(), Some("best"));
    let item = list(&["Fable", "Opus"]).to_item();
    let items: Vec<&str> = item
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(items, ["Fable", "Opus"]);
    assert!(list(&[]).to_item().as_array().unwrap().is_empty());

    assert_eq!(Value::Float(80.0).to_json(), json!(80.0));
    assert_eq!(Value::Int(60).to_json(), json!(60));
    assert_eq!(Value::Bool(false).to_json(), json!(false));
    assert_eq!(Value::Str("best".into()).to_json(), json!("best"));
    assert_eq!(list(&["Fable"]).to_json(), json!(["Fable"]));
    assert_eq!(list(&[]).to_json(), json!([]));

    assert_eq!(Value::Float(80.0).display(), "80");
    assert_eq!(Value::Float(99.9).display(), "99.9");
    assert_eq!(list(&["Fable", "Opus"]).display(), "Fable,Opus");
    assert_eq!(list(&[]).display(), "");
    assert_eq!(Source::Provider.as_str(), "provider");
    assert_eq!(Source::Global.as_str(), "global");
    assert_eq!(Source::Default.as_str(), "default");
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-engine --test settings`
Expected: compile errors, among them `unresolved imports tagteam_engine::settings::AutoStrategy, tagteam_engine::settings::Inspection, tagteam_engine::settings::KEYS, …` (E0432) and `struct Settings has no field named default_provider` (E0560).

- [ ] **Step 3: Implement**

Replace the whole of `crates/tagteam-engine/src/settings.rs` with the following. Kept from today's file: the placeholder list and `is_statusline_placeholder`, the `Reader`'s `table` and `warn`, every warning's wording, the model-name rules (`parse_models` is now `models_from_item` and `collapse_models`) and the format scan (`parse_format` is now `is_format`). Kept from 13a: `share_extra` and `is_share_name`, as the Re-sync note says.

```rust
//! `config.toml` (§6.4): the key registry and the forgiving reader. Every key is declared once,
//! in [`KEYS`], with its kind, its "must be …" phrase and whether a provider table may override
//! it. Reads, `config list` and `config get`, the strict writes and completions all go through
//! it, so none of them accepts a key or value another rejects.
//!
//! A read never fails: a missing file, a corrupt file and an invalid value each fall back to
//! the default, the last two with a warning for the caller to print. Every warning names the
//! full path of the settings file.

use std::io::ErrorKind;
use std::path::PathBuf;

use tagteam_core::{CLAUDE_CODE, ProviderId};
use tagteam_provider::Env;
use toml_edit::{DocumentMut, Item, TableLike};

pub const DEFAULT_THRESHOLD: f64 = 90.0;
pub const DEFAULT_INTERVAL_SECONDS: u32 = 60;
pub const DEFAULT_COOLDOWN_SECONDS: u32 = 300;
pub const DEFAULT_HYSTERESIS_PCT: f64 = 10.0;
pub const DEFAULT_UNHEALTHY_TICKS: u32 = 3;
pub const DEFAULT_HISTORY_RETENTION_DAYS: u32 = 180;
pub const DEFAULT_STATUSLINE_FORMAT: &str = "{account} · 5h {5h}% · 7d {7d}%{stale}";

/// The statusline placeholders §13.5 defines, without their braces. `{model:<name>}` is the one
/// parameterised placeholder; see [`is_statusline_placeholder`]. The `statusline.format` check
/// and the statusline renderer share this list.
pub const STATUSLINE_PLACEHOLDERS: &[&str] = &[
    "account", "position", "email", "5h", "7d", "5h_reset", "7d_reset", "spend", "stale",
];

/// The prefix of the parameterised `{model:<name>}` placeholder.
pub const STATUSLINE_MODEL_PREFIX: &str = "model:";

/// Entry names tagteam keeps for itself in a profile (§12.2): `run.share_extra` never names one.
const OWN_PREFIX: &str = ".tagteam-";

/// Whether `name`, the text between a placeholder's braces, is one of §13.5's placeholders:
/// one of [`STATUSLINE_PLACEHOLDERS`], or `model:` followed by a model name. A model name is
/// non-empty, equals its trimmed form (the renderer matches it as written) and holds no brace.
pub fn is_statusline_placeholder(name: &str) -> bool {
    STATUSLINE_PLACEHOLDERS.contains(&name)
        || name
            .strip_prefix(STATUSLINE_MODEL_PREFIX)
            .is_some_and(|model| {
                !model.is_empty() && model == model.trim() && !model.contains(['{', '}'])
            })
}

/// One entry of the source home, as `run.share_extra` names it: not empty, not `.` or `..`,
/// and without a `/`. A dot inside a name is fine.
pub fn is_share_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains('/')
}

/// A provider id as `default_provider` and a `provider.<id>` table name it: 1 to 64 lowercase
/// ASCII letters, digits and dashes, with no dash at either end (`claude-code`).
fn is_provider_id(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !s.starts_with('-')
        && !s.ends_with('-')
}

/// `ui.color`. `NO_COLOR`, `FORCE_COLOR` and `--no-color` are the CLI's to apply on top.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    Auto,
    Always,
    Never,
}

impl ColorMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ColorMode::Auto => "auto",
            ColorMode::Always => "always",
            ColorMode::Never => "never",
        }
    }
}

/// `autoswitch.strategy` (§11): which account `auto` moves to. M3b's `auto` is its first reader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoStrategy {
    Best,
    ConsumeFirst,
}

impl AutoStrategy {
    pub fn as_str(self) -> &'static str {
        match self {
            AutoStrategy::Best => "best",
            AutoStrategy::ConsumeFirst => "consume-first",
        }
    }
}

/// What a key holds: how it is read from the file, parsed from the command line and written.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum KeyKind {
    /// A number in `min..=max`. The file may hold it as an integer.
    Float { min: f64, max: f64 },
    /// A whole number in `min..=max`.
    Int { min: i64, max: i64 },
    /// A TOML boolean in the file; `true/false/1/0/yes/no` on the command line.
    Bool,
    /// One of these strings, exactly.
    Choice(&'static [&'static str]),
    /// `autoswitch.models`: model display names, or `all` alone. A name repeated in another
    /// case collapses into its first spelling.
    Models,
    /// `statusline.format`: plain text and §13.5's placeholders.
    Format,
    /// `run.share_extra`: entry names of the source home (§12.2).
    ShareNames,
    /// `default_provider`: a well-formed provider id. Whether this build has that provider is
    /// the CLI's check.
    Provider,
}

/// One §6.4 key.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Key {
    /// The dotted name `config` takes: `autoswitch.threshold`, or `default_provider` at the top
    /// level.
    pub name: &'static str,
    pub kind: KeyKind,
    /// Whether a `[provider.<id>.<table>]` table may override it.
    pub per_provider: bool,
}

const STRATEGIES: &[&str] = &["best", "consume-first"];
const COLORS: &[&str] = &["auto", "always", "never"];

/// Every §6.4 key, in the order of the spec's table.
pub const KEYS: &[Key] = &[
    Key {
        name: "default_provider",
        kind: KeyKind::Provider,
        per_provider: false,
    },
    Key {
        name: "autoswitch.threshold",
        kind: KeyKind::Float {
            min: 50.0,
            max: 99.9,
        },
        per_provider: true,
    },
    Key {
        name: "autoswitch.interval_seconds",
        kind: KeyKind::Int { min: 15, max: 3600 },
        per_provider: true,
    },
    Key {
        name: "autoswitch.cooldown_seconds",
        kind: KeyKind::Int {
            min: 0,
            max: 86_400,
        },
        per_provider: true,
    },
    Key {
        name: "autoswitch.hysteresis_pct",
        kind: KeyKind::Float {
            min: 0.0,
            max: 50.0,
        },
        per_provider: true,
    },
    Key {
        name: "autoswitch.strategy",
        kind: KeyKind::Choice(STRATEGIES),
        per_provider: true,
    },
    Key {
        name: "autoswitch.include_api_key_accounts",
        kind: KeyKind::Bool,
        per_provider: true,
    },
    Key {
        name: "autoswitch.unhealthy_ticks",
        kind: KeyKind::Int { min: 1, max: 100 },
        per_provider: true,
    },
    Key {
        name: "autoswitch.models",
        kind: KeyKind::Models,
        per_provider: true,
    },
    Key {
        name: "usage.history_retention_days",
        kind: KeyKind::Int { min: 1, max: 3650 },
        per_provider: false,
    },
    Key {
        name: "statusline.format",
        kind: KeyKind::Format,
        per_provider: true,
    },
    Key {
        name: "run.share_extra",
        kind: KeyKind::ShareNames,
        per_provider: true,
    },
    Key {
        name: "ui.color",
        kind: KeyKind::Choice(COLORS),
        per_provider: false,
    },
];

/// The registry's key called `name`, a dotted name without any `provider.<id>.` prefix.
pub fn key(name: &str) -> Option<&'static Key> {
    KEYS.iter().find(|k| k.name == name)
}

impl Key {
    /// The table the key lives in: `autoswitch` for `autoswitch.threshold`, `None` for a key at
    /// the top level.
    pub fn table(&self) -> Option<&'static str> {
        let name: &'static str = self.name;
        name.rsplit_once('.').map(|(table, _)| table)
    }

    /// The key's name within its table: `threshold` for `autoswitch.threshold`.
    pub fn leaf(&self) -> &'static str {
        let name: &'static str = self.name;
        name.rsplit_once('.').map_or(name, |(_, leaf)| leaf)
    }

    /// What a valid value is, as every warning and refusal words it: "must be a number from 50
    /// to 99.9".
    pub fn expect(&self) -> String {
        match self.kind {
            KeyKind::Float { min, max } => format!("must be a number from {min} to {max}"),
            KeyKind::Int { min, max } => {
                let unit = if self.name.ends_with("_days") {
                    " of days"
                } else if self.name.ends_with("_seconds") {
                    " of seconds"
                } else {
                    ""
                };
                format!("must be a whole number{unit} from {min} to {max}")
            }
            KeyKind::Bool => "must be true or false".to_owned(),
            KeyKind::Choice(values) => format!("must be {}", one_of(values)),
            KeyKind::Models => {
                "must be a model name, a list of model names, or [\"all\"] alone".to_owned()
            }
            KeyKind::Format => format!(
                "must be a non-empty string using only the placeholders {} and {{{STATUSLINE_MODEL_PREFIX}<name>}}, each closed",
                STATUSLINE_PLACEHOLDERS
                    .iter()
                    .map(|name| format!("{{{name}}}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            KeyKind::ShareNames => "must be an entry name or a list of entry names".to_owned(),
            KeyKind::Provider => {
                "must be a provider id of lowercase letters, digits and dashes, such as \"claude-code\""
                    .to_owned()
            }
        }
    }

    /// The value `item` holds, read forgivingly from the file: `None` when it is not valid for
    /// this key, and the caller warns with [`Key::expect`] and tries the next table. Only
    /// `ShareNames` reports item by item: an item that is not an entry name is dropped through
    /// `warn`, which gets the text that follows the key in the warning, and the rest stand.
    pub fn parse_item(&self, item: &Item, warn: &mut dyn FnMut(String)) -> Option<Value> {
        match self.kind {
            KeyKind::Float { min, max } => {
                let n = item
                    .as_float()
                    .or_else(|| item.as_integer().map(|i| i as f64))?;
                (min..=max).contains(&n).then_some(Value::Float(n))
            }
            KeyKind::Int { min, max } => {
                let n = item.as_integer()?;
                (min..=max).contains(&n).then_some(Value::Int(n))
            }
            KeyKind::Bool => item.as_bool().map(Value::Bool),
            KeyKind::Choice(values) => {
                let s = item.as_str()?;
                values.contains(&s).then(|| Value::Str(s.to_owned()))
            }
            KeyKind::Models => models_from_item(item).map(Value::List),
            KeyKind::Format => {
                let s = item.as_str()?;
                is_format(s).then(|| Value::Str(s.to_owned()))
            }
            KeyKind::ShareNames => share_names_from_item(item, warn).map(Value::List),
            KeyKind::Provider => {
                let s = item.as_str()?;
                is_provider_id(s).then(|| Value::Str(s.to_owned()))
            }
        }
    }

    /// A value typed on the command line, read strictly (§6.4). Numbers are taken as typed and
    /// never clamped. Booleans are `true/false/1/0/yes/no`. Lists are comma-separated, each item
    /// trimmed, with no empty item, and `''` is the empty list. `all` in `autoswitch.models`
    /// stands alone, and a `run.share_extra` item is not one of tagteam's own (`.tagteam-*`).
    /// `Err` is the reason, starting with "must be".
    pub fn parse_arg(&self, raw: &str) -> Result<Value, String> {
        match self.kind {
            KeyKind::Float { min, max } => raw
                .parse::<f64>()
                .ok()
                .filter(|n| (min..=max).contains(n))
                .map(Value::Float)
                .ok_or_else(|| self.expect()),
            KeyKind::Int { min, max } => raw
                .parse::<i64>()
                .ok()
                .filter(|n| (min..=max).contains(n))
                .map(Value::Int)
                .ok_or_else(|| self.expect()),
            KeyKind::Bool => match raw {
                "true" | "1" | "yes" => Ok(Value::Bool(true)),
                "false" | "0" | "no" => Ok(Value::Bool(false)),
                _ => Err("must be true, false, 1, 0, yes or no".to_owned()),
            },
            KeyKind::Choice(values) if values.contains(&raw) => Ok(Value::Str(raw.to_owned())),
            KeyKind::Choice(_) => Err(self.expect()),
            KeyKind::Models => {
                let names = list_arg(raw, "model names")?;
                collapse_models(names).map(Value::List).ok_or_else(|| {
                    "must be \"all\" alone, or model names without \"all\"".to_owned()
                })
            }
            KeyKind::Format if is_format(raw) => Ok(Value::Str(raw.to_owned())),
            KeyKind::Format => Err(self.expect()),
            KeyKind::ShareNames => {
                let names = list_arg(raw, "entry names")?;
                if let Some(bad) = names
                    .iter()
                    .find(|n| !is_share_name(n) || n.starts_with(OWN_PREFIX))
                {
                    return Err(format!(
                        "must be entry names of the source home, each without a `/`, not `.` or `..`, and not `{OWN_PREFIX}*`: {bad:?} is not one"
                    ));
                }
                let mut unique: Vec<String> = Vec::with_capacity(names.len());
                for name in names {
                    if !unique.contains(&name) {
                        unique.push(name);
                    }
                }
                Ok(Value::List(unique))
            }
            KeyKind::Provider if is_provider_id(raw) => Ok(Value::Str(raw.to_owned())),
            KeyKind::Provider => Err(self.expect()),
        }
    }
}

/// `"a"`, `"a" or "b"`, `"a", "b" or "c"`.
fn one_of(values: &[&str]) -> String {
    let quoted: Vec<String> = values.iter().map(|v| format!("{v:?}")).collect();
    match quoted.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
        None => String::new(),
    }
}

/// A list on the command line (§6.4): comma-separated, each item trimmed, and no item empty.
/// `''` alone is the empty list.
fn list_arg(raw: &str, what: &str) -> Result<Vec<String>, String> {
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    let items: Vec<&str> = raw.split(',').map(str::trim).collect();
    if items.iter().any(|item| item.is_empty()) {
        return Err(format!(
            "must be {what} separated by commas, with no empty item ('' alone is the empty list)"
        ));
    }
    Ok(items.into_iter().map(str::to_owned).collect())
}

/// A model name or a list of them, as the file holds them. Names are trimmed, and an empty one
/// makes the whole value invalid.
fn models_from_item(item: &Item) -> Option<Vec<String>> {
    let names: Vec<String> = match item.as_str() {
        Some(one) => vec![model_name(one)?],
        None => item
            .as_array()?
            .iter()
            .map(|v| v.as_str().and_then(model_name))
            .collect::<Option<_>>()?,
    };
    collapse_models(names)
}

fn model_name(s: &str) -> Option<String> {
    let trimmed = s.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// §6.4's rule for model names, from the file or the command line alike: a name repeated in
/// another case collapses into its first spelling, and `all` (any case) cannot be mixed with
/// names.
fn collapse_models(names: Vec<String>) -> Option<Vec<String>> {
    let mut unique: Vec<String> = Vec::with_capacity(names.len());
    for candidate in names {
        if !unique
            .iter()
            .any(|n| n.to_lowercase() == candidate.to_lowercase())
        {
            unique.push(candidate);
        }
    }
    let mixed = unique.len() > 1 && unique.iter().any(|n| n.eq_ignore_ascii_case("all"));
    (!mixed).then_some(unique)
}

/// A non-empty format whose every `{…}` is one of §13.5's placeholders. The scan is the
/// renderer's: a `{` runs to the next `}`, and an unclosed `{` is invalid.
fn is_format(s: &str) -> bool {
    if s.trim().is_empty() {
        return false;
    }
    let mut rest = s;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}').map(|i| open + i) else {
            return false;
        };
        if !is_statusline_placeholder(&rest[open + 1..close]) {
            return false;
        }
        rest = &rest[close + 1..];
    }
    true
}

/// `run.share_extra` as the file holds it: a name or a list of names, or `None` for any other
/// value. Within a list, an item that is not a string, or not an entry name
/// ([`is_share_name`]), is dropped with a warning naming it; the rest stand, and a repeated
/// name collapses into its first.
fn share_names_from_item(item: &Item, warn: &mut dyn FnMut(String)) -> Option<Vec<String>> {
    let items: Vec<Option<&str>> = match (item.as_str(), item.as_array()) {
        (Some(one), _) => vec![Some(one)],
        (None, Some(list)) => list.iter().map(|v| v.as_str()).collect(),
        (None, None) => return None,
    };
    let mut names: Vec<String> = Vec::new();
    for item in items {
        match item {
            Some(name) if is_share_name(name) => {
                if !names.iter().any(|n| n == name) {
                    names.push(name.to_owned());
                }
            }
            Some(name) => warn(format!(
                "entry {name:?} is not an entry name of the source home (ignored)"
            )),
            None => warn("holds an item that is not a string (ignored)".to_owned()),
        }
    }
    Some(names)
}

/// A key's value, whatever its kind.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Float(f64),
    Int(i64),
    Bool(bool),
    Str(String),
    List(Vec<String>),
}

impl Value {
    /// The TOML a write stores: a list is an array of strings, a boolean a TOML boolean.
    pub fn to_item(&self) -> Item {
        match self {
            Value::Float(n) => toml_edit::value(*n),
            Value::Int(n) => toml_edit::value(*n),
            Value::Bool(b) => toml_edit::value(*b),
            Value::Str(s) => toml_edit::value(s.as_str()),
            Value::List(items) => toml_edit::value(
                items
                    .iter()
                    .map(String::as_str)
                    .collect::<toml_edit::Array>(),
            ),
        }
    }

    /// The value as `config get` prints it, in the form `config set` takes: a list joined with
    /// commas, and the empty list as the empty string.
    pub fn display(&self) -> String {
        match self {
            Value::Float(n) => n.to_string(),
            Value::Int(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Str(s) => s.clone(),
            Value::List(items) => items.join(","),
        }
    }

    /// The value under `--json`: a list is an array.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Value::Float(n) => serde_json::json!(n),
            Value::Int(n) => serde_json::json!(n),
            Value::Bool(b) => serde_json::json!(b),
            Value::Str(s) => serde_json::json!(s),
            Value::List(items) => serde_json::json!(items),
        }
    }
}

/// Where a key's effective value came from (§6.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Neither table holds a valid value.
    Default,
    /// The key's own table: `[autoswitch]`, or the top level.
    Global,
    /// The provider's `[provider.<id>.<table>]`.
    Provider,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Default => "default",
            Source::Global => "global",
            Source::Provider => "provider",
        }
    }
}

/// One key as `config list` shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyState {
    pub key: &'static Key,
    /// The effective value: what `Settings::load` gives.
    pub value: Value,
    pub default: Value,
    pub source: Source,
}

/// The settings file as one provider's commands read it (§6.4): each key's effective value and
/// source, in [`KEYS`] order, the keys the registry does not know, and the read's warnings.
#[derive(Debug, Clone, PartialEq)]
pub struct Inspection {
    pub path: PathBuf,
    /// Whether there is anything at `path` to read; a file that cannot be read still exists.
    pub exists: bool,
    pub keys: Vec<KeyState>,
    /// Dotted names in the file that no registry key accounts for, sorted. Under
    /// `provider.<id>`, for any id, a key is known only when a provider table may override it.
    pub unknown: Vec<String>,
    pub warnings: Vec<String>,
}

/// `$XDG_CONFIG_HOME/tagteam/config.toml` (§5), whether or not it exists.
pub fn config_path(env: &Env) -> PathBuf {
    env.config_dir().join("config.toml")
}

/// The file as `provider`'s commands read it: forgiving, and read exactly as `Settings::load`
/// reads it. Never creates anything.
pub fn inspect(env: &Env, provider: &ProviderId) -> Inspection {
    let read = read_file(env, provider);
    let defaults = Settings::default();
    let keys = KEYS
        .iter()
        .zip(read.sources)
        .map(|(key, source)| KeyState {
            key,
            value: read.settings.value(key),
            default: defaults.value(key),
            source,
        })
        .collect();
    Inspection {
        path: read.path,
        exists: read.exists,
        keys,
        unknown: read.unknown,
        warnings: read.warnings,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// `default_provider`, read from the top level only. Whether this build has it is the
    /// CLI's to check; it falls back to claude-code.
    pub default_provider: ProviderId,
    /// `autoswitch.threshold`: 50–99.9. The provider's own table first.
    pub threshold: f64,
    /// `autoswitch.interval_seconds`: 15–3600. The provider's own table first.
    pub interval_seconds: u32,
    /// `autoswitch.cooldown_seconds`: 0–86400. The provider's own table first.
    pub cooldown_seconds: u32,
    /// `autoswitch.hysteresis_pct`: 0–50. The provider's own table first.
    pub hysteresis_pct: f64,
    /// `autoswitch.strategy`. The provider's own table first.
    pub strategy: AutoStrategy,
    /// `autoswitch.include_api_key_accounts`. The provider's own table first.
    pub include_api_key_accounts: bool,
    /// `autoswitch.unhealthy_ticks`: 1–100. The provider's own table first.
    pub unhealthy_ticks: u32,
    /// `autoswitch.models`: model display names, or `all`. The provider's own table first.
    pub models: Vec<String>,
    /// `usage.history_retention_days`: 1–3650.
    pub history_retention_days: u32,
    /// `statusline.format`. The provider's own table first.
    pub statusline_format: String,
    /// `ui.color`.
    pub color: ColorMode,
    /// `run.share_extra`: entry names of the source home a profile shares besides the
    /// provider's allowlist (§6.4, §12.2). The provider's own table first. A name that is not
    /// an entry name (`is_share_name`) is dropped here with a warning; the link sync drops a
    /// known-private one, also with a warning.
    pub share_extra: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            default_provider: ProviderId::new(CLAUDE_CODE),
            threshold: DEFAULT_THRESHOLD,
            interval_seconds: DEFAULT_INTERVAL_SECONDS,
            cooldown_seconds: DEFAULT_COOLDOWN_SECONDS,
            hysteresis_pct: DEFAULT_HYSTERESIS_PCT,
            strategy: AutoStrategy::Best,
            include_api_key_accounts: false,
            unhealthy_ticks: DEFAULT_UNHEALTHY_TICKS,
            models: Vec::new(),
            history_retention_days: DEFAULT_HISTORY_RETENTION_DAYS,
            statusline_format: DEFAULT_STATUSLINE_FORMAT.to_owned(),
            color: ColorMode::Auto,
            share_extra: Vec::new(),
        }
    }
}

impl Settings {
    /// `config.toml`, as `provider`'s commands read it. Forgiving (§6.4): a missing file gives
    /// the defaults silently; a corrupt or unreadable file gives the defaults and one warning
    /// naming the path; an invalid value gives its default and one warning naming the path and
    /// the key. The rest of the file still applies.
    pub fn load(env: &Env, provider: &ProviderId) -> (Settings, Vec<String>) {
        let read = read_file(env, provider);
        (read.settings, read.warnings)
    }

    /// `key`'s value, as `config` shows it.
    pub fn value(&self, key: &Key) -> Value {
        match key.name {
            "default_provider" => Value::Str(self.default_provider.to_string()),
            "autoswitch.threshold" => Value::Float(self.threshold),
            "autoswitch.interval_seconds" => Value::Int(self.interval_seconds.into()),
            "autoswitch.cooldown_seconds" => Value::Int(self.cooldown_seconds.into()),
            "autoswitch.hysteresis_pct" => Value::Float(self.hysteresis_pct),
            "autoswitch.strategy" => Value::Str(self.strategy.as_str().to_owned()),
            "autoswitch.include_api_key_accounts" => Value::Bool(self.include_api_key_accounts),
            "autoswitch.unhealthy_ticks" => Value::Int(self.unhealthy_ticks.into()),
            "autoswitch.models" => Value::List(self.models.clone()),
            "usage.history_retention_days" => Value::Int(self.history_retention_days.into()),
            "statusline.format" => Value::Str(self.statusline_format.clone()),
            "run.share_extra" => Value::List(self.share_extra.clone()),
            "ui.color" => Value::Str(self.color.as_str().to_owned()),
            other => unreachable!("`{other}` is not a registry key"),
        }
    }

    /// Stores what `key.parse_item` gave. The registry's kinds and ranges make each pairing
    /// below the only one that can occur.
    fn apply(&mut self, key: &Key, value: Value) {
        match (key.name, value) {
            ("default_provider", Value::Str(id)) => self.default_provider = ProviderId::new(id),
            ("autoswitch.threshold", Value::Float(n)) => self.threshold = n,
            ("autoswitch.interval_seconds", Value::Int(n)) => self.interval_seconds = whole(n),
            ("autoswitch.cooldown_seconds", Value::Int(n)) => self.cooldown_seconds = whole(n),
            ("autoswitch.hysteresis_pct", Value::Float(n)) => self.hysteresis_pct = n,
            ("autoswitch.strategy", Value::Str(s)) => {
                self.strategy = match s.as_str() {
                    "consume-first" => AutoStrategy::ConsumeFirst,
                    _ => AutoStrategy::Best,
                }
            }
            ("autoswitch.include_api_key_accounts", Value::Bool(b)) => {
                self.include_api_key_accounts = b
            }
            ("autoswitch.unhealthy_ticks", Value::Int(n)) => self.unhealthy_ticks = whole(n),
            ("autoswitch.models", Value::List(names)) => self.models = names,
            ("usage.history_retention_days", Value::Int(n)) => {
                self.history_retention_days = whole(n)
            }
            ("statusline.format", Value::Str(s)) => self.statusline_format = s,
            ("run.share_extra", Value::List(names)) => self.share_extra = names,
            ("ui.color", Value::Str(s)) => {
                self.color = match s.as_str() {
                    "always" => ColorMode::Always,
                    "never" => ColorMode::Never,
                    _ => ColorMode::Auto,
                }
            }
            (name, value) => unreachable!("the registry never gives `{name}` {value:?}"),
        }
    }
}

/// An `Int` key's value as its field holds it: every `Int` range lies within `u32`.
fn whole(n: i64) -> u32 {
    u32::try_from(n).expect("every Int key's range lies within u32")
}

/// One read of the file for `provider`: what `Settings::load` and `inspect` both report.
struct FileRead {
    path: PathBuf,
    exists: bool,
    settings: Settings,
    /// Per [`KEYS`] entry, the table its value came from.
    sources: Vec<Source>,
    unknown: Vec<String>,
    warnings: Vec<String>,
}

fn read_file(env: &Env, provider: &ProviderId) -> FileRead {
    let mut read = FileRead {
        path: config_path(env),
        exists: true,
        settings: Settings::default(),
        sources: vec![Source::Default; KEYS.len()],
        unknown: Vec::new(),
        warnings: Vec::new(),
    };
    let text = match std::fs::read_to_string(&read.path) {
        Ok(text) => text,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            read.exists = false;
            return read;
        }
        Err(e) => {
            read.warnings.push(format!(
                "{}: cannot read the settings file ({}); using the defaults",
                read.path.display(),
                e.kind()
            ));
            return read;
        }
    };
    let Ok(doc) = text.parse::<DocumentMut>() else {
        read.warnings.push(format!(
            "{}: the settings file is not valid TOML; using the defaults",
            read.path.display()
        ));
        return read;
    };
    let shown = read.path.display().to_string();
    let mut reader = Reader {
        doc: &doc,
        path: &shown,
        warnings: Vec::new(),
    };
    for (key, source) in KEYS.iter().zip(read.sources.iter_mut()) {
        if let Some((value, from)) = reader.read_key(key, provider) {
            read.settings.apply(key, value);
            *source = from;
        }
    }
    read.warnings = reader.warnings;
    unknown_keys(doc.as_table(), &mut Vec::new(), &mut read.unknown);
    read.unknown.sort();
    read
}

struct Reader<'a> {
    doc: &'a DocumentMut,
    /// The settings file's full path, which every warning starts with.
    path: &'a str,
    warnings: Vec<String>,
}

impl<'a> Reader<'a> {
    fn warn(&mut self, message: String) {
        if !self.warnings.contains(&message) {
            self.warnings.push(message);
        }
    }

    /// The table at `path`, or `None` when it is absent. A segment that exists but is not a
    /// table warns and reads as absent.
    fn table(&mut self, path: &[&str]) -> Option<&'a dyn TableLike> {
        let doc: &'a DocumentMut = self.doc;
        let mut current: &'a dyn TableLike = doc.as_table();
        for (depth, segment) in path.iter().enumerate() {
            let item = current.get(segment)?;
            match item.as_table_like() {
                Some(table) => current = table,
                None => {
                    let dotted = path[..=depth].join(".");
                    self.warn(format!(
                        "{}: `{dotted}` must be a table (ignored)",
                        self.path
                    ));
                    return None;
                }
            }
        }
        Some(current)
    }

    /// `key`'s value for `provider`, and the table it came from: the first table that holds a
    /// valid one, the provider's own first when the key is per provider. A present value that
    /// is not valid warns, naming the dotted key, and the next table is tried.
    fn read_key(&mut self, key: &Key, provider: &ProviderId) -> Option<(Value, Source)> {
        let own: Vec<&str> = key.table().into_iter().collect();
        let mut tables: Vec<(Vec<&str>, Source)> = Vec::with_capacity(2);
        if key.per_provider {
            let mut path = vec!["provider", provider.as_str()];
            path.extend(&own);
            tables.push((path, Source::Provider));
        }
        tables.push((own, Source::Global));
        for (path, source) in tables {
            let Some(table) = self.table(&path) else {
                continue;
            };
            let Some(item) = table.get(key.leaf()) else {
                continue;
            };
            let dotted = path
                .iter()
                .copied()
                .chain([key.leaf()])
                .collect::<Vec<_>>()
                .join(".");
            let mut details = Vec::new();
            let value = key.parse_item(item, &mut |detail| details.push(detail));
            for detail in details {
                self.warn(format!("{}: `{dotted}` {detail}", self.path));
            }
            match value {
                Some(value) => return Some((value, source)),
                None => self.warn(format!(
                    "{}: `{dotted}` {} (ignored)",
                    self.path,
                    key.expect()
                )),
            }
        }
        None
    }
}

/// Where a dotted path of the file stands in the registry.
enum Place {
    Key,
    Table,
    Unknown,
}

/// `path`, from the top of the file. `provider` and `provider.<id>` are tables for any id, and
/// under them only a per-provider key, and its table, are known.
fn place(path: &[&str]) -> Place {
    match path {
        ["provider"] | ["provider", _] => Place::Table,
        ["provider", _, rest @ ..] => place_in(rest, true),
        rest => place_in(rest, false),
    }
}

fn place_in(path: &[&str], in_provider: bool) -> Place {
    let mut place = Place::Unknown;
    for k in KEYS.iter().filter(|k| k.per_provider || !in_provider) {
        match (k.table(), path) {
            (None, [leaf]) if *leaf == k.leaf() => return Place::Key,
            (Some(table), [t, leaf]) if *t == table && *leaf == k.leaf() => return Place::Key,
            (Some(table), [t]) if *t == table => place = Place::Table,
            _ => {}
        }
    }
    place
}

/// Appends to `unknown` the dotted name of every key under `table`, which sits at `path`, that
/// the registry does not know. A known table holding something other than a table is the
/// reader's to warn about, not an unknown key.
fn unknown_keys(table: &dyn TableLike, path: &mut Vec<String>, unknown: &mut Vec<String>) {
    for (name, item) in table.iter() {
        path.push(name.to_owned());
        let at = {
            let segments: Vec<&str> = path.iter().map(String::as_str).collect();
            place(&segments)
        };
        match at {
            Place::Key => {}
            Place::Table => {
                if let Some(inner) = item.as_table_like() {
                    unknown_keys(inner, path, unknown);
                }
            }
            Place::Unknown => every_key(item, path, unknown),
        }
        path.pop();
    }
}

/// Appends the dotted name of every key inside `item`, which sits at `path`: of each value in
/// a table, at any depth, or of `path` itself when it holds no table with a key in it.
fn every_key(item: &Item, path: &mut Vec<String>, unknown: &mut Vec<String>) {
    match item.as_table_like() {
        Some(table) if !table.is_empty() => {
            for (name, inner) in table.iter() {
                path.push(name.to_owned());
                every_key(inner, path, unknown);
                path.pop();
            }
        }
        _ => unknown.push(path.join(".")),
    }
}
```

Existing tests this changes: the four in `tests/settings.rs` named in Step 1. Every other `Settings` literal in the workspace uses `..Settings::default()` (`engine.rs`'s `the_engine_keeps_the_settings_it_was_built_with`, `tests/views_usage.rs`, M3a's and M4a's fixtures), so nothing else changes.

- [ ] **Step 4: Run the tests and see them pass**

Run: `cargo test -p tagteam-engine --test settings` — Expected: PASS (69 tests: 39 kept, 13a's 4, 26 new).
Run: `cargo test -p tagteam-engine` — Expected: PASS.
Run: `cargo test -p tagteam --features test-support` — Expected: PASS; the CLI's settings warnings read as before.

- [ ] **Step 5: Format, lint, commit**

```
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p tagteam-engine
git add crates/tagteam-engine/src/settings.rs crates/tagteam-engine/tests/settings.rs
git commit -m "Declare every settings key once and read each one through the registry"
```

#### 2b: Naming a key, and the settings error

- [ ] **Step 6: Write the failing tests**

In `crates/tagteam-engine/tests/settings.rs`, replace the import block Step 1 wrote with:

```rust
use std::fs;

use serde_json::json;
use tagteam_core::ProviderId;
use tagteam_engine::settings::{
    self, AutoStrategy, ColorMode, Inspection, KEYS, Key, KeyKind, KeyState,
    STATUSLINE_PLACEHOLDERS, Settings, SettingsError, Source, Value, inspect,
    is_statusline_placeholder, resolve,
};
use tagteam_provider::Env;
```

Append at the end of the file:

```rust
#[test]
fn a_key_resolves_with_or_without_its_provider() {
    let cc = ProviderId::new("claude-code");
    let fake = ProviderId::new("fake-agent");
    let threshold = reg("autoswitch.threshold");
    let cases = [
        ("autoswitch.threshold", None, None),
        ("autoswitch.threshold", Some(&cc), Some(&cc)),
        ("provider.claude-code.autoswitch.threshold", None, Some(&cc)),
        (
            "provider.claude-code.autoswitch.threshold",
            Some(&cc),
            Some(&cc),
        ),
        (
            "provider.fake-agent.autoswitch.threshold",
            None,
            Some(&fake),
        ),
    ];
    for (name, flag, want) in cases {
        let r = resolve(name, flag).unwrap();
        assert_eq!((r.key, r.provider.as_ref()), (threshold, want), "{name}");
    }
    let r = resolve("ui.color", None).unwrap();
    assert_eq!((r.key.name, r.provider), ("ui.color", None));
    let r = resolve("provider.claude-code.run.share_extra", None).unwrap();
    assert_eq!(r.key.name, "run.share_extra");
}

#[test]
fn a_key_that_does_not_resolve_says_why() {
    let cc = ProviderId::new("claude-code");
    let fake = ProviderId::new("fake-agent");
    for name in [
        "",
        "nope",
        "threshold",
        "autoswitch",
        "autoswitch.nope",
        "Autoswitch.threshold",
        "provider",
        "provider.claude-code",
        "provider.claude-code.nope",
        "provider.claude-code.provider.claude-code.autoswitch.threshold",
    ] {
        assert!(
            matches!(resolve(name, None), Err(SettingsError::UnknownKey(n)) if n == name),
            "{name:?}"
        );
    }
    for (name, flag) in [
        ("ui.color", Some(&cc)),
        ("provider.claude-code.ui.color", None),
        ("default_provider", Some(&cc)),
        ("provider.claude-code.default_provider", None),
        ("usage.history_retention_days", Some(&fake)),
    ] {
        assert!(
            matches!(resolve(name, flag), Err(SettingsError::NotPerProvider(_))),
            "{name}"
        );
    }
    assert!(matches!(
        resolve("provider.claude-code.autoswitch.models", Some(&fake)),
        Err(SettingsError::ProviderMismatch { prefix, flag })
            if prefix == "claude-code" && flag == "fake-agent"
    ));
    // A provider that is no id at all is still the caller's to refuse, as `--provider` is.
    let r = resolve("provider.Claude.autoswitch.threshold", None).unwrap();
    assert_eq!(r.provider, Some(ProviderId::new("Claude")));
}

#[test]
fn a_settings_error_tells_the_user_what_to_do() {
    assert_eq!(
        SettingsError::UnknownKey("autoswitch.thresold".into()).to_string(),
        "there is no setting `autoswitch.thresold`; `tagteam config list` shows them all"
    );
    assert_eq!(
        SettingsError::NotPerProvider("ui.color".into()).to_string(),
        "`ui.color` is the same for every provider, so no provider table can set it; name it without `provider.<id>.` and without --provider"
    );
    assert_eq!(
        SettingsError::ProviderMismatch {
            prefix: "claude-code".into(),
            flag: "fake-agent".into()
        }
        .to_string(),
        "`provider.claude-code.…` and `--provider fake-agent` name different providers; give only one of them"
    );
    assert_eq!(
        SettingsError::Invalid {
            key: "autoswitch.threshold".into(),
            reason: "must be a number from 50 to 99.9".into()
        }
        .to_string(),
        "`autoswitch.threshold` must be a number from 50 to 99.9"
    );
}
```

In `crates/tagteam-engine/src/error.rs`, in `kind_is_pinned_for_every_variant`, insert immediately before the last row, `(EngineError::Io(io::Error::other("x")), "io"),`:

```rust
            (
                EngineError::Settings(SettingsError::UnknownKey("x".into())),
                "invalid-input",
            ),
            (
                EngineError::Settings(SettingsError::NotPerProvider("ui.color".into())),
                "invalid-input",
            ),
            (
                EngineError::Settings(SettingsError::ProviderMismatch {
                    prefix: "a".into(),
                    flag: "b".into(),
                }),
                "invalid-input",
            ),
            (
                EngineError::Settings(SettingsError::Invalid {
                    key: "k".into(),
                    reason: "must be x".into(),
                }),
                "invalid-input",
            ),
            (
                EngineError::Settings(SettingsError::UnknownProvider("p".into())),
                "invalid-input",
            ),
            (
                EngineError::Settings(SettingsError::Corrupt {
                    path: PathBuf::from("x"),
                    detail: "d".into(),
                }),
                "settings-unreadable",
            ),
            (
                EngineError::Settings(SettingsError::Lock(LockError::Timeout(PathBuf::from("x")))),
                "lock-timeout",
            ),
            (
                EngineError::Settings(SettingsError::Lock(LockError::Compromised(PathBuf::from(
                    "x",
                )))),
                "lock",
            ),
            (
                EngineError::Settings(SettingsError::Io(io::Error::other("x"))),
                "io",
            ),
```

and add after that test, inside `mod tests`:

```rust

    /// Decision 4: the settings lock fails with the kinds an engine lock fails with, for every
    /// way a lock can fail.
    #[test]
    fn the_settings_lock_has_the_engine_lock_s_kinds() {
        let failures = || {
            vec![
                LockError::Timeout(PathBuf::from("x")),
                LockError::Compromised(PathBuf::from("x")),
                LockError::Io(io::Error::other("x")),
            ]
        };
        for (engine, settings) in failures().into_iter().zip(failures()) {
            assert_eq!(
                EngineError::Settings(SettingsError::Lock(settings)).kind(),
                EngineError::Lock(engine).kind()
            );
        }
    }
```

- [ ] **Step 7: Run them and see them fail**

Run: `cargo test -p tagteam-engine --test settings`
Expected: compile error `unresolved imports tagteam_engine::settings::SettingsError, tagteam_engine::settings::resolve` (E0432).

Run: `cargo test -p tagteam-engine --lib error`
Expected: compile errors `cannot find type SettingsError in this scope` (E0433/E0412) and `no variant or associated item named Settings found for enum EngineError` (E0599).

- [ ] **Step 8: Implement**

In `crates/tagteam-engine/src/settings.rs`, replace

```rust
use tagteam_provider::Env;
```

with

```rust
use tagteam_provider::{Env, LockError};
```

and insert after `inspect` (the function ending `warnings: read.warnings,\n    }\n}`), before `#[derive(Debug, Clone, PartialEq)]\npub struct Settings {`:

```rust
/// A key as a `config` command names it, and the provider table it names, if any.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub key: &'static Key,
    pub provider: Option<ProviderId>,
}

/// `provider.<id>.<key>`, or `<key>` with `--provider` as `flag`: the two spellings of one
/// entry (§6.4). Both may be given only when they name the same provider, and a provider table
/// is refused for a key no provider can override. Whether this build has the provider is the
/// caller's to check, as it checks `--provider`, so both spellings fail alike.
pub fn resolve(name: &str, flag: Option<&ProviderId>) -> Result<Resolved, SettingsError> {
    let unknown = || SettingsError::UnknownKey(name.to_owned());
    let (prefix, bare) = match name.strip_prefix("provider.") {
        Some(rest) => {
            let (id, bare) = rest.split_once('.').ok_or_else(unknown)?;
            (Some(id), bare)
        }
        None => (None, name),
    };
    let key = key(bare).ok_or_else(unknown)?;
    let provider = match (prefix, flag) {
        (Some(id), Some(flag)) if id != flag.as_str() => {
            return Err(SettingsError::ProviderMismatch {
                prefix: id.to_owned(),
                flag: flag.to_string(),
            });
        }
        (Some(id), _) => Some(ProviderId::new(id)),
        (None, flag) => flag.cloned(),
    };
    if provider.is_some() && !key.per_provider {
        return Err(SettingsError::NotPerProvider(key.name.to_owned()));
    }
    Ok(Resolved { key, provider })
}

/// Why a `config` command refused (§6.4). The engine reports each as `EngineError::Settings`.
#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("there is no setting `{0}`; `tagteam config list` shows them all")]
    UnknownKey(String),
    #[error(
        "`{0}` is the same for every provider, so no provider table can set it; name it without `provider.<id>.` and without --provider"
    )]
    NotPerProvider(String),
    #[error(
        "`provider.{prefix}.…` and `--provider {flag}` name different providers; give only one of them"
    )]
    ProviderMismatch { prefix: String, flag: String },
    #[error("`{key}` {reason}")]
    Invalid { key: String, reason: String },
    /// A `default_provider` value naming a provider this build does not have (Decision 2).
    #[error("unknown provider {0:?}; name one this build has, such as claude-code")]
    UnknownProvider(String),
    /// `set` and `unset` never edit a file that is not valid TOML (§6.4).
    #[error("{} is not valid TOML ({detail}); fix it, then retry", path.display())]
    Corrupt { path: PathBuf, detail: String },
    /// The settings lock (§4.3) could not be taken.
    #[error(transparent)]
    Lock(#[from] LockError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
```

In `crates/tagteam-engine/src/error.rs`, replace

```rust
use crate::store::StoreError;
```

with

```rust
use crate::settings::SettingsError;
use crate::store::StoreError;
```

Insert immediately before the `Io` variant (`#[error(transparent)]` / `Io(#[from] io::Error),`):

```rust
    /// A `config` command's refusal, or its failure to write (§6.4).
    #[error(transparent)]
    Settings(#[from] SettingsError),
```

In `kind()`, replace

```rust
            EngineError::Lock(LockError::Timeout(_)) => "lock-timeout",
            EngineError::Lock(_) => "lock",
```

with

```rust
            EngineError::Lock(e) | EngineError::Settings(SettingsError::Lock(e)) => lock_kind(e),
```

and insert immediately before `EngineError::Io(_) => "io",`:

```rust
            EngineError::Settings(
                SettingsError::UnknownKey(_)
                | SettingsError::NotPerProvider(_)
                | SettingsError::ProviderMismatch { .. }
                | SettingsError::Invalid { .. }
                | SettingsError::UnknownProvider(_),
            ) => "invalid-input",
            EngineError::Settings(SettingsError::Corrupt { .. }) => "settings-unreadable",
            EngineError::Settings(SettingsError::Io(_)) => "io",
```

Add after the closing brace of `impl EngineError`, before `#[cfg(test)]`:

```rust
/// A lock failure's kind, whichever lock it was: the settings lock's (`SettingsError::Lock`)
/// reads exactly as an engine lock's (Decision 4).
fn lock_kind(e: &LockError) -> &'static str {
    match e {
        LockError::Timeout(_) => "lock-timeout",
        LockError::Compromised(_) | LockError::Io(_) => "lock",
    }
}
```

- [ ] **Step 9: Run the tests and see them pass**

Run: `cargo test -p tagteam-engine --test settings` — Expected: PASS (72 tests).
Run: `cargo test -p tagteam-engine --lib error` — Expected: PASS, including `the_settings_lock_has_the_engine_lock_s_kinds`.
Run: `cargo test -p tagteam-engine` — Expected: PASS.

- [ ] **Step 10: Format, lint, commit**

```
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p tagteam-engine
git add crates/tagteam-engine/src/settings.rs crates/tagteam-engine/src/error.rs crates/tagteam-engine/tests/settings.rs
git commit -m "Resolve both spellings of a settings key and give each settings refusal its kind"
```

---

### Task 3: `config list`, `get` and `path`; `default_provider`

> **Re-sync.**
> - **M4a Task 8** restructures `run` and `build_engine` (`build_registry`, `locate`, `run_shell`, the located `env`). Re-apply Step 8's `build_engine` change onto M4a's `build_engine(ctx, registry, run_shell, env, provider)`. The last parameter becomes `flag: Option<&ProviderId>`. Its `let default_provider = ProviderId::new(CLAUDE_CODE);` and `let (settings, warnings) = Settings::load(&env, provider);` become `let chosen = command_settings(&env, &|p| registry.get(p).is_some(), |default| flag.cloned().unwrap_or_else(|| default.clone()));`, computed before `registry` moves into `EngineConfig`. `EngineConfig` then takes `default_provider: chosen.default` and `settings: chosen.settings`, and `build_engine` returns `(engine, chosen.warnings)`. In `run`, drop `resolved` and pass `provider_flag.as_ref()`.
> - **M4a Task 15** gives `statusline::engine` the signature Step 8 gives it here: `engine(ctx: Context, flag: Option<&str>) -> (Engine, ProviderId, Arc<LazyHttp>, Arc<NoKeychain>)`. Keep M4a's body and tests. In the body, replace `let default = ProviderId::new(CLAUDE_CODE);`, the `resolve_provider` call and `let (settings, _warnings) = Settings::load(&env, &provider);` with `let chosen = command_settings(&env, &|p| registry.get(p).is_some(), |default| resolve_provider(flag, &run_shell, &registry, &ctx.env, default));`. Use `chosen.default`, `chosen.settings` and `chosen.provider` where M4a used `default`, `settings` and `provider`. Then drop the unused `Settings` and `CLAUDE_CODE` imports. M4a's Decision 13, the statusline's provider order, then ends in `default_provider`. Of Step 8's statusline test edits, only the new test `without_provider_the_line_falls_back_from_a_default_this_build_lacks` remains to add.
> - **M3a Task 6:** `run`'s body is `run_command`'s. Its exhaustive `command_name` gains `Command::Config { .. } => "config",`.
> - **M4a Task 14:** `config_answers_inside_a_run_shell` uses its `common::cc_profile` fixture. On `3c1f458` there is neither that helper nor marker-based run-shell detection, so the test is written for the merged code.
> - **M4a Task 8's refusals:** `config` changes no account, so a readable marker refuses nothing (§6.4: "`config` works inside a run shell"). An unreadable marker refuses it, as it refuses every command but `statusline` (§12.8). Nothing to add.

§6.4: "**`config list [--provider P]`** shows every key with its effective value for the provider (`default_provider` when none is given) and its source: `default`, `global` or `provider`. Unknown keys found in the file follow, flagged. **`config get KEY [--provider P]`** prints the effective value alone, for scripts: a list comma-separated, or as an array under `--json`. … **`config path`** prints the file's path, whether or not it exists." "JSON shapes: `list` returns `{schemaVersion, path, provider, keys: [{key, value, default, source}], unknown: [key]}`; `get` returns `{schemaVersion, key, provider, value, source}`; … `path` returns `{schemaVersion, path, exists}`." §13.1: "Which provider a command acts on: 1. an explicit `--provider` … 3. `default_provider` (`claude-code`)". The table: `default_provider` must be "a registered `ProviderId`". Decision 2: the CLI checks it, and "a provider this build lacks warns and falls back to `claude-code`". Decision 14: "an unknown key still reaches the engine's `invalid-input` … rather than clap's usage error". This task adds the three read-only `config` commands over Task 2's `inspect` and `resolve`. It also makes `default_provider` the provider a command acts on when `--provider` is absent, for every command and for `statusline`. None of the `config` reads touches the store or the Keychain, and none creates anything (§5).

**Readings of the spec this task commits to:**
- **An unregistered `default_provider` warns on every command,** as any invalid setting does, `--provider` or not. Like every settings warning, it starts with the file's full path, not `config.toml:`. `statusline` drops it, as it drops every settings warning.
- **The file is read once per command,** for claude-code. It is read a second time only when the command acts on another provider, whose own tables must come first (§6.4). The warnings printed are those of that second read alone. `default_provider` sits at the top level, so either read gives it.
- **`config get` names its key without the prefix** in JSON (`"key": "statusline.format"`). The `provider` field names the provider whose view was read: the prefix's, else `--provider`, else the default. That is the default even for a key no provider table holds.
- **`config get KEY --provider P` with a key no provider table can hold is `invalid-input`** (Task 2's `resolve`), because it names the same entry as `provider.P.KEY`.
- **A provider named in the key's prefix that this build lacks is `unknown-provider`,** exactly as `--provider` is. `provider.<id>.<key>` and `<key> --provider <id>` name the same entry (§6.4), so the two spellings fail alike (Decision 4).
- **`config list`'s text form** is a `KEY  SOURCE  VALUE` table, in registry order. Each value is printed as `config get` prints it, except that an empty value shows as `''`, the argument `config set` takes for it. A block of unknown keys follows. The spec gives no text layout.
- **An unknown key is listed, never warned about.** Invalid values still warn on stderr, once, from the read every command does. The `config` commands do not print the same warnings a second time.
- **`config get` prints an empty list as an empty line.**
- **`config path`'s `exists` follows a symlink.** A link whose target is missing does not exist, which matches what a read finds there.
- **`--provider`'s help** now says the default is the `default_provider` setting.

**Files:**
- **3a:**
  - Modify: `Cargo.toml` (`clap`'s features, :18)
  - Modify: `crates/tagteam/src/cli.rs` (imports :1; `Command::Config` after `Command::Statusline`, :99-104; `ConfigAction`, `COMPLETED_PROVIDERS` and `ConfigKeyParser` at the end of the file)
  - Create: `crates/tagteam/src/config_cmd.rs`
  - Modify: `crates/tagteam/src/lib.rs` (module list, :8)
  - Modify: `crates/tagteam/src/app.rs` (imports :16, :26, :28; `dispatch`'s last arm, :686; new `App::config` after `dispatch`)
  - Test: `crates/tagteam/tests/config_cli.rs` (new)
- **3b:**
  - Modify: `crates/tagteam/src/app.rs` (new `CommandSettings` and `command_settings`, and `build_engine`, :141-182; `run` :310-314; `run_statusline` :376-378; unit tests after `an_empty_colour_variable_is_as_good_as_unset`, :935-941)
  - Modify: `crates/tagteam/src/statusline.rs` (imports :9, :13, :22, :282; `engine` :195-218; tests :598-653 and one new test)
  - Modify: `crates/tagteam/src/cli.rs` (`Cli.provider`'s doc, :20)
  - Test: `crates/tagteam/tests/config_cli.rs` (one more test)

**Interfaces:**
- Consumes:
  - Task 2: `settings::{inspect, resolve, config_path, Inspection, KeyState, Resolved, KEYS, DEFAULT_STATUSLINE_FORMAT}`, `Value::{display, to_json}`, `Source::as_str`, `Settings.default_provider`, `EngineError::Settings` (through `From<SettingsError>`).
  - Existing: `App::{print, provider}`, `App.provider_flag`, `Engine::{env, provider, default_provider, settings}`, `ProviderRegistry::get`, `fail`, `Failure`, `common::cmd`; M4a Task 14's `common::cc_profile`.
- Produces:
  - `tagteam::cli::Command::Config { action: ConfigAction }` and `pub enum ConfigAction { List, Get { key: String }, Path }` (`#[derive(Subcommand)]`; Task 4 adds `Set` and `Unset`).
  - `#[derive(Clone)] pub struct ConfigKeyParser`, `impl clap::builder::TypedValueParser<Value = String>`: it parses any UTF-8 string, and its `possible_values()` gives every `KEYS` name, then `provider.claude-code.<name>` for each per-provider key (Tasks 4 and 10).
  - `pub(crate) const COMPLETED_PROVIDERS: &[&str]` in `cli.rs` (`[CLAUDE_CODE]`; Task 10).
  - `crate::config_cmd::{list_human(&Inspection) -> String, list_json(&Inspection, &ProviderId) -> serde_json::Value, get_human(&KeyState) -> String, get_json(&KeyState, &ProviderId) -> serde_json::Value, path_human(&Path) -> String, path_json(&Path, bool) -> serde_json::Value}` (all `pub(crate)`; Task 4 adds its own).
  - `App::config(&mut self, action: ConfigAction) -> Result<(), Failure>` (Task 4 adds arms).
  - `pub(crate) struct CommandSettings { pub(crate) provider: ProviderId, pub(crate) default: ProviderId, pub(crate) settings: Settings, pub(crate) warnings: Vec<String> }` and `pub(crate) fn command_settings(env: &Env, registered: &dyn Fn(&ProviderId) -> bool, choose: impl FnOnce(&ProviderId) -> ProviderId) -> CommandSettings` in `app.rs`.
  - `fn build_engine(ctx: Context, flag: Option<&ProviderId>) -> (Engine, Vec<String>)`; `statusline::engine(ctx: Context, flag: Option<&str>) -> (Engine, ProviderId, Arc<LazyHttp>, Arc<NoKeychain>)`.

#### 3a: `config list`, `get` and `path`

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam/tests/config_cli.rs`:

```rust
//! `tagteam config list|get|path` through the binary (§6.4), and `default_provider` (§13.1).
//! Needs `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::cmd;
use serde_json::{Value, json};

/// Writes `text` as the `config.toml` of `root`'s fixture HOME, and returns its path.
fn write_config(root: &Path, text: &str) -> PathBuf {
    let dir = root.join("home/.config/tagteam");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    fs::write(&path, text).unwrap();
    path
}

/// A successful command's stdout and stderr.
fn ok(root: &Path, args: &[&str]) -> (String, String) {
    let out = cmd(root).args(args).assert().success().get_output().clone();
    (
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

/// A successful `--json` command's object; it warns about nothing.
fn ok_json(root: &Path, args: &[&str]) -> Value {
    let (out, err) = ok(root, args);
    assert_eq!(err, "", "{args:?}");
    serde_json::from_str(&out).unwrap()
}

#[test]
fn the_config_reads_create_nothing_and_never_ask_the_keychain() {
    // §5: a command that changes nothing creates nothing. A locked keychain refuses none of
    // them: `config` touches no Keychain item, so it runs no lock check.
    let d = tempfile::tempdir().unwrap();
    fs::create_dir_all(d.path().join("home")).unwrap();
    fs::create_dir_all(d.path().join("keychain")).unwrap();
    fs::write(d.path().join("keychain/LOCKED"), "").unwrap();
    let cases: [&[&str]; 6] = [
        &["config", "list"],
        &["config", "list", "--json"],
        &["config", "get", "autoswitch.threshold"],
        &[
            "config",
            "get",
            "provider.claude-code.autoswitch.models",
            "--json",
        ],
        &["config", "path"],
        &["config", "path", "--json"],
    ];
    for args in cases {
        cmd(d.path()).args(args).assert().success().stderr("");
    }
    assert!(
        fs::read_dir(d.path().join("home"))
            .unwrap()
            .next()
            .is_none(),
        "HOME must stay empty"
    );
    let keychain: Vec<_> = fs::read_dir(d.path().join("keychain"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(keychain, ["LOCKED"]);
}

#[test]
fn config_path_prints_the_path_whether_or_not_the_file_exists() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("home/.config/tagteam/config.toml");
    let (out, _) = ok(d.path(), &["config", "path"]);
    assert_eq!(out, format!("{}\n", path.display()));
    assert_eq!(
        ok_json(d.path(), &["config", "path", "--json"]),
        json!({"schemaVersion": 1, "path": path.display().to_string(), "exists": false})
    );
    write_config(d.path(), "");
    assert_eq!(
        ok_json(d.path(), &["config", "path", "--json"])["exists"],
        true
    );
    // An absolute XDG_CONFIG_HOME moves it (§5).
    let xdg = d.path().join("xdg");
    let out = cmd(d.path())
        .env("XDG_CONFIG_HOME", &xdg)
        .args(["config", "path"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        String::from_utf8(out).unwrap(),
        format!("{}\n", xdg.join("tagteam/config.toml").display())
    );
}

/// A file with a provider override, a global value and a key the registry does not know.
const MIXED: &str = "[autoswitch]\nthreshold = 80\nmodels = [\"Opus\"]\nthresold = 1\n\n\
                     [provider.claude-code.autoswitch]\nmodels = []\n";

#[test]
fn config_list_shows_every_key_with_its_value_and_source_then_the_unknown_ones() {
    let d = tempfile::tempdir().unwrap();
    write_config(d.path(), MIXED);
    let (out, err) = ok(d.path(), &["config", "list"]);
    assert_eq!(err, "", "an unknown key is listed, not warned about");
    assert_eq!(
        out,
        concat!(
            "KEY                                  SOURCE    VALUE\n",
            "default_provider                     default   claude-code\n",
            "autoswitch.threshold                 global    80\n",
            "autoswitch.interval_seconds          default   60\n",
            "autoswitch.cooldown_seconds          default   300\n",
            "autoswitch.hysteresis_pct            default   10\n",
            "autoswitch.strategy                  default   best\n",
            "autoswitch.include_api_key_accounts  default   false\n",
            "autoswitch.unhealthy_ticks           default   3\n",
            "autoswitch.models                    provider  ''\n",
            "usage.history_retention_days         default   180\n",
            "statusline.format                    default   {account} · 5h {5h}% · 7d {7d}%{stale}\n",
            "run.share_extra                      default   ''\n",
            "ui.color                             default   auto\n",
            "\n",
            "Unknown keys, ignored:\n",
            "  autoswitch.thresold\n",
        )
    );
}

#[test]
fn config_list_json_is_the_specs_shape() {
    let d = tempfile::tempdir().unwrap();
    let path = write_config(d.path(), MIXED);
    let row = |key: &str, value: Value, default: Value, source: &str| json!({"key": key, "value": value, "default": default, "source": source});
    let format = json!("{account} · 5h {5h}% · 7d {7d}%{stale}");
    assert_eq!(
        ok_json(d.path(), &["config", "list", "--json"]),
        json!({
            "schemaVersion": 1,
            "path": path.display().to_string(),
            "provider": "claude-code",
            "keys": [
                row("default_provider", json!("claude-code"), json!("claude-code"), "default"),
                row("autoswitch.threshold", json!(80.0), json!(90.0), "global"),
                row("autoswitch.interval_seconds", json!(60), json!(60), "default"),
                row("autoswitch.cooldown_seconds", json!(300), json!(300), "default"),
                row("autoswitch.hysteresis_pct", json!(10.0), json!(10.0), "default"),
                row("autoswitch.strategy", json!("best"), json!("best"), "default"),
                row("autoswitch.include_api_key_accounts", json!(false), json!(false), "default"),
                row("autoswitch.unhealthy_ticks", json!(3), json!(3), "default"),
                row("autoswitch.models", json!([]), json!([]), "provider"),
                row("usage.history_retention_days", json!(180), json!(180), "default"),
                row("statusline.format", format.clone(), format, "default"),
                row("run.share_extra", json!([]), json!([]), "default"),
                row("ui.color", json!("auto"), json!("auto"), "default"),
            ],
            "unknown": ["autoswitch.thresold"],
        })
    );
}

#[test]
fn config_get_prints_the_effective_value_alone() {
    let d = tempfile::tempdir().unwrap();
    write_config(
        d.path(),
        "[autoswitch]\nthreshold = 80\nmodels = [\"Fable\", \"Opus\"]\n\
         [provider.claude-code.statusline]\nformat = \"{7d}\"\n",
    );
    for (key, want) in [
        ("autoswitch.threshold", "80\n"),
        ("autoswitch.models", "Fable,Opus\n"),
        ("autoswitch.strategy", "best\n"),
        ("run.share_extra", "\n"),
        ("statusline.format", "{7d}\n"),
        ("default_provider", "claude-code\n"),
    ] {
        assert_eq!(
            ok(d.path(), &["config", "get", key]),
            (want.to_owned(), String::new()),
            "{key}"
        );
    }
    assert_eq!(
        ok_json(d.path(), &["config", "get", "autoswitch.models", "--json"]),
        json!({"schemaVersion": 1, "key": "autoswitch.models", "provider": "claude-code",
               "value": ["Fable", "Opus"], "source": "global"})
    );
    // `provider.<id>.<key>` and `<key> --provider <id>` name the same entry (§6.4).
    let cases: [&[&str]; 3] = [
        &[
            "config",
            "get",
            "provider.claude-code.statusline.format",
            "--json",
        ],
        &[
            "config",
            "get",
            "statusline.format",
            "--provider",
            "claude-code",
            "--json",
        ],
        &["config", "get", "statusline.format", "--json"],
    ];
    for args in cases {
        assert_eq!(
            ok_json(d.path(), args),
            json!({"schemaVersion": 1, "key": "statusline.format", "provider": "claude-code",
                   "value": "{7d}", "source": "provider"}),
            "{args:?}"
        );
    }
}

#[test]
fn a_key_config_cannot_name_is_invalid_input_not_a_usage_error() {
    // Decision 14: the KEY parser takes any string, so the registry's refusal is the one
    // reported, with the engine's kind.
    const NOT_PER_PROVIDER: &str = "`ui.color` is the same for every provider, so no provider table can set it; name it without `provider.<id>.` and without --provider";
    let d = tempfile::tempdir().unwrap();
    let cases: [(&[&str], &str); 4] = [
        (
            &["config", "get", "autoswitch.thresold"],
            "there is no setting `autoswitch.thresold`; `tagteam config list` shows them all",
        ),
        (
            &["config", "get", "provider.claude-code.ui.color"],
            NOT_PER_PROVIDER,
        ),
        (
            &["config", "get", "ui.color", "--provider", "claude-code"],
            NOT_PER_PROVIDER,
        ),
        (
            &[
                "config",
                "get",
                "provider.fake-agent.autoswitch.threshold",
                "--provider",
                "claude-code",
            ],
            "`provider.fake-agent.…` and `--provider claude-code` name different providers; give only one of them",
        ),
    ];
    for (args, message) in cases {
        cmd(d.path())
            .args(args)
            .assert()
            .code(1)
            .stdout("")
            .stderr(format!("tagteam: {message}\n"));
        let out = cmd(d.path())
            .args(args)
            .arg("--json")
            .assert()
            .code(1)
            .get_output()
            .stdout
            .clone();
        assert_eq!(
            serde_json::from_slice::<Value>(&out).unwrap(),
            json!({"schemaVersion": 1, "error": {"type": "invalid-input", "message": message}}),
            "{args:?}"
        );
    }
    // A provider this build lacks is refused alike in both spellings, as `--provider` always was.
    let cases: [(&[&str], &str); 3] = [
        (
            &[
                "config",
                "get",
                "provider.fake-agent.autoswitch.threshold",
                "--json",
            ],
            "fake-agent",
        ),
        (
            &[
                "config",
                "get",
                "autoswitch.threshold",
                "--provider",
                "fake-agent",
                "--json",
            ],
            "fake-agent",
        ),
        (
            &[
                "config",
                "get",
                "provider.Claude.autoswitch.threshold",
                "--json",
            ],
            "Claude",
        ),
    ];
    for (args, provider) in cases {
        let out = cmd(d.path())
            .args(args)
            .assert()
            .code(1)
            .get_output()
            .stdout
            .clone();
        assert_eq!(
            serde_json::from_slice::<Value>(&out).unwrap(),
            json!({"schemaVersion": 1, "error": {"type": "unknown-provider",
                   "message": format!("unknown provider {provider:?}")}}),
            "{args:?}"
        );
    }
}

#[test]
fn settings_warnings_go_to_stderr_and_config_still_answers() {
    let d = tempfile::tempdir().unwrap();
    let path = write_config(d.path(), "[autoswitch\n");
    let warning = format!(
        "warning: {}: the settings file is not valid TOML; using the defaults\n",
        path.display()
    );
    let (out, err) = ok(d.path(), &["config", "get", "autoswitch.threshold"]);
    assert_eq!((out.as_str(), err.as_str()), ("90\n", warning.as_str()));
    let (out, err) = ok(d.path(), &["config", "list", "--json"]);
    assert_eq!(err, warning);
    let v: Value = serde_json::from_str(&out).unwrap();
    assert!(
        v["keys"]
            .as_array()
            .unwrap()
            .iter()
            .all(|k| k["source"] == "default"),
        "{v}"
    );
}

#[test]
fn config_answers_inside_a_run_shell() {
    // §6.4: settings are not accounts, so a run shell (§12.8) refuses none of `config`.
    let d = tempfile::tempdir().unwrap();
    let (_profile, shell) = common::cc_profile(d.path(), "0192-not-managed");
    let path = write_config(d.path(), "[autoswitch]\nthreshold = 80\n");
    let inside = |args: &[&str]| -> String {
        let out = cmd(d.path())
            .env("CLAUDE_CONFIG_DIR", &shell)
            .args(args)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        String::from_utf8(out).unwrap()
    };
    assert_eq!(inside(&["config", "get", "autoswitch.threshold"]), "80\n");
    assert_eq!(inside(&["config", "path"]), format!("{}\n", path.display()));
    assert!(inside(&["config", "list"]).starts_with("KEY "));
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam --features test-support --test config_cli`
Expected: every test fails at its first `config` run with `Unexpected failure` and `code=2`: `config` is not a subcommand yet, so clap reports a usage error.

- [ ] **Step 3: Implement**

In `Cargo.toml`, replace

```toml
clap = { version = "4", features = ["derive"] }
```

with

```toml
clap = { version = "4", features = ["derive", "string"] }
```

In `crates/tagteam/src/cli.rs`, replace `use clap::{Parser, Subcommand};` with:

```rust
use std::ffi::OsStr;

use clap::builder::{PossibleValue, StringValueParser, TypedValueParser};
use clap::{Parser, Subcommand};
use tagteam_core::CLAUDE_CODE;
use tagteam_engine::settings::KEYS;
```

Add after the `Command::Statusline { … }` variant, as the last variant of `Command`:

```rust
    /// The settings in config.toml
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
```

and append at the end of the file, after `impl Command`:

```rust
/// `tagteam config` (§6.4).
#[derive(Subcommand)]
pub enum ConfigAction {
    /// Every setting, with its value and where the value comes from
    List,
    /// One setting's value
    Get {
        /// A setting, such as autoswitch.threshold or provider.claude-code.autoswitch.models
        #[arg(value_parser = ConfigKeyParser, hide_possible_values = true)]
        key: String,
    },
    /// Where the settings file is, whether or not it exists
    Path,
}

/// The providers this build registers, as completion offers them (§13.7).
pub(crate) const COMPLETED_PROVIDERS: &[&str] = &[CLAUDE_CODE];

/// A `config` command's KEY. Any UTF-8 string parses, so a key the registry does not know reaches
/// the engine's `invalid-input` (§6.4) rather than a usage error, while completion offers every
/// registry key and its `provider.<id>.` spelling (§13.7, Decision 14).
#[derive(Clone)]
pub struct ConfigKeyParser;

impl TypedValueParser for ConfigKeyParser {
    type Value = String;

    fn parse_ref(
        &self,
        cmd: &clap::Command,
        arg: Option<&clap::Arg>,
        value: &OsStr,
    ) -> Result<String, clap::Error> {
        StringValueParser::new().parse_ref(cmd, arg, value)
    }

    fn possible_values(&self) -> Option<Box<dyn Iterator<Item = PossibleValue> + '_>> {
        let bare = KEYS.iter().map(|k| k.name.to_owned());
        let prefixed = COMPLETED_PROVIDERS.iter().flat_map(|provider| {
            KEYS.iter()
                .filter(|k| k.per_provider)
                .map(move |k| format!("provider.{provider}.{}", k.name))
        });
        Some(Box::new(bare.chain(prefixed).map(PossibleValue::new)))
    }
}
```

Create `crates/tagteam/src/config_cmd.rs`:

```rust
//! `tagteam config list|get|path` (§6.4): the human and JSON forms of what `settings` reads.

use std::path::Path;

use serde_json::{Value, json};
use tagteam_core::ProviderId;
use tagteam_engine::settings::{Inspection, KeyState};

/// How `config list` shows an empty value: as the empty argument `config set` takes.
const EMPTY: &str = "''";

/// Every key in the registry's order, with the table its value comes from and the value, then
/// the keys the registry does not know.
pub(crate) fn list_human(inspection: &Inspection) -> String {
    let key_w = inspection
        .keys
        .iter()
        .map(|s| s.key.name.len())
        .fold("KEY".len(), usize::max);
    let mut out = format!("{:<key_w$}  {:<8}  VALUE\n", "KEY", "SOURCE");
    for s in &inspection.keys {
        out.push_str(&format!(
            "{:<key_w$}  {:<8}  {}\n",
            s.key.name,
            s.source.as_str(),
            shown(s)
        ));
    }
    if !inspection.unknown.is_empty() {
        out.push_str("\nUnknown keys, ignored:\n");
        for key in &inspection.unknown {
            out.push_str(&format!("  {key}\n"));
        }
    }
    out
}

fn shown(s: &KeyState) -> String {
    let text = s.value.display();
    if text.is_empty() {
        EMPTY.to_owned()
    } else {
        text
    }
}

/// `{schemaVersion, path, provider, keys: [{key, value, default, source}], unknown: [key]}`.
pub(crate) fn list_json(inspection: &Inspection, provider: &ProviderId) -> Value {
    let keys: Vec<Value> = inspection
        .keys
        .iter()
        .map(|s| {
            json!({
                "key": s.key.name,
                "value": s.value.to_json(),
                "default": s.default.to_json(),
                "source": s.source.as_str(),
            })
        })
        .collect();
    json!({
        "schemaVersion": 1,
        "path": inspection.path.display().to_string(),
        "provider": provider.as_str(),
        "keys": keys,
        "unknown": inspection.unknown,
    })
}

/// The value alone, for scripts: a list comma-separated, and the empty list as an empty line.
pub(crate) fn get_human(state: &KeyState) -> String {
    format!("{}\n", state.value.display())
}

/// `{schemaVersion, key, provider, value, source}`, with a list as an array.
pub(crate) fn get_json(state: &KeyState, provider: &ProviderId) -> Value {
    json!({
        "schemaVersion": 1,
        "key": state.key.name,
        "provider": provider.as_str(),
        "value": state.value.to_json(),
        "source": state.source.as_str(),
    })
}

pub(crate) fn path_human(path: &Path) -> String {
    format!("{}\n", path.display())
}

/// `{schemaVersion, path, exists}`.
pub(crate) fn path_json(path: &Path, exists: bool) -> Value {
    json!({"schemaVersion": 1, "path": path.display().to_string(), "exists": exists})
}
```

In `crates/tagteam/src/lib.rs`, replace `pub mod cli;` with:

```rust
pub mod cli;
mod config_cmd;
```

In `crates/tagteam/src/app.rs`, change three imports (today :16, :26 and :28):
- `use tagteam_engine::settings::{ColorMode, Settings};` becomes `use tagteam_engine::settings::{self, ColorMode, Settings};`
- `use crate::cli::{Cli, Command};` becomes `use crate::cli::{Cli, Command, ConfigAction};`
- `use crate::{history, render, root_guard, statusline};` becomes `use crate::{config_cmd, history, render, root_guard, statusline};`

In `App::dispatch`, after the arm `Command::Statusline { .. } => unreachable!("run answers statusline before dispatch"),`, add:

```rust
            Command::Config { action } => self.config(action)?,
```

and add this method to `impl App`, directly after `dispatch`:

```rust
    /// §6.4's reads. None touches the store or the Keychain, and none creates anything (§5).
    /// `list` and `get` show a read of their own, so they print that read's warnings, less
    /// those `run` already printed from the engine's read: the two agree unless the file
    /// changed in between, and then the warnings match what is shown.
    fn config(&mut self, action: ConfigAction) -> Result<(), Failure> {
        match action {
            ConfigAction::List => {
                let provider = self.provider();
                let inspection = settings::inspect(self.engine.env(), &provider);
                self.read_warnings(&inspection.warnings);
                self.print(
                    &config_cmd::list_human(&inspection),
                    config_cmd::list_json(&inspection, &provider),
                );
            }
            ConfigAction::Get { key } => {
                let resolved = settings::resolve(&key, self.provider_flag.as_ref())
                    .map_err(EngineError::from)?;
                // A provider named in the key is checked as `--provider` is.
                let provider = match resolved.provider {
                    Some(p) => {
                        self.engine.provider(&p)?;
                        p
                    }
                    None => self.provider(),
                };
                let inspection = settings::inspect(self.engine.env(), &provider);
                self.read_warnings(&inspection.warnings);
                let state = inspection
                    .keys
                    .iter()
                    .find(|s| s.key.name == resolved.key.name)
                    .expect("inspect reports every registry key");
                self.print(
                    &config_cmd::get_human(state),
                    config_cmd::get_json(state, &provider),
                );
            }
            ConfigAction::Path => {
                let path = settings::config_path(self.engine.env());
                let exists = path.exists();
                self.print(
                    &config_cmd::path_human(&path),
                    config_cmd::path_json(&path, exists),
                );
            }
        }
        Ok(())
    }

    /// Prints the warnings of a read `run` did not make (§6.4), less those `run` printed.
    fn read_warnings(&mut self, warnings: &[String]) {
        for w in warnings.iter().filter(|w| !self.settings_warnings.contains(w)) {
            let _ = writeln!(self.io.err, "warning: {w}");
        }
    }
```

The warnings `run` printed travel to the `App`, so `read_warnings` can leave them out. In `struct App`, after the field `provider_flag: Option<ProviderId>,` add:

```rust
    /// The settings warnings `run` printed from the engine's read (§6.4).
    settings_warnings: Vec<String>,
```

and in `run`, in the `App { … }` literal, after `provider_flag,` add:

```rust
        settings_warnings: warnings,
```

The loop that prints `warnings` borrows them (`for w in &warnings`) before the literal moves them.

- [ ] **Step 4: Run the tests and see them pass**

Run: `cargo test -p tagteam --features test-support --test config_cli` — Expected: PASS (8 tests).
Run: `cargo test -p tagteam --features test-support` — Expected: PASS.

- [ ] **Step 5: Format, lint, commit**

```
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p tagteam --features test-support
git add Cargo.toml crates/tagteam/src/cli.rs crates/tagteam/src/config_cmd.rs crates/tagteam/src/lib.rs crates/tagteam/src/app.rs crates/tagteam/tests/config_cli.rs
git commit -m "Show the settings with config list, get and path"
```

#### 3b: `default_provider`

- [ ] **Step 6: Write the failing tests**

In `crates/tagteam/src/app.rs`'s `mod tests`, insert before `fn an_unreadable_active_flag_after_a_commit_is_inactive_not_an_error` (and its `#[test]`):

```rust
    /// A fresh environment under `dir` whose `config.toml` holds `text`.
    fn env_with_config(dir: &std::path::Path, text: &str) -> Env {
        let env = Env::for_test(dir);
        std::fs::create_dir_all(env.config_dir()).unwrap();
        std::fs::write(settings::config_path(&env), text).unwrap();
        env
    }

    fn claude_code_only(p: &ProviderId) -> bool {
        p.as_str() == CLAUDE_CODE
    }

    fn and_other(p: &ProviderId) -> bool {
        [CLAUDE_CODE, "other"].contains(&p.as_str())
    }

    #[test]
    fn with_no_settings_file_a_command_acts_on_claude_code_and_creates_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path());
        let chosen = command_settings(&env, &claude_code_only, |d| d.clone());
        assert_eq!(
            (chosen.provider.as_str(), chosen.default.as_str()),
            (CLAUDE_CODE, CLAUDE_CODE)
        );
        assert!(chosen.warnings.is_empty(), "{:?}", chosen.warnings);
        assert!(!env.config_dir().exists());
    }

    #[test]
    fn without_provider_a_command_acts_on_a_registered_default_provider() {
        // §13.1 rule 3: the default provider's own tables come first (§6.4).
        let dir = tempfile::tempdir().unwrap();
        let env = env_with_config(
            dir.path(),
            "default_provider = \"other\"\n[statusline]\nformat = \"{5h}\"\n\
             [provider.other.statusline]\nformat = \"{7d}\"\n",
        );
        let chosen = command_settings(&env, &and_other, |d| d.clone());
        assert_eq!(
            (chosen.provider.as_str(), chosen.default.as_str()),
            ("other", "other")
        );
        assert_eq!(chosen.settings.statusline_format, "{7d}");
        assert!(chosen.warnings.is_empty(), "{:?}", chosen.warnings);
        // `--provider` still wins, with its own tables; the default stays what it is.
        let chosen = command_settings(&env, &and_other, |_| ProviderId::new(CLAUDE_CODE));
        assert_eq!(
            (chosen.provider.as_str(), chosen.default.as_str()),
            (CLAUDE_CODE, "other")
        );
        assert_eq!(chosen.settings.statusline_format, "{5h}");
    }

    #[test]
    fn a_default_provider_this_build_lacks_falls_back_to_claude_code_with_a_warning() {
        let dir = tempfile::tempdir().unwrap();
        let env = env_with_config(
            dir.path(),
            "default_provider = \"other\"\n[provider.other.statusline]\nformat = \"{7d}\"\n",
        );
        let chosen = command_settings(&env, &claude_code_only, |d| d.clone());
        assert_eq!(
            (chosen.provider.as_str(), chosen.default.as_str()),
            (CLAUDE_CODE, CLAUDE_CODE)
        );
        assert_eq!(
            chosen.settings.statusline_format,
            settings::DEFAULT_STATUSLINE_FORMAT,
            "claude-code's tables, not other's"
        );
        assert_eq!(
            chosen.settings.default_provider.as_str(),
            "other",
            "the setting itself reads as written"
        );
        assert_eq!(
            chosen.warnings,
            [format!(
                "{}: `default_provider` names other, which this build does not have; using claude-code",
                settings::config_path(&env).display()
            )]
        );
    }

    #[test]
    fn the_warnings_are_those_of_the_provider_the_command_acts_on_once() {
        let dir = tempfile::tempdir().unwrap();
        let env = env_with_config(
            dir.path(),
            "[autoswitch]\nthreshold = 5\n[provider.other.autoswitch]\nthreshold = 120\n",
        );
        let chosen = command_settings(&env, &and_other, |_| ProviderId::new("other"));
        assert_eq!(chosen.warnings.len(), 2, "{:?}", chosen.warnings);
        assert!(chosen.warnings[0].contains("`provider.other.autoswitch.threshold`"));
        assert!(chosen.warnings[1].contains("`autoswitch.threshold`"));
        let chosen = command_settings(&env, &and_other, |d| d.clone());
        assert_eq!(chosen.warnings.len(), 1, "{:?}", chosen.warnings);
        assert!(chosen.warnings[0].contains("`autoswitch.threshold`"));
    }
```

In `crates/tagteam/src/statusline.rs`'s `mod tests`:
- In `the_settings_are_those_of_the_provider_the_command_resolves`, replace `engine(ctx, &ProviderId::new(provider))` with `engine(ctx, Some(provider))`.
- In `the_engine_reaches_neither_the_keychain_nor_the_network`, replace `let (built, http, keychain) = engine(ctx, &provider);` with:

```rust
        let (built, resolved, http, keychain) = engine(ctx, None);
        assert_eq!(
            resolved, provider,
            "no --provider and no default_provider: claude-code"
        );
```

- Insert before `the_engine_reaches_neither_the_keychain_nor_the_network` (and its `#[test]`):

```rust
    #[test]
    fn without_provider_the_line_falls_back_from_a_default_this_build_lacks() {
        // §13.5's last step is `default_provider`; one this build lacks is claude-code, and the
        // status bar has nowhere to show the warning.
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path());
        std::fs::create_dir_all(env.config_dir()).unwrap();
        std::fs::write(
            env.config_dir().join("config.toml"),
            "default_provider = \"other\"\n[provider.other.statusline]\nformat = \"{7d}\"\n",
        )
        .unwrap();
        let ctx = Context {
            env,
            keychain: Arc::new(FakeKeychain::new()),
            platform: Platform::MacOs,
            api_base: None,
            stdout_terminal: false,
            no_color_env: false,
            force_color_env: false,
        };
        let (built, provider, _, _) = engine(ctx, None);
        assert_eq!(provider.as_str(), CLAUDE_CODE);
        assert_eq!(built.default_provider().as_str(), CLAUDE_CODE);
        assert_eq!(
            built.settings().statusline_format,
            DEFAULT_STATUSLINE_FORMAT
        );
    }
```

Append to `crates/tagteam/tests/config_cli.rs`:

```rust

#[test]
fn a_default_provider_this_build_lacks_warns_and_claude_code_is_used() {
    // §13.1 rule 3 and Decision 2: the setting reads as written, and the CLI falls back.
    let d = tempfile::tempdir().unwrap();
    let path = write_config(d.path(), "default_provider = \"fake-agent\"\n");
    let warning = format!(
        "warning: {}: `default_provider` names fake-agent, which this build does not have; using claude-code\n",
        path.display()
    );
    let (out, err) = ok(d.path(), &["config", "get", "default_provider"]);
    assert_eq!(
        (out.as_str(), err.as_str()),
        ("fake-agent\n", warning.as_str())
    );
    let (out, err) = ok(d.path(), &["config", "list", "--json"]);
    assert_eq!(err, warning);
    assert_eq!(
        serde_json::from_str::<Value>(&out).unwrap()["provider"],
        "claude-code"
    );
    let (out, err) = ok(d.path(), &["status", "--json"]);
    assert_eq!(err, warning);
    assert_eq!(
        out,
        "{\"schemaVersion\":1,\"provider\":\"claude-code\",\"active\":null}\n"
    );
    // The status bar has nowhere to show the warning, and falls back alike (§13.5).
    cmd(d.path())
        .arg("statusline")
        .assert()
        .success()
        .stdout("")
        .stderr("");
}
```

- [ ] **Step 7: Run them and see them fail**

Run: `cargo test -p tagteam --lib`
Expected: compile errors `cannot find function command_settings in this scope` (E0425) in `app.rs`'s tests, and `mismatched types` (E0308) at `engine(ctx, Some(provider))` and `engine(ctx, None)` (`expected &ProviderId`).

Run: `cargo test -p tagteam --features test-support --test config_cli a_default_provider`
Expected: FAIL. `config get default_provider` prints `fake-agent`, but its stderr is empty where the `default_provider` warning belongs.

- [ ] **Step 8: Implement**

In `crates/tagteam/src/app.rs`, replace `build_engine` and its doc comment (from `/// The engine for one command, and the settings warnings` to the function's closing brace, today :141-182) with:

```rust
/// The settings one command runs with (§6.4), and the providers it was read for.
pub(crate) struct CommandSettings {
    /// The provider the command acts on. Its own tables came first.
    pub(crate) provider: ProviderId,
    /// `default_provider` when this build has it, else claude-code (§13.1, Decision 2).
    pub(crate) default: ProviderId,
    pub(crate) settings: Settings,
    /// The settings warnings, then the CLI's own for a `default_provider` this build lacks.
    pub(crate) warnings: Vec<String>,
}

/// §13.1's rule 3 and Decision 2: without `--provider`, a command acts on `default_provider`
/// when `registered` has it, and on claude-code otherwise, with a warning. `choose` is given that
/// default and names the provider the command acts on. `default_provider` sits at the top
/// level, so any provider's read of the file gives it: the file is read for claude-code, and
/// read again only when `choose` names another provider, whose own tables come first.
pub(crate) fn command_settings(
    env: &Env,
    registered: &dyn Fn(&ProviderId) -> bool,
    choose: impl FnOnce(&ProviderId) -> ProviderId,
) -> CommandSettings {
    let fallback = ProviderId::new(CLAUDE_CODE);
    let (mut settings, mut warnings) = Settings::load(env, &fallback);
    let configured = settings.default_provider.clone();
    let (default, unregistered) = if registered(&configured) {
        (configured, None)
    } else {
        let warning = format!(
            "{}: `default_provider` names {configured}, which this build does not have; using {CLAUDE_CODE}",
            settings::config_path(env).display()
        );
        (fallback.clone(), Some(warning))
    };
    let provider = choose(&default);
    if provider != fallback {
        (settings, warnings) = Settings::load(env, &provider);
    }
    warnings.extend(unregistered);
    CommandSettings {
        provider,
        default,
        settings,
        warnings,
    }
}

/// The engine for one command, and the settings warnings for the caller to print (§6.4). The
/// settings are those of the provider the command acts on (`--provider`, else the default),
/// its own tables first. The HTTP adapter is built on its first request, never before (§13.5),
/// and the oracle sends through the same one.
fn build_engine(ctx: Context, flag: Option<&ProviderId>) -> (Engine, Vec<String>) {
    let mut cc = ClaudeCode::new(ctx.keychain.clone(), ctx.platform);
    if let Some(base) = &ctx.api_base {
        cc = cc.with_endpoints(Endpoints::with_base(base));
    }
    let registry = ProviderRegistry::new().with(Arc::new(cc));
    let vault = match ctx.platform {
        Platform::MacOs => Vault::new(Box::new(KeychainVault::new(ctx.keychain))),
        Platform::Linux => Vault::new(Box::new(FileVault::new(ctx.env.data_dir().join("vault")))),
    };
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    // `api_base` only ever carries a test base: it is sent to directly, never through whatever
    // proxy the machine's environment names, so test traffic cannot leave the machine.
    let direct = ctx.api_base.is_some();
    let http: Arc<dyn Http> = Arc::new(LazyHttp::new(move || -> Arc<dyn Http> {
        Arc::new(if direct {
            UreqHttp::direct()
        } else {
            UreqHttp::new()
        })
    }));
    let chosen = command_settings(&ctx.env, &|p| registry.get(p).is_some(), |default| {
        flag.cloned().unwrap_or_else(|| default.clone())
    });
    let engine = Engine::new(EngineConfig {
        env: ctx.env,
        registry,
        vault,
        // §7.6: asked at most once per credential within one command.
        oracle: Arc::new(CachingOracle::new(HttpOracle::new(
            http.clone(),
            clock.clone(),
        ))),
        clock,
        http,
        default_provider: chosen.default,
        settings: chosen.settings,
    });
    (engine, chosen.warnings)
}
```

In `run`, replace

```rust
    let provider_flag = cli.provider.map(ProviderId::new);
    let resolved = provider_flag
        .clone()
        .unwrap_or_else(|| ProviderId::new(CLAUDE_CODE));
    let (engine, warnings) = build_engine(ctx, &resolved);
```

with

```rust
    let provider_flag = cli.provider.map(ProviderId::new);
    let (engine, warnings) = build_engine(ctx, provider_flag.as_ref());
```

In `run_statusline`, replace

```rust
    let provider = provider.map_or_else(|| ProviderId::new(CLAUDE_CODE), ProviderId::new);
    let (no_color_env, force_color_env) = (ctx.no_color_env, ctx.force_color_env);
    let (engine, _http, _keychain) = statusline::engine(ctx, &provider);
```

with

```rust
    let (no_color_env, force_color_env) = (ctx.no_color_env, ctx.force_color_env);
    let (engine, provider, _http, _keychain) = statusline::engine(ctx, provider.as_deref());
```

In `crates/tagteam/src/statusline.rs`, change the imports:
- `use tagteam_core::{CLAUDE_CODE, ProviderId, Window, WindowKind};` becomes `use tagteam_core::{ProviderId, Window, WindowKind};`
- `use tagteam_engine::settings::{STATUSLINE_MODEL_PREFIX, Settings, is_statusline_placeholder};` becomes `use tagteam_engine::settings::{STATUSLINE_MODEL_PREFIX, is_statusline_placeholder};`
- `use crate::app::Context;` becomes `use crate::app::{Context, command_settings};`
- in `mod tests`, `use tagteam_core::{PollBudget, PollPlan};` becomes `use tagteam_core::{CLAUDE_CODE, PollBudget, PollPlan};`

and replace `engine` and its doc comment (today :195-218) with:

```rust
/// The engine the fast path runs on, built with walls rather than trust (§13.5): a Keychain that
/// refuses every call, no profile oracle, and a lazy HTTP port whose adapter could send nothing
/// even if it were built. The provider is `flag`, else the default (`app::command_settings`).
/// The settings are that provider's, and their warnings are dropped, since a status bar has
/// nowhere to show them. Returns the engine, the provider, and the walls, so a test can prove
/// nothing reached them.
pub(crate) fn engine(
    ctx: Context,
    flag: Option<&str>,
) -> (Engine, ProviderId, Arc<LazyHttp>, Arc<NoKeychain>) {
    let keychain = Arc::new(NoKeychain::default());
    let http = Arc::new(LazyHttp::new(|| Arc::new(NoHttp) as Arc<dyn Http>));
    let registry =
        ProviderRegistry::new().with(Arc::new(ClaudeCode::new(keychain.clone(), ctx.platform)));
    let chosen = command_settings(&ctx.env, &|p| registry.get(p).is_some(), |default| {
        flag.map_or_else(|| default.clone(), ProviderId::new)
    });
    let engine = Engine::new(EngineConfig {
        registry,
        vault: Vault::new(Box::new(KeychainVault::new(keychain.clone()))),
        oracle: Arc::new(NoOracle),
        clock: Arc::new(SystemClock),
        http: http.clone(),
        default_provider: chosen.default,
        settings: chosen.settings,
        env: ctx.env,
    });
    (engine, chosen.provider, http, keychain)
}
```

In `crates/tagteam/src/cli.rs`, replace the doc comment of `Cli.provider`,

```rust
    /// The agent CLI to act on (default: claude-code)
```

with

```rust
    /// The agent CLI to act on (default: the default_provider setting, else claude-code)
```

- [ ] **Step 9: Run the tests and see them pass**

Run: `cargo test -p tagteam --lib` — Expected: PASS, including the four `command_settings` tests and `without_provider_the_line_falls_back_from_a_default_this_build_lacks`.
Run: `cargo test -p tagteam --features test-support --test config_cli` — Expected: PASS (9 tests).
Run: `cargo test -p tagteam --features test-support` — Expected: PASS. `tests/app.rs`'s `settings_are_read_for_the_provider_the_command_resolves` still warns for `--provider other` alone. That flag is not claude-code, so `command_settings` reads the file again for `other`.

- [ ] **Step 10: Format, lint, commit**

```
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p tagteam --features test-support
git add crates/tagteam/src/app.rs crates/tagteam/src/statusline.rs crates/tagteam/src/cli.rs crates/tagteam/tests/config_cli.rs
git commit -m "Act on default_provider when --provider is absent, falling back to claude-code"
```

---

### Task 4: `config set` and `unset`

**Re-sync.** This task is written against M3a and M4a as merged:
- **M3a Task 1:** `Env.cancel` and `FlockGuard::lock(path, timeout, cancel: &Cancel)`. On
  3c1f458, `lock` takes no token and `Env` has no `cancel`.
- **M3a Task 2:** `EngineError::signal()`, `Engine::cancel()` and
  `LockError::Interrupted { path, signal }`. Task 2 owns `EngineError::Settings` and its kinds
  (an interrupted settings-lock wait is `interrupted` through its `lock_kind`). This task adds
  only the settings arm to `signal()`, so a signal during the settings-lock wait exits 128 + n.
  In `crates/tagteam-engine/src/error.rs`, replace M3a's `signal()`:

  ```rust
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
  ```

  with:

  ```rust
      /// The signal behind an interruption, whichever carrier holds it: `Interrupted`,
      /// `Lock(LockError::Interrupted)`, `Provider(ProviderError::Lock(LockError::Interrupted))`,
      /// or the settings lock's `Settings(SettingsError::Lock(LockError::Interrupted))` (§14.1).
      /// `None` for every other error.
      pub fn signal(&self) -> Option<i32> {
          match self {
              EngineError::Interrupted(signal) => Some(*signal),
              EngineError::Lock(e)
              | EngineError::Provider(ProviderError::Lock(e))
              | EngineError::Settings(SettingsError::Lock(e)) => e.signal(),
              _ => None,
          }
      }
  ```

  and append to its `mod tests`, after `kind_is_pinned_for_every_variant`, in Step 1:

  ```rust
      #[test]
      fn an_interrupted_settings_lock_wait_is_an_interruption() {
          // §14.1: the settings lock's wait is a cancellation point like any other, so its
          // interruption carries the signal the CLI exits with.
          let e = EngineError::Settings(SettingsError::Lock(LockError::Interrupted {
              path: PathBuf::from("locks/config.lock"),
              signal: 2,
          }));
          assert_eq!(e.signal(), Some(2));
          assert_eq!(e.kind(), "interrupted");
      }
  ```

  Before the `signal()` arm it fails with `left: None, right: Some(2)`. Step 1's
  `a_signal_ends_the_settings_lock_wait_as_interrupted` pins the same end to end.
- **M4a Task 7:**
  - `Provider::share_policy(&self, env) -> SharePolicy`;
  - FakeAgent's `sessions` capability and its private list, which holds `procs`;
  - Claude Code's private list, `CC_PRIVATE`, which holds `.tagteam-*`.
- **M4a Task 13:** `crates/tagteam-engine/src/profiles.rs` and its private
  `fn is_private(policy: &SharePolicy, name: &str) -> bool`, the matcher link sync uses (the
  policy's patterns through `entry_matches`, plus `.tagteam-`). This task makes it `pub(crate)`.
- **M4a Task 8:**
  - `Fx::{write_marker, shell_env, engine_located}`, `Engine::run_shell()` and
    `tagteam_provider::profile::RunShell`;
  - the CLI's refusal of every command but `statusline` under an *unreadable* marker only.

§6.4: "`set` and `unset` are strict. They refuse to write to a corrupt file. `set` refuses an
unknown key, a key that a provider table cannot override (`provider.<id>.ui.color`), a value of
the wrong type and a value outside its range, each with `invalid-input`; it never clamps."

"`set` and `unset` hold the settings lock (`locks/config.lock`, §5): a standalone `flock`,
outside the lock order (§4.3), waited for up to 5 s, with each wait a cancellation point
(§14.1). Under it they read the file, edit it with `toml_edit`, and replace it with the atomic
writer (§9.5), through a symlink to its target. A new file is created 0600 […]; an existing file
keeps its mode. Only `set` creates the file. `config` works inside a run shell."

This task builds that write path in two parts:
- **Part A** is the engine's `config` module: `Engine::config_set` and `Engine::config_unset`
  over Task 2's registry. It adds the checks only the engine can make: a registered
  `default_provider`, and no known-private `run.share_extra` name.
- **Part B** is `tagteam config set` and `tagteam config unset`, with §6.4's
  `{schemaVersion, ok, key, value, changed}`.

A running `auto` picking a change up by mtime (§11.4) is M3b's, and is out of M5a's scope.

**Readings of the spec this task commits to:**
- **`unset` is as strict about the entry as `set`.** §6.4 lists the unknown-key and
  provider-table refusals under `set`; the skeleton's Global Constraints apply them to both.
  - `unset` refuses three things rather than answering "not set":
    - an unknown key, with `invalid-input`;
    - an entry no provider table may hold, with `invalid-input`;
    - a provider this build lacks, with `unknown-provider`, in the prefix spelling and under
      `--provider` alike, as `config get` refuses it (Decision 4).

    A typo is reported, never silently "succeeded".
  - An unknown key that `config list` flags is removed by editing the file.
  - `unset` never checks the value it removes.
- **`--provider` names the provider table's entry for every key.** §6.4 says the two
  spellings name the same entry in every `config` command.
  - So `config set ui.color never --provider claude-code` refuses, as
    `provider.claude-code.ui.color` does.
  - `SettingsChange::key` is the full dotted entry whichever spelling was used, for example
    `provider.claude-code.autoswitch.models`.
- **The known-private check takes the providers the entry reaches.**
  - A provider's own `run.share_extra` is checked against that provider's `share_policy`.
  - The global one reaches every profile, so it is checked against every registered provider
    with the `sessions` capability.
  - The match is M4a's `is_private`: the policy's patterns, and `.tagteam-*`.
- **"Changes nothing" is about the file.** `changed` is false in two cases: the edit would
  leave the file's bytes as they are, or a `set` finds the value already there as the
  forgiving reader takes it. Two examples: `threshold = 85` for `85`, and `models = "Fable"`
  for `Fable`.
  - Then nothing is written, so the mtime stays and a running `auto` sees no change (§11.4).
  - Nothing is created either: the first look is taken without the lock, so even
    `locks/config.lock` does not appear (§5).
  - That answer stands as of that read. A writer that lands after it is ordered after this
    command, exactly as after a locked read.
- **Only `set` creates.**
  - On an absent file, `set` makes the config directory 0700 (`ensure_private_dir`) and the
    file 0600, through the atomic writer's preserve policy.
  - An existing file, or a symlink's target, keeps its mode.
  - A dangling link's target is created, as the atomic writer does (§9.5).
  - `unset` never creates the file. A file it empties stays in place, empty, since it may be a
    link into a dotfiles repository.
- **Comments.**
  - `set` replaces only the value, so it keeps two comments: those above the key (the key's
    decor) and the one at the end of its line (the value's decor).
  - `unset` removes the key with its own comments.
  - A table `unset` empties is removed too, walking up through `provider.<id>` and
    `provider`, unless the table "holds a comment". That means a comment above its header or
    on the header's line, or, for an inline table, on its line. `toml_edit` attaches a comment
    between keys to the key below it, so an empty table has no other place to hold one.
- **New tables keep the file's style.**
  - `provider` and `provider.<id>` are made implicit and the key's own table explicit, so a new
    override reads `[provider.claude-code.autoswitch]`.
  - A table made under a dotted table is dotted, and one made under an inline table is inline.
  - An existing segment that is not a table (`autoswitch = 3`, `[[provider]]`) refuses with
    `invalid-input`, naming it. So does an entry that is itself a table, whether a table
    (`[autoswitch.threshold]`), an inline table (`threshold = { … }`) or an array of tables, for
    `set` and `unset` alike. Replacing or removing it would delete what the person wrote.
- **`default_provider` is a top-level key.**
  - `toml_edit` writes top-level keys after any other top-level keys and before the first table
    header, so it never lands inside a table.
  - A comment directly above the first header belongs to that table, so the new key lands above
    that comment. A test pins this. tagteam does not guess which comment heads the file.
- **Two whitespace effects.**
  - An edit drops a blank first line the file did not have, which removing the first table
    would otherwise leave behind.
  - `toml_edit` writes LF, so a CRLF file comes back LF throughout after its first edit.
    tagteam runs on macOS and Linux only (§1.2).
- **What counts as corrupt.** A file that is not UTF-8, or not TOML (a duplicate key included),
  is `Corrupt`, with kind `settings-unreadable`. Any other read error, such as permission
  denied or a directory, is `Io`. Both reads refuse, the first look and the one under the lock,
  and neither ever writes.
- **What is logged.**
  - Each write logs one INFO line, `settings written key=<entry> action="set"` (or
    `"unset"`), once the settings lock is released: nothing else is taken while it is held
    (§4.3).
  - The value is never logged. `statusline.format` is free text, so it may hold an email or a
    name, which §14.2 never logs. The file already holds the value.
  - A change to nothing logs nothing.

**Files:**
- **Part A:**
  - Create: `crates/tagteam-engine/src/config.rs`
  - Modify: `crates/tagteam-engine/src/lib.rs` (`pub mod config;` after `pub mod collect;`,
    line 6 on 3c1f458)
  - Modify: `crates/tagteam-engine/src/profiles.rs` (M4a Task 13: `fn is_private` becomes
    `pub(crate)`)
  - Modify: `crates/tagteam-engine/src/error.rs` (M3a Task 2's `EngineError::signal()` and one
    unit test; the Re-sync note holds both, since `signal()` does not exist on 3c1f458)
  - Create: `crates/tagteam-engine/tests/config.rs`
- **Part B:**
  - Modify: `crates/tagteam/src/cli.rs` (`enum ConfigAction`, Task 3)
  - Modify: `crates/tagteam/src/app.rs` (Task 3's `match` over `ConfigAction`)
  - Modify: `crates/tagteam/src/config_cmd.rs` (Task 3's module: two new functions)
  - Test: `crates/tagteam/tests/config_cli.rs` (Task 3 creates it; this part appends a module)

**Interfaces:**
- Consumes:
  - **Task 2** (`tagteam_engine::settings`):
    - `Key { name, kind, per_provider }` and `Key::{table, leaf, parse_item, parse_arg}`;
    - `KeyKind::{Float, Int, Bool, Choice, Models, Format, ShareNames, Provider}` and `KEYS`;
    - `Value::{Float, Int, Bool, Str, List}` and `Value::{to_item, to_json}`. `Value` derives
      `Debug, Clone, PartialEq`, which `SettingsChange`'s derive and `put`'s comparison need.
    - `config_path(&Env) -> PathBuf`;
    - `resolve(&str, Option<&ProviderId>) -> Result<Resolved, SettingsError>`;
    - `SettingsError::{UnknownKey, NotPerProvider, ProviderMismatch, Invalid, UnknownProvider, Corrupt, Lock, Io}`;
    - `EngineError::Settings(#[from] SettingsError)` with its kinds (Task 2);
    - `Settings::load(&Env, &ProviderId) -> (Settings, Vec<String>)`, and
      `Settings.{threshold, models, color}`.
  - **Task 3:**
    - `Command::Config { action: ConfigAction }`, `ConfigAction`, `ConfigKeyParser`;
    - `crate::config_cmd`;
    - the `ConfigAction` `match` in `app.rs`;
    - `crates/tagteam/tests/config_cli.rs`, which declares `mod common;` at its root, and
      `App::config`, which returns `Result<(), Failure>`.
  - **M3a:** `Env.cancel`, `FlockGuard::lock(&Path, Duration, &Cancel)`,
    `LockError::Interrupted { path, signal }`, `EngineError::signal()` and `Engine::cancel()`.
  - **M4a:**
    - `Provider::share_policy(&self, &Env) -> SharePolicy`, and `Capabilities.sessions`;
    - `crate::profiles::is_private(&SharePolicy, &str) -> bool`;
    - `Fx::{write_marker, shell_env, engine_located}`, `Engine::run_shell() -> &RunShell`,
      and `RunShell::Inside`.
  - **Existing:**
    - `tagteam_provider::atomic::{write_atomic, ensure_private_dir}`;
    - `hooks::point` and `Engine::on_point` (`test-hooks`);
    - `ProviderRegistry::all`, and `Engine::provider`, whose `EngineError::UnknownProvider`
      (kind `unknown-provider`) refuses a provider this build lacks;
    - `App::{print, provider_flag}`;
    - the engine test helpers `Fx::engine_with_env`, `FakeFx`, `capture_logs` and `cc`;
    - the binary test helpers `common::{cmd, std_cmd}`.
- Produces:
  - `tagteam_engine::config::SettingsChange { pub key: String, pub value: Option<Value>, pub changed: bool, pub path: PathBuf }`,
    deriving `Debug, Clone, PartialEq`;
  - `tagteam_engine::config::SETTINGS_LOCK_TIMEOUT: Duration` (5 s);
  - `Engine::config_set(&self, name: &str, provider: Option<&ProviderId>, raw: &str) -> Result<SettingsChange, EngineError>`;
  - `Engine::config_unset(&self, name: &str, provider: Option<&ProviderId>) -> Result<SettingsChange, EngineError>`;
  - the hook point `settings-read`: under the settings lock, after the read, before the edit;
  - after M3a, `EngineError::signal()` also reads `Settings(SettingsError::Lock(LockError::Interrupted { .. }))` (the Re-sync note);
  - `ConfigAction::Set { key: String, value: String }` and `ConfigAction::Unset { key: String }`;
  - `config_cmd::change_human(&SettingsChange) -> String` and
    `config_cmd::change_json(&SettingsChange) -> serde_json::Value` (crate-private);
  - the INFO line `settings written key=<entry> action="set"` or `action="unset"` (§14.2).

**Spec:**
- §6.4: "One registry"; "`set` and `unset` are strict"; "Values on the command line";
  "Key-specific rules"; "Provider overrides" (the two spellings); "Commands" (`set`, `unset`,
  an absent key, a table left empty); "Writing"; the JSON shape of `set` and `unset`.
- §4.3: the settings lock is outside the lock order, taken alone.
- §5: the settings lock's path. Directories are created lazily, 0700, and a command that
  changes nothing creates nothing.
- §9.5: the atomic writer, which writes through a symlink to its target.
- §12.2: the known-private list, `.tagteam-*` included.
- §14.1: each wait for a lock is a cancellation point.
- §14.2: INFO records settings writes; an email or a name is never logged.
- §13.1 and §13.2: exit codes, and the JSON error envelope.
- Decisions 2, 3, 4 and 14. Review Focus 1, and Review Focus 2 with Task 2.

#### Part A: the engine's strict writer

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam-engine/tests/config.rs`:

```rust
//! `config set` and `unset` (§6.4): strict edits of one entry of `config.toml` that keep every
//! comment, write through a symlink and hold the settings lock.

mod common;

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;
use std::sync::Barrier;
use std::thread;
use std::time::Instant;

use common::{FakeFx, Fx, capture_logs, cc};
use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::EngineError;
use tagteam_engine::config::{SETTINGS_LOCK_TIMEOUT, SettingsChange};
use tagteam_engine::settings::{
    ColorMode, KEYS, KeyKind, Settings, SettingsError, Value, config_path,
};
use tagteam_provider::FlockGuard;
use tagteam_provider::profile::RunShell;

/// Writes `text` as the fixture's settings file, making its directory first.
fn write_config(fx: &Fx, text: &str) {
    let path = config_path(&fx.env);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// The fixture's settings file.
fn config_text(fx: &Fx) -> String {
    fs::read_to_string(config_path(&fx.env)).unwrap()
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

fn set(fx: &Fx, name: &str, raw: &str) -> SettingsChange {
    fx.engine
        .config_set(name, None, raw)
        .unwrap_or_else(|e| panic!("config set {name} {raw:?}: {e}"))
}

fn unset(fx: &Fx, name: &str) -> SettingsChange {
    fx.engine
        .config_unset(name, None)
        .unwrap_or_else(|e| panic!("config unset {name}: {e}"))
}

/// The `error.type` of a `config set` that must refuse.
fn refusal(fx: &Fx, name: &str, provider: Option<&ProviderId>, raw: &str) -> &'static str {
    fx.engine
        .config_set(name, provider, raw)
        .unwrap_err()
        .kind()
}

fn list(names: &[&str]) -> Value {
    Value::List(names.iter().map(|n| n.to_string()).collect())
}

/// A file as a person keeps it: comments above tables and keys and at the ends of lines, and
/// spacing tagteam would not write itself.
const COMMENTED: &str = "\
# tagteam settings, kept in my dotfiles

[autoswitch]
# 85 leaves room for a long session
threshold = 80.0 # was 90
models = [\"Fable\"]   # the one that runs out

# colours off in CI
[ui]
color = \"never\"
";

#[test]
fn a_set_rewrites_its_value_alone_and_every_comment_survives() {
    // §6.4: `set` writes only the key it is given, through `toml_edit`. Byte for byte, the rest
    // of the file is what the person wrote, and an override added then removed leaves no trace.
    let fx = Fx::new();
    write_config(&fx, COMMENTED);

    set(&fx, "autoswitch.threshold", "85.5");
    let updated = COMMENTED.replace("threshold = 80.0 # was 90", "threshold = 85.5 # was 90");
    assert_eq!(config_text(&fx), updated);

    set(&fx, "provider.claude-code.autoswitch.models", "");
    assert_eq!(
        config_text(&fx),
        format!("{updated}\n[provider.claude-code.autoswitch]\nmodels = []\n")
    );

    unset(&fx, "provider.claude-code.autoswitch.models");
    assert_eq!(config_text(&fx), updated, "the emptied tables went with it");
}

#[test]
fn set_creates_the_file_0600_in_a_0700_directory_and_reports_the_entry() {
    // §5, §6.4: only `set` creates the file. A new file is 0600 whatever the umask, in a
    // directory made 0700.
    let fx = Fx::new();
    let path = config_path(&fx.env);
    assert!(!fx.env.config_dir().exists());

    let change = set(&fx, "ui.color", "never");

    assert_eq!(
        change,
        SettingsChange {
            key: "ui.color".into(),
            value: Some(Value::Str("never".into())),
            changed: true,
            path: path.clone(),
        }
    );
    assert_eq!(config_text(&fx), "[ui]\ncolor = \"never\"\n");
    assert_eq!(mode(&path), 0o600);
    assert_eq!(mode(&fx.env.config_dir()), 0o700);
}

#[test]
fn an_existing_file_keeps_its_mode() {
    // §6.4: "an existing file keeps its mode".
    let fx = Fx::new();
    write_config(&fx, "[ui]\ncolor = \"auto\"\n");
    let path = config_path(&fx.env);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

    set(&fx, "ui.color", "never");

    assert_eq!(config_text(&fx), "[ui]\ncolor = \"never\"\n");
    assert_eq!(mode(&path), 0o644);
}

#[test]
fn a_symlinked_file_is_written_through_to_its_target_and_stays_a_link() {
    // Review Focus 1: `config.toml` as chezmoi or a dotfiles repo leaves it, here a relative
    // link into the repo. The write lands in the target with its comments, the link stays a
    // link, the target keeps its mode, and neither directory keeps a temporary file.
    let fx = Fx::new();
    let dotfiles = fx.dir.path().join("dotfiles");
    fs::create_dir_all(&dotfiles).unwrap();
    let target = dotfiles.join("tagteam.toml");
    fs::write(
        &target,
        "# shared across my machines\n[ui]\ncolor = \"auto\" # for now\n",
    )
    .unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).unwrap();
    let link = config_path(&fx.env);
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    // `<root>/home/.config/tagteam/config.toml` → `<root>/dotfiles/tagteam.toml`
    symlink("../../../dotfiles/tagteam.toml", &link).unwrap();

    let change = set(&fx, "ui.color", "never");

    assert!(change.changed);
    assert_eq!(change.path, link);
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::read_link(&link).unwrap(),
        Path::new("../../../dotfiles/tagteam.toml")
    );
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "# shared across my machines\n[ui]\ncolor = \"never\" # for now\n"
    );
    assert_eq!(mode(&target), 0o640);
    assert_eq!(fs::read_dir(&dotfiles).unwrap().count(), 1);
    assert_eq!(fs::read_dir(link.parent().unwrap()).unwrap().count(), 1);

    unset(&fx, "ui.color");
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "# shared across my machines\n[ui]\n",
        "the emptied table keeps the comment above its header"
    );
}

#[test]
fn an_empty_list_override_beats_a_non_empty_global_list() {
    // Review Focus 2: `''` writes `[]` into the provider's own table, which the reader takes
    // over the global list. Another provider still reads the global list.
    let fx = Fx::new();
    write_config(&fx, "[autoswitch]\nmodels = [\"Fable\"]\n");

    let change = set(&fx, "provider.claude-code.autoswitch.models", "");

    assert_eq!(change.value, Some(list(&[])));
    assert_eq!(
        config_text(&fx),
        "[autoswitch]\nmodels = [\"Fable\"]\n\n[provider.claude-code.autoswitch]\nmodels = []\n"
    );
    let (settings, warnings) = Settings::load(&fx.env, &cc());
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(settings.models.is_empty());
    let (other, _) = Settings::load(&fx.env, &ProviderId::new("fake-agent"));
    assert_eq!(other.models, ["Fable"]);
}

#[test]
fn lists_are_typed_comma_separated_with_each_name_trimmed() {
    // Review Focus 2, §6.4 "Values on the command line": names typed with a space are trimmed
    // names. An empty item refuses, and so does `all` beside a name. A refusal writes nothing.
    let fx = Fx::new();
    for raw in ["Fable,,Opus", "all,Fable", "Fable,"] {
        assert_eq!(
            refusal(&fx, "autoswitch.models", None, raw),
            "invalid-input",
            "{raw:?}"
        );
    }
    assert!(!config_path(&fx.env).exists());

    let change = set(&fx, "autoswitch.models", "Fable, opus");
    assert_eq!(change.value, Some(list(&["Fable", "opus"])));
    assert_eq!(
        config_text(&fx),
        "[autoswitch]\nmodels = [\"Fable\", \"opus\"]\n"
    );
    assert_eq!(
        set(&fx, "autoswitch.models", "all").value,
        Some(list(&["all"]))
    );
}

#[test]
fn the_spec_s_ranges_and_spellings_hold_at_both_ends_and_nothing_is_clamped() {
    // §6.4's table, end to end. Each bound is taken as typed, and one step past it refuses
    // with `invalid-input`. A refusal writes nothing, so a value is never clamped.
    let fx = Fx::new();
    let cases: &[(&str, &[&str], &[&str])] = &[
        (
            "autoswitch.threshold",
            &["50", "99.9"],
            &["49.9", "99.95", "high"],
        ),
        (
            "autoswitch.interval_seconds",
            &["15", "3600"],
            &["14", "3601", "60.5"],
        ),
        (
            "autoswitch.cooldown_seconds",
            &["0", "86400"],
            &["-1", "86401"],
        ),
        ("autoswitch.hysteresis_pct", &["0", "50"], &["-0.1", "50.1"]),
        ("autoswitch.unhealthy_ticks", &["1", "100"], &["0", "101"]),
        (
            "usage.history_retention_days",
            &["1", "3650"],
            &["0", "3651"],
        ),
        (
            "autoswitch.strategy",
            &["best", "consume-first"],
            &["fastest", ""],
        ),
        (
            "autoswitch.include_api_key_accounts",
            &["yes", "0"],
            &["on", "2", ""],
        ),
        ("ui.color", &["auto", "always", "never"], &["sometimes"]),
        (
            "statusline.format",
            &["{account} {5h}%", "{model:Fable}"],
            &["", "{bogus}", "{5h"],
        ),
        ("run.share_extra", &["hook-data", ""], &["a/b", ".."]),
    ];
    for (name, taken, refused) in cases {
        for raw in *refused {
            let before = fs::read(config_path(&fx.env)).ok();
            assert_eq!(
                refusal(&fx, name, None, raw),
                "invalid-input",
                "{name} {raw:?}"
            );
            assert_eq!(
                fs::read(config_path(&fx.env)).ok(),
                before,
                "{name} {raw:?} wrote"
            );
        }
        for raw in *taken {
            assert!(set(&fx, name, raw).changed, "{name} {raw:?}");
        }
    }
    let (settings, warnings) = Settings::load(&fx.env, &cc());
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(settings.threshold, 99.9);
    assert_eq!(settings.color, ColorMode::Never);
}

#[test]
fn every_numeric_key_in_the_registry_refuses_a_step_past_either_end() {
    // §6.4: `set` never clamps. Each range comes from the registry, so a key added to it is
    // covered without a new case here.
    let fx = Fx::new();
    for key in KEYS {
        let outside = match key.kind {
            KeyKind::Float { min, max } => [min - 0.01, max + 0.01].map(|v| v.to_string()),
            KeyKind::Int { min, max } => [min - 1, max + 1].map(|v| v.to_string()),
            _ => continue,
        };
        for raw in outside {
            assert_eq!(
                refusal(&fx, key.name, None, &raw),
                "invalid-input",
                "{} {raw}",
                key.name
            );
        }
    }
    assert!(
        !config_path(&fx.env).exists(),
        "nothing was written, clamped or otherwise"
    );
}

/// A value `set` takes for a key of `kind`: the bottom of a range, or the last choice.
fn sample(kind: &KeyKind) -> String {
    match kind {
        KeyKind::Float { min, .. } => min.to_string(),
        KeyKind::Int { min, .. } => min.to_string(),
        KeyKind::Bool => "yes".into(),
        KeyKind::Choice(choices) => choices.last().unwrap().to_string(),
        KeyKind::Models => "Fable, opus".into(),
        KeyKind::Format => "{account} · {7d}%".into(),
        KeyKind::ShareNames => "hook-data".into(),
        KeyKind::Provider => cc().to_string(),
    }
}

#[test]
fn every_key_set_through_the_registry_reads_back_without_a_warning() {
    // §6.4 "One registry": the forgiving reader takes whatever `set` writes as it is, from the
    // global table and from a provider's.
    let fx = Fx::new();
    for key in KEYS {
        let raw = sample(&key.kind);
        set(&fx, key.name, &raw);
        if key.per_provider {
            fx.engine
                .config_set(key.name, Some(&cc()), &raw)
                .unwrap_or_else(|e| panic!("{} --provider: {e}", key.name));
        }
    }
    let (_, warnings) = Settings::load(&fx.env, &cc());
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn an_unknown_key_refuses_for_set_and_unset() {
    // §6.4, Decision 14: the CLI passes any key through, and the engine refuses one the
    // registry lacks with `invalid-input`. A typo is reported, never "not set".
    let fx = Fx::new();
    write_config(&fx, "[autoswitch]\ntreshold = 80\n");
    for name in [
        "autoswitch.treshold",
        "provider.claude-code.autoswitch.treshold",
        "threshold",
        "",
    ] {
        assert_eq!(refusal(&fx, name, None, "80"), "invalid-input", "{name:?}");
        assert_eq!(
            fx.engine.config_unset(name, None).unwrap_err().kind(),
            "invalid-input",
            "{name:?}"
        );
    }
    assert_eq!(config_text(&fx), "[autoswitch]\ntreshold = 80\n");
}

#[test]
fn a_key_no_provider_table_may_hold_refuses_under_either_spelling() {
    // §6.4: `provider.<id>.ui.color` and `ui.color --provider <id>` name the same entry, which
    // no provider table may hold. `set` and `unset` both refuse it.
    let fx = Fx::new();
    for (name, raw) in [
        ("ui.color", "never"),
        ("usage.history_retention_days", "30"),
        ("default_provider", "claude-code"),
    ] {
        let prefixed = format!("provider.claude-code.{name}");
        assert_eq!(
            refusal(&fx, &prefixed, None, raw),
            "invalid-input",
            "{prefixed}"
        );
        assert_eq!(
            refusal(&fx, name, Some(&cc()), raw),
            "invalid-input",
            "{name} --provider"
        );
        assert_eq!(
            fx.engine.config_unset(&prefixed, None).unwrap_err().kind(),
            "invalid-input"
        );
        assert_eq!(
            fx.engine
                .config_unset(name, Some(&cc()))
                .unwrap_err()
                .kind(),
            "invalid-input"
        );
    }
    assert!(!config_path(&fx.env).exists());
}

#[test]
fn a_prefix_and_a_flag_naming_two_providers_refuse_and_naming_one_are_one_entry() {
    // §6.4: a prefix and `--provider` that disagree refuse, and nothing is written. When they
    // agree, or only one is given, they all name the one entry.
    let ffx = FakeFx::new();
    let fake = ffx.fake_provider();
    let engine = &ffx.engine;
    assert_eq!(
        engine
            .config_set(
                "provider.claude-code.autoswitch.threshold",
                Some(&fake),
                "80"
            )
            .unwrap_err()
            .kind(),
        "invalid-input"
    );
    assert!(!config_path(&ffx.fx.env).exists());

    let by_prefix = engine
        .config_set("provider.fake-agent.autoswitch.threshold", None, "80")
        .unwrap();
    let by_flag = engine
        .config_set("autoswitch.threshold", Some(&fake), "80")
        .unwrap();
    let by_both = engine
        .config_set(
            "provider.fake-agent.autoswitch.threshold",
            Some(&fake),
            "80",
        )
        .unwrap();

    assert_eq!(by_prefix.key, "provider.fake-agent.autoswitch.threshold");
    assert!(by_prefix.changed);
    assert_eq!(
        by_flag,
        SettingsChange {
            changed: false,
            ..by_prefix.clone()
        }
    );
    assert_eq!(by_both, by_flag);
    assert_eq!(
        config_text(&ffx.fx),
        "[provider.fake-agent.autoswitch]\nthreshold = 80.0\n"
    );
}

#[test]
fn a_provider_this_engine_does_not_register_refuses() {
    // Decision 4: a provider this build lacks, named by a prefix or by `--provider`, is
    // `unknown-provider` for `set` and `unset` alike, as for `config get`. As
    // `default_provider`'s value it is `invalid-input`. `fake-agent` is well formed, but this
    // fixture registers Claude Code alone.
    let fx = Fx::new();
    let fake = ProviderId::new("fake-agent");
    for (name, flag) in [
        ("provider.fake-agent.autoswitch.threshold", None),
        ("autoswitch.threshold", Some(&fake)),
        ("provider.fake-agent.autoswitch.threshold", Some(&fake)),
    ] {
        assert!(
            matches!(
                fx.engine.config_set(name, flag, "80"),
                Err(EngineError::UnknownProvider(id)) if id == "fake-agent"
            ),
            "{name} {flag:?}"
        );
        assert_eq!(
            fx.engine.config_unset(name, flag).unwrap_err().kind(),
            "unknown-provider",
            "{name} {flag:?}"
        );
    }
    assert!(matches!(
        fx.engine.config_set("default_provider", None, "fake-agent"),
        Err(EngineError::Settings(SettingsError::UnknownProvider(id))) if id == "fake-agent"
    ));
    assert_eq!(
        refusal(&fx, "default_provider", None, "fake-agent"),
        "invalid-input"
    );
    assert!(!config_path(&fx.env).exists());
}

#[test]
fn default_provider_takes_a_registered_provider_and_lands_above_every_table() {
    // Decision 2: `default_provider` is a top-level key, so it goes before the first table
    // header, never into a table. toml_edit keeps a comment above a header as that table's
    // own, so the new key goes above such a comment too.
    let ffx = FakeFx::new();
    write_config(
        &ffx.fx,
        "# my tagteam settings\n\n[autoswitch]\nthreshold = 80\n",
    );

    let change = ffx
        .engine
        .config_set("default_provider", None, "fake-agent")
        .unwrap();

    assert_eq!(change.value, Some(Value::Str("fake-agent".into())));
    assert_eq!(
        config_text(&ffx.fx),
        "default_provider = \"fake-agent\"\n# my tagteam settings\n\n[autoswitch]\nthreshold = 80\n"
    );
    ffx.engine
        .config_set("default_provider", None, "claude-code")
        .unwrap();
    assert_eq!(
        config_text(&ffx.fx),
        "default_provider = \"claude-code\"\n# my tagteam settings\n\n[autoswitch]\nthreshold = 80\n",
        "updated where it stands"
    );

    // After other top-level keys, and still above the first header.
    let fx = Fx::new();
    write_config(&fx, "autoswitch.threshold = 80\n\n[ui]\ncolor = \"auto\"\n");
    set(&fx, "default_provider", "claude-code");
    assert_eq!(
        config_text(&fx),
        "autoswitch.threshold = 80\ndefault_provider = \"claude-code\"\n\n[ui]\ncolor = \"auto\"\n"
    );
}

#[test]
fn run_share_extra_refuses_a_name_each_profile_keeps_private() {
    // §6.4 "Key-specific rules", §12.2: `set` refuses a known-private name, matched exactly or
    // by pattern, and tagteam's own `.tagteam-*`, which a read only ignores with a warning.
    let fx = Fx::new();
    for raw in [
        ".credentials.json",
        "hook-data, daemon.log",
        "x.lock",
        ".claude-staging-oauth.json",
        "policy-limits.json",
        ".tagteam-links.json",
    ] {
        assert_eq!(
            refusal(&fx, "run.share_extra", None, raw),
            "invalid-input",
            "{raw}"
        );
        assert_eq!(
            refusal(&fx, "provider.claude-code.run.share_extra", None, raw),
            "invalid-input",
            "{raw}"
        );
    }
    assert!(!config_path(&fx.env).exists());
    assert!(matches!(
        fx.engine.config_set("run.share_extra", None, "hook-data, .credentials.json"),
        Err(EngineError::Settings(SettingsError::Invalid { key, reason }))
            if key == "run.share_extra" && reason.contains(".credentials.json")
    ));
    assert_eq!(
        set(&fx, "run.share_extra", "hook-data, .my-tool").value,
        Some(list(&["hook-data", ".my-tool"]))
    );
}

#[test]
fn the_global_share_extra_is_checked_against_every_provider_with_sessions() {
    // §6.4: the global `run.share_extra` reaches every provider's profiles, so a name any of
    // them keeps private refuses. A provider's own entry answers to that provider's list alone.
    // FakeAgent keeps `procs` private; Claude Code does not.
    let ffx = FakeFx::new();
    let set_in = |name: &str, raw: &str| ffx.engine.config_set(name, None, raw);

    assert_eq!(
        set_in("run.share_extra", "procs").unwrap_err().kind(),
        "invalid-input"
    );
    assert_eq!(
        set_in("provider.fake-agent.run.share_extra", "procs")
            .unwrap_err()
            .kind(),
        "invalid-input"
    );
    assert!(
        set_in("provider.claude-code.run.share_extra", "procs")
            .unwrap()
            .changed
    );
    assert!(
        set_in("provider.fake-agent.run.share_extra", ".credentials.json")
            .unwrap()
            .changed
    );
    assert_eq!(
        config_text(&ffx.fx),
        "[provider.claude-code.run]\nshare_extra = [\"procs\"]\n\n\
         [provider.fake-agent.run]\nshare_extra = [\".credentials.json\"]\n"
    );
}

#[test]
fn a_corrupt_file_is_refused_for_set_and_unset_and_left_byte_identical() {
    // §6.4: `set` and `unset` refuse to write to a corrupt file, which a read only warns about.
    let fx = Fx::new();
    let path = config_path(&fx.env);
    for (bytes, detail) in [
        (
            &b"[ui]\ncolor = \"never\"\n[autoswitch\n"[..],
            "not valid TOML (line 3",
        ),
        (&b"[ui]\ncolor = \"\xff\"\n"[..], "not UTF-8"),
    ] {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        for result in [
            fx.engine.config_set("ui.color", None, "never"),
            fx.engine.config_unset("ui.color", None),
        ] {
            let err = result.unwrap_err();
            assert_eq!(err.kind(), "settings-unreadable", "{err:?}");
            assert!(
                matches!(
                    &err,
                    EngineError::Settings(SettingsError::Corrupt { path: p, detail: d })
                        if *p == path && d.starts_with(detail)
                ),
                "{err:?}"
            );
        }
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn a_change_to_nothing_writes_nothing_and_creates_nothing() {
    // §6.4: `unset` of an absent key changes nothing and succeeds. §5: a command that changes
    // nothing creates nothing: not the file, not its directory, not even the settings lock.
    let fx = Fx::new();
    let quiet = |change: SettingsChange| assert!(!change.changed, "{change:?}");

    quiet(unset(&fx, "ui.color"));
    quiet(unset(&fx, "provider.claude-code.autoswitch.models"));
    assert!(!fx.env.config_dir().exists());

    // The value already there, as a read takes it: an integer threshold, a single model name.
    let text = "[autoswitch]\nthreshold = 85\nmodels = \"Fable\"\n";
    write_config(&fx, text);
    quiet(set(&fx, "autoswitch.threshold", "85"));
    quiet(set(&fx, "autoswitch.models", "Fable"));
    quiet(unset(&fx, "ui.color"));
    quiet(unset(&fx, "provider.claude-code.autoswitch.threshold"));
    assert_eq!(config_text(&fx), text);
    assert!(
        !fx.env.data_dir().join("locks").exists(),
        "no lock was taken"
    );
}

#[test]
fn unset_removes_the_tables_it_empties_unless_one_holds_a_comment() {
    // §6.4: `unset` removes a table it leaves empty, walking up through `provider.<id>`, unless
    // the table holds a comment above its header or on its line. A key's own comments, above
    // it and at the end of its line, go with the key.
    let cases: &[(&str, &str, &str)] = &[
        (
            "[ui]\ncolor = \"auto\"\n\n[provider.claude-code.autoswitch]\nmodels = []\n",
            "provider.claude-code.autoswitch.models",
            "[ui]\ncolor = \"auto\"\n",
        ),
        (
            "# for the work account\n[provider.claude-code.autoswitch]\nmodels = []\n",
            "provider.claude-code.autoswitch.models",
            "# for the work account\n[provider.claude-code.autoswitch]\n",
        ),
        (
            "[autoswitch] # tuned by hand\nthreshold = 80\n",
            "autoswitch.threshold",
            "[autoswitch] # tuned by hand\n",
        ),
        (
            "[provider.claude-code.autoswitch]\nmodels = []\n\n\
             [provider.claude-code.statusline]\nformat = \"{5h}%\"\n",
            "provider.claude-code.autoswitch.models",
            "[provider.claude-code.statusline]\nformat = \"{5h}%\"\n",
        ),
        (
            "[autoswitch]\nthreshold = 80\n\n[ui]\ncolor = \"auto\"\n",
            "autoswitch.threshold",
            "[ui]\ncolor = \"auto\"\n",
        ),
        (
            "[autoswitch]\n# was 95\nthreshold = 80 # for now\ncooldown_seconds = 600\n",
            "autoswitch.threshold",
            "[autoswitch]\ncooldown_seconds = 600\n",
        ),
        (
            "default_provider = \"claude-code\"\n\n[ui]\ncolor = \"auto\"\n",
            "default_provider",
            "[ui]\ncolor = \"auto\"\n",
        ),
        ("[ui]\ncolor = \"auto\"\n", "ui.color", ""),
    ];
    for (before, name, after) in cases {
        let fx = Fx::new();
        write_config(&fx, before);
        assert!(unset(&fx, name).changed, "{name} in {before:?}");
        assert_eq!(config_text(&fx), *after, "{name} in {before:?}");
    }
}

#[test]
fn dotted_keys_already_in_the_file_are_updated_where_they_stand() {
    // A top-level `autoswitch.threshold = 80` is the `[autoswitch]` table written with dotted
    // keys. `set` updates it in place and adds a sibling in the same style.
    let fx = Fx::new();
    write_config(
        &fx,
        "autoswitch.threshold = 80 # mine\n\n[ui]\ncolor = \"auto\"\n",
    );

    set(&fx, "autoswitch.threshold", "85.5");
    set(&fx, "autoswitch.cooldown_seconds", "600");

    assert_eq!(
        config_text(&fx),
        "autoswitch.threshold = 85.5 # mine\nautoswitch.cooldown_seconds = 600\n\n[ui]\ncolor = \"auto\"\n"
    );
    let (settings, warnings) = Settings::load(&fx.env, &cc());
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(settings.threshold, 85.5);
}

#[test]
fn a_segment_or_an_entry_that_is_a_table_refuses_naming_it() {
    // An existing segment that is not a table, or an entry that is a table, would have to be
    // deleted to write the value. `set` refuses instead and leaves the file alone.
    let cases = [
        ("autoswitch = 3\n", "autoswitch.threshold", "`autoswitch`"),
        (
            "[[provider]]\nid = 1\n",
            "provider.claude-code.autoswitch.threshold",
            "`provider`",
        ),
        (
            "[autoswitch.threshold]\nx = 1\n",
            "autoswitch.threshold",
            "a table",
        ),
        (
            "[autoswitch]\nthreshold = { custom = 1 }\n",
            "autoswitch.threshold",
            "a table",
        ),
    ];
    for (text, name, named) in cases {
        let fx = Fx::new();
        write_config(&fx, text);
        match fx.engine.config_set(name, None, "85.5") {
            Err(EngineError::Settings(SettingsError::Invalid { key, reason })) => {
                assert_eq!(key, name);
                assert!(reason.contains(named), "{reason}");
            }
            other => panic!("{name} in {text:?}: {other:?}"),
        }
        assert_eq!(config_text(&fx), text);
    }
}

#[test]
fn an_unset_of_an_entry_that_is_a_table_refuses_and_keeps_what_it_holds() {
    let cases = [
        "[autoswitch.threshold]\nx = 1\n",
        "[autoswitch]\nthreshold = { custom = 1 }\n",
        "[[autoswitch.threshold]]\nx = 1\n",
    ];
    for text in cases {
        let fx = Fx::new();
        write_config(&fx, text);
        match fx.engine.config_unset("autoswitch.threshold", None) {
            Err(EngineError::Settings(SettingsError::Invalid { key, reason })) => {
                assert_eq!(key, "autoswitch.threshold");
                assert!(reason.contains("a table"), "{reason}");
            }
            other => panic!("{text:?}: {other:?}"),
        }
        assert_eq!(config_text(&fx), text);
    }
}

#[test]
fn a_held_settings_lock_times_out_after_five_seconds_with_lock_timeout() {
    // §6.4: the settings lock is waited for up to 5 s. A write that cannot take it writes
    // nothing.
    let fx = Fx::new();
    let held = FlockGuard::try_lock(&fx.env.data_dir().join("locks/config.lock"))
        .unwrap()
        .unwrap();
    let started = Instant::now();

    let err = fx.engine.config_set("ui.color", None, "never").unwrap_err();

    assert!(started.elapsed() >= SETTINGS_LOCK_TIMEOUT);
    assert_eq!(err.kind(), "lock-timeout");
    assert!(!config_path(&fx.env).exists());
    drop(held);
    assert!(set(&fx, "ui.color", "never").changed);
}

#[test]
fn a_signal_ends_the_settings_lock_wait_as_interrupted() {
    // §6.4, §14.1: each wait for the settings lock is a cancellation point. The token is
    // checked before the first attempt, so a signal already recorded takes nothing.
    let fx = Fx::new();
    fx.engine.cancel().request(15); // SIGTERM

    let err = fx.engine.config_set("ui.color", None, "never").unwrap_err();

    assert_eq!(err.kind(), "interrupted");
    assert_eq!(err.signal(), Some(15));
    assert!(!config_path(&fx.env).exists());
}

#[test]
fn two_writers_in_two_engines_at_once_both_land() {
    // §6.4: each read, edit and write happens under the settings lock, so two processes that
    // change different keys at once keep both changes.
    let fx = Fx::new();
    let engines = [
        fx.engine_with_env(fx.env.clone()),
        fx.engine_with_env(fx.env.clone()),
    ];
    let start = Barrier::new(2);
    thread::scope(|s| {
        let writes = [("autoswitch.threshold", "85.5"), ("ui.color", "never")];
        for (engine, (name, raw)) in engines.iter().zip(writes) {
            let start = &start;
            s.spawn(move || {
                start.wait();
                assert!(engine.config_set(name, None, raw).unwrap().changed);
            });
        }
    });
    let (settings, warnings) = Settings::load(&fx.env, &cc());
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(
        (settings.threshold, settings.color),
        (85.5, ColorMode::Never)
    );
}

#[test]
fn config_writes_inside_a_run_shell() {
    // §6.4: "`config` works inside a run shell (§12.8): settings are not accounts". The run
    // shell writes the same file the outer home does.
    let fx = Fx::new();
    let profile = fx.dir.path().join("profile");
    fx.write_marker(&profile, &AccountId::from_string("0192"), &fx.env);
    let engine = fx.engine_located(fx.shell_env(&profile));
    assert!(matches!(engine.run_shell(), RunShell::Inside { .. }));

    let change = engine.config_set("ui.color", None, "never").unwrap();

    assert!(change.changed);
    assert_eq!(change.path, config_path(&fx.env));
    assert_eq!(config_text(&fx), "[ui]\ncolor = \"never\"\n");
}

#[test]
fn a_write_logs_one_info_line_naming_the_key_and_never_the_value() {
    // §14.2: settings writes are INFO. A value can be free text, which may hold an email or a
    // name the log never records, so the line names the key alone.
    let fx = Fx::new();
    let name = "provider.claude-code.statusline.format";
    let key_field = format!("key={name}");
    // `tracing` caches whether a call site is enabled the first time it fires. While a
    // capture's subscriber is the only one alive, a call site that another test's thread fires
    // first is cached as disabled until the next subscriber is created. So the line fires once
    // here, and the captures below each create a subscriber after it is cached.
    capture_logs(|| set(&fx, "ui.color", "never"));

    let (_, logs) = capture_logs(|| set(&fx, name, "me@example.com {5h}%"));
    let info: Vec<&String> = logs.iter().filter(|l| l.contains("INFO")).collect();
    assert_eq!(info.len(), 1, "{logs:?}");
    assert!(
        info[0].contains("settings written") && info[0].contains(&key_field),
        "{info:?}"
    );
    assert!(
        logs.iter().all(|l| !l.contains("me@example.com")),
        "{logs:?}"
    );

    let (_, logs) = capture_logs(|| set(&fx, name, "me@example.com {5h}%"));
    assert!(
        logs.iter().all(|l| !l.contains("INFO")),
        "a change to nothing logs nothing: {logs:?}"
    );

    let (_, logs) = capture_logs(|| unset(&fx, name));
    assert_eq!(
        logs.iter()
            .filter(|l| l.contains("INFO") && l.contains(&key_field))
            .count(),
        1,
        "{logs:?}"
    );
}

#[cfg(feature = "test-hooks")]
mod hooks {
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;
    use std::time::Duration;

    use super::*;

    /// The second writer's thread, started from inside the first writer's lock.
    type Waiter = Arc<Mutex<Option<JoinHandle<Result<bool, String>>>>>;

    #[test]
    fn a_writer_waits_while_another_holds_the_settings_lock_and_both_keys_land() {
        // §6.4: the read, the edit and the write happen under the lock. A second writer starts
        // while the first holds the lock between its read and its write. Without the lock it
        // would read the file before the first write lands, and the first write would then
        // drop its key.
        let fx = Fx::new();
        let other = Arc::new(fx.engine_with_env(fx.env.clone()));
        let waiter: Waiter = Arc::default();
        let (writer, slot) = (other.clone(), waiter.clone());
        fx.engine.on_point(
            "settings-read",
            Box::new(move || {
                let writer = writer.clone();
                *slot.lock().unwrap() = Some(thread::spawn(move || {
                    writer
                        .config_set("ui.color", None, "never")
                        .map(|change| change.changed)
                        .map_err(|e| e.to_string())
                }));
                // The second writer is now waiting for the lock this one holds.
                thread::sleep(Duration::from_millis(300));
            }),
        );

        assert!(set(&fx, "autoswitch.threshold", "85.5").changed);

        let second = waiter
            .lock()
            .unwrap()
            .take()
            .expect("the second writer started");
        assert_eq!(second.join().unwrap(), Ok(true));
        assert_eq!(
            config_text(&fx),
            "[autoswitch]\nthreshold = 85.5\n\n[ui]\ncolor = \"never\"\n"
        );
    }
}
```

Add the `error.rs` unit test the Re-sync note gives.

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-engine --test config`
Expected: compile errors: `unresolved import tagteam_engine::config` (E0432), and
`no method named config_set found for struct Engine` (E0599).

Run: `cargo test -p tagteam-engine --lib error::tests`
Expected: `an_interrupted_settings_lock_wait_is_an_interruption` fails with
`assertion left == right failed`, where `left: None, right: Some(2)` (the Re-sync note).

- [ ] **Step 3: Implement**

In `crates/tagteam-engine/src/profiles.rs` (M4a Task 13), make the known-private matcher
visible to the crate. Replace

```rust
fn is_private(policy: &SharePolicy, name: &str) -> bool {
```

with

```rust
pub(crate) fn is_private(policy: &SharePolicy, name: &str) -> bool {
```

In `crates/tagteam-engine/src/lib.rs`, after `pub mod collect;`, add:

```rust
pub mod config;
```

Make the `error.rs` change the Re-sync note gives: the settings arm of `signal()`.

Create `crates/tagteam-engine/src/config.rs`:

```rust
//! `tagteam config set` and `unset` (§6.4): strict edits of one entry of `config.toml`. They go
//! through `toml_edit`, so every comment and every other entry keeps its bytes. Reads stay in
//! `settings`, which never needs an engine. A write needs this engine (Decision 3) for two
//! things. Its registry: `default_provider` must name a provider there, and `run.share_extra`
//! may not name what those providers' profiles keep private. Its cancel token, which the
//! settings lock waits on.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tagteam_core::ProviderId;
use tagteam_provider::FlockGuard;
use tagteam_provider::atomic::{ensure_private_dir, write_atomic};
use toml_edit::{Decor, DocumentMut, Item, Table, TableLike};

use crate::engine::Engine;
use crate::error::EngineError;
use crate::hooks;
use crate::profiles::is_private;
use crate::settings::{Key, KeyKind, SettingsError, Value, config_path, resolve};

/// How long a write waits for the settings lock (§4.3, §6.4). Each poll of the wait is a
/// cancellation point (§14.1).
pub const SETTINGS_LOCK_TIMEOUT: Duration = Duration::from_secs(5);

/// The settings lock, under the data dir (§5): a standalone `flock`, outside the lock order.
const SETTINGS_LOCK: &str = "locks/config.lock";

/// What one `config set` or `config unset` did: §6.4's `{key, value, changed}`, and the file.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingsChange {
    /// The entry's dotted name in the file. A provider table's entry starts `provider.<id>.`,
    /// whether the command named it that way or with `--provider`.
    pub key: String,
    /// What `set` wrote, or found there already; `None` for `unset`.
    pub value: Option<Value>,
    /// Whether the file changed. When it did not, nothing was written or created.
    pub changed: bool,
    /// The settings file, as `config path` names it: a symlink is not resolved.
    pub path: PathBuf,
}

/// One entry of the file, resolved and checked against this engine's registry.
struct Entry {
    key: &'static Key,
    provider: Option<ProviderId>,
    /// The dotted name, as `SettingsChange::key` reports it.
    name: String,
    /// The tables that hold the entry, outermost first. A provider table's entry: `provider`,
    /// the provider's id, then the key's table. A global entry: the key's table, or none.
    tables: Vec<String>,
}

impl Engine {
    /// `config set NAME RAW` (§6.4), with `provider` from `--provider`. The value is checked
    /// as strictly as the registry declares, then against this engine's providers, and is
    /// never clamped. A value the file already holds, as a read takes it, writes nothing.
    pub fn config_set(
        &self,
        name: &str,
        provider: Option<&ProviderId>,
        raw: &str,
    ) -> Result<SettingsChange, EngineError> {
        let entry = self.settings_entry(name, provider)?;
        let value = entry
            .key
            .parse_arg(raw)
            .map_err(|reason| invalid(&entry, reason))?;
        self.check_value(&entry, &value)?;
        self.edit(entry, Some(value))
    }

    /// `config unset NAME` (§6.4): removes the entry, then each table that leaves empty, unless
    /// it holds a comment. An absent entry, or an absent file, changes nothing and succeeds.
    pub fn config_unset(
        &self,
        name: &str,
        provider: Option<&ProviderId>,
    ) -> Result<SettingsChange, EngineError> {
        let entry = self.settings_entry(name, provider)?;
        self.edit(entry, None)
    }

    /// `name` as the registry resolves it, with `flag` from `--provider`. `resolve` refuses a
    /// key no provider table may hold under either spelling (§6.4). A provider this engine does
    /// not register is refused as `--provider` is, with `unknown-provider`, in both spellings
    /// (Decision 4).
    fn settings_entry(&self, name: &str, flag: Option<&ProviderId>) -> Result<Entry, EngineError> {
        let resolved = resolve(name, flag)?;
        let key = resolved.key;
        let mut tables = Vec::new();
        let mut entry_name = String::new();
        if let Some(p) = &resolved.provider {
            self.provider(p)?;
            tables.extend(["provider".to_owned(), p.to_string()]);
            entry_name = format!("provider.{p}.");
        }
        tables.extend(key.table().map(str::to_owned));
        entry_name.push_str(key.name);
        Ok(Entry {
            key,
            provider: resolved.provider,
            name: entry_name,
            tables,
        })
    }

    /// The checks only the engine can make (§6.4 "Key-specific rules"):
    /// - `default_provider` must name a provider this build registers.
    /// - A `run.share_extra` name must not be one a profile keeps private (§12.2). A provider's
    ///   own entry is checked against that provider's list. The global entry reaches every
    ///   profile, so it is checked against the list of every provider that has sessions.
    fn check_value(&self, entry: &Entry, value: &Value) -> Result<(), SettingsError> {
        match (&entry.key.kind, value) {
            (KeyKind::Provider, Value::Str(id)) => {
                if self.registry.get(&ProviderId::new(id.as_str())).is_none() {
                    return Err(SettingsError::UnknownProvider(id.clone()));
                }
            }
            (KeyKind::ShareNames, Value::List(names)) => {
                for p in self.registry.all() {
                    let concerned = match &entry.provider {
                        Some(id) => p.id() == *id,
                        None => p.capabilities().sessions,
                    };
                    if !concerned {
                        continue;
                    }
                    let policy = p.share_policy(&self.env);
                    if let Some(private) = names.iter().find(|name| is_private(&policy, name)) {
                        return Err(invalid(
                            entry,
                            format!(
                                "{private} stays private to each {} profile, so it cannot be shared",
                                p.display_name()
                            ),
                        ));
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// One edit of the file, in two passes:
    /// 1. A first look without the lock. When the edit changes nothing it returns here, so it
    ///    creates nothing, not even the lock file (§5).
    /// 2. Otherwise, under the settings lock: the read, the edit, and the write. The atomic
    ///    writer replaces the file through a symlink to its target, keeps an existing file's
    ///    mode and creates a new one 0600 (§6.4).
    ///
    /// Both reads refuse a corrupt file, which is never written. A write is logged once the
    /// lock is released: nothing else is taken while it is held (§4.3).
    fn edit(&self, entry: Entry, value: Option<Value>) -> Result<SettingsChange, EngineError> {
        let path = config_path(&self.env);
        if apply(&entry, value.as_ref(), read(&path)?)?.is_none() {
            return Ok(SettingsChange {
                key: entry.name,
                value,
                changed: false,
                path,
            });
        }
        let changed = {
            let _lock = FlockGuard::lock(
                &self.env.data_dir().join(SETTINGS_LOCK),
                SETTINGS_LOCK_TIMEOUT,
                &self.env.cancel,
            )
            .map_err(SettingsError::Lock)?;
            let current = read(&path)?;
            hooks::point(self, "settings-read")?;
            match apply(&entry, value.as_ref(), current)? {
                None => false,
                Some(text) => {
                    ensure_private_dir(&self.env.config_dir()).map_err(SettingsError::Io)?;
                    write_atomic(&path, text.as_bytes(), 0o600).map_err(SettingsError::Io)?;
                    true
                }
            }
        };
        if changed {
            // The key alone: a value can be free text (`statusline.format`) that holds an email
            // or a name, which the log never records (§14.2).
            let action = if value.is_some() { "set" } else { "unset" };
            tracing::info!(key = %entry.name, action, "settings written");
        }
        Ok(SettingsChange {
            key: entry.name,
            value,
            changed,
            path,
        })
    }
}

/// Why `set` and `unset` refuse an entry that is a table, inline or not.
const HOLDS_A_TABLE: &str = "the settings file holds a table there, not a value";

fn invalid(entry: &Entry, reason: String) -> SettingsError {
    SettingsError::Invalid {
        key: entry.name.clone(),
        reason,
    }
}

/// The file's text and document, or `None` when there is no file. A file that is not UTF-8,
/// or not TOML, is `Corrupt`, and a strict write never touches it (§6.4).
fn read(path: &Path) -> Result<Option<(String, DocumentMut)>, SettingsError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(SettingsError::Io(e)),
    };
    let corrupt = |detail: String| SettingsError::Corrupt {
        path: path.to_path_buf(),
        detail,
    };
    let text = String::from_utf8(bytes).map_err(|_| corrupt("not UTF-8".to_owned()))?;
    let doc = text.parse::<DocumentMut>().map_err(|e| {
        let at = e.span().map_or(0, |span| span.start).min(text.len());
        let line = text.as_bytes()[..at]
            .iter()
            .filter(|&&b| b == b'\n')
            .count()
            + 1;
        let what = e.message().lines().next().unwrap_or_default();
        corrupt(format!("not valid TOML (line {line}: {what})"))
    })?;
    Ok(Some((text, doc)))
}

/// The file's new text after the edit, or `None` when the edit leaves the file as it is.
/// Three edits leave it as it is: a `set` of the value already there, as a read takes it; an
/// `unset` of an absent entry; any `unset` when there is no file.
fn apply(
    entry: &Entry,
    value: Option<&Value>,
    file: Option<(String, DocumentMut)>,
) -> Result<Option<String>, SettingsError> {
    let (before, mut doc) = match file {
        Some(file) => file,
        None if value.is_none() => return Ok(None),
        None => (String::new(), DocumentMut::new()),
    };
    let tables: Vec<&str> = entry.tables.iter().map(String::as_str).collect();
    let edited = match value {
        Some(value) => {
            let table = table_at(doc.as_table_mut(), &tables, 0).map_err(|segment| {
                invalid(
                    entry,
                    format!("`{segment}` in the settings file is not a table"),
                )
            })?;
            put(table, entry, value)?
        }
        None => {
            if entry_is_table(doc.as_table(), &tables, entry.key.leaf()) {
                return Err(invalid(entry, HOLDS_A_TABLE.to_owned()));
            }
            remove_entry(doc.as_table_mut(), &tables, entry.key.leaf())
        }
    };
    if !edited {
        return Ok(None);
    }
    let after = tidy(&before, doc.to_string());
    Ok((after != before).then_some(after))
}

/// The table at `path[at..]` under `table`, made where absent:
/// - `provider` and `provider.<id>` are made implicit, so they get no header of their own;
/// - the last table on the path is made explicit (§6.4);
/// - a table made under a dotted table is dotted too, and one made under an inline table is
///   inline, so the file keeps its style.
///
/// `Err` names the first segment that exists but is not a table.
fn table_at<'t>(
    table: &'t mut dyn TableLike,
    path: &[&str],
    at: usize,
) -> Result<&'t mut dyn TableLike, String> {
    let Some(&segment) = path.get(at) else {
        return Ok(table);
    };
    if !table.contains_key(segment) {
        let mut made = Table::new();
        made.set_implicit(at + 1 < path.len());
        made.set_dotted(table.is_dotted());
        table.insert(segment, Item::Table(made));
    }
    match table.get_mut(segment).and_then(Item::as_table_like_mut) {
        Some(child) => table_at(child, path, at + 1),
        None => Err(path[..=at].join(".")),
    }
}

/// Writes `value` as the entry's leaf in `table`. An entry already there keeps its decor: the
/// comments above its line stay with its key, and the comment at the end of its line with its
/// value. Returns `false` when the entry already holds `value`, as a read takes it.
fn put(table: &mut dyn TableLike, entry: &Entry, value: &Value) -> Result<bool, SettingsError> {
    let mut item = value.to_item();
    match table.get_mut(entry.key.leaf()) {
        Some(old) => {
            if entry.key.parse_item(old, &mut |_: String| {}).as_ref() == Some(value) {
                return Ok(false);
            }
            // An inline table is a value to toml_edit, but a table to the file's reader.
            let Some(decor) = old
                .as_value()
                .filter(|v| !v.is_inline_table())
                .map(|v| v.decor().clone())
            else {
                return Err(invalid(entry, HOLDS_A_TABLE.to_owned()));
            };
            if let Some(new) = item.as_value_mut() {
                *new.decor_mut() = decor;
            }
            *old = item;
        }
        None => {
            table.insert(entry.key.leaf(), item);
        }
    }
    Ok(true)
}

/// Whether the entry `leaf` in the table at `path` under `table` is itself a table: a table, an
/// inline table or an array of tables. `unset` refuses one rather than delete what it holds.
fn entry_is_table(table: &dyn TableLike, path: &[&str], leaf: &str) -> bool {
    let mut current = table;
    for segment in path {
        match current.get(segment).and_then(Item::as_table_like) {
            Some(child) => current = child,
            None => return false,
        }
    }
    current
        .get(leaf)
        .is_some_and(|item| item.is_table_like() || item.is_array_of_tables())
}

/// Removes `leaf` from the table at `path` under `table`, then each table on `path` the
/// removal leaves empty, deepest first, unless that table holds a comment. Returns whether
/// `leaf` was there.
fn remove_entry(table: &mut dyn TableLike, path: &[&str], leaf: &str) -> bool {
    let Some((&first, rest)) = path.split_first() else {
        return table.remove(leaf).is_some();
    };
    let Some(child) = table.get_mut(first).and_then(Item::as_table_like_mut) else {
        return false;
    };
    let removed = remove_entry(child, rest, leaf);
    let emptied = removed && child.is_empty();
    if emptied && !holds_comment(table, first) {
        table.remove(first);
    }
    removed
}

/// Whether the table `name` in `parent` holds a comment (§6.4). Where the comment may sit
/// depends on how the table is written:
/// - a `[header]` table: above its header, or at the end of the header's line;
/// - an inline table: above its line, or at the end of it.
///
/// A comment between keys belongs to the key below it, so an empty table holds no other.
fn holds_comment(parent: &dyn TableLike, name: &str) -> bool {
    let own = match parent.get(name) {
        Some(Item::Table(table)) => Some(table.decor()),
        Some(Item::Value(value)) => Some(value.decor()),
        _ => None,
    };
    let key = parent.key(name).map(|key| key.leaf_decor());
    own.into_iter().chain(key).any(has_comment)
}

fn has_comment(decor: &Decor) -> bool {
    [decor.prefix(), decor.suffix()]
        .into_iter()
        .flatten()
        .any(|raw| raw.as_str().is_some_and(|text| text.contains('#')))
}

/// The new text, without a blank first line that the file did not have. Removing the first
/// table would otherwise leave the next table's leading blank line at the top.
fn tidy(before: &str, after: String) -> String {
    if before.starts_with('\n') {
        after
    } else {
        after.trim_start_matches('\n').to_owned()
    }
}
```

- [ ] **Step 4: Run the tests and see them pass**

Run: `cargo test -p tagteam-engine --test config` — Expected: PASS. The lock-timeout test takes
about 5 s.
Run: `cargo test -p tagteam-engine --features test-hooks --test config` — Expected: PASS,
including `hooks::a_writer_waits_while_another_holds_the_settings_lock_and_both_keys_land`.
Run: `cargo test -p tagteam-engine --lib error::tests` — Expected: PASS.
Run: `cargo test -p tagteam-engine --features test-hooks` — Expected: PASS. This also checks that
the M4a link-sync tests still pass with `is_private` now `pub(crate)`.

- [ ] **Step 5: Format, lint, commit**

```bash
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p tagteam-engine --features test-hooks
git add crates/tagteam-engine/src/config.rs crates/tagteam-engine/src/lib.rs \
  crates/tagteam-engine/src/profiles.rs crates/tagteam-engine/src/error.rs \
  crates/tagteam-engine/tests/config.rs
git commit -m "Write one settings entry strictly under the settings lock, keeping every comment"
```

#### Part B: `tagteam config set` and `unset`

**Wording.** The human lines name the entry and show the value as the file now holds it, as
TOML: a string keeps its quotes and an empty list reads `[]`. Task 2's `display()` would print
nothing for an empty list, which is the very value Review Focus 2 sets. A changed file is named,
because with a symlinked `config.toml` that is the useful part.
- `Set provider.claude-code.autoswitch.models = [] in /home/u/.config/tagteam/config.toml.`
- `ui.color is already "never".`
- `Removed ui.color from /home/u/.config/tagteam/config.toml.`
- `ui.color is not set.`

- [ ] **Step 1: Write the failing tests**

Append to `crates/tagteam/tests/config_cli.rs`, which Task 3 created:

```rust
/// `config set` and `config unset` through the real binary (§6.4).
mod set_and_unset {
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::path::{Path, PathBuf};
    use std::process::Stdio;

    use serde_json::{Value, json};
    use tagteam_engine::settings::config_path;
    use tagteam_provider::Env;

    use super::common::{cmd, std_cmd};

    /// The settings file of the binary's environment under `root`.
    fn settings_file(root: &Path) -> PathBuf {
        config_path(&Env::for_test(root))
    }

    /// The exit code of `args --json`, and the one object it printed.
    fn run_json(root: &Path, args: &[&str]) -> (i32, Value) {
        let out = cmd(root).args(args).arg("--json").output().unwrap();
        (
            out.status.code().unwrap(),
            serde_json::from_slice(&out.stdout).unwrap(),
        )
    }

    #[test]
    fn set_and_unset_answer_with_the_spec_s_json_shape() {
        // §6.4: `{schemaVersion, ok, key, value, changed}`, with `value` null for `unset`.
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let models = "provider.claude-code.autoswitch.models";

        assert_eq!(
            run_json(root, &["config", "set", models, "Fable, opus"]),
            (
                0,
                json!({"schemaVersion": 1, "ok": true, "key": models, "value": ["Fable", "opus"], "changed": true})
            )
        );
        // `--provider` names the same entry, which already holds the list.
        assert_eq!(
            run_json(
                root,
                &[
                    "config",
                    "set",
                    "autoswitch.models",
                    "Fable,opus",
                    "--provider",
                    "claude-code"
                ]
            ),
            (
                0,
                json!({"schemaVersion": 1, "ok": true, "key": models, "value": ["Fable", "opus"], "changed": false})
            )
        );
        assert_eq!(
            run_json(root, &["config", "set", "autoswitch.threshold", "85.5"]),
            (
                0,
                json!({"schemaVersion": 1, "ok": true, "key": "autoswitch.threshold", "value": 85.5, "changed": true})
            )
        );
        for changed in [true, false] {
            assert_eq!(
                run_json(
                    root,
                    &["config", "unset", "autoswitch.models", "-p", "claude-code"]
                ),
                (
                    0,
                    json!({"schemaVersion": 1, "ok": true, "key": models, "value": null, "changed": changed})
                )
            );
        }
        assert_eq!(
            fs::read_to_string(settings_file(root)).unwrap(),
            "[autoswitch]\nthreshold = 85.5\n"
        );
    }

    #[test]
    fn a_refused_value_or_key_exits_1_with_invalid_input_and_writes_nothing() {
        // §6.4, §13.1. `ConfigKeyParser` passes any key, so an unknown key reaches the engine:
        // exit 1, not clap's 2 (Decision 14). A value that starts with `-` reaches the engine
        // too.
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        for args in [
            &["config", "set", "autoswitch.threshold", "100"][..],
            &["config", "set", "autoswitch.cooldown_seconds", "-1"],
            &["config", "set", "autoswitch.treshold", "80"],
            &["config", "set", "provider.claude-code.ui.color", "never"],
            &[
                "config",
                "set",
                "ui.color",
                "never",
                "--provider",
                "claude-code",
            ],
            &[
                "config",
                "set",
                "provider.claude-code.autoswitch.models",
                "all,Fable",
            ],
            &["config", "set", "default_provider", "codex"],
            &["config", "unset", "autoswitch.treshold"],
        ] {
            let (code, v) = run_json(root, args);
            assert_eq!(code, 1, "{args:?}");
            assert_eq!(v["schemaVersion"], 1, "{args:?}");
            assert_eq!(v["error"]["type"], "invalid-input", "{args:?}: {v}");
        }
        // A provider this build lacks is `unknown-provider` in both spellings, as for `get`
        // (Decision 4).
        for args in [
            &["config", "set", "provider.codex.autoswitch.threshold", "80"][..],
            &[
                "config",
                "set",
                "autoswitch.threshold",
                "80",
                "--provider",
                "codex",
            ],
            &["config", "unset", "provider.codex.autoswitch.threshold"],
        ] {
            let (code, v) = run_json(root, args);
            assert_eq!(code, 1, "{args:?}");
            assert_eq!(v["error"]["type"], "unknown-provider", "{args:?}: {v}");
        }
        // A missing VALUE is a usage error.
        cmd(root)
            .args(["config", "set", "ui.color"])
            .assert()
            .code(2);
        // Without `--json`, the refusal is one line on stderr.
        cmd(root)
            .args(["config", "set", "autoswitch.threshold", "100"])
            .assert()
            .code(1)
            .stdout("")
            .stderr(predicates::str::starts_with("tagteam: "));
        assert!(!settings_file(root).exists());
    }

    #[test]
    fn set_and_unset_say_what_they_changed() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let file = settings_file(root).display().to_string();
        let says = |args: &[&str], text: String| {
            cmd(root)
                .args(args)
                .assert()
                .success()
                .stdout(text)
                .stderr("");
        };

        says(
            &["config", "set", "ui.color", "never"],
            format!("Set ui.color = \"never\" in {file}.\n"),
        );
        says(
            &["config", "set", "ui.color", "never"],
            "ui.color is already \"never\".\n".into(),
        );
        says(
            &[
                "config",
                "set",
                "provider.claude-code.autoswitch.models",
                "",
            ],
            format!("Set provider.claude-code.autoswitch.models = [] in {file}.\n"),
        );
        says(
            &["config", "unset", "ui.color"],
            format!("Removed ui.color from {file}.\n"),
        );
        says(
            &["config", "unset", "ui.color"],
            "ui.color is not set.\n".into(),
        );
    }

    #[test]
    fn a_corrupt_file_is_refused_with_settings_unreadable_and_left_as_it_was() {
        // §6.4: `set` and `unset` refuse to write to a corrupt file.
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let file = settings_file(root);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "[ui\ncolor = \"never\"\n").unwrap();
        for args in [
            &["config", "set", "ui.color", "auto"][..],
            &["config", "unset", "ui.color"],
        ] {
            let (code, v) = run_json(root, args);
            assert_eq!(code, 1, "{args:?}");
            assert_eq!(v["error"]["type"], "settings-unreadable", "{args:?}: {v}");
        }
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "[ui\ncolor = \"never\"\n"
        );
    }

    #[test]
    fn a_symlinked_settings_file_is_written_through_and_stays_a_link() {
        // Review Focus 1, through the binary, with an absolute link (the engine test uses a
        // relative one).
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let target = root.join("dotfiles/tagteam.toml");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(
            &target,
            "# from my dotfiles\n[ui]\ncolor = \"auto\" # for now\n",
        )
        .unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
        let link = settings_file(root);
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        symlink(&target, &link).unwrap();

        cmd(root)
            .args(["config", "set", "ui.color", "never"])
            .assert()
            .success();

        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "# from my dotfiles\n[ui]\ncolor = \"never\" # for now\n"
        );
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o7777,
            0o644
        );
    }

    #[test]
    fn processes_setting_different_keys_at_once_all_land() {
        // §6.4: the settings lock orders each read, edit and write across processes, so no
        // process writes over another's key.
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let writes = [
            ("autoswitch.threshold", "85.5", "threshold = 85.5"),
            (
                "autoswitch.interval_seconds",
                "120",
                "interval_seconds = 120",
            ),
            (
                "autoswitch.cooldown_seconds",
                "600",
                "cooldown_seconds = 600",
            ),
            ("autoswitch.unhealthy_ticks", "4", "unhealthy_ticks = 4"),
            (
                "usage.history_retention_days",
                "30",
                "history_retention_days = 30",
            ),
            ("ui.color", "never", "color = \"never\""),
        ];
        let children: Vec<_> = writes
            .iter()
            .map(|&(key, value, _)| {
                std_cmd(root)
                    .args(["config", "set", key, value])
                    .stdout(Stdio::null())
                    .stderr(Stdio::piped())
                    .spawn()
                    .unwrap()
            })
            .collect();
        for child in children {
            let out = child.wait_with_output().unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        let text = fs::read_to_string(settings_file(root)).unwrap();
        for (key, _, line) in writes {
            assert!(text.lines().any(|l| l == line), "{key} was lost:\n{text}");
        }
    }
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam --features test-support --test config_cli set_and_unset`
Expected: the six `set_and_unset::` tests fail, because `config set` and `config unset` are not
subcommands yet: clap exits 2.
- `set_and_unset_answer_with_the_spec_s_json_shape` fails with `assertion left == right
  failed`, where `left` is `(2, {"schemaVersion":1,"error":{"type":"usage",…}})`.
- The other five fail on their exit-code or `success()` assertions.

- [ ] **Step 3: Implement**

In `crates/tagteam/src/cli.rs`, add these two variants to Task 3's `enum ConfigAction`, after
`Get` and before `Path`:

```rust
    /// Set KEY to VALUE in config.toml
    ///
    /// Numbers are written as typed, and booleans (also 1/0 and yes/no) as true or false. A
    /// list is comma-separated names, and '' is an empty list. --provider, or a provider.<id>.
    /// prefix, names a provider's own entry.
    Set {
        #[arg(value_parser = ConfigKeyParser, hide_possible_values = true)]
        key: String,
        /// Taken as typed, even when it starts with '-'
        #[arg(allow_hyphen_values = true)]
        value: String,
    },
    /// Remove KEY from config.toml, so the global value or the default applies again
    Unset {
        #[arg(value_parser = ConfigKeyParser, hide_possible_values = true)]
        key: String,
    },
```

clap 4.6 still parses a known flag after KEY as a flag: `config set KEY --json` is a missing
VALUE, which exits 2. An unknown `-…` value, such as `-1`, reaches the engine. A value that is
itself a known flag needs `--` before it.

In `crates/tagteam/src/config_cmd.rs`, add `use tagteam_engine::config::SettingsChange;` to the
imports, and append:

```rust
/// `config set` and `unset` for a person (§6.4). The value is shown as the file now holds it,
/// as TOML, so a string keeps its quotes and an empty list reads `[]`. A changed file is named;
/// with a symlinked `config.toml` that is the useful part.
pub(crate) fn change_human(change: &SettingsChange) -> String {
    let path = change.path.display();
    match (&change.value, change.changed) {
        (Some(value), true) => format!("Set {} = {} in {path}.\n", change.key, value.to_item()),
        (Some(value), false) => format!("{} is already {}.\n", change.key, value.to_item()),
        (None, true) => format!("Removed {} from {path}.\n", change.key),
        (None, false) => format!("{} is not set.\n", change.key),
    }
}

/// §6.4: `{schemaVersion, ok, key, value, changed}`, with `value` null for `unset`.
pub(crate) fn change_json(change: &SettingsChange) -> serde_json::Value {
    serde_json::json!({
        "schemaVersion": 1,
        "ok": true,
        "key": change.key,
        "value": change.value.as_ref().map_or(serde_json::Value::Null, |v| v.to_json()),
        "changed": change.changed,
    })
}
```

In `crates/tagteam/src/app.rs`, in the `match` over `ConfigAction` that Task 3 added for
`Command::Config`, add these arms before the `ConfigAction::Path` arm:

```rust
            ConfigAction::Set { key, value } => {
                let change = self
                    .engine
                    .config_set(&key, self.provider_flag.as_ref(), &value)?;
                self.print(
                    &config_cmd::change_human(&change),
                    config_cmd::change_json(&change),
                );
            }
            ConfigAction::Unset { key } => {
                let change = self
                    .engine
                    .config_unset(&key, self.provider_flag.as_ref())?;
                self.print(
                    &config_cmd::change_human(&change),
                    config_cmd::change_json(&change),
                );
            }
```

`Command::touches_keychain` stays false for `config`, so a locked keychain never blocks a
settings write.

- [ ] **Step 4: Run the tests and see them pass**

Run: `cargo test -p tagteam --features test-support --test config_cli` — Expected: PASS
(Task 3's tests and the six `set_and_unset::` tests).
Run: `cargo test -p tagteam --features test-support` — Expected: PASS.
Run: `cargo test -p tagteam --lib` — Expected: PASS (no features).

- [ ] **Step 5: Format, lint, commit**

```bash
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p tagteam --features test-support
git add crates/tagteam/src/cli.rs crates/tagteam/src/app.rs crates/tagteam/src/config_cmd.rs \
  crates/tagteam/tests/config_cli.rs
git commit -m "Add config set and config unset"
```

---

### Task 5: The log file

§14.2: "The file is `$XDG_STATE_HOME/tagteam/tagteam.log` (§5), mode 0600 in a 0700 directory.
It is opened on the first event that passes its filter … When it exceeds 1 MiB it is rotated:
`.1` becomes `.2`, the file becomes `.1`, and the oldest is dropped." Several processes share
the file. "Each event is one `write` on a descriptor opened with `O_APPEND`, so lines never
interleave. The process that finds the file over 1 MiB rotates it while holding a try-only
`flock` on `tagteam.log.lock`, re-checking the size under that lock; a process that cannot take
the lock does not rotate. Before each write, a process checks that the path's inode still
matches its descriptor, and reopens the path when it does not." Finally, "A file that cannot be
opened, written or rotated disables file logging for the rest of the process." This task builds
`logfile::LogFile`, the `MakeWriter` behind Task 6's file layer (Decision 7). It touches nothing
until the first line arrives. It turns each event into one `write(2)`, rotates under a try-only
lock, follows a rotation another process made, and disables itself on the first failure. Task 6
decides what `--debug` prints when that happens, through a callback.

**Readings of the spec this task commits to:**
- **"Exceeds 1 MiB" means `len > limit`.** It is checked before each write, against the file
  the path names. The line that crosses the limit is written to the old file, and the next
  write rotates it. A line is never split across two files.
- **"Inode" is the pair `(st_dev, st_ino)`.** A path that no longer exists, because a person
  deleted it, counts as moved on: the path is reopened, which creates a new file.
- **After a rotation attempt, the inode check runs again.** It does not matter whether this
  process rotated or found another process had. The line therefore goes to the file the path
  now names.
  - A line can still land in `.1` (kept) only if this process pauses between that check and
    its write.
  - Across two rotations it can land in a file already gone. That is §14.2's one best-effort
    case, and it is accepted.
- **Directory and file modes.**
  - The directory is created with `ensure_private_dir`, which also creates missing parents at
    0700, as every tagteam directory is (§5). An existing directory's mode is left alone.
  - The file is created with mode 0600 (`O_CREAT` with that mode; the umask can only narrow
    it). An existing file keeps its mode and is never chmod'ed (§5).
- **A short write is a failure.** The rest of the line could no longer join its start. A write
  that `EINTR` interrupted before it wrote anything is repeated; it is still one successful
  `write(2)`.
- **Disabling is per `LogFile`, so per process.** The first failure's cause goes once to an
  optional `OnDisable` callback, which Task 6 sets only under `--debug`. Nothing else prints:
  §14.2 says "silently unless `--debug`".
- **The rotation lock is outside §4.3's order** because it is only ever tried. A line logged
  under the mutation or account locks may try it. Nothing is taken while it is held: only two
  renames run under it.
- **"Nothing waits" holds within the process too.** No lock is waited for. An in-process mutex
  serializes this process's own threads around one `stat`, one `fstat` and one `write`.
- **The one-write rule is pinned with a write counter, not a racing reader.** A test-only
  counter of `write(2)` calls proves the pieces a formatter writes reach the file as one call.
  A reader looking for a torn line cannot be made deterministic, because Linux may expose a
  multi-page write in progress.

**Files:**
- Create: `crates/tagteam/src/logfile.rs`
- Modify: `crates/tagteam/src/lib.rs` (the module list, after `mod history;`, line 9)

**Interfaces:**
- Consumes:
  - `tagteam_provider::FlockGuard::try_lock(path: &Path) -> io::Result<Option<FlockGuard>>`. It
    creates the lock file 0600 and its parent 0700. The `flock` belongs to the open file
    description, so a second `LogFile` in one process contends with the first exactly as
    another process would.
  - `tagteam_provider::atomic::ensure_private_dir(path: &Path) -> io::Result<()>`.
  - `tracing_subscriber::fmt::MakeWriter<'a>`.
- Produces (crate-private, `crate::logfile`):
  - `pub(crate) const FILE_NAME: &str = "tagteam.log"`. M5b's doctor can name the log as
    `Env::state_dir().join(FILE_NAME)`.
  - `pub(crate) type OnDisable = Box<dyn Fn(&Path, &io::Error) + Send + Sync>`
  - `pub(crate) struct LogFile`, with:
    - `LogFile::new(state_dir: PathBuf) -> LogFile`, limit 1 MiB;
    - `#[cfg(test)] LogFile::with_limit(state_dir: PathBuf, limit: u64) -> LogFile`;
    - `LogFile::on_disable(self, report: OnDisable) -> LogFile`.
  - `impl<'a> MakeWriter<'a> for LogFile { type Writer = EventWriter<'a>; }`
  - `pub(crate) struct EventWriter<'a>`, which implements `io::Write`. It gathers the bytes and
    hands them to the file as one `write(2)` when dropped.

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam/src/logfile.rs` containing only this test module (Step 3 puts the
implementation above it):

```rust
#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Arc, Barrier};

    use super::*;

    fn mode(p: &Path) -> u32 {
        fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    /// The file's text; empty when it does not exist.
    fn read(p: &Path) -> String {
        fs::read_to_string(p).unwrap_or_default()
    }

    /// The directory's entries, by name, sorted.
    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    /// The kept lines, oldest first: `.2`, then `.1`, then the file.
    fn kept(dir: &Path) -> String {
        ["tagteam.log.2", "tagteam.log.1", FILE_NAME]
            .iter()
            .map(|n| read(&dir.join(n)))
            .collect()
    }

    /// One event's line, handed over as a formatter hands it: in two pieces, then dropped.
    fn log(file: &LogFile, line: &str) {
        let mut w = file.make_writer();
        let (head, tail) = line.split_at(line.len() / 2);
        w.write_all(head.as_bytes()).unwrap();
        w.write_all(tail.as_bytes()).unwrap();
    }

    #[test]
    fn nothing_is_created_before_the_first_line() {
        // §14.2: the file is opened on the first event, so a command that logs nothing
        // creates nothing (§5).
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("state/tagteam");
        let file = LogFile::new(dir.clone());
        drop(file.make_writer());
        assert!(!d.path().join("state").exists());
        log(&file, "one\n");
        assert_eq!(read(&dir.join(FILE_NAME)), "one\n");
    }

    #[test]
    fn the_file_is_0600_in_a_0700_directory() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("state/tagteam");
        let file = LogFile::with_limit(dir.clone(), 8);
        log(&file, "first line\n");
        log(&file, "second line\n"); // the first is over the limit: it rotates to `.1`
        assert_eq!(mode(&d.path().join("state")), 0o700);
        assert_eq!(mode(&dir), 0o700);
        for name in [FILE_NAME, "tagteam.log.1", LOCK_NAME] {
            assert_eq!(mode(&dir.join(name)), 0o600, "{name}");
        }
    }

    #[test]
    fn each_line_is_one_write_however_it_was_formatted() {
        // Decision 7: the pieces a formatter writes are gathered, and the line reaches the
        // file as one write(2), which O_APPEND keeps whole among other processes' lines.
        let d = tempfile::tempdir().unwrap();
        let file = LogFile::new(d.path().to_path_buf());
        for n in 0..5 {
            log(&file, &format!("line {n}\n"));
        }
        assert_eq!(file.writes.load(Ordering::SeqCst), 5);
        assert_eq!(
            read(&d.path().join(FILE_NAME)),
            "line 0\nline 1\nline 2\nline 3\nline 4\n"
        );
    }

    #[test]
    fn rotation_keeps_the_file_and_two_older_ones() {
        // §14.2: `.1` becomes `.2`, the file becomes `.1`, and the oldest is dropped.
        let d = tempfile::tempdir().unwrap();
        let file = LogFile::with_limit(d.path().to_path_buf(), 100);
        // 30 bytes each: four fill a file past 100, and the fifth rotates it.
        let lines: Vec<String> = (0..40)
            .map(|n| format!("line {n:02} {}\n", "x".repeat(21)))
            .collect();
        for line in &lines {
            log(&file, line);
        }
        assert_eq!(
            names(d.path()),
            [FILE_NAME, "tagteam.log.1", "tagteam.log.2", LOCK_NAME]
        );
        assert_eq!(
            kept(d.path()),
            lines[28..].concat(),
            "the last 12, in order"
        );
        for old in ["tagteam.log.1", "tagteam.log.2"] {
            assert_eq!(read(&d.path().join(old)).len(), 120, "{old}");
        }
    }

    #[test]
    fn a_process_that_cannot_take_the_rotation_lock_does_not_rotate() {
        // §14.2: rotation is only tried. Whoever holds the lock is rotating, and nothing waits.
        let d = tempfile::tempdir().unwrap();
        let file = LogFile::with_limit(d.path().to_path_buf(), 10);
        log(&file, "over the limit already\n");
        // Its own open file description: to `flock`, another process.
        let held = FlockGuard::try_lock(&d.path().join(LOCK_NAME))
            .unwrap()
            .unwrap();
        log(&file, "second\n");
        assert!(!d.path().join("tagteam.log.1").exists());
        assert_eq!(
            read(&d.path().join(FILE_NAME)),
            "over the limit already\nsecond\n"
        );
        drop(held);
        log(&file, "third\n");
        assert_eq!(
            read(&d.path().join("tagteam.log.1")),
            "over the limit already\nsecond\n"
        );
        assert_eq!(read(&d.path().join(FILE_NAME)), "third\n");
    }

    #[test]
    fn a_file_moved_away_is_noticed_and_the_path_reopened() {
        // §14.2: another process rotated it, or a person moved or deleted it. The inode check
        // reopens the path, so the next line starts a new file.
        let d = tempfile::tempdir().unwrap();
        let file = LogFile::new(d.path().to_path_buf());
        log(&file, "before\n");
        fs::rename(d.path().join(FILE_NAME), d.path().join("tagteam.log.1")).unwrap();
        log(&file, "after\n");
        assert_eq!(read(&d.path().join("tagteam.log.1")), "before\n");
        assert_eq!(read(&d.path().join(FILE_NAME)), "after\n");
        assert_eq!(mode(&d.path().join(FILE_NAME)), 0o600);
        fs::remove_file(d.path().join(FILE_NAME)).unwrap();
        log(&file, "recreated\n");
        assert_eq!(read(&d.path().join(FILE_NAME)), "recreated\n");
    }

    #[test]
    fn a_log_that_cannot_be_opened_is_disabled_once_and_quietly() {
        // §14.2: logging never fails a command. A state directory under a regular file can
        // never be created, by root either.
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("blocker"), "").unwrap();
        let reports = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&reports);
        let file =
            LogFile::new(d.path().join("blocker/tagteam")).on_disable(Box::new(move |path, _| {
                seen.lock().unwrap().push(path.to_path_buf())
            }));
        log(&file, "one\n");
        log(&file, "two\n");
        assert_eq!(
            *reports.lock().unwrap(),
            [d.path().join("blocker/tagteam/tagteam.log")],
            "reported once, and never tried again"
        );
        assert!(file.disabled.load(Ordering::SeqCst));
        assert_eq!(file.writes.load(Ordering::SeqCst), 0);
    }

    /// Writer `w`'s line `n`, 100 bytes, padded with `w` itself: a line cut short, or spliced
    /// with the other writer's, no longer parses.
    fn numbered(w: char, n: usize) -> String {
        format!("{w} {n:05} {}\n", w.to_string().repeat(91))
    }

    /// The writer and number of a whole `numbered` line; a panic naming any other line.
    fn parse(line: &str) -> (char, usize) {
        let parts: Vec<&str> = line.split(' ').collect();
        let whole = parts.len() == 3
            && parts[0].chars().count() == 1
            && parts[1].len() == 5
            && parts[2] == parts[0].repeat(91);
        assert!(whole, "a line was cut or interleaved: {line:?}");
        (parts[0].chars().next().unwrap(), parts[1].parse().unwrap())
    }

    #[test]
    fn two_processes_logging_across_rotations_keep_every_line_whole_and_in_order() {
        // §15.2 and Review Focus 4. Each `LogFile` has its own descriptor, and tries the
        // rotation lock through its own open file description, as two processes do.
        const LINES: usize = 2000;
        let d = tempfile::tempdir().unwrap();
        let start = Arc::new(Barrier::new(2));
        let writers: Vec<_> = ['a', 'b']
            .into_iter()
            .map(|w| {
                let (dir, start) = (d.path().to_path_buf(), Arc::clone(&start));
                std::thread::spawn(move || {
                    let file = LogFile::with_limit(dir, 64 * 1024);
                    start.wait();
                    for n in 0..LINES {
                        log(&file, &numbered(w, n));
                    }
                    assert!(!file.disabled.load(Ordering::SeqCst), "{w} was disabled");
                })
            })
            .collect();
        for w in writers {
            w.join().unwrap();
        }
        assert!(
            d.path().join("tagteam.log.2").exists(),
            "it rotated, more than once"
        );
        let lines: Vec<(char, usize)> = kept(d.path()).lines().map(parse).collect();
        assert!(!lines.is_empty());
        for w in ['a', 'b'] {
            let seen: Vec<usize> = lines
                .iter()
                .filter(|(c, _)| *c == w)
                .map(|(_, n)| *n)
                .collect();
            // A writer whose lines were all rotated out keeps none; any it keeps run without
            // a gap from the first kept to its last.
            if let Some(&first) = seen.first() {
                assert_eq!(seen, (first..LINES).collect::<Vec<_>>(), "{w}");
            }
        }
    }
}
```

In `crates/tagteam/src/lib.rs`, replace:

```rust
mod history;
pub mod prompt;
```

with:

```rust
mod history;
// Only its own tests use it until the logging setup (Task 6) writes through it.
#[allow(dead_code)]
mod logfile;
pub mod prompt;
```

These tests use threads, never `fork`. No lib test in this crate forks today. If one ever does,
take a `FORK_GUARD`, as `tagteam-provider`'s lib tests do. Without one, a child forked while
`a_process_that_cannot_take_the_rotation_lock_does_not_rotate` drops its guard would hold the
`flock` a moment longer.

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam --lib logfile`
Expected: the build fails. The test module is the whole file, so it reports
`error[E0425]: cannot find value \`FILE_NAME\` in this scope`, `error[E0433]: failed to resolve:
use of undeclared type \`LogFile\``, and `error[E0412]` / `E0433` for `Path`, `fs`, `Mutex`,
`Ordering`, `FlockGuard` and `LOCK_NAME`.

- [ ] **Step 3: Implement**

In `crates/tagteam/src/logfile.rs`, insert above `#[cfg(test)]`:

```rust
//! §14.2's log file: `tagteam.log` in the state directory, private, rotated past 1 MiB to `.1`
//! and `.2`, and shared by every tagteam process. It is opened on the first line that reaches
//! it, and nothing ever waits on it.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

use tagteam_provider::FlockGuard;
use tagteam_provider::atomic::ensure_private_dir;
use tracing_subscriber::fmt::MakeWriter;

/// §5, §14.2.
pub(crate) const FILE_NAME: &str = "tagteam.log";
const LOCK_NAME: &str = "tagteam.log.lock";
/// §14.2: a file over 1 MiB is rotated.
const LIMIT: u64 = 1024 * 1024;

/// Told once why the file was disabled: `--debug`'s notice (§14.2).
pub(crate) type OnDisable = Box<dyn Fn(&Path, &io::Error) + Send + Sync>;

/// One process's log file (Decision 7). Every line is one `write(2)` on an `O_APPEND`
/// descriptor, so the lines of several processes never interleave. The first failure to open,
/// write or rotate it disables it for the rest of the process: logging never fails a command.
pub(crate) struct LogFile {
    dir: PathBuf,
    path: PathBuf,
    lock: PathBuf,
    limit: u64,
    /// The descriptor lines go to: opened on the first one, reopened when the path moves on.
    file: Mutex<Option<File>>,
    disabled: AtomicBool,
    on_disable: Option<OnDisable>,
    /// Every `write(2)` made, so a test can tell that a line was one.
    #[cfg(test)]
    writes: std::sync::atomic::AtomicUsize,
}

impl LogFile {
    /// `tagteam.log` in `state_dir`, rotated past 1 MiB. Nothing is touched before a line
    /// arrives.
    pub(crate) fn new(state_dir: PathBuf) -> Self {
        Self::at(state_dir, LIMIT)
    }

    /// `new` with a rotation limit small enough for a test to cross.
    #[cfg(test)]
    pub(crate) fn with_limit(state_dir: PathBuf, limit: u64) -> Self {
        Self::at(state_dir, limit)
    }

    fn at(dir: PathBuf, limit: u64) -> Self {
        Self {
            path: dir.join(FILE_NAME),
            lock: dir.join(LOCK_NAME),
            dir,
            limit,
            file: Mutex::new(None),
            disabled: AtomicBool::new(false),
            on_disable: None,
            #[cfg(test)]
            writes: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Calls `report` with the path and the cause when the file is disabled.
    pub(crate) fn on_disable(mut self, report: OnDisable) -> Self {
        self.on_disable = Some(report);
        self
    }

    /// `tagteam.log.<n>`.
    fn rotated(&self, n: u8) -> PathBuf {
        self.dir.join(format!("{FILE_NAME}.{n}"))
    }

    /// Hands one finished line to the file. It never waits on another process and never
    /// reports an error: the first failure disables the file, and `on_disable` hears why.
    fn append(&self, line: &[u8]) {
        if line.is_empty() || self.disabled.load(Ordering::Acquire) {
            return;
        }
        let mut slot = self.file.lock().unwrap_or_else(PoisonError::into_inner);
        // Another thread may have disabled it while this one waited.
        if self.disabled.load(Ordering::Acquire) {
            return;
        }
        if let Err(e) = self.append_to(&mut slot, line) {
            *slot = None;
            self.disabled.store(true, Ordering::Release);
            if let Some(report) = &self.on_disable {
                report(&self.path, &e);
            }
        }
    }

    /// Before the write, the path must still name the descriptor, and a file over the limit is
    /// rotated unless another process is rotating it (§14.2). Then the one write.
    fn append_to(&self, slot: &mut Option<File>, line: &[u8]) -> io::Result<()> {
        let mut file = self.current(slot.take())?;
        if file.metadata()?.len() > self.limit {
            self.rotate()?;
            file = self.current(Some(file))?;
        }
        let written = self.write_once(&file, line);
        *slot = Some(file);
        written
    }

    /// `held` while the path still names it (the same device and inode); otherwise the path,
    /// opened afresh. A path that is gone counts as moved on.
    fn current(&self, held: Option<File>) -> io::Result<File> {
        if let Some(file) = held {
            let named = match fs::metadata(&self.path) {
                Ok(m) => Some((m.dev(), m.ino())),
                Err(e) if e.kind() == io::ErrorKind::NotFound => None,
                Err(e) => return Err(e),
            };
            let ours = file.metadata()?;
            if named == Some((ours.dev(), ours.ino())) {
                return Ok(file);
            }
        }
        self.open()
    }

    /// The path, for appending: its directory created 0700 and the file 0600 when they are
    /// new (§5). An existing file keeps its mode.
    fn open(&self) -> io::Result<File> {
        ensure_private_dir(&self.dir)?;
        OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .open(&self.path)
    }

    /// `.1` becomes `.2` and the file `.1`, under a try-only lock, and only if the file is still
    /// over the limit once the lock is held: another process may have rotated it meanwhile. A
    /// lock someone holds means they are rotating, so this process writes on without. Nothing
    /// else is taken while it is held (§4.3).
    fn rotate(&self) -> io::Result<()> {
        let Some(_rotating) = FlockGuard::try_lock(&self.lock)? else {
            return Ok(());
        };
        match fs::metadata(&self.path) {
            Ok(m) if m.len() > self.limit => {}
            Ok(_) => return Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        }
        rename_present(&self.rotated(1), &self.rotated(2))?;
        rename_present(&self.path, &self.rotated(1))
    }

    /// One `write(2)` of the whole line. A write a signal interrupted before it wrote anything
    /// is made again; a short write is a failure, since the rest of the line could no longer
    /// join its start.
    fn write_once(&self, mut file: &File, line: &[u8]) -> io::Result<()> {
        loop {
            #[cfg(test)]
            self.writes.fetch_add(1, Ordering::SeqCst);
            match file.write(line) {
                Ok(n) if n == line.len() => return Ok(()),
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "a line was written only in part",
                    ));
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
    }
}

/// Renames `from` to `to`; a `from` that is already gone is no error.
fn rename_present(from: &Path, to: &Path) -> io::Result<()> {
    match fs::rename(from, to) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// One event's line, gathered as the formatter writes it and handed to the file whole when it
/// is dropped (Decision 7).
pub(crate) struct EventWriter<'a> {
    log: &'a LogFile,
    line: Vec<u8>,
}

impl Write for EventWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.line.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for EventWriter<'_> {
    fn drop(&mut self) {
        self.log.append(&self.line);
    }
}

impl<'a> MakeWriter<'a> for LogFile {
    type Writer = EventWriter<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        EventWriter {
            log: self,
            line: Vec::new(),
        }
    }
}

```

- [ ] **Step 4: Run the tests and see them pass**

Run: `cargo test -p tagteam --lib logfile`. Expected: PASS, 8 tests.
Then `cargo test -p tagteam --features test-support`. Expected: PASS. Nothing outside the module
uses `LogFile` yet.

- [ ] **Step 5: Format, lint, commit**

`cargo fmt --all && cargo fmt --all --check`,
`cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings`,
`cargo clippy --workspace --all-targets -- -D warnings`, then
`cargo test -p tagteam --features test-support`. Then:

```
git add crates/tagteam/src/logfile.rs crates/tagteam/src/lib.rs
git commit -m "Write the log file a whole line at a time, rotating it under a try-only lock"
```

---

### Task 6: The logging setup

§14.2 sets four rules for the subscriber:
- "The file records INFO and above by default. `--debug` records DEBUG and above, in the file
  and on stderr. `TAGTEAM_LOG` replaces the file's filter with an `EnvFilter` directive …, and
  `off` disables the file. An invalid directive keeps the default, with a warning on stderr.
  Otherwise stderr shows ERROR events only."
- "One line per event: a UTC timestamp to the millisecond, the pid, the level, the module, the
  message, then `key=value` fields … A path under the home directory is written `~/…`."
- "A panic is logged at ERROR, with its location, before the process unwinds."
- "Logging never fails a command … silently unless `--debug`."

This task builds `logging`, which assembles the subscriber from five parts:
- M2's stderr layer, now filtering out `tagteam::panic`;
- the file layer over Task 5's `LogFile`, with Decision 6's filter, which `TAGTEAM_LOG`
  replaces whole;
- tagteam's own line format (Decision 8);
- the panic hook (Decision 9);
- `--debug`'s notice when the file cannot be written.

It is installed once, with `set_global_default`, at the process boundary (Decision 5). `app::run`
no longer sets logging up.

**Re-sync (M3a Task 6, Task 1, M4a Task 8):**
- **The order at the process boundary.** Parse; the `statusline` drain;
  `Context::from_process()` (Task 1's `HOME` check, which returns before any logging on its
  `Err` arm); `logging::init`; `signals::install` (M3a); `TtyPrompter::new(cancel)` (M3a); then
  `app::run`.
- **Where the deletion lands after M3a.** The `init_logging` call this task deletes lives in
  M3a's `run_command`, not `run`. M4a Task 8 restructures `Context::from_process`, but `ctx.env`,
  `ctx.no_color_env` and `Env::{home, state_dir}` keep their names.
- **`hooks.rs`.** M3a Task 6 adds a pause point to `hooks.rs`. Step 3's change to `point` is
  additive.
- **The unlocked `Io.err`.** M3a Task 6's text for `main_with_args` still binds
  `std::io::stderr().lock()` for the whole command. Carry Step 3's replacement onto it: the
  `(out, err)` line takes an unlocked `std::io::stderr()`.

**Readings of the spec this task commits to:**
- **Logging is set up after the `HOME` check.** The log's path derives from `HOME`. A `HOME`
  failure is reported with no logging at all, and `statusline` prints nothing.
- **`TAGTEAM_LOG` replaces the file's filter, whatever `--debug` says.**
  - `--debug` still turns stderr to DEBUG.
  - `off`, in any case and with surrounding spaces, means no file layer at all.
  - An empty value is as good as unset, as `NO_COLOR`'s rule in `app.rs` has it.
  - A value that is not UTF-8 is invalid.
  - An invalid value gives one `warning:` line on stderr, before the command runs, and the
    default filter.
- **The stderr layer is M2's, unchanged** apart from leaving out `tagteam::panic`.
  - It keeps the default `fmt` format and timestamps, and has no target.
  - It is coloured only on a terminal and without `--no-color` or `NO_COLOR`.
  - `FORCE_COLOR` is not consulted for log lines, as today.
- **How a field is written** is tracing's default field formatting: the message first, then each
  field as `key=value`.
  - A string field is quoted, with Debug escapes (`kind="switch"`).
  - A `%` field is written bare (`account=0192…`).
  - A carriage return or newline anywhere in the finished line is escaped (`\r`, `\n`), so one
    event is one line.
  - ANSI escapes in a message are already escaped by tracing-subscriber.
- **The `~/` rewrite:**
  - The home path, with trailing slashes trimmed and one `/` appended, is replaced by `~/`
    anywhere in the finished line.
  - The home itself without a slash after it is left alone, as is a sibling such as
    `/Users/meg` for home `/Users/me`.
  - A home of `/`, or one that is not UTF-8, rewrites nothing. With `/`, every absolute path
    would become `~/…`.
- **The timestamp** is `format_iso8601` of the whole seconds, with `.mmm` before the `Z`. Its
  clock is `SystemTime`, not the engine's `Clock`: the subscriber exists before any engine.
- **The file layer's own write errors.** It sets `log_internal_errors(false)`, so a formatting or
  write error never prints tracing-subscriber's own message on stderr. `LogFile` never returns a
  write error to the layer anyway.
- **`--debug` and a file that cannot be written** give one stderr line, from the thread that
  found it:
  `warning: the log file <path> cannot be written (<cause>); nothing more is logged to it`.
  Without `--debug` nothing is printed.
- **`Io.err` is no longer a held lock.** `main_with_args` held `std::io::stderr().lock()` for the
  whole command. `list` and `status` collect usage on scoped threads, one per account (§8.3),
  while the main thread waits for them. A collector thread's event that reaches the stderr layer
  then blocks on that lock, and the command hangs: under `--debug` whenever a collected
  account's refresh is refused (the gate's quarantine WARN), and for any ERROR from a collector
  thread even without it. This is §14.2's "logging never fails a command", so it is fixed here.
  Each write still takes stderr's lock for its own line, and the prompter still locks stderr
  around a prompt. `a_collector_thread_s_log_line_under_debug_never_hangs_the_command` pins it.
- **A panic's message is logged only when it is a literal** (`&'static str` payload). Otherwise
  the line reads `panic: (a formatted message, not logged)`, with the location either way. A
  formatted message can carry runtime values, such as serde's `invalid type: string "…"`, which
  quotes the string it met, and that may be a token (B.69). The default hook still prints the
  whole message on stderr.
- **`TAGTEAM_TEST_FAIL_AT=panic:<name>`** panics the real binary at hook point `<name>`, as
  `Engine::fail_at("panic:<name>")` already does in-process. It exists only with `test-hooks`,
  so a release build has none; the panic test needs it.

**Files:**
- Create: `crates/tagteam/src/logging.rs`
- Create: `crates/tagteam/tests/logging.rs`
- Modify: `crates/tagteam/src/lib.rs`:
  - the module list: replace Task 5's `#[allow(dead_code)] mod logfile;`;
  - `main_with_args`: `logging::init` after the `ctx` binding, and the unlocked `Io.err`.
- Modify: `crates/tagteam/src/app.rs`:
  - delete `init_logging` (184–199 on `3c1f458`);
  - delete its call and the `color` binding at the top of `run` (296–297; M3a moves them into
    `run_command`).
- Modify: `crates/tagteam-engine/src/hooks.rs` (`point`, the doc comment and the injection
  checks, lines 4–31)

**Interfaces:**
- Consumes:
  - Task 5: `logfile::LogFile::{new, on_disable}`, `logfile::OnDisable`, and
    `impl MakeWriter for LogFile`.
  - Task 1: `app::Context::from_process() -> Result<Context, EnvError>`.
  - Existing:
    - `tagteam_cc::usage::format_iso8601(epoch_s: i64) -> String`;
    - `Env::state_dir(&self) -> PathBuf`, `Env.home: PathBuf`;
    - `Context.no_color_env: bool`; `Cli.{debug, no_color}`;
    - `tests/common/mod.rs`'s `cmd`, `std_cmd`, `seed_home`, `login`, `two_fresh_accounts` and
      `expire_vault`; `MockServer`, `MockReply`.
- Produces (crate-private, `crate::logging`):
  - `pub(crate) const TAGTEAM_LOG: &str = "TAGTEAM_LOG"`
  - `pub(crate) const PANIC_TARGET: &str = "tagteam::panic"`
  - `pub(crate) struct LogConfig { pub debug: bool, pub color: bool, pub state_dir: PathBuf,
    pub home: PathBuf, pub filter: Option<OsString> }`
  - `pub(crate) fn init(cfg: LogConfig, err: &mut dyn std::io::Write)`
  - For tests: `TAGTEAM_TEST_FAIL_AT=panic:<name>` panics the real binary at hook point
    `<name>` (needs `tagteam-engine/test-hooks`).
  - For Task 7: every tracing event at the file's filter is one line of Decision 8's shape, in
    `Env::state_dir()/tagteam.log`. `crates/tagteam/tests/logging.rs` exists with the helpers
    `state_dir`, `quiet`, `log_text` and `assert_line_shape`, and the constants `TAGTEAM_LOG` and
    `API_BASE`.

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam/tests/logging.rs`:

```rust
//! §14.2 through the real binary: where the log is and with what modes, what its filter
//! takes, the `statusline` fast path that never opens it, a log that cannot be written, a
//! panic's line, and a collector thread's log line under `--debug`. Needs `--features
//! test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use common::{cmd, expire_vault, login, seed_home, std_cmd, two_fresh_accounts};
use serde_json::{Value, json};
use tagteam_provider::mock_server::{MockReply, MockServer};
use tagteam_provider::{Env, FileKeychain};

const TAGTEAM_LOG: &str = "TAGTEAM_LOG";
const API_BASE: &str = "TAGTEAM_TEST_API_BASE";
/// §9.4 step 4's WARN line for a capture nothing could attribute.
const CAPTURED: &str = "captured an unverified live credential";

/// The fixture's default state directory, `~/.local/state/tagteam` (§5).
fn state_dir(root: &Path) -> PathBuf {
    Env::for_test(root).state_dir()
}

fn mode(p: &Path) -> u32 {
    fs::metadata(p).unwrap().permissions().mode() & 0o777
}

/// The binary with its log file off: a fixture's setup, so only the command under test logs.
fn quiet(root: &Path) -> assert_cmd::Command {
    let mut c = cmd(root);
    c.env(TAGTEAM_LOG, "off");
    c
}

/// `a@x.co` at position 1 and `b@x.co` at 2, `b` live, and then Claude Code rotated b's
/// credential in place. With every endpoint offline nothing attributes the rotation, so
/// `switch 1` captures it into b's vault with a WARN line (§9.4 step 4), which the file
/// records by default and stderr does not, and the oracle's missing answer is a DEBUG line.
fn rotated_live_login(root: &Path) {
    let env = Env::for_test(root);
    let kc = FileKeychain::new(root.join("keychain"));
    seed_home(&env);
    login(&env, &kc, "a@x.co", "", "rt-a");
    quiet(root).arg("add").assert().success();
    login(&env, &kc, "b@x.co", "", "rt-b");
    quiet(root).arg("add").assert().success();
    login(&env, &kc, "b@x.co", "", "rt-b-rotated");
}

/// The log's text, oldest first across its rotations; empty when there is none.
fn log_text(dir: &Path) -> String {
    ["tagteam.log.2", "tagteam.log.1", "tagteam.log"]
        .iter()
        .map(|n| fs::read_to_string(dir.join(n)).unwrap_or_default())
        .collect()
}

/// Decision 8's line: `<UTC time to the millisecond> <pid> <LEVEL> <target>: …`.
fn assert_line_shape(line: &str, level: &str, target: &str) {
    let parts: Vec<&str> = line.splitn(5, ' ').collect();
    assert_eq!(parts.len(), 5, "{line}");
    let time = parts[0].as_bytes();
    assert!(
        time.len() == 24 && time[10] == b'T' && time[19] == b'.' && time[23] == b'Z',
        "{line}"
    );
    assert!(parts[1].parse::<u32>().is_ok(), "{line}");
    assert_eq!((parts[2], parts[3]), (level, format!("{target}:").as_str()));
}

#[test]
fn a_switch_logs_to_a_private_file_in_the_state_directory() {
    let d = tempfile::tempdir().unwrap();
    rotated_live_login(d.path());
    let dir = state_dir(d.path());
    assert!(!dir.exists(), "the setup logged nothing");
    cmd(d.path())
        .args(["switch", "1", "--json"])
        .assert()
        .success()
        .stderr("");
    assert_eq!(mode(&dir), 0o700);
    assert_eq!(mode(&dir.join("tagteam.log")), 0o600);
    let log = log_text(&dir);
    let line = log
        .lines()
        .find(|l| l.contains(CAPTURED))
        .unwrap_or_else(|| panic!("no capture line in:\n{log}"));
    assert_line_shape(line, "WARN", "tagteam_engine::switch");
    assert!(
        !log.contains("the profile oracle gave no answer"),
        "DEBUG stays out by default:\n{log}"
    );
    let home = d.path().join("home").display().to_string();
    assert!(!log.contains(&home), "paths under HOME are ~/…:\n{log}");
}

#[test]
fn debug_adds_tagteam_s_debug_lines_to_the_file() {
    let d = tempfile::tempdir().unwrap();
    rotated_live_login(d.path());
    cmd(d.path())
        .args(["switch", "1", "--debug"])
        .assert()
        .success();
    let log = log_text(&state_dir(d.path()));
    let line = log
        .lines()
        .find(|l| l.contains("the profile oracle gave no answer"))
        .unwrap_or_else(|| panic!("no DEBUG line in:\n{log}"));
    assert_line_shape(line, "DEBUG", "tagteam_engine::oracle");
}

#[test]
fn xdg_state_home_moves_the_log() {
    let d = tempfile::tempdir().unwrap();
    rotated_live_login(d.path());
    let state = d.path().join("state");
    cmd(d.path())
        .env("XDG_STATE_HOME", &state)
        .args(["switch", "1"])
        .assert()
        .success();
    assert!(log_text(&state.join("tagteam")).contains(CAPTURED));
    assert!(!state_dir(d.path()).exists());
}

#[test]
fn tagteam_log_off_writes_no_file() {
    let d = tempfile::tempdir().unwrap();
    rotated_live_login(d.path());
    quiet(d.path()).args(["switch", "1"]).assert().success();
    assert!(!state_dir(d.path()).exists());
}

#[test]
fn tagteam_log_replaces_the_whole_file_filter() {
    // Only the switch module, at WARN: nothing else the switch logs reaches the file.
    let d = tempfile::tempdir().unwrap();
    rotated_live_login(d.path());
    cmd(d.path())
        .env(TAGTEAM_LOG, "tagteam_engine::switch=warn")
        .args(["switch", "1"])
        .assert()
        .success();
    let log = log_text(&state_dir(d.path()));
    assert!(log.contains(CAPTURED), "{log}");
    for line in log.lines() {
        assert_eq!(
            line.split(' ').nth(3),
            Some("tagteam_engine::switch:"),
            "{line}"
        );
    }
}

#[test]
fn an_invalid_tagteam_log_warns_once_and_keeps_the_default() {
    let d = tempfile::tempdir().unwrap();
    rotated_live_login(d.path());
    let out = cmd(d.path())
        .env(TAGTEAM_LOG, "tagteam=loud")
        .args(["switch", "1", "--json"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.starts_with("warning: TAGTEAM_LOG is not a valid filter")
            && stderr.lines().count() == 1,
        "{stderr}"
    );
    // B.36: the warning is on stderr; stdout is still exactly one object.
    serde_json::from_slice::<Value>(&out.stdout).unwrap();
    assert!(log_text(&state_dir(d.path())).contains(CAPTURED));
}

#[test]
fn statusline_with_the_default_filter_opens_no_log() {
    // §14.2, Review Focus 4: the status bar runs every few seconds and logs at DEBUG at most,
    // so by default it never creates, opens or rotates the log.
    let d = tempfile::tempdir().unwrap();
    rotated_live_login(d.path());
    let out = cmd(d.path())
        .arg("statusline")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(!out.is_empty());
    assert!(!state_dir(d.path()).exists());
}

#[test]
fn a_log_that_cannot_be_written_fails_no_command() {
    // §14.2, §15.2. The state directory is under a regular file, so it can never be made, by
    // root either. Silent by default; `--debug` says so, once.
    for debug in [false, true] {
        let d = tempfile::tempdir().unwrap();
        rotated_live_login(d.path());
        fs::write(d.path().join("blocker"), "").unwrap();
        let mut c = cmd(d.path());
        c.env("XDG_STATE_HOME", d.path().join("blocker/state"))
            .args(["switch", "1", "--json"]);
        if debug {
            c.arg("--debug");
        }
        let out = c.assert().success().get_output().clone();
        assert_eq!(
            serde_json::from_slice::<Value>(&out.stdout).unwrap()["switched"],
            true
        );
        let stderr = String::from_utf8(out.stderr).unwrap();
        let notices = stderr.matches("cannot be written").count();
        assert_eq!(notices, usize::from(debug), "{stderr}");
    }
}

#[test]
fn a_panic_is_logged_once_with_its_location_and_stderr_keeps_the_default_hook() {
    // Decision 9: logged at ERROR before the process unwinds, and left out of stderr, where
    // the default hook prints the panic.
    let d = tempfile::tempdir().unwrap();
    rotated_live_login(d.path());
    let out = cmd(d.path())
        .env("TAGTEAM_TEST_FAIL_AT", "panic:after-journal")
        .args(["switch", "1"])
        .assert()
        .code(101)
        .get_output()
        .clone();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("panicked at") && stderr.contains("injected panic at after-journal"),
        "{stderr}"
    );
    assert!(!stderr.contains("location="), "{stderr}");
    let log = log_text(&state_dir(d.path()));
    let lines: Vec<&str> = log
        .lines()
        .filter(|l| l.contains(" tagteam::panic: "))
        .collect();
    assert_eq!(lines.len(), 1, "{log}");
    assert_line_shape(lines[0], "ERROR", "tagteam::panic");
    assert!(
        lines[0].contains("location=crates/tagteam-engine/src/hooks.rs:"),
        "{}",
        lines[0]
    );
    assert!(
        !lines[0].contains("after-journal"),
        "a formatted message is left to stderr: {}",
        lines[0]
    );
}

#[test]
fn a_collector_thread_s_log_line_under_debug_never_hangs_the_command() {
    // `list` collects each account on a thread of its own (§8.3). a's refresh is refused with
    // invalid_grant, so the gate quarantines a from that thread with a WARN line, which
    // `--debug` prints on stderr. The main thread waits for that thread meanwhile: it must not
    // be holding stderr.
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    let (a, _b) = two_fresh_accounts(root);
    expire_vault(root, &a, -60_000);
    let server = MockServer::start();
    server.on(
        "POST",
        "/v1/oauth/token",
        MockReply::Json {
            status: 400,
            body: json!({"error": "invalid_grant"}),
        },
    );
    let mut child = std_cmd(root)
        .env(API_BASE, server.base_url())
        .args(["list", "--debug"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("`list --debug` hung");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("quarantined"), "{stderr}");
    assert_eq!(server.hits("POST", "/v1/oauth/token"), 1);
}
```

Create `crates/tagteam/src/logging.rs` containing only its test module. Step 3 puts the module
above it. Do not add `mod logging;` to `lib.rs` yet: Step 2 runs the binary tests first, against
today's binary.

```rust
#[cfg(test)]
mod tests {
    use std::os::unix::ffi::OsStringExt;
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

    use tracing_subscriber::fmt::MakeWriter;

    use super::*;

    /// One capture at a time: `tracing` keeps one maximum level, and one cache of which call
    /// sites are enabled, across every subscriber alive in the process, so captures running
    /// side by side can lose each other's lines.
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

    fn one_at_a_time() -> MutexGuard<'static, ()> {
        ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 2026-10-01T18:42:33.581Z, Decision 8's example.
    fn decision_8_clock() -> i64 {
        1_790_880_153_581
    }

    /// Whatever a layer wrote, shared with the test.
    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);

    impl Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for Captured {
        type Writer = Captured;

        fn make_writer(&'a self) -> Captured {
            self.clone()
        }
    }

    impl Captured {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    /// The file layer as `init` builds it, writing to memory under `directive`, with Decision
    /// 8's clock and pid 4242: what the file would hold after `emit`.
    fn file_lines(home: &str, directive: &str, emit: impl FnOnce()) -> String {
        let _serial = one_at_a_time();
        let out = Captured::default();
        let layer = tracing_subscriber::fmt::layer()
            .with_writer(out.clone())
            .with_ansi(false)
            .event_format(Line::with_clock(Path::new(home), 4242, decision_8_clock))
            .with_filter(EnvFilter::new(directive));
        tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), emit);
        out.text()
    }

    /// The stderr layer's filter over a memory writer: what stderr would show after `emit`.
    fn stderr_lines(debug: bool, emit: impl FnOnce()) -> String {
        let _serial = one_at_a_time();
        let out = Captured::default();
        let layer = tracing_subscriber::fmt::layer()
            .with_writer(out.clone())
            .with_ansi(false)
            .without_time()
            .with_target(false)
            .with_filter(stderr_filter(debug));
        tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), emit);
        out.text()
    }

    #[test]
    fn a_line_is_utc_time_pid_level_target_then_the_message_and_its_fields() {
        let out = file_lines("/home/u", FILE_FILTER, || {
            tracing::info!(
                target: "tagteam_engine::switch",
                account = %"0192aa",
                position = 3,
                kind = "switch",
                "switched"
            );
        });
        assert_eq!(
            out,
            "2026-10-01T18:42:33.581Z 4242 INFO tagteam_engine::switch: switched account=0192aa position=3 kind=\"switch\"\n"
        );
    }

    #[test]
    fn the_time_is_utc_to_the_millisecond() {
        assert_eq!(timestamp(1_790_880_153_581), "2026-10-01T18:42:33.581Z");
        assert_eq!(timestamp(1_790_880_153_007), "2026-10-01T18:42:33.007Z");
        assert_eq!(timestamp(0), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn a_path_under_home_is_written_with_a_tilde() {
        // §14.2: `<home>/` anywhere in the line, in the message and the values alike.
        for home in ["/Users/me", "/Users/me/"] {
            let line = Line::with_clock(Path::new(home), 1, decision_8_clock);
            assert_eq!(
                line.finish(
                    0,
                    &Level::WARN,
                    "tagteam_cc::live",
                    "moved /Users/me/.claude.json path=/Users/me/.local/share/tagteam/x"
                ),
                "1970-01-01T00:00:00.000Z 1 WARN tagteam_cc::live: moved ~/.claude.json path=~/.local/share/tagteam/x\n",
                "{home}"
            );
        }
        // The home itself, and a sibling that only starts like it, are not under it.
        let line = Line::with_clock(Path::new("/Users/me"), 1, decision_8_clock);
        assert!(
            line.finish(0, &Level::INFO, "t", "a=/Users/me b=/Users/meg/x")
                .ends_with(" t: a=/Users/me b=/Users/meg/x\n")
        );
        // A home of `/` would write every absolute path `~/…`: nothing is rewritten.
        let root = Line::with_clock(Path::new("/"), 1, decision_8_clock);
        assert!(
            root.finish(0, &Level::INFO, "t", "p=/etc/x")
                .ends_with(" t: p=/etc/x\n")
        );
    }

    #[test]
    fn a_value_holding_a_newline_stays_on_its_line() {
        let out = file_lines("/home/u", FILE_FILTER, || {
            tracing::warn!(target: "tagteam_engine::x", "first\nsecond\r");
        });
        assert_eq!(
            out,
            "2026-10-01T18:42:33.581Z 4242 WARN tagteam_engine::x: first\\nsecond\\r\n"
        );
    }

    #[test]
    fn the_file_takes_info_from_tagteam_and_warn_from_the_rest() {
        // Decision 6, and with `--debug` tagteam's DEBUG too.
        let emit = || {
            tracing::info!(target: "tagteam_engine::store", "kept");
            tracing::debug!(target: "tagteam_engine::store", "debug");
            tracing::info!(target: "tagteam_cc::live", "kept");
            tracing::error!(target: "tagteam::panic", "kept");
            tracing::info!(target: "ureq::unversioned", "dropped");
            tracing::warn!(target: "ureq::unversioned", "kept");
        };
        let out = file_lines("/h", FILE_FILTER, emit);
        assert_eq!(out.matches(": kept").count(), 4, "{out}");
        assert!(
            !out.contains(": debug") && !out.contains(": dropped"),
            "{out}"
        );
        let out = file_lines("/h", DEBUG_FILE_FILTER, emit);
        assert_eq!(out.matches(": kept").count(), 4, "{out}");
        assert_eq!(out.matches(": debug").count(), 1, "{out}");
        assert!(!out.contains(": dropped"), "{out}");
    }

    #[test]
    fn tagteam_log_replaces_the_file_filter_and_off_removes_the_file() {
        let pick = |debug: bool, var: Option<&str>| {
            let mut err = Vec::new();
            let got = file_directive(debug, var.map(OsStr::new), &mut err);
            assert!(err.is_empty(), "{}", String::from_utf8_lossy(&err));
            got
        };
        assert_eq!(pick(false, None).as_deref(), Some(FILE_FILTER));
        assert_eq!(pick(true, None).as_deref(), Some(DEBUG_FILE_FILTER));
        assert_eq!(pick(false, Some("")).as_deref(), Some(FILE_FILTER));
        assert_eq!(
            pick(true, Some("tagteam_engine=trace")).as_deref(),
            Some("tagteam_engine=trace"),
            "the whole filter, --debug or not"
        );
        assert_eq!(pick(false, Some("off")), None);
        assert_eq!(pick(true, Some(" OFF ")), None);
    }

    #[test]
    fn an_invalid_tagteam_log_keeps_the_default_with_one_warning() {
        let cases = [
            (OsString::from("tagteam=loud"), false, FILE_FILTER),
            (
                OsString::from_vec(vec![b't', 0xff]),
                true,
                DEBUG_FILE_FILTER,
            ),
        ];
        for (var, debug, default) in cases {
            let mut err = Vec::new();
            let got = file_directive(debug, Some(var.as_os_str()), &mut err);
            assert_eq!(got.as_deref(), Some(default), "{var:?}");
            let err = String::from_utf8(err).unwrap();
            assert!(
                err.starts_with("warning: TAGTEAM_LOG is not a valid filter (")
                    && err.ends_with("; the log keeps its default\n")
                    && err.lines().count() == 1,
                "{err:?}"
            );
        }
    }

    #[test]
    fn stderr_shows_errors_or_with_debug_diagnostics_and_never_a_panic() {
        let emit = || {
            tracing::error!(target: "tagteam_engine::refresh", "an error");
            tracing::warn!(target: "tagteam_engine::switch", "a warning");
            tracing::debug!(target: "tagteam_engine::oracle", "a diagnostic");
            tracing::trace!(target: "tagteam_engine::oracle", "a trace");
            tracing::error!(target: PANIC_TARGET, "panic: boom");
        };
        let quiet = stderr_lines(false, emit);
        assert!(quiet.contains("an error"), "{quiet}");
        assert!(
            !quiet.contains("a warning") && !quiet.contains("boom"),
            "{quiet}"
        );
        let debug = stderr_lines(true, emit);
        assert!(
            debug.contains("an error")
                && debug.contains("a warning")
                && debug.contains("a diagnostic"),
            "{debug}"
        );
        assert!(
            !debug.contains("a trace") && !debug.contains("boom"),
            "{debug}"
        );
    }
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam --features test-support --test logging`
Expected: 8 of 10 fail. The run takes about 30 s, because the hang test waits out its deadline.
- `a_switch_logs_to_a_private_file_in_the_state_directory`: panics in `mode` on `metadata`:
  `No such file or directory`.
- `debug_adds_tagteam_s_debug_lines_to_the_file`: `no DEBUG line in:`.
- `xdg_state_home_moves_the_log` and `tagteam_log_replaces_the_whole_file_filter`: the log is
  empty, so `contains(CAPTURED)` fails.
- `an_invalid_tagteam_log_warns_once_and_keeps_the_default`: stderr is empty.
- `a_log_that_cannot_be_written_fails_no_command`: the `--debug` round counts 0 notices, not 1.
- `a_panic_is_logged_once_with_its_location_and_stderr_keeps_the_default_hook`: exit 0, not
  101.
- `a_collector_thread_s_log_line_under_debug_never_hangs_the_command`: `` `list --debug` hung ``
  after 30 s.

`tagteam_log_off_writes_no_file` and `statusline_with_the_default_filter_opens_no_log` pass
already. Today's binary writes no file at all; they are guards for Step 3.

Then add `mod logging;` to `crates/tagteam/src/lib.rs` after `mod logfile;`, and run
`cargo test -p tagteam --lib logging`.
Expected: the build fails with unresolved names, among them `Line`, `file_directive`,
`stderr_filter`, `timestamp`, `FILE_FILTER`, `DEBUG_FILE_FILTER`, `PANIC_TARGET`, `EnvFilter`,
`Level`, `Path` and `OsStr`.

- [ ] **Step 3: Implement**

In `crates/tagteam/src/logging.rs`, insert above `#[cfg(test)]`:

```rust
//! §14.2's subscriber, installed once at the process boundary (Decision 5): the stderr layer
//! as M2 had it, and the log file with a filter and a line format of its own.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use tagteam_cc::usage::format_iso8601;
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::filter::{EnvFilter, LevelFilter, Targets};
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;

use crate::logfile::LogFile;

/// Replaces the file's filter (§14.2).
pub(crate) const TAGTEAM_LOG: &str = "TAGTEAM_LOG";
/// A panic's log line (Decision 9), which stderr leaves out.
pub(crate) const PANIC_TARGET: &str = "tagteam::panic";
/// Decision 6: the tagteam crates at INFO, everything else at WARN; DEBUG with `--debug`.
const FILE_FILTER: &str =
    "warn,tagteam=info,tagteam_core=info,tagteam_provider=info,tagteam_cc=info,tagteam_engine=info";
const DEBUG_FILE_FILTER: &str = "warn,tagteam=debug,tagteam_core=debug,tagteam_provider=debug,tagteam_cc=debug,tagteam_engine=debug";

/// What logging needs from the process boundary.
pub(crate) struct LogConfig {
    /// `--debug`.
    pub debug: bool,
    /// Neither `--no-color` nor `NO_COLOR`: stderr's lines may be coloured, on a terminal.
    pub color: bool,
    /// `Env::state_dir`, where `tagteam.log` lives (§5).
    pub state_dir: PathBuf,
    /// `Env::home`, whose paths the file writes as `~/…`.
    pub home: PathBuf,
    /// `TAGTEAM_LOG`, as the environment holds it.
    pub filter: Option<OsString>,
}

/// Installs the subscriber, and then the panic hook, once per process: with
/// `set_global_default`, never `try_init`, so the `log` crate's records (ureq's, rustls's) are
/// not bridged into it (Decision 5). An invalid `TAGTEAM_LOG` is one warning on `err`.
pub(crate) fn init(cfg: LogConfig, err: &mut dyn Write) {
    let file = file_directive(cfg.debug, cfg.filter.as_deref(), err).map(|directive| {
        tracing_subscriber::fmt::layer()
            .with_writer(log_file(&cfg))
            .with_ansi(false)
            .log_internal_errors(false)
            .event_format(Line::new(&cfg.home))
            .with_filter(EnvFilter::new(directive))
    });
    let stderr = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(cfg.color && std::io::stderr().is_terminal())
        .with_target(false)
        .with_filter(stderr_filter(cfg.debug));
    let subscriber = tracing_subscriber::registry().with(stderr).with(file);
    if tracing::subscriber::set_global_default(subscriber).is_ok() {
        install_panic_hook();
    }
}

/// M2's stderr: ERROR, or DEBUG with `--debug`. Never a panic's line: the default hook prints
/// the panic there already (Decision 9).
fn stderr_filter(debug: bool) -> Targets {
    let level = if debug {
        LevelFilter::DEBUG
    } else {
        LevelFilter::ERROR
    };
    Targets::new()
        .with_default(level)
        .with_target(PANIC_TARGET, LevelFilter::OFF)
}

/// The file's filter (Decision 6), or `None` for no file at all. `TAGTEAM_LOG` replaces the
/// default whole, `--debug` or not; `off` turns the file off. A value that does not parse
/// keeps the default and says so once on `err`. An empty value is as good as unset.
fn file_directive(debug: bool, var: Option<&OsStr>, err: &mut dyn Write) -> Option<String> {
    let default = if debug {
        DEBUG_FILE_FILTER
    } else {
        FILE_FILTER
    };
    let Some(var) = var.filter(|v| !v.is_empty()) else {
        return Some(default.to_owned());
    };
    let parsed = match var.to_str() {
        Some(v) if v.trim().eq_ignore_ascii_case("off") => return None,
        Some(v) => EnvFilter::try_new(v)
            .map(|_| v.to_owned())
            .map_err(|e| e.to_string()),
        None => Err("it is not UTF-8".to_owned()),
    };
    Some(parsed.unwrap_or_else(|why| {
        let _ = writeln!(
            err,
            "warning: {TAGTEAM_LOG} is not a valid filter ({why}); the log keeps its default"
        );
        default.to_owned()
    }))
}

/// The file layer's writer. Under `--debug`, a log that cannot be written says so once on
/// stderr; otherwise it falls silent (§14.2).
fn log_file(cfg: &LogConfig) -> LogFile {
    let file = LogFile::new(cfg.state_dir.clone());
    if !cfg.debug {
        return file;
    }
    file.on_disable(Box::new(|path, e| {
        let _ = writeln!(
            std::io::stderr(),
            "warning: the log file {} cannot be written ({e}); nothing more is logged to it",
            path.display()
        );
    }))
}

/// Decision 8's line: `2026-10-01T18:42:33.581Z 4242 INFO tagteam_engine::switch: message
/// key=value …`, with every `<home>/` written `~/`, and on one line whatever a value holds.
struct Line {
    /// `<home>/`; `None` for a home of `/` (every path would be under it) or one that is not
    /// UTF-8.
    home: Option<String>,
    pid: u32,
    now_ms: fn() -> i64,
}

impl Line {
    fn new(home: &Path) -> Self {
        Self::with_clock(home, std::process::id(), now_ms)
    }

    fn with_clock(home: &Path, pid: u32, now_ms: fn() -> i64) -> Self {
        let home = home
            .to_str()
            .map(|h| h.trim_end_matches('/'))
            .filter(|h| !h.is_empty())
            .map(|h| format!("{h}/"));
        Self { home, pid, now_ms }
    }

    /// The finished line, newline included. `fields` is the message, then `key=value` pairs.
    fn finish(&self, now_ms: i64, level: &Level, target: &str, fields: &str) -> String {
        let mut line = format!(
            "{} {} {level} {target}: {fields}",
            timestamp(now_ms),
            self.pid
        );
        if let Some(home) = &self.home {
            line = line.replace(home.as_str(), "~/");
        }
        let mut line = line.replace('\r', "\\r").replace('\n', "\\n");
        line.push('\n');
        line
    }
}

impl<S, N> FormatEvent<S, N> for Line
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let mut fields = String::new();
        ctx.field_format()
            .format_fields(Writer::new(&mut fields), event)?;
        let meta = event.metadata();
        writer.write_str(&self.finish((self.now_ms)(), meta.level(), meta.target(), &fields))
    }
}

/// UTC to the millisecond: `format_iso8601`'s seconds, then `.mmm` before the `Z`.
fn timestamp(now_ms: i64) -> String {
    let seconds = format_iso8601(now_ms.div_euclid(1000));
    format!(
        "{}.{:03}Z",
        seconds.trim_end_matches('Z'),
        now_ms.rem_euclid(1000)
    )
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// Decision 9: a panic is logged at ERROR, with its location, before the process unwinds
/// (§14.2); the default hook then prints it on stderr as before. Its message is logged only
/// when it is a literal: a formatted one can carry runtime values (serde's `invalid type:
/// string "…"` quotes the string it met), and stderr shows it anyway.
fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map_or_else(String::new, |l| format!("{}:{}", l.file(), l.line()));
        let message = info
            .payload()
            .downcast_ref::<&'static str>()
            .copied()
            .unwrap_or("(a formatted message, not logged)");
        tracing::error!(target: PANIC_TARGET, location = %location, "panic: {message}");
        default(info);
    }));
}

```

In `crates/tagteam/src/lib.rs`, the module list now reads (Task 5's `#[allow(dead_code)]` and its
comment go):

```rust
mod history;
mod logfile;
mod logging;
pub mod prompt;
```

Still in `lib.rs`, `main_with_args`: immediately after the statement that binds `ctx` from
`app::Context::from_process()` (Task 1's `match`; its `Err` arm has already returned), insert:

```rust
    // §14.2, Decision 5: logging is a process concern, set up here once and never by
    // `app::run`, which in-process tests drive. After the HOME check, since the log's path
    // derives from HOME, and before the command runs.
    logging::init(
        logging::LogConfig {
            debug: cli.debug,
            color: !cli.no_color && !ctx.no_color_env,
            state_dir: ctx.env.state_dir(),
            home: ctx.env.home.clone(),
            filter: std::env::var_os(logging::TAGTEAM_LOG),
        },
        &mut std::io::stderr(),
    );
```

and replace:

```rust
    let (mut out, mut err) = (std::io::stdout().lock(), std::io::stderr().lock());
```

with:

```rust
    // stderr's lock is never held across the command: a collector thread's log line (§8.3)
    // takes it while this thread waits for that thread.
    let (mut out, mut err) = (std::io::stdout().lock(), std::io::stderr());
```

After M3a, the tail of `main_with_args` reads, in order:
1. the `ctx` binding;
2. `logging::init(…)`;
3. `if let Err(e) = signals::install(&ctx.env.cancel) { … }`;
4. `let mut prompter = prompt::TtyPrompter::new(ctx.env.cancel.clone());`;
5. the `(out, err)` line above;
6. `app::run(cli, ctx, &mut app::Io { … })`.

In `crates/tagteam/src/app.rs`, delete the whole of `init_logging`, from its doc comment
(`/// Logs are diagnostics, on stderr: ERROR by default, …`) to its closing brace, and the blank
line after it. Then replace the top of `run` (M3a: `run_command`):

```rust
pub fn run(cli: Cli, ctx: Context, io: &mut Io<'_>) -> i32 {
    let color = !cli.no_color && !ctx.no_color_env;
    init_logging(cli.debug, color);
    let json = cli.json;
```

with:

```rust
pub fn run(cli: Cli, ctx: Context, io: &mut Io<'_>) -> i32 {
    let json = cli.json;
```

`app.rs` keeps its `IsTerminal` import, which `Context::from_process` uses.

In `crates/tagteam-engine/src/hooks.rs`, replace the doc comment's lines:

```rust
/// injects an error there (`"<name>"`) or a panic (`"panic:<name>"`) for the rollback tests;
/// `TAGTEAM_TEST_FAIL_AT=<name>` injects the error for a process whose engine a test cannot
/// reach (the real binary).
```

with:

```rust
/// injects an error there (`"<name>"`) or a panic (`"panic:<name>"`) for the rollback tests;
/// `TAGTEAM_TEST_FAIL_AT` does the same (`<name>` or `panic:<name>`) for a process whose engine
/// a test cannot reach (the real binary).
```

and replace, in `point`:

```rust
    let injected = *engine.fail_at.lock().unwrap();
    if injected == Some(name) || std::env::var("TAGTEAM_TEST_FAIL_AT").as_deref() == Ok(name) {
        return Err(EngineError::InvalidInput(format!(
            "injected failure at {name}"
        )));
    }
    if injected.and_then(|n| n.strip_prefix("panic:")) == Some(name) {
        panic!("injected panic at {name}");
    }
```

with:

```rust
    let injected = *engine.fail_at.lock().unwrap();
    let from_env = std::env::var("TAGTEAM_TEST_FAIL_AT").ok();
    if injected == Some(name) || from_env.as_deref() == Some(name) {
        return Err(EngineError::InvalidInput(format!(
            "injected failure at {name}"
        )));
    }
    let panic_at = |n: &str| n.strip_prefix("panic:") == Some(name);
    if injected.is_some_and(panic_at) || from_env.as_deref().is_some_and(panic_at) {
        panic!("injected panic at {name}");
    }
```

- [ ] **Step 4: Run the tests and see them pass**

Run:
- `cargo test -p tagteam --lib logging`: expected PASS, 9 tests (8 in `logging`, plus `logfile`'s
  concurrency test, whose name contains "logging").
- `cargo test -p tagteam --features test-support --test logging`: expected PASS, 10 tests.
- `cargo test -p tagteam --features test-support --test cli`: expected PASS. This includes
  `a_routine_switch_is_quiet_and_debug_shows_the_diagnostics` (stderr unchanged) and
  `a_fresh_machine_lists_nothing_and_creates_nothing` (no event fires there, so no log
  directory).
- `cargo test --workspace --features tagteam/test-support`: expected PASS.

- [ ] **Step 5: Format, lint, commit**

`cargo fmt --all && cargo fmt --all --check`,
`cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings`,
`cargo clippy --workspace --all-targets -- -D warnings`, then
`cargo test -p tagteam --features test-support` and
`cargo test -p tagteam-engine --features test-hooks`. Then:

```
git add crates/tagteam/src/logging.rs crates/tagteam/src/lib.rs crates/tagteam/src/app.rs \
  crates/tagteam-engine/src/hooks.rs crates/tagteam/tests/logging.rs
git commit -m "Log to a private file, set up once at the process boundary"
```

---

### Task 7: What the log records, and the redaction pin

§14.2 has three rules for this task:
- **INFO's content.** "What INFO records: the state changes and decisions a user may later need
  to reconstruct … switches and their rollbacks, recovery, quarantines and their clearing,
  refresh outcomes, captures, replacements, rescues and displacements … and settings writes.
  Routine reads, each usage fetch, and everything `statusline` does log at DEBUG at most."
- **The spelling.** "An account is named as `account=<id> position=<n>`, the only spelling. A
  path under the home directory is written `~/…`."
- **What is never logged.** "Never logged, at any level: an email, label or organization name;
  a token, key or credential, or any part of one; a passphrase; an export's contents; a
  request's `Authorization` header or body. A fingerprint may appear as its first 12 hex
  digits."

§14 adds: "Contained errors are logged, never discarded … A timeout and a failure to spawn a
process are different causes, and are reported as such."

§15.3 pins it: "Every command, run at TRACE against the fixture home, leaves no fixture email,
organization name, token, key or passphrase in the log." So does B.69: "No log line, at any
level, holds an email, organization name, token, key, credential or passphrase."

This task does four things:
- It puts INFO lines at Decision 10's central sites: the store's event insert, `Vault::store`,
  the rescue writer, `displace`, and the recovery decision. Two more sites belong to other
  tasks, and this task touches neither:
  - Task 4's settings write, in `Engine::config_set` and `config_unset`;
  - Task 8's purge of a displaced entry, in `Engine::purge_displaced`.
- It gives every log line that names an account the one spelling.
- It logs the two contained errors M1's review left (L323, L349) with their causes.
- It logs a switch's rollback and every refresh outcome, and keeps everything `statusline` does
  at DEBUG (Part D).

It pins all of it with engine-level line tests and with the binary redaction test.

**Re-sync (M4a Tasks 1–4, 8–10, 14 and 15, M3a Tasks 2, 9 and 11, Task 8):**
- **`commit_switch`.** M4a gives it an `epoch: i64` parameter. Part A's two added lines go
  after its `tx.commit()?;` all the same.
- **`views.rs`.** M4a adds a second `id = %row.id` there, in `in_session`'s `tracing::warn!`.
  Part B renames it too; the Step 4 grep finds it.
- **`switch.rs`.** M3a Task 11 and M4a Task 3 edit it. Part B's edit is anchored by its message,
  `"captured an unverified live credential into the vault; …"`, not by line.
- **The recovery loop.** M3a Task 2 changes `engine.rs`'s loop to return an interruption. The
  loop's own `warn!` keeps its fields, and Part A's line is inside `recover_one`, so neither
  moves.
- **`displace`.** Task 8 rewrites it twice. Both rewrites keep this task's INFO line; the second
  logs it once the displaced lock is released.
- **`Rollback` (Part D).** M3a Task 11 changes what the rollback's undos do: they take CC's
  storage-write lock. M4a Task 3 changes the transaction around them. Neither edits
  `struct Rollback`, its literal in `transact`, or `fail`. Part D replaces `fail` whole, anchored
  on `fn fail(mut self, cause: EngineError) -> EngineError {` and the doc line above it; its two
  smaller edits anchor on fields both keep.
- **`refresh_stored` (Part D).** M3a Task 9 edits a comment in its body, M4a Task 9 replaces
  `owner_of`, and M4a Task 10 replaces the whole function, keeping its doc comment and signature
  verbatim. Part D's anchor is that doc comment and signature, so the wrapper lands on their
  version, and their body becomes `run_gate`'s, unchanged. `log_gate`'s arms already cover M4a's
  `Owned(Session)`, `Conflict` and `Transient { kind: "profile-unreadable" }`. (M3a Task 5's
  cancellation points are in `collect.rs`, before the gate is called, not inside it.)
- **`refresh_active` (Part D).** M3a Task 2 (the self-heal's `publish` call) and M4a Task 4
  (step 2's `Replaced` check) edit its loop, which becomes `run_active_refresh`'s body. Neither
  touches the doc comment or the lines up to `let hint = self.corroborate(p, &row, &live)?;`,
  Part D's anchor. M4a Task 4 adds `ActiveOutcome::Replaced`, so `log_active`'s `match` needs an
  arm for it, before the `NotNeeded { reconciled: false }` arm:
  `Ok(ActiveOutcome::Replaced) => tracing::debug!(provider = %provider, account, outcome = "replaced", "{OUTCOME}"),`.
  It is DEBUG because nothing is sent or written (§7.5 step 2).
- **`account_view_with` (Part D).** M4a Task 14 replaces `account_view` and `account_view_with`,
  giving the latter `in_session: bool`, and Task 15 replaces `Engine::statusline` with two calls
  to it. Re-apply Part D onto them:
  - `status_bar: bool` goes after `in_session`, with Part D's doc sentence;
  - `account_view` passes `false` after its computed `in_session`;
  - the failed read's `tracing::warn!` (Part B's `account =`) becomes Part D's
    `if status_bar { debug } else { warn }`;
  - both of Task 15's calls, `(row, false, false, false)` and `(row, true, false, false)`, gain
    `, true`.

  M4a's `in_session` WARN is not on the status bar's path: `statusline` passes
  `in_session = false` and never calls it (M4a's Decision 17).

**Readings of the spec this task commits to:**
- **An events row is logged once it is durably written.**
  - `insert_event` logs after its INSERT, and `commit_switch` after its transaction commits and
    the connection is released. A commit that fails logs nothing.
  - The line is `event recorded provider=… kind="…" from_account=… to_account=… trigger="…"
    source="…"`, with an absent field left out.
  - `detail` is never logged. It is free-form JSON (today only a quarantine's reason), and
    nothing bounds what a later kind puts in it.
- **A line naming two accounts uses `from_account` and `to_account`.** §14.2's one spelling,
  `account=<id> position=<n>`, cannot name two accounts in one line. The event line and the
  recovery line name the outgoing account and the target, after the `from_id` and `to_id`
  columns. Both end in `account=<id>`, so `grep account=<id>` still finds every line about an
  account, and no line is ambiguous. A line that names one account keeps `account=`. Two lines
  per event would lose the event's unity.
- **No position is looked up to complete a line.**
  - The store's insert, `Vault::store`, the rescue writer and `recover_one` hold IDs only, so
    their lines carry `account=…` or `from_account`/`to_account` without `position`.
  - "Where the position is at hand" means a row is already in scope. Every existing line that
    has one keeps `position=`.
- **`Vault::store` logs a verified write only.** The line is
  `stored a credential in the vault account=… fp=<12 hex> new_generation=<bool>`.
  - `new_generation` is false only when the same generation was rewritten.
  - `fp` is left out when the provider has no fingerprint for the bytes.
  - A failed store logs nothing here. Its caller handles the error, and the refresh gate
    already logs it at ERROR.
- **The rescue writer** logs after the file is in place:
  `kept a refreshed token in rescue/ until the vault takes it account=… fp=<successor's 12 hex>`.
- **`displace`** logs its entry's own ID (`displaced=<epoch>-<fp12>-<rand6>`), its reason and its
  provider. It never logs the identity the bytes were attributed to.
- **The recovery decision** is logged once per journal row each time a command recovers:
  `recovering an interrupted switch provider=… from_account=… to_account=…
  direction="forward"|"backward"|"undecidable"`.
  - `undecidable` therefore recurs on every account-changing command until `switch --force`
    settles it. That repetition is the decision a user needs to see.
  - A forward finish also produces a `kind="switch-recovered"` event line.
- **The one spelling.**
  - The three `id = %…` fields become `account = %…`.
  - The three lines that named an account by `position` alone gain `account = %…`: two in
    `active.rs` and one in `switch.rs`.
  - Fields that are not accounts keep their names: `provider`, `kind`, `reason`, `displaced`,
    `fp`, `path`.
- **A temp file that cannot be removed** (L323) is logged at WARN with its path (as `~/…`, by
  Task 6's formatter) and the OS error. One that is already gone is not: it was never left
  behind.
- **`disambiguation_failed`** (L349) now reports three causes:
  - `rc <n>: the -g disambiguation call failed`;
  - `the -g disambiguation call did not finish in time`;
  - `the -g disambiguation call could not run: <OS error>`.

  A spawn failure's text is `std::io::Error`'s, never `security`'s output. `-g`'s stderr still
  never appears, since it is the channel the secret is printed on.
- **What the redaction pin checks:**
  - Whole identities (emails and the organization name). A label is the email for Claude Code.
  - Every 13-character window of every secret: refresh and access tokens before and after a
    refresh, the API key and the setup token.
  - That those secrets did go over the wire: bearers, the refresh body and the token reply.
  - That every line's target is a tagteam crate, so no `log` record is bridged in
    (Decision 5).
  - It passes `TAGTEAM_LOG=trace` explicitly, because `std_cmd` clears the environment.
- **A switch's rollback is logged where it happens, in `Rollback::fail`** (Part D).
  - Every write put back: WARN `rolled back a switch provider=… from_account=… to_account=…
    kind="<the cause's kind>"`.
  - Something left: ERROR `a switch was not fully rolled back; its journal row stays for
    recovery: <what>`, with the same fields. `<what>` is what `Drop`'s ERROR already prints:
    undo descriptions, Keychain services and paths, and provider and store errors, never bytes.
  - The cause is named by its `kind()` alone, since its text can carry a label ("Deliberately
    absent").
  - The line names both accounts, as the switch's event line would have: nothing else in the
    log says which switch was undone. `Rollback` keeps their IDs for it.
  - `Drop`'s two lines, a panic's rollback, stay as they are. The panic's own ERROR line goes
    with them.
- **Every refresh outcome is one line, from a wrapper** (Part D).
  - `refresh_stored` and `refresh_active` keep their signatures and become thin wrappers around
    their bodies, renamed `run_gate` and `run_active_refresh`. Every `return` and `?` is covered
    without editing it, and so is every path M3a and M4a add to the bodies. The line is logged
    after every lock the body took is released.
  - The gate's line is `refresh gate outcome account=… outcome="…"`, plus `reason` for `dead`,
    `kind` and `rescued` for `transient`, `by` for `owned`, and the error's `kind` for `error`.
    No row is in scope, so no `position`.
  - §7.5's line is `active-token refresh outcome provider=… account=… outcome="…"`, plus
    `reconciled`, `reason` and `kind` as above. `account` is left out when the refresh ended
    before the live login's account was known.
  - The level: INFO once a request was sent or state changed, DEBUG otherwise.
    - Gate: `refreshed`, `dead`, `systemic`, `transient`, `unpersisted` and `error` are INFO;
      `already-fresh`, `busy`, `owned` and `conflict` are DEBUG.
    - §7.5: `not-needed` is INFO only when reconciliation changed something; every other
      outcome is INFO.
  - An error is INFO, named by its `kind()`. The wrapper cannot tell where it was raised, and
    one raised after the request is a state change: a quarantine that could not be recorded, or
    a successor kept in `rescue/`.
  - The level follows the outcome, not the path that returned it.
    - A `transient` the gate returns before any request (`vault-absent`, `not-refreshable`,
      `vault-unreadable`, `rescue-unreadable`) and a `dead` for a standing quarantine are INFO,
      though nothing was sent. Telling them apart would mean editing every `return`. The
      collector skips quarantined and unrefreshable accounts before it calls the gate, so it
      rarely reaches either.
    - An `already-fresh` returned after the request, because the vault moved while it was in
      flight, is DEBUG. That path's own ERROR and rescue lines record it.
  - Never logged: a systemic refusal's own words, which may quote the account, and a transport
    error's text. A `transient`'s `kind` is its token (`ambiguous`, `http-503`).
- **`statusline` logs at DEBUG at most** (Part D).
  - The one INFO-or-above line it can reach is `account_view_with`'s WARN for a usage row it
    cannot read. Part D's audit lists everything else on its path.
  - `account_view_with` takes `status_bar: bool`, true only from `Engine::statusline`, and logs
    that failure at DEBUG there.
  - An account command's result (`account_view`) keeps the WARN: §14 logs a contained error
    with its cause.
  - A flag rather than a view-kind enum: M4a's `in_session` set the precedent of explicit flags
    on this function, and an enum would rewrite M4a's signature.

**Files:**
- Modify: `crates/tagteam-engine/src/store/mod.rs`:
  - new `log_event`, after `event_from_row`, which ends near line 281;
  - `commit_switch` (894–907);
  - `insert_event` (909–912).
- Modify: `crates/tagteam-engine/src/vault.rs` (`Vault::store`, 118–141)
- Modify: `crates/tagteam-engine/src/rescue.rs` (`Engine::write_rescue`, 76–108)
- Modify: `crates/tagteam-engine/src/displace.rs` (`displace`, 11–37)
- Modify: `crates/tagteam-engine/src/recover.rs`:
  - `Direction` (19–23), gaining `name`;
  - `Engine::recover_one` (118–149).
- Modify (Part B):
  - `crates/tagteam/src/app.rs` (`or_inactive`, 220–230);
  - `crates/tagteam-engine/src/views.rs` (`account_view_with`, 446–453);
  - `crates/tagteam-engine/src/lifecycle.rs` (`commit_login`'s cleanup, 361–365);
  - `crates/tagteam-engine/src/active.rs` (`publish`, 508–513 and 568–573);
  - `crates/tagteam-engine/src/switch.rs` (`settle_outgoing`, 1364–1368).
- Modify (Part C):
  - `crates/tagteam-provider/Cargo.toml` (`tracing`; dev `tracing-subscriber`);
  - `Cargo.lock`;
  - `crates/tagteam-provider/src/atomic.rs` (`Temp::drop`, 76–82, and its tests);
  - `crates/tagteam-provider/src/security.rs` (`disambiguation_failed`, 279–289, and its
    tests).
- Modify (Part D):
  - `crates/tagteam-engine/src/switch.rs` (`struct Rollback`, 364–375; `Rollback::fail`,
    395–412; the guard's literal in `transact`, 1229–1237);
  - `crates/tagteam-engine/src/refresh.rs` (new `log_gate` after `log_lost`, 261–269;
    `refresh_stored`'s doc comment and signature, 300–310);
  - `crates/tagteam-engine/src/active.rs` (the `tagteam_core` import; new `log_active` after
    `Reconciled`, 86–93; `refresh_active`'s head, 96–108);
  - `crates/tagteam-engine/src/views.rs` (`account_view` and `account_view_with`, 430–453;
    `Engine::statusline`'s call, 627–630);
  - `crates/tagteam/Cargo.toml` (dev `rusqlite`) and `Cargo.lock`.
- Create: `crates/tagteam-engine/tests/log_lines.rs`
- Modify: `crates/tagteam/tests/logging.rs` (Task 6's file: imports, doc, and the redaction pin;
  `statusline_with_the_default_filter_opens_no_log` in Part D)

**Interfaces:**
- Consumes:
  - Task 6's `logging` (the file layer and Decision 8's line).
  - `crates/tagteam/tests/logging.rs`'s `state_dir`, `log_text`, `TAGTEAM_LOG` and `API_BASE`.
  - `tests/common/mod.rs` (engine): `Fx` with its fields and its methods `add`, `login`,
    `switch_to`, `rotate_live`, `vault_bytes`, `script_refresh` and `env`; and the helpers
    `capture_logs`, `crashed_switch`, `write_target_credential`, `due` and `vault_fp`.
  - `tests/common/mod.rs` (CLI): `cmd`, `seed_home`, `expire_vault`.
  - Part D: `Fx::{live_credential, set_live_credential, provider, kc}`, `Fx::endpoints()`,
    `Engine::{fail_at, on_point}` (`test-hooks`), `ScriptedHttp::{push, push_json}`,
    `AccountLock::acquire`, `Engine::{refresh_active, statusline, account_view}`, and
    `crates/tagteam/tests/logging.rs`'s `rotated_live_login`.
  - `tagteam_cc::{ItemKind, keychain_account, keychain_service}`.
  - `tagteam_provider::splice::replace_top_level`.
  - `Fingerprint::short12(&self) -> &str`.
  - Tasks 3 and 4: `tagteam config path|set|get|list|unset`.
- Produces:
  - The INFO line formats above. Task 8 keeps `displace`'s line and spells its purge line
    `displaced = %id`, logged after the lock (Decision 10). Task 4 adds the settings line.
  - `fn log_event(e: &EventRow)` (store-private).
  - `Direction::name(&self) -> &'static str` (recover-private).
  - Part D: `fn log_gate(id: &AccountId, result: &Result<GateOutcome, EngineError>)` and
    `Engine::run_gate` (refresh-private); `fn log_active(provider: &ProviderId, account:
    Option<&AccountId>, result: &Result<ActiveOutcome, EngineError>)` and
    `Engine::run_active_refresh` (active-private); `Rollback`'s `from` and `to`; and
    `account_view_with`'s fourth parameter, `status_bar: bool`.
  - Part D: `tagteam` dev-depends on `rusqlite`.
  - `tagteam-provider` depends on `tracing`.
  - `every_command_at_trace_leaves_no_identity_or_secret_in_the_log`, which Tasks 9 and 10
    extend: Task 9 appends `displaced`, and Task 10 `completions`, in their last steps.

#### Part A: INFO at the central sites, and the redaction pin

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam-engine/tests/log_lines.rs`:

```rust
//! §14.2's INFO lines from the engine's central sites (Decision 10): every `events` row, every
//! vault generation written, a rescue, a displacement and a recovery decision. Each names its
//! accounts by ID, never by email (B.35).
mod common;

use std::sync::{Mutex, MutexGuard, PoisonError};

use common::{Fx, capture_logs, crashed_switch, due, vault_fp, write_target_credential};
use tagteam_engine::vault::SERVICE;

/// One test at a time: each captures through a subscriber of its own, and `tracing` caches
/// whether a call site is enabled across every subscriber alive in the process, so a test
/// running beside another can find a call site cached as disabled and capture nothing.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn one_at_a_time() -> MutexGuard<'static, ()> {
    ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The lines among `logs` at `level` whose message contains `message`.
fn at<'a>(logs: &'a [String], level: &str, message: &str) -> Vec<&'a str> {
    logs.iter()
        .map(|l| l.trim_start())
        .filter(|l| l.starts_with(level) && l.contains(message))
        .collect()
}

/// The one INFO line whose message contains `message`.
fn one<'a>(logs: &'a [String], message: &str) -> &'a str {
    let found = at(logs, "INFO", message);
    assert_eq!(found.len(), 1, "one {message:?} line in {logs:#?}");
    found[0]
}

/// The value of `key` in `line`, as written: a string field keeps its quotes.
fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.split(' ')
        .find_map(|w| w.strip_prefix(key)?.strip_prefix('='))
}

fn no_email(logs: &[String]) {
    assert!(
        logs.iter().all(|l| !l.contains("@x.co")),
        "an email was logged: {logs:#?}"
    );
}

#[test]
fn an_add_logs_the_generation_it_stored_and_its_event() {
    let _serial = one_at_a_time();
    let fx = Fx::new();
    let (a, logs) = capture_logs(|| fx.add("a@x.co", "rt-a"));
    let stored = one(&logs, "stored a credential in the vault");
    let fp = vault_fp(&fx, &a); // `sha256:` and 64 hex digits; the line keeps the first 12
    assert_eq!(
        [
            field(stored, "account"),
            field(stored, "fp"),
            field(stored, "new_generation"),
        ],
        [Some(a.as_str()), Some(&fp[7..19]), Some("true")],
        "{stored}"
    );
    let event = one(&logs, "event recorded");
    assert_eq!(
        [
            field(event, "kind"),
            field(event, "to_account"),
            field(event, "from_account"),
            field(event, "source"),
        ],
        [Some("\"add\""), Some(a.as_str()), None, Some("\"cli\"")],
        "{event}"
    );
    no_email(&logs);
}

#[test]
fn a_switch_logs_its_event_naming_both_accounts() {
    let _serial = one_at_a_time();
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let (out, logs) = capture_logs(|| fx.switch_to(&a, false).unwrap());
    assert!(out.switched, "{}", out.message);
    let event = one(&logs, "event recorded");
    assert_eq!(
        [
            field(event, "kind"),
            field(event, "from_account"),
            field(event, "to_account"),
            field(event, "trigger"),
            field(event, "source"),
        ],
        [
            Some("\"switch\""),
            Some(b.as_str()),
            Some(a.as_str()),
            Some("\"manual\""),
            Some("\"cli\""),
        ],
        "{event}"
    );
    no_email(&logs);
}

#[test]
fn a_rescue_is_logged_by_account_and_fingerprint() {
    // The vault refuses the refreshed successor, so `rescue/` keeps it (§6.3).
    let _serial = one_at_a_time();
    let fx = Fx::new();
    let a = due(&fx);
    let snapshot = fx.vault_bytes(&a).unwrap();
    fx.script_refresh(Some("rt-a2"));
    fx.kc.set_fail_write(SERVICE, true);
    let (_, logs) = capture_logs(|| {
        fx.engine
            .refresh_stored(fx.cc.as_ref(), &a, &snapshot)
            .unwrap()
    });
    fx.kc.set_fail_write(SERVICE, false);
    let line = one(&logs, "kept a refreshed token in rescue/");
    assert_eq!(field(line, "account"), Some(a.as_str()), "{line}");
    let fp = field(line, "fp").unwrap_or_default();
    assert!(
        fp.len() == 12 && fp.bytes().all(|b| b.is_ascii_hexdigit()),
        "{line}"
    );
    assert!(logs.iter().all(|l| !l.contains("rt-a2")), "{logs:#?}");
    no_email(&logs);
}

#[test]
fn a_displacement_is_logged_without_the_login_it_displaced() {
    // A forced switch over an unmanaged login saves that login's credential (§6.3, §9.2).
    let _serial = one_at_a_time();
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.login("stranger@x.co", "rt-s");
    let (out, logs) = capture_logs(|| fx.switch_to(&a, true).unwrap());
    assert!(out.switched, "{}", out.message);
    let line = one(&logs, "saved a credential to displaced/");
    let id = field(line, "displaced").unwrap_or_default();
    assert!(
        fx.env
            .data_dir()
            .join("displaced")
            .join(format!("{id}.json"))
            .exists(),
        "{line}"
    );
    assert_eq!(
        field(line, "reason"),
        Some("\"forced-activation\""),
        "{line}"
    );
    assert!(
        logs.iter()
            .all(|l| !l.contains("stranger") && !l.contains("rt-s")),
        "{logs:#?}"
    );
}

#[test]
fn a_recovery_logs_which_way_it_went() {
    // §9.6: the live credential decides. Landed, the switch finishes forward and commits a
    // `switch-recovered` row; not landed, it finishes backward and records none.
    let _serial = one_at_a_time();
    for (landed, direction) in [(true, "\"forward\""), (false, "\"backward\"")] {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let b = fx.add("b@x.co", "rt-b");
        crashed_switch(&fx, &b, &a);
        if landed {
            write_target_credential(&fx, &a);
        }
        let (_, logs) = capture_logs(|| drop(fx.engine.mutation_guard().unwrap()));
        let line = one(&logs, "recovering an interrupted switch");
        assert_eq!(
            [
                field(line, "direction"),
                field(line, "from_account"),
                field(line, "to_account"),
            ],
            [Some(direction), Some(b.as_str()), Some(a.as_str())],
            "{line}"
        );
        let recovered = at(&logs, "INFO", "event recorded")
            .iter()
            .any(|l| field(l, "kind") == Some("\"switch-recovered\""));
        assert_eq!(recovered, landed, "{logs:#?}");
        no_email(&logs);
    }
}
```

In `crates/tagteam/tests/logging.rs` (Task 6's), replace the module's doc comment:

```rust
//! §14.2 through the real binary: where the log is and with what modes, what its filter
//! takes, the `statusline` fast path that never opens it, a log that cannot be written, a
//! panic's line, and a collector thread's log line under `--debug`. Needs `--features
//! test-support`.
```

with:

```rust
//! §14.2 through the real binary: where the log is and with what modes, what its filter
//! takes, the `statusline` fast path that never opens it, a log that cannot be written, a
//! panic's line, a collector thread's log line under `--debug`, and that no line holds an
//! identity or a secret (B.69). Needs `--features test-support`.
```

Then replace the `use` lines:

```rust
use common::{cmd, expire_vault, login, seed_home, std_cmd, two_fresh_accounts};
use serde_json::{Value, json};
use tagteam_provider::mock_server::{MockReply, MockServer};
use tagteam_provider::{Env, FileKeychain};
```

with:

```rust
use common::{cmd, expire_vault, login, seed_home, std_cmd, two_fresh_accounts};
use serde_json::{Value, json};
use tagteam_cc::{ItemKind, keychain_account, keychain_service};
use tagteam_provider::mock_server::{MockReply, MockServer};
use tagteam_provider::splice::replace_top_level;
use tagteam_provider::{Env, FileKeychain, Keychain};
```

and append to the end of the file:

```rust
// No identity and no secret in any line, at any level (§14.2, §15.3 "Logs", B.69).

const USAGE: &str = "/api/oauth/usage";
const PROFILE: &str = "/api/oauth/profile";
const TOKEN: &str = "/v1/oauth/token";
const ORG_UUID: &str = "org-zq-7735";
const ORG_NAME: &str = "Zqorg Redaction Holdings";
const ALPHA_EMAIL: &str = "zq-alpha-7731@redact.test";
const BRAVO_EMAIL: &str = "zq-bravo-7732@redact.test";
const KEY_EMAIL: &str = "zq-key-7733@redact.test";
const SETUP_EMAIL: &str = "zq-setup-7734@redact.test";
const ALPHA_RT: &str = "zqrt-alpha-Kp7wXr2mQv9sLt4nBy6c";
const ALPHA_AT: &str = "zqat-alpha-Hj3kPw8xRt5vNm2qZs7d";
const ALPHA_RT_NEXT: &str = "zqrt-alpha-next-Pz5wKq8mXr3vTn7y";
const ALPHA_AT_NEXT: &str = "zqat-alpha-next-Lk4xWp9zQm2rVs6t";
const BRAVO_RT: &str = "zqrt-bravo-Wq4zLp9kXv2mRt7nHs3j";
const BRAVO_AT: &str = "zqat-bravo-Tn6yMk3wQp8xVr5zLs2h";
const BRAVO_RT_ROTATED: &str = "zqrt-bravo-rotated-Gx7pKw2zRm9qTv4s";
const BRAVO_AT_ROTATED: &str = "zqat-bravo-rotated-Fy3nVq6kWp8zXm2r";
const API_KEY: &str = "sk-ant-api03-zqkey-Rw8pXk3mQz7vTn2sLy5h";
const SETUP_TOKEN: &str = "sk-ant-oat01-zqsetup-Mx6kPw2zRq9vTs4nLy7j";
/// No line holds one of these whole.
const IDENTITIES: [&str; 5] = [ALPHA_EMAIL, BRAVO_EMAIL, KEY_EMAIL, SETUP_EMAIL, ORG_NAME];
/// No line holds 13 consecutive characters of one of these.
const SECRETS: [&str; 10] = [
    ALPHA_RT,
    ALPHA_AT,
    ALPHA_RT_NEXT,
    ALPHA_AT_NEXT,
    BRAVO_RT,
    BRAVO_AT,
    BRAVO_RT_ROTATED,
    BRAVO_AT_ROTATED,
    API_KEY,
    SETUP_TOKEN,
];

/// What `claude /login` leaves behind, as `common::login` writes it, but with this fixture's
/// organization and tokens: strings that nothing else in a run could produce.
fn login_as(root: &Path, email: &str, rt: &str, at: &str) {
    let env = Env::for_test(root);
    let path = env.home.join(".claude.json");
    let account = json!({"emailAddress": email, "organizationUuid": ORG_UUID,
                         "organizationName": ORG_NAME, "accountUuid": format!("uuid-{email}")});
    let doc = fs::read(&path).unwrap();
    fs::write(
        &path,
        replace_top_level(&doc, "oauthAccount", &account).unwrap(),
    )
    .unwrap();
    let credential = json!({"claudeAiOauth": {"accessToken": at, "refreshToken": rt,
                            "refreshTokenExpiresAt": 1_797_000_000_000i64}});
    FileKeychain::new(root.join("keychain"))
        .upsert(
            &keychain_service(&env, ItemKind::OAuth),
            &keychain_account(&env),
            credential.to_string().as_bytes(),
        )
        .unwrap();
}

/// The binary at TRACE, with every endpoint on `server`.
fn traced(root: &Path, server: &MockServer) -> assert_cmd::Command {
    let mut c = cmd(root);
    c.env(TAGTEAM_LOG, "trace").env(API_BASE, server.base_url());
    c
}

/// A server for the usage fetches (the recorded reply) and for a refresh, which hands out a's
/// next tokens. The profile oracle answers nothing until `oracle_names_bravo`.
fn redaction_server() -> MockServer {
    let server = MockServer::start();
    let usage: Value = serde_json::from_str(include_str!(
        "../../tagteam-cc/tests/fixtures/endpoints/usage-200.json"
    ))
    .unwrap();
    server.on(
        "GET",
        USAGE,
        MockReply::Json {
            status: 200,
            body: usage["body"].clone(),
        },
    );
    server.on(
        "POST",
        TOKEN,
        MockReply::Json {
            status: 200,
            body: json!({"access_token": ALPHA_AT_NEXT, "refresh_token": ALPHA_RT_NEXT,
                         "expires_in": 28800, "scope": "user:inference user:profile"}),
        },
    );
    server
}

/// From now on the profile oracle names b, with the organization's name, for any token: `add`
/// asks it too, and would refuse a's login as b's.
fn oracle_names_bravo(server: &MockServer) {
    server.on(
        "GET",
        PROFILE,
        MockReply::Json {
            status: 200,
            body: json!({"account": {"uuid": format!("uuid-{BRAVO_EMAIL}"), "email": BRAVO_EMAIL},
                         "organization": {"uuid": ORG_UUID, "name": ORG_NAME}}),
        },
    );
}

#[test]
fn every_command_at_trace_leaves_no_identity_or_secret_in_the_log() {
    // §15.3 "Logs", B.69 and Review Focus 5: every command, at TRACE, against a home whose
    // emails, organization name, tokens and keys are strings nothing else contains, while
    // the tokens go over the wire (usage bearers, the oracle's bearer and its answer, a
    // refresh's body and reply, `add-token`'s key and setup token).
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    let server = redaction_server();
    let run = |args: &[&str]| {
        traced(root, &server).args(args).assert().success();
    };
    seed_home(&Env::for_test(root));
    login_as(root, ALPHA_EMAIL, ALPHA_RT, ALPHA_AT);
    run(&["add"]);
    login_as(root, BRAVO_EMAIL, BRAVO_RT, BRAVO_AT);
    run(&["add", "--alias", "zqb"]);
    // Collects both: a with its vault token, b with the live one.
    let listed = traced(root, &server)
        .args(["list", "--json"])
        .output()
        .unwrap();
    let listed: Value = serde_json::from_slice(&listed.stdout).unwrap();
    let alpha = listed["accounts"][0]["id"].as_str().unwrap().to_owned();
    run(&["list"]);
    run(&["status"]);
    run(&["status", "--json"]);
    run(&["alias", "1", "zqa"]);
    run(&["alias"]);
    run(&["alias", "1", "--unset"]);
    run(&["disable", ALPHA_EMAIL]);
    run(&["enable", ALPHA_EMAIL]);
    run(&["move", "1", "2"]);
    run(&["move", "2", "1"]);
    run(&["history", "1"]);
    run(&["history", "2", "--csv"]);
    run(&["statusline"]);
    // Claude Code rotated b in place, and a's access token is about to expire: `switch 1`
    // asks the oracle about b's new token, captures it, and refreshes a through the gate.
    login_as(root, BRAVO_EMAIL, BRAVO_RT_ROTATED, BRAVO_AT_ROTATED);
    expire_vault(root, &alpha, 60_000);
    oracle_names_bravo(&server);
    run(&["switch", "1"]);
    run(&["switch", "2", "--json"]);
    run(&["add-token", API_KEY, "--email", KEY_EMAIL, "--json"]);
    traced(root, &server)
        .args(["add-token", "-", "--email", SETUP_EMAIL])
        .write_stdin(format!("{SETUP_TOKEN}\n"))
        .assert()
        .success();
    run(&["remove", KEY_EMAIL]);
    run(&["remove", SETUP_EMAIL, "--json"]);
    run(&["config", "path"]);
    run(&["config", "set", "ui.color", "never"]);
    run(&["config", "get", "ui.color"]);
    run(&["config", "list", "--json"]);
    run(&["config", "unset", "ui.color"]);
    run(&["list"]);

    // The secrets were sent: the log is clean because it never writes them.
    let sent = server.requests();
    let bearer = |at: &str| {
        sent.iter().any(|r| {
            r.headers
                .iter()
                .any(|(k, v)| k == "authorization" && *v == format!("Bearer {at}"))
        })
    };
    assert!(
        bearer(ALPHA_AT) && bearer(BRAVO_AT) && bearer(BRAVO_AT_ROTATED),
        "usage and oracle bearers"
    );
    assert!(
        sent.iter()
            .any(|r| r.path == TOKEN && String::from_utf8_lossy(&r.body).contains(ALPHA_RT)),
        "the refresh sent a's refresh token"
    );

    let log = log_text(&state_dir(root));
    assert!(
        log.contains(" INFO tagteam_engine::store: ")
            && log.contains(" INFO tagteam_engine::vault: "),
        "the commands logged their state changes:\n{log}"
    );
    for line in log.lines() {
        let target = line.split(' ').nth(3).unwrap_or_default();
        assert!(
            target.starts_with("tagteam"),
            "only tagteam's own events; no `log` record is bridged in (Decision 5): {line}"
        );
    }
    for identity in IDENTITIES {
        assert!(!log.contains(identity), "{identity} is in the log:\n{log}");
    }
    for secret in SECRETS {
        for part in secret.as_bytes().windows(13) {
            let part = std::str::from_utf8(part).unwrap();
            assert!(
                !log.contains(part),
                "{part:?}, part of a secret, is in the log:\n{log}"
            );
        }
    }
}
```

The profile reply is registered only before `switch 1` because `add` asks the oracle too. It
would refuse a's login if the oracle named b: "the live credential belongs to
zq-bravo-7732@redact.test, not zq-alpha-7731@redact.test".

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-engine --test log_lines`
Expected: 5 fail, each with `assertion \`left == right\` failed: one "<message>" line in [...]`
and `left: 0, right: 1`. The messages are:
- `stored a credential in the vault`
- `event recorded`
- `kept a refreshed token in rescue/`
- `saved a credential to displaced/`
- `recovering an interrupted switch`

Run: `cargo test -p tagteam --features test-support --test logging every_command`
Expected: FAIL with `the commands logged their state changes:`. At TRACE the file holds only
the oracle's DEBUG lines so far.

- [ ] **Step 3: Implement**

In `crates/tagteam-engine/src/store/mod.rs`, after `event_from_row` (ending
`        detail: json_col(r, "detail")?,\n    })\n}`), and before
`/// Maps UNIQUE violations to named errors.`, insert:

```rust
/// §14.2 and Decision 10: every `events` row is logged at INFO once it is written, naming its
/// accounts by ID only. `detail` is never logged: it is free-form JSON, and nothing bounds what
/// a later kind puts in it.
fn log_event(e: &EventRow) {
    tracing::info!(
        provider = %e.provider,
        kind = e.kind.as_str(),
        from_account = e.from_id.as_ref().map(tracing::field::display),
        to_account = e.to_id.as_ref().map(tracing::field::display),
        trigger = e.trigger.as_deref(),
        source = e.source.as_str(),
        "event recorded"
    );
}
```

In the same file, replace:

```rust
        Self::insert_event_on(&tx, event)?;
        tx.execute(DELETE_JOURNAL_SQL, [provider.as_str()])?;
        tx.commit()?;
        Ok(())
    }

    pub fn insert_event(&self, e: &EventRow) -> Result<(), StoreError> {
        Self::insert_event_on(&self.lock(), e)?;
        Ok(())
    }
```

with:

```rust
        Self::insert_event_on(&tx, event)?;
        tx.execute(DELETE_JOURNAL_SQL, [provider.as_str()])?;
        tx.commit()?;
        drop(c);
        log_event(event);
        Ok(())
    }

    pub fn insert_event(&self, e: &EventRow) -> Result<(), StoreError> {
        Self::insert_event_on(&self.lock(), e)?;
        log_event(e);
        Ok(())
    }
```

In `crates/tagteam-engine/src/vault.rs`, replace the whole of `Vault::store`, doc comment
included:

```rust
    /// Writes under the account lock. The current generation moves to `.prev` only when the
    /// fingerprint changes; the write is verified by reading back.
    pub fn store(
        &self,
        lock: &AccountLock,
        bytes: &[u8],
        fingerprint: &dyn Fn(&[u8]) -> Option<Fingerprint>,
    ) -> Result<(), VaultError> {
        let id = lock.id();
        match self.backend.read(id.as_str()) {
            Read::Present(old) => {
                if fingerprint(&old) != fingerprint(bytes) {
                    self.backend.write(&prev_key(id), &old)?;
                }
            }
            Read::Absent => {}
            Read::Unreadable(e) => return Err(VaultError::Unreadable(e)),
        }
        self.backend.write(id.as_str(), bytes)?;
        match self.backend.read(id.as_str()) {
            Read::Present(v) if v == bytes => Ok(()),
            _ => Err(VaultError::Verify),
        }
    }
```

with:

```rust
    /// Writes under the account lock. The current generation moves to `.prev` only when the
    /// fingerprint changes; the write is verified by reading back. A verified write is logged
    /// at INFO (§14.2, Decision 10): the account's ID, and the generation's fingerprint as its
    /// first 12 hex digits.
    pub fn store(
        &self,
        lock: &AccountLock,
        bytes: &[u8],
        fingerprint: &dyn Fn(&[u8]) -> Option<Fingerprint>,
    ) -> Result<(), VaultError> {
        let id = lock.id();
        let fp = fingerprint(bytes);
        let new_generation = match self.backend.read(id.as_str()) {
            Read::Present(old) => {
                let changed = fingerprint(&old) != fp;
                if changed {
                    self.backend.write(&prev_key(id), &old)?;
                }
                changed
            }
            Read::Absent => true,
            Read::Unreadable(e) => return Err(VaultError::Unreadable(e)),
        };
        self.backend.write(id.as_str(), bytes)?;
        match self.backend.read(id.as_str()) {
            Read::Present(v) if v == bytes => {}
            _ => return Err(VaultError::Verify),
        }
        tracing::info!(
            account = %id,
            fp = fp.as_ref().map(|f| tracing::field::display(f.short12())),
            new_generation,
            "stored a credential in the vault"
        );
        Ok(())
    }
```

In `crates/tagteam-engine/src/rescue.rs`, `Engine::write_rescue`, replace:

```rust
        write_atomic_private(
            &path,
            &serde_json::to_vec(&envelope).expect("a Value always serializes"),
            0o600,
        )?;
        Ok(path)
    }
```

with:

```rust
        write_atomic_private(
            &path,
            &serde_json::to_vec(&envelope).expect("a Value always serializes"),
            0o600,
        )?;
        tracing::info!(
            account = %id,
            fp = %successor_fp.short12(),
            "kept a refreshed token in rescue/ until the vault takes it"
        );
        Ok(path)
    }
```

In `crates/tagteam-engine/src/displace.rs`, replace the end of `displace`:

```rust
        identity: identity.cloned(),
    })?;
    Ok(id)
}
```

with:

```rust
        identity: identity.cloned(),
    })?;
    // The entry's own ID, never the identity it was attributed to (§14.2).
    tracing::info!(
        provider = %provider,
        displaced = %id,
        reason,
        "saved a credential to displaced/"
    );
    Ok(id)
}
```

In `crates/tagteam-engine/src/recover.rs`, replace:

```rust
enum Direction {
    Forward(String),
    Backward(String),
    Undecidable,
}
```

with:

```rust
enum Direction {
    Forward(String),
    Backward(String),
    Undecidable,
}

impl Direction {
    /// As the recovery's INFO line names it (§14.2: recovery is a decision a user may need to
    /// reconstruct).
    fn name(&self) -> &'static str {
        match self {
            Direction::Forward(_) => "forward",
            Direction::Backward(_) => "backward",
            Direction::Undecidable => "undecidable",
        }
    }
}
```

and in `Engine::recover_one`, replace:

```rust
        let live = p.read_live_auth(&self.env);
        match self.direction(p, &store, row, &live, hints)? {
            Direction::Forward(fp) => {
```

with:

```rust
        let live = p.read_live_auth(&self.env);
        let direction = self.direction(p, &store, row, &live, hints)?;
        tracing::info!(
            provider = %row.provider,
            from_account = row.from_id.as_ref().map(tracing::field::display),
            to_account = %row.to_id,
            direction = direction.name(),
            "recovering an interrupted switch"
        );
        match direction {
            Direction::Forward(fp) => {
```

**The audit of existing `tracing` calls** (Review Focus 5) found nothing to fix. Every call at
`3c1f458`, with what it interpolates:

| Site | Interpolates | Why it holds no identity or secret |
|---|---|---|
| `app.rs` `or_inactive` | `position`, `id` (Part B renames it), `e.kind()` | a kind, not a message |
| `engine.rs` recovery loop | `provider`, `{e}` from `recover_one` | its errors are store, lock, provider, vault and unreadable ones, `NoSuchAccount(<id>)` and `RecoveryBlocked` (a lock path); none carries a label |
| `quarantine.rs` `quarantine` | `position`, `account`, `reason` | — |
| `refresh.rs` (×8) | `position`, `account`, `reason`, and `{e}` / `{cause}` from `persist_generation`, `Received::keep`, `quarantine` and the gate's later steps | vault, store, unreadable, I/O and displacement errors only. A vault write error is `KeychainError` text, which is `security`'s stderr for `add-generic-password`, an error message (`describe()`); only `-g` prints a secret there, and it is never surfaced |
| `rescue.rs` `settle_rescues` | `position`, `account`, an `io::Error` | — |
| `switch.rs` rollback drop | the names of entries that failed to restore (Keychain services, file paths) | `ProviderError::Incomplete` holds names, never bytes |
| `switch.rs` re-plan | `position`, `account`, a `StoreError` | — |
| `switch.rs` capture | `position` (Part B adds `account`) | — |
| `active.rs` (×2) | `position` (Part B adds `account`), `{why}` / `{w}` | lock, unreadable, provider and displacement errors; the warnings name `displaced/<id>.json` files |
| `lifecycle.rs` cleanup | `id` (Part B renames it), a `VaultError` | as above |
| `views.rs` (×2) | `position`, `id` (Part B), `kind`; a `StoreError` | — |
| `oracle.rs` | `provider` | — |
| `recover.rs` | `provider`, the warnings from `save_unheld` | they name `displaced/<id>.json` |
| `tagteam-cc` `live.rs` (×3), `provider.rs` | restore-failure names; `KeychainError` | as above |

`Identity`'s `Debug` already redacts, and no call formats an `Identity`, a credential or a
request. `ureq` and `rustls` log through the `log` crate, which Decision 5 leaves unbridged, so
nothing of theirs reaches the file even at TRACE. The redaction pin's target check holds that.

- [ ] **Step 4: Run the tests and see them pass**

Run: `cargo test -p tagteam-engine --test log_lines`. Expected: PASS, 5 tests.
Run: `cargo test -p tagteam --features test-support --test logging`. Expected: PASS, 11 tests.
Then `cargo test --workspace --features tagteam/test-support`. Expected: PASS. In particular,
`switch.rs`'s `a_re_plan_that_cannot_be_stored_never_fails_the_switch` still finds exactly one
ERROR line and no email.

- [ ] **Step 5: Format, lint, commit**

`cargo fmt --all && cargo fmt --all --check`,
`cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings`,
`cargo clippy --workspace --all-targets -- -D warnings`, then
`cargo test --workspace --features tagteam/test-support`. Then:

```
git add crates/tagteam-engine/src/store/mod.rs crates/tagteam-engine/src/vault.rs \
  crates/tagteam-engine/src/rescue.rs crates/tagteam-engine/src/displace.rs \
  crates/tagteam-engine/src/recover.rs crates/tagteam-engine/tests/log_lines.rs \
  crates/tagteam/tests/logging.rs
git commit -m "Log every event row, vault write, rescue, displacement and recovery at INFO"
```

#### Part B: One spelling for an account

- [ ] **Step 1: Write the failing test**

Append to `crates/tagteam-engine/tests/log_lines.rs`:

```rust
#[test]
fn an_unverified_capture_names_the_account_by_id_and_position() {
    // §9.4 step 4's WARN line, in §14.2's one spelling: `account=<id> position=<n>`. Claude
    // Code rotated b in place, and the fixture's oracle has no answer.
    let _serial = one_at_a_time();
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.rotate_live("rt-b2");
    let (out, logs) = capture_logs(|| fx.switch_to(&a, false).unwrap());
    assert!(out.switched, "{}", out.message);
    let found = at(&logs, "WARN", "captured an unverified live credential");
    assert_eq!(found.len(), 1, "{logs:#?}");
    assert_eq!(
        (field(found[0], "account"), field(found[0], "position")),
        (Some(b.as_str()), Some("2")),
        "{}",
        found[0]
    );
    no_email(&logs);
}
```

- [ ] **Step 2: Run it and see it fail**

Run: `cargo test -p tagteam-engine --test log_lines an_unverified_capture`
Expected: FAIL. `left: (None, Some("2"))`, `right: (Some("<b's id>"), Some("2"))`.

Run: `rg -n '^\s*id = %' crates/*/src`
Expected: three hits: `app.rs:224`, `views.rs:450` and `lifecycle.rs:363`. After M4a there is a
fourth, in `views.rs`'s `in_session`.

Run: `rg -n -U '(?m)^\s+position(?: = [a-z_.]+)?,\n\s+"' crates/*/src`
Expected: three hits: `active.rs:510`, `active.rs:570` and `switch.rs:1366`.

- [ ] **Step 3: Implement**

`crates/tagteam/src/app.rs`, `or_inactive`: replace `            id = %id,` with
`            account = %id,`.

`crates/tagteam-engine/src/views.rs`, `account_view_with`: replace
`                    id = %row.id,` with `                    account = %row.id,`. After M4a,
make the same replacement in `in_session`'s `tracing::warn!`.

`crates/tagteam-engine/src/lifecycle.rs`, `commit_login`'s vault cleanup: replace
`                            id = %prep.id,` with `                            account = %prep.id,`.

`crates/tagteam-engine/src/active.rs`, `publish`: replace

```rust
            tracing::warn!(
                position = row.position,
                "a refreshed credential was not published to the live store: {why}"
```

with

```rust
            tracing::warn!(
                position = row.position,
                account = %row.id,
                "a refreshed credential was not published to the live store: {why}"
```

and replace

```rust
            tracing::warn!(
                position = row.position,
                "publishing a refreshed credential: {w}"
```

with

```rust
            tracing::warn!(
                position = row.position,
                account = %row.id,
                "publishing a refreshed credential: {w}"
```

`crates/tagteam-engine/src/switch.rs`, the outgoing capture: replace

```rust
                    tracing::warn!(
                        position = out.position,
                        "captured an unverified live credential into the vault; .prev keeps the previous generation"
```

with

```rust
                    tracing::warn!(
                        position = out.position,
                        account = %out.id,
                        "captured an unverified live credential into the vault; .prev keeps the previous generation"
```

- [ ] **Step 4: Run the tests and see them pass**

Run: `cargo test -p tagteam-engine --test log_lines`. Expected: PASS, 6 tests.
Run both `rg` commands from Step 2. Expected: no output.
Run: `cargo test -p tagteam --features test-support --test cli`. Expected: PASS. The quiet-switch
test still finds `captured an unverified live credential` under `--debug`.

- [ ] **Step 5: Format, lint, commit**

`cargo fmt --all && cargo fmt --all --check`, both clippy runs as in Part A, then
`cargo test --workspace --features tagteam/test-support`. Then:

```
git add crates/tagteam/src/app.rs crates/tagteam-engine/src/views.rs \
  crates/tagteam-engine/src/lifecycle.rs crates/tagteam-engine/src/active.rs \
  crates/tagteam-engine/src/switch.rs crates/tagteam-engine/tests/log_lines.rs
git commit -m "Name an account in every log line by its ID and position"
```

#### Part C: Contained errors, with their causes

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-provider/Cargo.toml`, the tests need the dependencies, so add them first.
Replace:

```toml
thiserror.workspace = true

[dev-dependencies]
tempfile.workspace = true
```

with:

```toml
thiserror.workspace = true
tracing.workspace = true

[dev-dependencies]
tempfile.workspace = true
tracing-subscriber.workspace = true
```

In `crates/tagteam-provider/src/atomic.rs`'s test module, insert before
`    #[test]\n    fn private_dirs_are_0700() {`:

```rust
    /// The `tracing` lines `f` sends on this thread, level first, without times.
    fn logged(f: impl FnOnce()) -> Vec<String> {
        #[derive(Clone, Default)]
        struct Lines(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
        impl Write for Lines {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Lines {
            type Writer = Lines;
            fn make_writer(&'a self) -> Lines {
                self.clone()
            }
        }
        let lines = Lines::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(lines.clone())
            .with_ansi(false)
            .without_time()
            .finish();
        tracing::subscriber::with_default(subscriber, f);
        let text = String::from_utf8(lines.0.lock().unwrap().clone()).unwrap();
        text.lines().map(str::to_owned).collect()
    }

    #[test]
    fn a_temp_file_that_cannot_be_removed_is_logged_with_its_cause() {
        // §14 (L323): a contained error is logged at WARN with its cause, never discarded. The
        // check swaps the temp file for a non-empty directory of the same name, which no
        // `remove_file` deletes, as root or not, and then refuses to publish.
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.json");
        let logs = logged(|| {
            let r = write_atomic_with(&p, b"new", 0o600, || {
                let tmp = fs::read_dir(d.path())?
                    .filter_map(Result::ok)
                    .map(|e| e.path())
                    .find(|t| {
                        t.file_name()
                            .is_some_and(|n| n.to_string_lossy().starts_with(".c.json.tagteam-"))
                    })
                    .ok_or_else(|| io::Error::other("no temp file"))?;
                fs::remove_file(&tmp)?;
                fs::create_dir(&tmp)?;
                fs::write(tmp.join("keep"), "")?;
                Err(io::Error::other("lock lost"))
            });
            assert!(r.is_err());
        });
        let warnings: Vec<&String> = logs.iter().filter(|l| l.contains("WARN")).collect();
        assert_eq!(warnings.len(), 1, "{logs:?}");
        assert!(
            warnings[0].contains("could not remove a temporary file")
                && warnings[0].contains(".c.json.tagteam-"),
            "{}",
            warnings[0]
        );
    }

```

No other test in `tagteam-provider` captures logs, and no other code there logs, so this
capture needs no turn-taking guard (`log_lines.rs`'s `one_at_a_time` explains the race it
prevents).

In `crates/tagteam-provider/src/security.rs`'s test module, insert before
`    #[test]\n    fn a_failed_dash_g_never_echoes_its_stderr() {`:

```rust
    #[test]
    fn a_dash_g_timeout_and_a_dash_g_that_cannot_run_are_told_apart() {
        // §14 (L349): a timeout and a failure to spawn are different causes.
        let detail = |r: RunResult| {
            let s = Scripted::default().then(ok(b"cafe\n")).then(r);
            match cli(&s, None).find("svc", "acct") {
                Read::Unreadable(e) => e.detail,
                other => panic!("expected Unreadable, got {other:?}"),
            }
        };
        assert_eq!(
            detail(RunResult::TimedOut),
            "the -g disambiguation call did not finish in time"
        );
        assert_eq!(
            detail(RunResult::SpawnFailed(
                "Resource temporarily unavailable (os error 35)".into()
            )),
            "the -g disambiguation call could not run: Resource temporarily unavailable (os error 35)"
        );
    }

```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-provider --lib -- a_temp_file_that_cannot a_dash_g_timeout_and`
Expected: both fail.
- `a_temp_file_that_cannot_be_removed_is_logged_with_its_cause`: `left: 0, right: 1`. The
  failed removal is discarded.
- `a_dash_g_timeout_and_a_dash_g_that_cannot_run_are_told_apart`:
  `left: "the -g disambiguation call failed"`,
  `right: "the -g disambiguation call did not finish in time"`.

- [ ] **Step 3: Implement**

In `crates/tagteam-provider/src/atomic.rs`, replace:

```rust
impl Drop for Temp<'_> {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_file(self.path);
        }
    }
}
```

with:

```rust
impl Drop for Temp<'_> {
    /// A temp file that cannot be removed is left behind: a contained error, logged at WARN
    /// with its cause, never discarded (§14). One that is already gone was never left behind.
    fn drop(&mut self) {
        if self.published {
            return;
        }
        match fs::remove_file(self.path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => tracing::warn!(
                path = %self.path.display(),
                "could not remove a temporary file: {e}"
            ),
            _ => {}
        }
    }
}
```

In `crates/tagteam-provider/src/security.rs`, replace:

```rust
/// `disambiguate`'s own failures (a bad rc, a timeout, a spawn failure) never surface
/// `-g`'s stderr, unlike `describe()`/`unreadable()`: `-g`'s stderr is the channel
/// `security` prints the secret on.
fn disambiguation_failed(r: &RunResult) -> ReadError {
    let detail = match r {
        RunResult::Exited { code, .. } => format!("rc {code}: the -g disambiguation call failed"),
        RunResult::TimedOut | RunResult::SpawnFailed(_) => {
            "the -g disambiguation call failed".to_owned()
        }
    };
    ReadError::new("keychain", detail)
}
```

with:

```rust
/// `disambiguate`'s own failures (a bad rc, a timeout, a spawn failure) never surface
/// `-g`'s stderr, unlike `describe()`/`unreadable()`: `-g`'s stderr is the channel
/// `security` prints the secret on. A timeout and a failure to run are different causes,
/// and say so (§14); a spawn failure's text is the operating system's, never `security`'s.
fn disambiguation_failed(r: &RunResult) -> ReadError {
    let detail = match r {
        RunResult::Exited { code, .. } => format!("rc {code}: the -g disambiguation call failed"),
        RunResult::TimedOut => "the -g disambiguation call did not finish in time".to_owned(),
        RunResult::SpawnFailed(e) => format!("the -g disambiguation call could not run: {e}"),
    };
    ReadError::new("keychain", detail)
}
```

- [ ] **Step 4: Run the tests and see them pass**

Run: `cargo test -p tagteam-provider --lib`. Expected: PASS. The existing
`a_failed_dash_g_never_echoes_its_stderr` and `a_dash_g_timeout_is_unreadable` still pass.
Then `cargo test --workspace --features tagteam/test-support`. Expected: PASS.
`Cargo.lock` gains `tracing` and `tracing-subscriber` in `tagteam-provider`'s dependency list,
with no new package.

- [ ] **Step 5: Format, lint, commit**

`cargo fmt --all && cargo fmt --all --check`,
`cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings`,
`cargo clippy --workspace --all-targets -- -D warnings`, then
`cargo test --workspace --features tagteam/test-support` and `cargo test --workspace`. Then:

```
git add crates/tagteam-provider/Cargo.toml Cargo.lock crates/tagteam-provider/src/atomic.rs \
  crates/tagteam-provider/src/security.rs
git commit -m "Log a temp file left behind, and tell a security timeout from a spawn failure"
```

#### Part D: Rollbacks, refresh outcomes and a quiet statusline

Part A logs what the gate and a switch write, but three of §14.2's rules still have gaps:
- **"Switches and their rollbacks."** A switch that fails after a live write rolls back in
  `Rollback::fail`. `fail` disarms the guard, so `Drop`'s WARN never fires, and a rolled-back
  switch writes no `events` row. Nothing records the rollback.
- **"Refresh outcomes."** The gate (§7.3) and the active-token refresh (§7.5) log only what they
  write: vault generations, rescues and quarantine events. A request that timed out, a systemic
  refusal, a refresh that found nothing to do, or one that ended in an error leaves no line.
- **"Everything `statusline` does logs at DEBUG at most."** `Engine::statusline` reaches
  `account_view_with`'s WARN when the live account's `usage_state` row cannot be read, so the
  status bar opens the log under the default filter, on every tick.

- [ ] **Step 1: Write the failing tests**

The binary test corrupts a store row directly, as the engine's tests do, so `tagteam` gains
`rusqlite` as a dev-dependency. In `crates/tagteam/Cargo.toml`'s `[dev-dependencies]`, replace:

```toml
predicates.workspace = true
tempfile.workspace = true
```

with:

```toml
predicates.workspace = true
rusqlite.workspace = true
tempfile.workspace = true
```

In `crates/tagteam-engine/tests/log_lines.rs`, replace the module's doc comment and `use` lines:

```rust
//! §14.2's INFO lines from the engine's central sites (Decision 10): every `events` row, every
//! vault generation written, a rescue, a displacement and a recovery decision. Each names its
//! accounts by ID, never by email (B.35).
mod common;

use std::sync::{Mutex, MutexGuard, PoisonError};

use common::{Fx, capture_logs, crashed_switch, due, vault_fp, write_target_credential};
use tagteam_engine::vault::SERVICE;
```

with:

```rust
//! §14.2's lines from the engine's central sites (Decision 10): every `events` row, every
//! vault generation written, a rescue, a displacement, a recovery decision, a switch's
//! rollback and every refresh outcome, and the status bar's DEBUG ceiling. Each names its
//! accounts by ID, never by email (B.35).
mod common;

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use common::{Fx, capture_logs, crashed_switch, due, vault_fp, write_target_credential};
use serde_json::json;
use tagteam_core::AccountId;
use tagteam_engine::account_lock::AccountLock;
use tagteam_engine::active::{ActiveOutcome, ActiveTrigger};
use tagteam_engine::vault::SERVICE;
use tagteam_engine::views::StatuslineView;
use tagteam_provider::Clock;
use tagteam_provider::http::{HttpError, Method};
```

and append to the end of the file:

```rust
/// The lines one gate call on `id` logs, with the vault's bytes as the caller's snapshot.
fn gate_logs(fx: &Fx, id: &AccountId) -> Vec<String> {
    let snapshot = fx.vault_bytes(id).unwrap();
    capture_logs(|| {
        fx.engine
            .refresh_stored(fx.cc.as_ref(), id, &snapshot)
            .unwrap()
    })
    .1
}

/// The one line among `logs` whose message contains `message`, at whichever level.
fn only<'a>(logs: &'a [String], message: &str) -> &'a str {
    let found: Vec<&str> = logs
        .iter()
        .map(|l| l.trim_start())
        .filter(|l| l.contains(message))
        .collect();
    assert_eq!(found.len(), 1, "one {message:?} line in {logs:#?}");
    found[0]
}

/// No line holds any part of the fixture's tokens: `rt-a…` and `at-rt-a…`.
fn no_token(logs: &[String]) {
    assert!(
        logs.iter().all(|l| !l.contains("rt-a")),
        "a token was logged: {logs:#?}"
    );
}

#[test]
fn a_gate_refresh_logs_its_outcome_once_at_its_level() {
    // §14.2: refresh outcomes are INFO once a request was sent or state changed, and a gate
    // that did neither logs at DEBUG. No line holds a token, or a systemic refusal's own words.
    let _serial = one_at_a_time();
    let gate = "refresh gate outcome";

    // The request was sent, and its successor stored.
    let fx = Fx::new();
    let a = due(&fx);
    fx.script_refresh(Some("rt-a2"));
    let logs = gate_logs(&fx, &a);
    let line = only(&logs, gate);
    assert!(line.starts_with("INFO"), "{line}");
    assert_eq!(
        (field(line, "account"), field(line, "outcome")),
        (Some(a.as_str()), Some("\"refreshed\"")),
        "{line}"
    );
    no_token(&logs);

    // The request was sent and its reply never came.
    let fx = Fx::new();
    let a = due(&fx);
    fx.http.push(
        Method::Post,
        &Fx::endpoints().token,
        Err(HttpError::Ambiguous("timed out reading the reply".into())),
    );
    let logs = gate_logs(&fx, &a);
    let line = only(&logs, gate);
    assert!(line.starts_with("INFO"), "{line}");
    assert_eq!(
        [
            field(line, "outcome"),
            field(line, "kind"),
            field(line, "rescued"),
        ],
        [Some("\"transient\""), Some("\"ambiguous\""), Some("false")],
        "{line}"
    );
    assert!(!line.contains("timed out"), "{line}");
    no_token(&logs);

    // The endpoint refused the request itself, in words that name the account.
    let fx = Fx::new();
    let a = due(&fx);
    fx.http.push_json(
        Method::Post,
        &Fx::endpoints().token,
        400,
        json!({"error": "invalid_client", "error_description": "no client for a@x.co"}),
    );
    let logs = gate_logs(&fx, &a);
    let line = only(&logs, gate);
    assert!(line.starts_with("INFO"), "{line}");
    assert_eq!(field(line, "outcome"), Some("\"systemic\""), "{line}");
    assert!(logs.iter().all(|l| !l.contains("no client")), "{logs:#?}");
    no_email(&logs);
    no_token(&logs);

    // Another process holds the account lock: nothing is sent, nothing changes.
    let fx = Fx::new();
    let a = due(&fx);
    fx.script_refresh(Some("rt-a2"));
    let _held = AccountLock::acquire(&fx.env, &a, Duration::ZERO).unwrap();
    let logs = gate_logs(&fx, &a);
    let line = only(&logs, gate);
    assert!(line.starts_with("DEBUG"), "{line}");
    assert_eq!(field(line, "outcome"), Some("\"busy\""), "{line}");
}

#[test]
fn an_active_token_refresh_logs_its_outcome_by_provider_and_account() {
    // §14.2 and §7.5: the live token's refresh is INFO once it sent a request; one that found
    // the token still fresh, and changed nothing, logs at DEBUG.
    let _serial = one_at_a_time();
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let mut live = fx.live_credential().unwrap();
    live["claudeAiOauth"]["expiresAt"] = json!(fx.clock.now_ms());
    fx.set_live_credential(live.to_string().as_bytes());
    fx.script_refresh(Some("rt-a2"));
    let active = "active-token refresh outcome";

    let (out, logs) = capture_logs(|| {
        fx.engine
            .refresh_active(&fx.provider(), ActiveTrigger::Expired)
            .unwrap()
    });
    assert_eq!(out, ActiveOutcome::Refreshed);
    let line = one(&logs, active);
    assert_eq!(
        [
            field(line, "provider"),
            field(line, "account"),
            field(line, "outcome"),
        ],
        [Some("claude-code"), Some(a.as_str()), Some("\"refreshed\"")],
        "{line}"
    );
    no_token(&logs);

    let (out, logs) = capture_logs(|| {
        fx.engine
            .refresh_active(&fx.provider(), ActiveTrigger::Expired)
            .unwrap()
    });
    assert_eq!(out, ActiveOutcome::NotNeeded { reconciled: false });
    let line = only(&logs, active);
    assert!(line.starts_with("DEBUG"), "{line}");
    assert_eq!(field(line, "outcome"), Some("\"not-needed\""), "{line}");
}

#[test]
fn an_unreadable_usage_row_is_a_warning_for_a_command_and_a_debug_line_for_the_status_bar() {
    // §14.2: everything `statusline` does logs at DEBUG at most, so a status bar that cannot
    // read the live account's usage never opens the log. An account command's result logs the
    // same failure at WARN (§14: a contained error is logged with its cause).
    let _serial = one_at_a_time();
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    rusqlite::Connection::open(fx.env.data_dir().join("tagteam.db"))
        .unwrap()
        .execute(
            "INSERT OR REPLACE INTO usage_state (account_id, fetched_at) VALUES (?1, 'soon')",
            [a.as_str()],
        )
        .unwrap();
    let usage = "could not read the account's usage";

    let (view, logs) = capture_logs(|| fx.engine.statusline(&fx.provider()).unwrap());
    assert!(matches!(view, StatuslineView::Managed { .. }));
    let line = only(&logs, usage);
    assert!(line.starts_with("DEBUG"), "{line}");
    assert_eq!(
        (field(line, "account"), field(line, "kind")),
        (Some(a.as_str()), Some("\"store\"")),
        "{line}"
    );
    assert!(
        logs.iter()
            .all(|l| l.trim_start().starts_with("DEBUG") || l.trim_start().starts_with("TRACE")),
        "the status bar logs at DEBUG at most: {logs:#?}"
    );

    let row = fx.engine.store().unwrap().account(&a).unwrap().unwrap();
    let (_, logs) = capture_logs(|| fx.engine.account_view(row, true));
    assert!(only(&logs, usage).starts_with("WARN"), "{logs:#?}");
}

#[cfg(feature = "test-hooks")]
mod hooks {
    use tagteam_cc::{ItemKind, keychain_service};
    use tagteam_engine::EngineError;

    use super::*;

    #[test]
    fn a_rolled_back_switch_names_its_accounts_and_its_cause_s_kind() {
        // §14.2: switches and their rollbacks. An error after the switch wrote a's credential
        // puts every byte back; the line names the cause by its kind, never by its text, which
        // can hold a label.
        let _serial = one_at_a_time();
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let b = fx.add("b@x.co", "rt-b");
        fx.engine.fail_at(Some("after-credential"));
        let (err, logs) = capture_logs(|| fx.switch_to(&a, false).unwrap_err());
        assert!(matches!(err, EngineError::RolledBack(_)), "{err}");
        let found = at(&logs, "WARN", "rolled back a switch");
        assert_eq!(found.len(), 1, "{logs:#?}");
        assert_eq!(
            [
                field(found[0], "provider"),
                field(found[0], "from_account"),
                field(found[0], "to_account"),
                field(found[0], "kind"),
            ],
            [
                Some("claude-code"),
                Some(b.as_str()),
                Some(a.as_str()),
                Some("\"invalid-input\""),
            ],
            "{}",
            found[0]
        );
        assert!(
            logs.iter().all(|l| !l.contains("injected failure")),
            "the cause's text: {logs:#?}"
        );
        no_email(&logs);
    }

    #[test]
    fn a_rollback_that_fails_too_is_an_error_naming_what_it_left() {
        // §9.4 step 10: the credential undo cannot write b's credential back, so the journal
        // row stays for recovery (§9.6), and the line says so.
        let _serial = one_at_a_time();
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        fx.add("b@x.co", "rt-b");
        let kc = fx.kc.clone();
        let svc = keychain_service(&fx.env, ItemKind::OAuth);
        fx.engine.on_point(
            "after-identity",
            Box::new(move || kc.set_fail_write(&svc, true)),
        );
        fx.engine.fail_at(Some("after-identity"));
        let (err, logs) = capture_logs(|| fx.switch_to(&a, false).unwrap_err());
        assert!(matches!(err, EngineError::RollbackFailed { .. }), "{err}");
        let found = at(&logs, "ERROR", "a switch was not fully rolled back");
        assert_eq!(found.len(), 1, "{logs:#?}");
        assert!(
            found[0].contains("its journal row stays for recovery: restore the live credential"),
            "{}",
            found[0]
        );
        assert_eq!(
            field(found[0], "kind"),
            Some("\"invalid-input\""),
            "{}",
            found[0]
        );
        assert!(
            logs.iter().all(|l| !l.contains("injected failure")),
            "the cause's text: {logs:#?}"
        );
        no_email(&logs);
    }
}
```

Every test in this file takes `one_at_a_time`, and none of these call sites fires on another
thread, so no capture needs a warm-up (Decision 16). The two rollback tests need `fail_at` and
`on_point`, so they sit in a `test-hooks` module, as `config.rs`'s do.

In `crates/tagteam/tests/logging.rs` (Task 6's), replace the whole of
`statusline_with_the_default_filter_opens_no_log`:

```rust
#[test]
fn statusline_with_the_default_filter_opens_no_log() {
    // §14.2, Review Focus 4: the status bar runs every few seconds and logs at DEBUG at most,
    // so by default it never creates, opens or rotates the log.
    let d = tempfile::tempdir().unwrap();
    rotated_live_login(d.path());
    let out = cmd(d.path())
        .arg("statusline")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(!out.is_empty());
    assert!(!state_dir(d.path()).exists());
}
```

with:

```rust
/// Gives `email`'s account a `usage_state` row the store cannot read: text where it keeps an
/// integer.
fn unreadable_usage_state(root: &Path, email: &str) {
    rusqlite::Connection::open(Env::for_test(root).data_dir().join("tagteam.db"))
        .unwrap()
        .execute(
            "INSERT OR REPLACE INTO usage_state (account_id, fetched_at) \
             SELECT id, 'soon' FROM accounts WHERE email = ?1",
            [email],
        )
        .unwrap();
}

#[test]
fn statusline_with_the_default_filter_opens_no_log() {
    // §14.2, Review Focus 4: the status bar runs every few seconds and logs at DEBUG at most,
    // so by default it never creates, opens or rotates the log. Nor when the live account's
    // usage cannot be read: an account command's result logs that at WARN, the status bar at
    // DEBUG, and its line still names the account.
    for unreadable in [false, true] {
        let d = tempfile::tempdir().unwrap();
        rotated_live_login(d.path());
        if unreadable {
            unreadable_usage_state(d.path(), "b@x.co");
        }
        let out = cmd(d.path())
            .arg("statusline")
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let line = String::from_utf8(out).unwrap();
        assert!(
            line.starts_with("b · "),
            "unreadable usage {unreadable}: {line:?}"
        );
        assert!(
            !state_dir(d.path()).exists(),
            "unreadable usage {unreadable}: {}",
            log_text(&state_dir(d.path()))
        );
    }
}
```

`rotated_live_login` leaves b live, at position 2, and the line names it `b`.

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-engine --features test-hooks --test log_lines`
Expected: 5 fail, and the 6 earlier tests pass:
- `a_gate_refresh_logs_its_outcome_once_at_its_level` and
  `an_active_token_refresh_logs_its_outcome_by_provider_and_account`: `assertion \`left ==
  right\` failed: one "refresh gate outcome" line in [...]` (`"active-token refresh outcome"`
  in the second), with `left: 0, right: 1`.
- `an_unreadable_usage_row_is_a_warning_for_a_command_and_a_debug_line_for_the_status_bar`:
  panics at `line.starts_with("DEBUG")`, printing the line it found:
  `WARN could not read the account's usage position=1 account=<a's id> kind="store"`.
- `hooks::a_rolled_back_switch_names_its_accounts_and_its_cause_s_kind` and
  `hooks::a_rollback_that_fails_too_is_an_error_naming_what_it_left`: `left: 0, right: 1`, with
  no WARN and no ERROR line about the rollback.

Run: `cargo test -p tagteam --features test-support --test logging statusline`
Expected: FAIL in the second pass, with `unreadable usage true: <time> <pid> WARN
tagteam_engine::views: could not read the account's usage position=2 account=<b's id>
kind="store"`. The first pass, with a readable row, already opens no log.

- [ ] **Step 3: Implement**

**`crates/tagteam-engine/src/switch.rs`.** In `struct Rollback`, replace:

```rust
    provider: &'a ProviderId,
    prior: Option<Box<JournalRow>>,
```

with:

```rust
    provider: &'a ProviderId,
    /// The switch's outgoing account and its target, which its log lines name (§14.2).
    from: Option<AccountId>,
    to: AccountId,
    prior: Option<Box<JournalRow>>,
```

Replace the whole of `Rollback::fail`, from its doc line to its closing brace:

```rust
    /// Rolls back after `cause`, and says whether that worked.
    fn fail(mut self, cause: EngineError) -> EngineError {
        let partial = match &cause {
            EngineError::Provider(ProviderError::RestoreFailed { restore, .. }) => {
                vec![format!("restoring a partial write: {restore}")]
            }
            _ => vec![],
        };
        let failed = self.roll_back(partial);
        if failed.is_empty() {
            EngineError::RolledBack(cause.to_string())
        } else {
            EngineError::RollbackFailed {
                cause: cause.to_string(),
                failed: failed.join("; "),
            }
        }
    }
```

with:

```rust
    /// Rolls back after `cause`, and says whether that worked. Either way the rollback is
    /// logged (§14.2: switches and their rollbacks), at WARN when every write was put back and
    /// at ERROR naming what was not. The cause is named by its `kind()` alone: its text may
    /// hold a label.
    fn fail(mut self, cause: EngineError) -> EngineError {
        let partial = match &cause {
            EngineError::Provider(ProviderError::RestoreFailed { restore, .. }) => {
                vec![format!("restoring a partial write: {restore}")]
            }
            _ => vec![],
        };
        let failed = self.roll_back(partial);
        let from = self.from.as_ref().map(tracing::field::display);
        if failed.is_empty() {
            tracing::warn!(
                provider = %self.provider,
                from_account = from,
                to_account = %self.to,
                kind = cause.kind(),
                "rolled back a switch"
            );
            EngineError::RolledBack(cause.to_string())
        } else {
            tracing::error!(
                provider = %self.provider,
                from_account = from,
                to_account = %self.to,
                kind = cause.kind(),
                "a switch was not fully rolled back; its journal row stays for recovery: {}",
                failed.join("; ")
            );
            EngineError::RollbackFailed {
                cause: cause.to_string(),
                failed: failed.join("; "),
            }
        }
    }
```

In `transact`, the guard's literal: replace

```rust
            provider,
            prior,
            in_flight: false,
            armed: true,
        };
```

with:

```rust
            provider,
            from: outgoing.as_ref().map(|o| o.id.clone()),
            to: target.id.clone(),
            prior,
            in_flight: false,
            armed: true,
        };
```

**`crates/tagteam-engine/src/refresh.rs`.** After `log_lost` (ending
`        "a refreshed token was lost: {cause}"\n    );\n}`), insert:

```rust
/// §14.2's refresh outcome, one line per gate call, naming the account by ID. INFO once a
/// request was sent or state changed, and for an error, which is named by its `kind()` alone;
/// DEBUG when the gate sent nothing and changed nothing: another process holds the lock or has
/// refreshed already, or the token is someone else's to refresh. A systemic refusal's own
/// words are never logged.
fn log_gate(id: &AccountId, result: &Result<GateOutcome, EngineError>) {
    const OUTCOME: &str = "refresh gate outcome";
    match result {
        Ok(GateOutcome::Refreshed(_)) => {
            tracing::info!(account = %id, outcome = "refreshed", "{OUTCOME}");
        }
        Ok(GateOutcome::Dead(reason)) => tracing::info!(
            account = %id,
            outcome = "dead",
            reason = reason.as_str(),
            "{OUTCOME}"
        ),
        Ok(GateOutcome::Systemic(_)) => {
            tracing::info!(account = %id, outcome = "systemic", "{OUTCOME}");
        }
        Ok(GateOutcome::Transient { kind, rescued }) => tracing::info!(
            account = %id,
            outcome = "transient",
            kind = kind.as_str(),
            rescued,
            "{OUTCOME}"
        ),
        Ok(GateOutcome::Unpersisted) => {
            tracing::info!(account = %id, outcome = "unpersisted", "{OUTCOME}");
        }
        Err(e) => tracing::info!(
            account = %id,
            outcome = "error",
            kind = e.kind(),
            "{OUTCOME}"
        ),
        Ok(GateOutcome::AlreadyFresh(_)) => {
            tracing::debug!(account = %id, outcome = "already-fresh", "{OUTCOME}");
        }
        Ok(GateOutcome::Busy) => {
            tracing::debug!(account = %id, outcome = "busy", "{OUTCOME}");
        }
        Ok(GateOutcome::Owned(by)) => tracing::debug!(
            account = %id,
            outcome = "owned",
            by = ?by,
            "{OUTCOME}"
        ),
        Ok(GateOutcome::Conflict) => {
            tracing::debug!(account = %id, outcome = "conflict", "{OUTCOME}");
        }
    }
}
```

Then replace `refresh_stored`'s doc comment and signature:

```rust
    /// The refresh gate (§7.3): the only place a stored refresh token is ever sent. The
    /// account lock is only tried, never waited for, and is held from here until the result
    /// is persisted, across the request: at most one refresh per account is in flight, and a
    /// suspended holder is never preempted. `snapshot` is the vault bytes the caller decided
    /// on; step 4 compares against it.
    pub fn refresh_stored(
        &self,
        p: &dyn Provider,
        id: &AccountId,
        snapshot: &[u8],
    ) -> Result<GateOutcome, EngineError> {
```

with:

```rust
    /// The refresh gate (§7.3): the only place a stored refresh token is ever sent. The
    /// account lock is only tried, never waited for, and is held from here until the result
    /// is persisted, across the request: at most one refresh per account is in flight, and a
    /// suspended holder is never preempted. `snapshot` is the vault bytes the caller decided
    /// on; step 4 compares against it. Whichever way it ends, its outcome is logged once
    /// (`log_gate`), after the account lock is released.
    pub fn refresh_stored(
        &self,
        p: &dyn Provider,
        id: &AccountId,
        snapshot: &[u8],
    ) -> Result<GateOutcome, EngineError> {
        let result = self.run_gate(p, id, snapshot);
        log_gate(id, &result);
        result
    }

    /// `refresh_stored`'s steps. Every path out of here, an early `return` and a `?` included,
    /// is one outcome, which its caller logs.
    fn run_gate(
        &self,
        p: &dyn Provider,
        id: &AccountId,
        snapshot: &[u8],
    ) -> Result<GateOutcome, EngineError> {
```

The rest of the old body is now `run_gate`'s, unchanged.

**`crates/tagteam-engine/src/active.rs`.** Replace:

```rust
use tagteam_core::{Fingerprint, OracleVerdict, ProviderId};
```

with:

```rust
use tagteam_core::{AccountId, Fingerprint, OracleVerdict, ProviderId};
```

After `struct Reconciled` (ending `    retire: Vec<PathBuf>,\n}`), insert:

```rust
/// §14.2's refresh outcome for §7.5, one line per call, naming the provider and, once it is
/// known, the live account. INFO once a request was sent or state changed, and for an error,
/// which is named by its `kind()` alone; DEBUG for a token that needed nothing. A systemic
/// refusal's own words are never logged.
fn log_active(
    provider: &ProviderId,
    account: Option<&AccountId>,
    result: &Result<ActiveOutcome, EngineError>,
) {
    const OUTCOME: &str = "active-token refresh outcome";
    let account = account.map(tracing::field::display);
    match result {
        Ok(ActiveOutcome::Refreshed) => {
            tracing::info!(provider = %provider, account, outcome = "refreshed", "{OUTCOME}");
        }
        Ok(ActiveOutcome::PersistedNotPublished) => tracing::info!(
            provider = %provider,
            account,
            outcome = "persisted-not-published",
            "{OUTCOME}"
        ),
        Ok(ActiveOutcome::PublishedOnly) => tracing::info!(
            provider = %provider,
            account,
            outcome = "published-only",
            "{OUTCOME}"
        ),
        Ok(ActiveOutcome::NotNeeded { reconciled: true }) => tracing::info!(
            provider = %provider,
            account,
            outcome = "not-needed",
            reconciled = true,
            "{OUTCOME}"
        ),
        Ok(ActiveOutcome::Dead(reason)) => tracing::info!(
            provider = %provider,
            account,
            outcome = "dead",
            reason = reason.as_str(),
            "{OUTCOME}"
        ),
        Ok(ActiveOutcome::Systemic(_)) => {
            tracing::info!(provider = %provider, account, outcome = "systemic", "{OUTCOME}");
        }
        Ok(ActiveOutcome::Transient { kind }) => tracing::info!(
            provider = %provider,
            account,
            outcome = "transient",
            kind = kind.as_str(),
            "{OUTCOME}"
        ),
        Ok(ActiveOutcome::Unpersisted) => {
            tracing::info!(provider = %provider, account, outcome = "unpersisted", "{OUTCOME}");
        }
        Err(e) => tracing::info!(
            provider = %provider,
            account,
            outcome = "error",
            kind = e.kind(),
            "{OUTCOME}"
        ),
        Ok(ActiveOutcome::NotNeeded { reconciled: false }) => tracing::debug!(
            provider = %provider,
            account,
            outcome = "not-needed",
            "{OUTCOME}"
        ),
    }
}
```

Then replace the head of `refresh_active`, from its doc comment to the oracle's call:

```rust
    /// §7.5. The mutation lock (recovering first), the live account's lock, then CC's
    /// credential locks only; the config lock is taken after the request, around the live
    /// write alone. The oracle is asked before any lock (§7.6).
    pub fn refresh_active(
        &self,
        provider: &ProviderId,
        trigger: ActiveTrigger,
    ) -> Result<ActiveOutcome, EngineError> {
        self.refuse_inside_run_shell()?;
        let provider_arc = self.provider(provider)?;
        let p = provider_arc.as_ref();
        let (row, live) = self.active_login(p, provider)?;
        let hint = self.corroborate(p, &row, &live)?;
```

with:

```rust
    /// §7.5. The mutation lock (recovering first), the live account's lock, then CC's
    /// credential locks only; the config lock is taken after the request, around the live
    /// write alone. The oracle is asked before any lock (§7.6). Whichever way it ends, its
    /// outcome is logged once (`log_active`), after every lock is released.
    pub fn refresh_active(
        &self,
        provider: &ProviderId,
        trigger: ActiveTrigger,
    ) -> Result<ActiveOutcome, EngineError> {
        let mut account = None;
        let result = self.run_active_refresh(provider, trigger, &mut account);
        log_active(provider, account.as_ref(), &result);
        result
    }

    /// `refresh_active`'s steps. `account` is set as soon as the live login's account is
    /// known, so the caller's line names it however the refresh ends.
    fn run_active_refresh(
        &self,
        provider: &ProviderId,
        trigger: ActiveTrigger,
        account: &mut Option<AccountId>,
    ) -> Result<ActiveOutcome, EngineError> {
        self.refuse_inside_run_shell()?;
        let provider_arc = self.provider(provider)?;
        let p = provider_arc.as_ref();
        let (row, live) = self.active_login(p, provider)?;
        *account = Some(row.id.clone());
        let hint = self.corroborate(p, &row, &live)?;
```

The rest of the old body is now `run_active_refresh`'s, unchanged.

**`crates/tagteam-engine/src/views.rs`.** Replace:

```rust
    pub fn account_view(&self, row: AccountRow, active: bool) -> AccountView {
        self.account_view_with(row, active, true)
    }

    /// `account_view`, with or without pace on its windows (see `usage_view`).
    fn account_view_with(&self, row: AccountRow, active: bool, with_pace: bool) -> AccountView {
```

with:

```rust
    pub fn account_view(&self, row: AccountRow, active: bool) -> AccountView {
        self.account_view_with(row, active, true, false)
    }

    /// `account_view`, with or without pace on its windows (see `usage_view`). A usage read
    /// that fails is logged at WARN, or at DEBUG for the status bar (`status_bar`), where
    /// everything logs at DEBUG at most (§14.2).
    fn account_view_with(
        &self,
        row: AccountRow,
        active: bool,
        with_pace: bool,
        status_bar: bool,
    ) -> AccountView {
```

In the same function, replace the failed read's line (Part B's spelling):

```rust
            .unwrap_or_else(|e| {
                tracing::warn!(
                    position = row.position,
                    account = %row.id,
                    kind = e.kind(),
                    "could not read the account's usage"
                );
```

with:

```rust
            .unwrap_or_else(|e| {
                if status_bar {
                    tracing::debug!(
                        position = row.position,
                        account = %row.id,
                        kind = e.kind(),
                        "could not read the account's usage"
                    );
                } else {
                    tracing::warn!(
                        position = row.position,
                        account = %row.id,
                        kind = e.kind(),
                        "could not read the account's usage"
                    );
                }
```

In `Engine::statusline`, replace:

```rust
            // The line shows no pace, so none is computed.
            Some(row) => StatuslineView::Managed {
                account: self.account_view_with(row, true, false),
            },
```

with:

```rust
            // The line shows no pace, so none is computed, and logs at DEBUG at most (§14.2).
            Some(row) => StatuslineView::Managed {
                account: self.account_view_with(row, true, false, true),
            },
```

**What `statusline` reaches.** Every `warn!`, `error!` and `info!` in the workspace was checked
against the status bar's path. After this part, none of them can fire on it:
- `main_with_args` runs the parse, the stdin drain, `Context::from_process` and
  `logging::init`. None of them logs; `logging::init` writes an invalid `TAGTEAM_LOG`'s warning
  to stderr.
- `app::run` takes the `statusline` branch right after `root_guard::refuse_root`. That is
  before `build_engine` and before anything takes the mutation guard, so `engine.rs`'s recovery
  WARN never runs.
- `statusline::engine` runs `command_settings`, a settings read whose warnings it drops
  (`settings.rs` does not log), and `Engine::new`.
- `Engine::statusline` runs, in order:
  - the registry;
  - `existing_store`: `Store::open_existing` and `migrate` write no `events` row, so `log_event`
    never runs;
  - `live_login`: `.claude.json` through `live_identity`, which logs nothing, and a cache write
    whose failure is already DEBUG;
  - `find_by_identity_key`;
  - `account_view_with`, whose WARN this part makes DEBUG here.
- What it never runs:
  - `tagteam-cc`'s lines (`live.rs`, `provider.rs`), which are all in its writes and restores;
  - `atomic.rs`'s, which is in an atomic write;
  - any gate, oracle or §7.5 line: its Keychain is `NoKeychain` and its HTTP `NoHttp`.
- After M4a, `run_shell` and `shell_account` log nothing. `statusline` passes
  `in_session = false`, so `in_session`'s WARN stays off this path (M4a's Decision 17).
- A panic logs at ERROR whatever the command (Decision 9). It is a bug, not something
  `statusline` does.

- [ ] **Step 4: Run the tests and see them pass**

Run: `cargo test -p tagteam-engine --features test-hooks --test log_lines`. Expected: PASS, 11
tests. Without `--features test-hooks`, 9: the two rollback tests need it.
Run: `cargo test -p tagteam --features test-support --test logging`. Expected: PASS, 11 tests.
Run: `cargo test -p tagteam-engine --features test-hooks`. Expected: PASS, the gate, active,
switch and rollback suites included. `switch.rs`'s
`a_re_plan_that_cannot_be_stored_never_fails_the_switch` still finds exactly one ERROR line.
`Cargo.lock` gains `rusqlite` in `tagteam`'s dependency list, with no new package.

- [ ] **Step 5: Format, lint, commit**

`cargo fmt --all && cargo fmt --all --check`, both clippy runs as in Part A, then
`cargo test --workspace --features tagteam/test-support`. Then:

```
git add Cargo.lock crates/tagteam/Cargo.toml crates/tagteam-engine/src/switch.rs \
  crates/tagteam-engine/src/refresh.rs crates/tagteam-engine/src/active.rs \
  crates/tagteam-engine/src/views.rs crates/tagteam-engine/tests/log_lines.rs \
  crates/tagteam/tests/logging.rs
git commit -m "Log a switch's rollback and every refresh outcome, and keep statusline at DEBUG"
```

---

### Task 8: Displaced entries in the engine

> **Re-sync (M3a, M4a).** This task is written against `main` at `3c1f458`, except for one call.
> - **`FlockGuard::lock`.** Part C uses M3a Task 1's signature,
>   `FlockGuard::lock(path, timeout, cancel: &Cancel)`, with a fresh `Cancel::new()` (Decision 11).
>   On `3c1f458` the call is `FlockGuard::lock(path, timeout)`, and there is no `Cancel` import.
> - **`switch.rs`, `transact` and `save_unheld`.** M3a Task 11 (`critical_env` in `apply`), M3a
>   Task 9 (`plan.notes` in the warnings), M4a Task 3 (`settle_outgoing`'s facts) and M4a Task 5
>   (the step-7 loop becomes `for entry in &doomed`) change the surroundings. Part D's delta is
>   anchored on text those tasks keep:
>   1. A new `own_secret` binding goes just before the comment `// What steps 2 and 4 settle,
>      and the vaults below: step 7's rule never saves it again.`
>   2. The direct branch's `displace_live` call takes `attributed(...)`.
>   3. Both `save_unheld` calls gain `own_secret.as_deref(),`.
>   4. Step 6 and its `from_secret` binding are left as they are.
> - **`recover.rs`, `finish_forward`.** M3a Task 11 (`critical_env` in `clear_other_axis`) and
>   M4a Task 3 (the commit epoch) leave Part D's two edits untouched: the deleted
>   `live_identity` read and the `None, None` arguments.
> - **`active.rs`, `publish`.** M3a Tasks 2 and 11 change its `env`, and M4a Task 5 its loop. Part
>   D only adds one `None,` to each `save_unheld` call.
> - **`error.rs`.** M3a Task 2 adds `Interrupted(i32)`. `NoSuchDisplaced` goes after
>   `NoSuchAccount` either way.
> - **Task 7** (this plan) adds `log_event` to `store/mod.rs` and an INFO line to `displace`.
>   Part A inserts after `log_event`, and Parts B and C keep `displace`'s line.

§6.3: "`displaced/` holds credentials that were not tagteam's to keep … Each file is `<id>.json`,
with `id = <epoch s>-<fp12>-<rand6>` … The file is written first and its row second … Both are
written under the displaced lock (`locks/displaced.lock`): a leaf `flock`, held only around one
entry's file and row … `displaced --purge` holds it around each deletion, so it never deletes a
file whose row is still to be inserted. A row's `identity` is the identity the displacing code
attributed the bytes to, or null. A secret found on the other auth axis is never attributed to
the outgoing login." The listing "joins rows and files: the ID, provider, time, reason, identity
(and the position of the managed account it names, if any) and the fingerprint's first 12 hex
digits. A file with no row is listed as `unrecorded`, with the time its name carries; a row whose
file is gone is listed as `file missing` … it reads only the store and the directory, under no
lock." `--purge` "deletes each named entry: the file first, verified gone, then the row. Every ID
is checked before anything is deleted; an unknown one is an error that names it … Each deletion
holds the displaced lock, waited for up to 5 s."

This task builds:
- `Store::displaced_rows` and `Store::delete_displaced` (Part A);
- the ID check and `Engine::displaced` (Part B);
- the displaced lock around the writer and the purge (Part C);
- Decision 13's attribution (Part D).

**Readings of the spec this task commits to:**
- **What a file with no row records.** Its `at` is the seconds its name carries, × 1000. A
  number too large for an `i64` saturates and is never wrapped. Its provider, reason, fingerprint
  and identity are `None`.
- **A row's empty `fingerprint`** (written when `displace` had none) is `None`.
- **`account` follows §6.1.** It is found by the provider's identity key. A key match whose
  `account_uuid` conflicts with the identity's names a different account (a recycled email), so
  it maps to `None`. A uuid missing on either side is no conflict. `None` too when the row's
  provider is not registered, or cannot read the identity.
- **A `displaced/` that cannot be listed is an error,** never taken for an empty one (§4.3:
  "Unreadable is never collapsed into absent"). Only `NotFound` is "no files".
- **A row whose ID is not a displaced ID** (only a hand-edited store has one) is still listed:
  it is the store's. It is never matched to a file, since no path is built from it, and
  `--purge` refuses it.
- **A purge deletes the path itself, never what a symlink there points to,** and verifies it is
  gone with `lstat` (`symlink_metadata`).
- **The purge checks IDs in order:** first every ID's form, with no I/O, then each one's
  presence. The first failure is the error. A repeated ID is deleted once and reported once.
- **The purge opens the store per deletion, under the lock** (`existing_store`). A store that a
  concurrent writer creates while the purge waits is seen, so that writer's row is deleted with
  its file.
- **The purge reports every ID it was given,** including one that another purge deleted between
  the check and the deletion. It is gone either way.
- **The displaced lock nests under CC's storage-write lock.** After M3a Task 11, a Keychain
  fallback's `before_fallback` displaces the entry it is about to delete while the storage-write
  lock is held, and `displace` now takes the displaced lock. §4.3 and §9.1 allow exactly that
  nesting (amended at `d14a860`): the displaced lock is the innermost leaf, and nothing waits
  under it.
- **Lines are logged once the displaced lock is released.** `displace` keeps Task 7's INFO line
  and a purge logs one INFO line per deleted entry (`deleted a displaced credential
  displaced=<id>`, Decision 10), each after its locked block. The ID's middle part is the
  fingerprint's first 12 hex digits, which the log may hold (§14.2).
- **Decision 13 holds wherever a live secret is displaced** (§6.3: "its row carries an identity
  only when the bytes themselves name one"):
  - step 7's `save_unheld` names the live identity only for `own_secret`, the live login's own;
  - §9.4 step 2's direct branch, which displaces the live secret on both axes, applies the same
    rule, so `own_secret` is computed before step 2;
  - recovery's forward finish (§9.6) passes `None` for both, and attributes nothing: every
    entry it clears sits on the other auth axis, and §9.6 never takes the live identity as
    evidence of whose credential an entry is.
- **The live login's own secret is on its own axis.** With a stored account live (`outgoing`),
  it is the live secret on that account's axis, and `None` when that axis is empty. With none,
  it is the credential entry's: an unmanaged login is named by `oauthAccount`, the entry's axis.
  Step 6's `from_secret` is not used for this. It falls back to the entry when the outgoing
  axis is empty, so under an API-key account whose key is gone it would be a stray OAuth login
  in the entry, and that login would be filed under the API-key account (§6.3: "A secret found
  on the other auth axis is never attributed to the outgoing login"). The journal's `from_fp`
  keeps that fallback, unchanged: §9.6 recovery reads it.

**Files:**
- Modify: `crates/tagteam-engine/src/lib.rs` (`mod displace;`, line 6)
- Modify: `crates/tagteam-engine/src/displace.rs` (the whole file, 1–37)
- Modify: `crates/tagteam-engine/src/store/mod.rs`:
  - new `displaced_from_row` after Task 7's `log_event`, before `/// Maps UNIQUE violations to
    named errors.`;
  - `displaced_rows` and `delete_displaced` after `insert_displaced`, 971–977.
- Modify: `crates/tagteam-engine/src/error.rs`:
  - `NoSuchDisplaced` after `NoSuchAccount`, 63;
  - `kind`, 134;
  - the pin test, 232.
- Modify: `crates/tagteam-engine/src/switch.rs`:
  - new `attributed` after `unless_forced`, 231–243;
  - `transact`, 1123–1253;
  - `save_unheld`, 1438–1467.
- Modify: `crates/tagteam-engine/src/active.rs` (`publish`, 529–537 and 549–557)
- Modify: `crates/tagteam-engine/src/recover.rs` (`finish_forward`, 229 and 265–273)
- Test: `crates/tagteam-engine/tests/store.rs`, `crates/tagteam-engine/tests/displaced.rs` (new),
  `crates/tagteam-engine/tests/log_lines.rs` (Task 7's; one test),
  and unit tests in `displace.rs` and `error.rs`

**Interfaces:**
- Consumes:
  - M3a Task 1: `FlockGuard::lock(path: &Path, timeout: Duration, cancel: &Cancel) -> Result<FlockGuard, LockError>`, `Cancel::new()`
  - Existing:
    - `FlockGuard::try_lock(path: &Path) -> io::Result<Option<FlockGuard>>`
    - `Store::insert_displaced(&DisplacedRow)`
    - `Store::find_by_identity_key(&ProviderId, &str) -> Result<Option<AccountRow>, StoreError>`
    - `Engine::{store, existing_store, provider, env, now_ms}`
    - `Provider::{parse_identity, identity_key}`
    - `write_atomic_private`, `ensure_private_dir`
    - `Fingerprint::short12`
- Produces:
  - In `tagteam_engine::displace` (now `pub mod`):
    - `pub const DISPLACED_LOCK: &str = "locks/displaced.lock"` (relative to `Env::data_dir()`)
    - `pub fn is_displaced_id(s: &str) -> bool`
    - `#[derive(Debug, Clone, PartialEq)] pub struct DisplacedEntry { pub id: String, pub provider: Option<ProviderId>, pub at_ms: i64, pub reason: Option<String>, pub fingerprint: Option<String>, pub identity: Option<serde_json::Value>, pub account: Option<u32>, pub file_present: bool, pub recorded: bool }`
    - `#[derive(Debug, Clone, PartialEq)] pub struct DisplacedList { pub dir: PathBuf, pub entries: Vec<DisplacedEntry> }`
  - On `Engine`:
    - `pub fn displaced(&self) -> Result<DisplacedList, EngineError>`
    - `pub fn known_displaced(&self, ids: &[String]) -> Result<Vec<String>, EngineError>`
    - `pub fn purge_displaced(&self, ids: &[String]) -> Result<Vec<String>, EngineError>`
  - `Store::displaced_rows(&self) -> Result<Vec<DisplacedRow>, StoreError>` (newest first: `at`
    desc, then `id` desc) and `Store::delete_displaced(&self, id: &str) -> Result<bool, StoreError>`
  - `EngineError::NoSuchDisplaced(String)`, kind `"no-such-displaced"`, message
    `no displaced credential matches "<id>"; `tagteam displaced` lists them`
  - `Engine::save_unheld(…, live_identity: Option<&Identity>, own_secret: Option<&[u8]>, warnings)`
    (crate-private)

---

#### Part A: the store's displaced rows

- [ ] **Step 1: Write the failing test**

In `crates/tagteam-engine/tests/store.rs`, replace the import line
`use tagteam_engine::store::{EventRow, JournalRow, LoginMeta, NewAccount, Store, StoreError};`
with:

```rust
use tagteam_engine::store::{
    DisplacedRow, EventRow, JournalRow, LoginMeta, NewAccount, Store, StoreError,
};
```

and append at the end of the file:

```rust
#[test]
fn displaced_rows_are_newest_first_and_a_delete_says_whether_a_row_went() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("t.db")).unwrap();
    let row = |id: &str, at: i64| DisplacedRow {
        id: id.into(),
        provider: cc(),
        at,
        reason: "displaced-live-login".into(),
        fingerprint: "sha256:00".into(),
        identity: (at == 2).then(|| json!({"emailAddress": "a@x.co"})),
    };
    for (id, at) in [
        ("1-000000000000-aaaaaa", 1),
        ("2-000000000000-bbbbbb", 2),
        ("2-000000000000-cccccc", 2),
    ] {
        s.insert_displaced(&row(id, at)).unwrap();
    }
    // By time, then by ID, both descending.
    assert_eq!(
        s.displaced_rows().unwrap(),
        [
            row("2-000000000000-cccccc", 2),
            row("2-000000000000-bbbbbb", 2),
            row("1-000000000000-aaaaaa", 1),
        ]
    );
    assert!(s.delete_displaced("2-000000000000-bbbbbb").unwrap());
    assert!(!s.delete_displaced("2-000000000000-bbbbbb").unwrap());
    assert_eq!(s.displaced_rows().unwrap().len(), 2);
}
```

- [ ] **Step 2: Run it and see it fail**

Run: `cargo test -p tagteam-engine --test store displaced_rows`
Expected: compile errors: `no method named `displaced_rows` found for struct `Store`` and
`no method named `delete_displaced` found for struct `Store``.

- [ ] **Step 3: Implement**

In `crates/tagteam-engine/src/store/mod.rs`, after Task 7's `fn log_event`, immediately before
`/// Maps UNIQUE violations to named errors.`, insert:

```rust
fn displaced_from_row(r: &Row<'_>) -> rusqlite::Result<DisplacedRow> {
    Ok(DisplacedRow {
        id: r.get("id")?,
        provider: ProviderId::new(r.get::<_, String>("provider")?),
        at: r.get("at")?,
        reason: r.get("reason")?,
        fingerprint: r.get("fingerprint")?,
        identity: json_col(r, "identity")?,
    })
}

```

After `insert_displaced` (the last method of `impl Store`, lines 971–977), insert:

```rust

    /// Every displaced row (§6.3), newest first: by `at`, then by ID, both descending.
    pub fn displaced_rows(&self) -> Result<Vec<DisplacedRow>, StoreError> {
        let c = self.lock();
        let mut stmt = c.prepare("SELECT * FROM displaced ORDER BY at DESC, id DESC")?;
        let rows = stmt
            .query_map([], displaced_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Deletes the displaced row `id`; `true` when there was one.
    pub fn delete_displaced(&self, id: &str) -> Result<bool, StoreError> {
        Ok(self.exec("DELETE FROM displaced WHERE id = ?1", &[&id])? > 0)
    }
```

- [ ] **Step 4: Run it and see it pass**

Run: `cargo test -p tagteam-engine --test store`. Expected: PASS.

- [ ] **Step 5: Format, lint, commit**

```
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p tagteam-engine --test store
git add crates/tagteam-engine/src/store/mod.rs crates/tagteam-engine/tests/store.rs
git commit -m "Read displaced rows newest first and delete one by ID"
```

---

#### Part B: displaced IDs and the listing

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam-engine/tests/displaced.rs`:

```rust
//! `displaced/` (§6.3): the listing joins rows and files, a purge deletes the file and then the
//! row under the displaced lock, and a displaced row names only the live login's own secret
//! (L467).

mod common;

use std::fs::{self, Permissions};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use common::Fx;
use serde_json::{Value, json};
use tagteam_core::ProviderId;
use tagteam_engine::displace::{DisplacedEntry, DisplacedList};
use tagteam_engine::store::DisplacedRow;

/// Three entries' IDs, newest first by the time each carries.
const NEWEST: &str = "1790000300-0123456789ab-aaaaaa";
const MIDDLE: &str = "1790000250-fedcba987654-bbbbbb";
const OLDEST: &str = "1790000200-00112233aabb-cccccc";

fn dir(fx: &Fx) -> PathBuf {
    fx.env.data_dir().join("displaced")
}

/// The fingerprint a planted row records: its ID's 12 hex digits, then zeros.
fn fingerprint(id: &str) -> String {
    format!("sha256:{}{}", id.split('-').nth(1).unwrap(), "0".repeat(52))
}

/// A file in `displaced/` named `name`, as `displace` leaves one (or anything else there).
fn plant_file(fx: &Fx, name: &str) {
    fs::create_dir_all(dir(fx)).unwrap();
    fs::write(dir(fx).join(name), format!("credential of {name}")).unwrap();
}

/// A row as `displace` records one.
fn plant_row(fx: &Fx, id: &str, at: i64, identity: Option<Value>) {
    fx.engine
        .store()
        .unwrap()
        .insert_displaced(&DisplacedRow {
            id: id.into(),
            provider: fx.provider(),
            at,
            reason: "displaced-live-login".into(),
            fingerprint: fingerprint(id),
            identity,
        })
        .unwrap();
}

/// `a` and `b` stored at 1 and 2, and three entries:
/// - NEWEST: a row and its file, naming `b`;
/// - MIDDLE: a file with no row;
/// - OLDEST: a row whose file is gone, naming a login tagteam does not manage.
fn three_entries() -> Fx {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    plant_file(&fx, &format!("{NEWEST}.json"));
    plant_row(
        &fx,
        NEWEST,
        1_790_000_300_500,
        Some(Fx::oauth_account("b@x.co")),
    );
    plant_file(&fx, &format!("{MIDDLE}.json"));
    plant_row(
        &fx,
        OLDEST,
        1_790_000_200_000,
        Some(Fx::oauth_account("stranger@x.co")),
    );
    fx
}

#[test]
fn the_listing_joins_rows_and_files_newest_first() {
    let fx = three_entries();
    // Decision 12: a name that is not `<displaced ID>.json` is never listed. That covers the
    // atomic writer's temp files, another case, another suffix, and no suffix.
    for foreign in [
        "notes.txt",
        ".1790000400-0123456789ab-dddddd.json.tagteam-42-0000abcd",
        "1790000400-0123456789AB-dddddd.json",
        "1790000400-0123456789ab-dddddd.json.bak",
        "1790000400-0123456789ab-dddddd",
    ] {
        plant_file(&fx, foreign);
    }
    let cc = Some(fx.provider());
    assert_eq!(
        fx.engine.displaced().unwrap(),
        DisplacedList {
            dir: dir(&fx),
            entries: vec![
                DisplacedEntry {
                    id: NEWEST.into(),
                    provider: cc.clone(),
                    at_ms: 1_790_000_300_500,
                    reason: Some("displaced-live-login".into()),
                    fingerprint: Some(fingerprint(NEWEST)),
                    identity: Some(Fx::oauth_account("b@x.co")),
                    account: Some(2),
                    file_present: true,
                    recorded: true,
                },
                // Unrecorded: the time its name carries, and nothing else known.
                DisplacedEntry {
                    id: MIDDLE.into(),
                    provider: None,
                    at_ms: 1_790_000_250_000,
                    reason: None,
                    fingerprint: None,
                    identity: None,
                    account: None,
                    file_present: true,
                    recorded: false,
                },
                DisplacedEntry {
                    id: OLDEST.into(),
                    provider: cc,
                    at_ms: 1_790_000_200_000,
                    reason: Some("displaced-live-login".into()),
                    fingerprint: Some(fingerprint(OLDEST)),
                    identity: Some(Fx::oauth_account("stranger@x.co")),
                    account: None,
                    file_present: false,
                    recorded: true,
                },
            ],
        }
    );
}

#[test]
fn account_is_the_managed_account_the_identity_names_unless_its_uuid_conflicts() {
    // §6.1: an identity key names an account, and an account uuid that conflicts makes it a
    // different one, such as a recycled email. A missing uuid is no conflict.
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let cases = [
        (
            json!({"emailAddress": "a@x.co", "organizationUuid": "", "accountUuid": "uuid-a@x.co"}),
            Some(1),
        ),
        (
            json!({"emailAddress": "a@x.co", "organizationUuid": ""}),
            Some(1),
        ),
        (
            json!({"emailAddress": "a@x.co", "organizationUuid": "", "accountUuid": "uuid-recycled"}),
            None,
        ),
        (
            json!({"emailAddress": "a@x.co", "organizationUuid": "org-1"}),
            None,
        ),
        (json!({"organizationUuid": ""}), None),
    ];
    let id = |i: usize| format!("179000000{i}-0123456789ab-aaaaaa");
    for (i, (identity, _)) in cases.iter().enumerate() {
        plant_row(
            &fx,
            &id(i),
            1_790_000_000_000 + i as i64,
            Some(identity.clone()),
        );
    }
    // A provider this build does not register cannot read its identity. An empty
    // fingerprint is none.
    fx.engine
        .store()
        .unwrap()
        .insert_displaced(&DisplacedRow {
            id: id(9),
            provider: ProviderId::new("fake-agent"),
            at: 1,
            reason: "displaced-live-login".into(),
            fingerprint: String::new(),
            identity: Some(cases[0].0.clone()),
        })
        .unwrap();
    let list = fx.engine.displaced().unwrap();
    let entry = |i: usize| list.entries.iter().find(|e| e.id == id(i)).unwrap();
    for (i, (identity, account)) in cases.iter().enumerate() {
        assert_eq!(entry(i).account, *account, "{identity}");
        assert_eq!(entry(i).identity.as_ref(), Some(identity));
    }
    assert_eq!(
        (entry(9).account, entry(9).fingerprint.as_deref()),
        (None, None)
    );
}

#[test]
fn a_fresh_home_lists_nothing_and_creates_nothing() {
    // §5: a command that changes nothing creates nothing.
    let fx = Fx::new();
    assert_eq!(
        fx.engine.displaced().unwrap(),
        DisplacedList {
            dir: dir(&fx),
            entries: Vec::new(),
        }
    );
    assert!(!fx.env.data_dir().exists());
}

#[test]
fn a_directory_that_cannot_be_listed_is_an_error_not_an_empty_listing() {
    // §4.3: unreadable is never collapsed into absent.
    let fx = three_entries();
    fs::set_permissions(dir(&fx), Permissions::from_mode(0o000)).unwrap();
    let result = fx.engine.displaced();
    fs::set_permissions(dir(&fx), Permissions::from_mode(0o700)).unwrap();
    assert_eq!(result.unwrap_err().kind(), "io");
}
```

The unit tests for `is_displaced_id`, `new_id` and `id_at_ms` are the `#[cfg(test)] mod tests`
block at the end of the new `displace.rs` in Step 3. Write that block into the old file first:
it does not compile until those functions exist.

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-engine --test displaced`
Expected: compile errors: `could not find `displace` in `tagteam_engine`` (the module is private)
and `no method named `displaced` found for struct `Engine``.

- [ ] **Step 3: Implement**

In `crates/tagteam-engine/src/lib.rs`, replace `mod displace;` with `pub mod displace;`.

Replace the whole of `crates/tagteam-engine/src/displace.rs` with the file below. Its `displace` is
today's, with the ID minted by `new_id`, and keeps Task 7's INFO line word for word:

```rust
//! `displaced/` (§6.3): credentials that were not tagteam's to keep. The writer stashes one
//! before it is overwritten, and `tagteam displaced` lists the entries and purges them.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;
use tagteam_core::{Fingerprint, ProviderId};
use tagteam_provider::Env;
use tagteam_provider::atomic::{ensure_private_dir, write_atomic_private};

use crate::engine::Engine;
use crate::error::EngineError;
use crate::store::{DisplacedRow, Store};

/// One entry of `tagteam displaced` (§6.3): a row, a file, or both, joined by ID.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplacedEntry {
    /// `<epoch s>-<fp12>-<rand6>`, the file's stem.
    pub id: String,
    /// `None` for a file with no row.
    pub provider: Option<ProviderId>,
    /// The row's time. For a file with no row, the second its name carries.
    pub at_ms: i64,
    pub reason: Option<String>,
    /// The row's whole fingerprint. `None` with no row, or for a row that recorded none.
    pub fingerprint: Option<String>,
    /// The identity the displacing code attributed the bytes to, if any: provider-owned JSON.
    pub identity: Option<Value>,
    /// The position of the managed account `identity` names, if any.
    pub account: Option<u32>,
    pub file_present: bool,
    /// Whether a row records the entry.
    pub recorded: bool,
}

/// The listing: the directory the files are in, and every entry, newest first.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplacedList {
    pub dir: PathBuf,
    pub entries: Vec<DisplacedEntry>,
}

/// `^[0-9]{1,19}-[0-9a-f]{12}-[a-z0-9]{6}$` (Decision 12). This is the only form of ID a path is
/// ever built from, so a name that would leave `displaced/` is never an ID.
pub fn is_displaced_id(s: &str) -> bool {
    let mut parts = s.split('-');
    let (Some(epoch), Some(fp12), Some(rand6), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    (1..=19).contains(&epoch.len())
        && epoch.bytes().all(|b| b.is_ascii_digit())
        && fp12.len() == 12
        && fp12
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && rand6.len() == 6
        && rand6
            .bytes()
            .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase())
}

/// A new entry's ID (§5): the epoch second, the fingerprint's first 12 hex digits (twelve zeros
/// when it has none), and six random lowercase letters and digits.
fn new_id(now_ms: i64, fp: Option<&Fingerprint>) -> String {
    let fp12 = fp.map_or("000000000000", |f| f.short12());
    let rand6: String = (0..6)
        .map(|_| fastrand::alphanumeric().to_ascii_lowercase())
        .collect();
    format!("{}-{fp12}-{rand6}", now_ms / 1000)
}

/// When an entry with no row was displaced: the epoch second its ID carries, in ms. A number
/// too large for the clock saturates rather than wrapping.
fn id_at_ms(id: &str) -> i64 {
    id.split('-')
        .next()
        .and_then(|s| s.parse::<i64>().ok())
        .map_or(i64::MAX, |s| s.saturating_mul(1000))
}

fn displaced_dir(env: &Env) -> PathBuf {
    env.data_dir().join("displaced")
}

/// The IDs of `dir`'s entries named `<displaced ID>.json` (Decision 12). Anything else there is
/// ignored, including the atomic writer's temp files. An absent directory has none. One that
/// cannot be listed is an error, never taken for an empty one (§4.3).
fn displaced_files(dir: &Path) -> Result<BTreeSet<String>, EngineError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(e) => return Err(e.into()),
    };
    let mut ids = BTreeSet::new();
    for entry in entries {
        let name = entry?.file_name();
        if let Some(id) = name
            .to_str()
            .and_then(|n| n.strip_suffix(".json"))
            .filter(|stem| is_displaced_id(stem))
        {
            ids.insert(id.to_owned());
        }
    }
    Ok(ids)
}

/// Stashes live credential bytes that are about to be overwritten (§6.3). Forensic and
/// write-only: always a plain 0600 file, never a Keychain item. The file is written first and
/// its row second, so a failed insert leaves a file with no row, never a row that names nothing.
pub(crate) fn displace(
    engine: &Engine,
    provider: &ProviderId,
    bytes: &[u8],
    fp: Option<&Fingerprint>,
    reason: &str,
    identity: Option<&Value>,
) -> Result<String, EngineError> {
    let dir = displaced_dir(engine.env());
    ensure_private_dir(&dir)?;
    let now = engine.now_ms();
    let id = new_id(now, fp);
    write_atomic_private(&dir.join(format!("{id}.json")), bytes, 0o600)?;
    engine.store()?.insert_displaced(&DisplacedRow {
        id: id.clone(),
        provider: provider.clone(),
        at: now,
        reason: reason.to_owned(),
        fingerprint: fp.map(|f| f.as_str().to_owned()).unwrap_or_default(),
        identity: identity.cloned(),
    })?;
    // The entry's own ID, never the identity it was attributed to (§14.2).
    tracing::info!(
        provider = %provider,
        displaced = %id,
        reason,
        "saved a credential to displaced/"
    );
    Ok(id)
}

impl Engine {
    /// `tagteam displaced` (§6.3): every entry, newest first, joining the store's rows with the
    /// directory's files by ID. A file with no row is listed unrecorded, at the time its name
    /// carries; a row with no file, with its file missing. It reads only, under no lock, and
    /// creates nothing when the store or the directory is absent (§5).
    pub fn displaced(&self) -> Result<DisplacedList, EngineError> {
        let dir = displaced_dir(&self.env);
        let mut files = displaced_files(&dir)?;
        let mut entries = Vec::new();
        if let Some(store) = self.existing_store()? {
            for row in store.displaced_rows()? {
                // Only a displaced ID is ever in `files`, so no path is built from a row's ID.
                let file_present = files.remove(&row.id);
                let account =
                    self.displaced_account(&store, &row.provider, row.identity.as_ref())?;
                entries.push(DisplacedEntry {
                    id: row.id,
                    provider: Some(row.provider),
                    at_ms: row.at,
                    reason: Some(row.reason),
                    fingerprint: Some(row.fingerprint).filter(|f| !f.is_empty()),
                    identity: row.identity,
                    account,
                    file_present,
                    recorded: true,
                });
            }
        }
        entries.extend(files.into_iter().map(|id| DisplacedEntry {
            at_ms: id_at_ms(&id),
            id,
            provider: None,
            reason: None,
            fingerprint: None,
            identity: None,
            account: None,
            file_present: true,
            recorded: false,
        }));
        entries.sort_by(|a, b| b.at_ms.cmp(&a.at_ms).then_with(|| b.id.cmp(&a.id)));
        Ok(DisplacedList { dir, entries })
    }

    /// The position of the managed account `identity` names (§6.3), found through the row's
    /// provider by identity key. A key match whose account uuid conflicts is a different account
    /// (§6.1). `None` when the provider is not registered or cannot read the identity.
    fn displaced_account(
        &self,
        store: &Store,
        provider: &ProviderId,
        identity: Option<&Value>,
    ) -> Result<Option<u32>, EngineError> {
        let (Some(raw), Ok(p)) = (identity, self.provider(provider)) else {
            return Ok(None);
        };
        let Ok(identity) = p.parse_identity(raw) else {
            return Ok(None);
        };
        let key = p.identity_key(&identity);
        Ok(store
            .find_by_identity_key(provider, key.as_str())?
            .filter(|row| match (&row.account_uuid, &identity.account_uuid) {
                (Some(stored), Some(named)) => stored == named,
                _ => true,
            })
            .map(|row| row.position))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_displaced_id_is_an_epoch_twelve_hex_digits_and_six_lowercase_characters() {
        // Decision 12: the only names a path is ever built from.
        for good in [
            "1790000000-0123456789ab-a1b2c3",
            "1-000000000000-zzzzzz",
            "9223372036854775807-abcdefabcdef-000000",
        ] {
            assert!(is_displaced_id(good), "{good:?}");
        }
        for bad in [
            "",
            "x",
            "../../tagteam.db",
            "1790000000-0123456789AB-a1b2c3",
            "1790000000-0123456789ab-A1B2C3",
            "-0123456789ab-a1b2c3",
            "12345678901234567890-0123456789ab-a1b2c3",
            "1790000000-0123456789a-a1b2c3",
            "1790000000-0123456789ag-a1b2c3",
            "1790000000-0123456789ab-a1b2c",
            "1790000000-0123456789ab-a1b2c3-x",
            "1790000000-0123456789ab-a1b2c3.json",
            "1790000000-0123456789ab-a1b2c/",
            " 1790000000-0123456789ab-a1b2c3",
            "١٧٩٠-0123456789ab-a1b2c3",
        ] {
            assert!(!is_displaced_id(bad), "{bad:?}");
        }
    }

    #[test]
    fn every_id_the_writer_mints_is_a_displaced_id() {
        let known = Fingerprint::of_secret(b"rt-1");
        for _ in 0..1000 {
            for fp in [Some(&known), None] {
                let id = new_id(1_790_000_000_123, fp);
                assert!(is_displaced_id(&id), "{id}");
                assert!(id.starts_with("1790000000-"), "{id}");
            }
        }
        assert!(new_id(0, None).starts_with("0-000000000000-"));
    }

    #[test]
    fn a_file_with_no_row_is_dated_by_its_name() {
        assert_eq!(
            id_at_ms("1790000250-fedcba987654-bbbbbb"),
            1_790_000_250_000
        );
        assert_eq!(
            id_at_ms("9223372036854775807-abcdefabcdef-000000"),
            i64::MAX,
            "saturates"
        );
        assert_eq!(
            id_at_ms("9999999999999999999-abcdefabcdef-000000"),
            i64::MAX,
            "past i64"
        );
    }
}
```

- [ ] **Step 4: Run them and see them pass**

Run: `cargo test -p tagteam-engine --lib displace::tests` and
`cargo test -p tagteam-engine --test displaced`. Expected: PASS (three and four tests).

Run: `cargo test -p tagteam-engine`. Expected: PASS. The writer's behaviour is unchanged: it
still names its files `<epoch>-<fp12>-<rand6>.json`, now through `new_id`.

- [ ] **Step 5: Format, lint, commit**

```
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p tagteam-engine
git add crates/tagteam-engine/src/lib.rs crates/tagteam-engine/src/displace.rs \
  crates/tagteam-engine/tests/displaced.rs
git commit -m "List displaced credentials by joining their rows and files"
```

---

#### Part C: the displaced lock, and the purge

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-engine/tests/displaced.rs`, replace the import block (from
`use std::fs::{self, Permissions};` to `use tagteam_engine::store::DisplacedRow;`) with:

```rust
use std::fs::{self, Permissions};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use common::{Fx, STRAY_API_KEY};
use serde_json::{Value, json};
use tagteam_core::ProviderId;
use tagteam_engine::displace::{DISPLACED_LOCK, DisplacedEntry, DisplacedList};
use tagteam_engine::store::DisplacedRow;
use tagteam_provider::FlockGuard;
```

and append at the end of the file:

```rust
/// Holds the displaced lock as another tagteam process would.
fn hold_lock(fx: &Fx) -> FlockGuard {
    FlockGuard::try_lock(&fx.env.data_dir().join(DISPLACED_LOCK))
        .unwrap()
        .expect("the displaced lock is free")
}

#[test]
fn purge_deletes_each_entry_file_and_row() {
    // §6.3: a row with its file, a file with no row, and a row whose file is gone.
    let fx = three_entries();
    plant_file(&fx, "notes.txt");
    let ids = [NEWEST, MIDDLE, OLDEST].map(str::to_owned);
    assert_eq!(fx.engine.purge_displaced(&ids).unwrap(), ids);
    assert!(fx.engine.displaced().unwrap().entries.is_empty());
    assert!(
        fx.engine
            .store()
            .unwrap()
            .displaced_rows()
            .unwrap()
            .is_empty()
    );
    let left: Vec<_> = fs::read_dir(dir(&fx))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(left, ["notes.txt"], "only what was never an entry is left");
}

#[test]
fn a_repeated_id_is_deleted_once() {
    let fx = three_entries();
    let ids = [NEWEST, NEWEST].map(str::to_owned);
    assert_eq!(fx.engine.purge_displaced(&ids).unwrap(), [NEWEST]);
}

#[test]
fn an_id_that_names_no_entry_is_refused_and_nothing_is_deleted() {
    // Review Focus 3, Decision 12. An ID is checked before any path is built from it, and
    // every ID before anything is deleted, so a valid one alongside it is kept too. A file
    // named in another case exists, and is still no entry.
    let fx = three_entries();
    plant_file(&fx, "1790000300-0123456789AB-aaaaaa.json");
    let before = fx.engine.displaced().unwrap();
    for bad in [
        "../../tagteam.db",
        "x",
        "",
        "1790000300-0123456789AB-aaaaaa",
        "1790000300-0123456789ab-AAAAAA",
        "1790000999-0123456789ab-zzzzzz",
    ] {
        for ids in [[NEWEST, bad], [bad, NEWEST]] {
            let ids = ids.map(str::to_owned);
            let err = fx.engine.purge_displaced(&ids).unwrap_err();
            assert_eq!(err.kind(), "no-such-displaced", "{ids:?}");
            assert_eq!(
                err.to_string(),
                format!("no displaced credential matches {bad:?}; `tagteam displaced` lists them")
            );
        }
    }
    assert_eq!(
        fx.engine.displaced().unwrap(),
        before,
        "nothing was deleted"
    );
    assert!(
        dir(&fx)
            .join("1790000300-0123456789AB-aaaaaa.json")
            .exists()
    );
    assert!(fx.env.data_dir().join("tagteam.db").exists());
}

#[test]
fn purging_on_a_fresh_home_refuses_and_creates_nothing() {
    let fx = Fx::new();
    let err = fx.engine.purge_displaced(&[NEWEST.to_owned()]).unwrap_err();
    assert_eq!(err.kind(), "no-such-displaced");
    assert!(!fx.env.data_dir().exists());
}

#[test]
fn a_file_that_cannot_be_deleted_keeps_its_row() {
    // §6.3: the file goes first, verified gone, and only then the row. A credential still on
    // disk never loses the row that names it.
    let fx = three_entries();
    fs::set_permissions(dir(&fx), Permissions::from_mode(0o500)).unwrap();
    let result = fx.engine.purge_displaced(&[NEWEST.to_owned()]);
    fs::set_permissions(dir(&fx), Permissions::from_mode(0o700)).unwrap();
    assert_eq!(result.unwrap_err().kind(), "io");
    let newest = fx.engine.displaced().unwrap().entries.remove(0);
    assert_eq!(
        (newest.id.as_str(), newest.recorded, newest.file_present),
        (NEWEST, true, true)
    );
}

#[test]
fn a_purge_waits_for_the_displaced_lock() {
    // §6.3: each deletion holds the displaced lock, so a purge never deletes a file whose row
    // a writer is still to insert. Here another process holds the lock for 300 ms.
    let fx = three_entries();
    let held = hold_lock(&fx);
    let file = dir(&fx).join(format!("{NEWEST}.json"));
    let file_ref = &file;
    let present_while_held = thread::scope(|s| {
        let other = s.spawn(move || {
            thread::sleep(Duration::from_millis(300));
            let present = file_ref.exists();
            drop(held);
            present
        });
        assert_eq!(
            fx.engine.purge_displaced(&[NEWEST.to_owned()]).unwrap(),
            [NEWEST]
        );
        other.join().unwrap()
    });
    assert!(
        present_while_held,
        "the purge deleted the file under another process's lock"
    );
    assert!(!file.exists());
}

#[test]
fn the_writer_holds_the_displaced_lock_around_the_file_and_its_row() {
    // §6.3: a switch displacing the stray key waits for the lock before writing anything.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    fx.put_managed_key(STRAY_API_KEY.as_bytes());
    let held = hold_lock(&fx);
    let displaced = dir(&fx);
    let displaced_ref = &displaced;
    let written_while_held = thread::scope(|s| {
        let other = s.spawn(move || {
            thread::sleep(Duration::from_millis(300));
            let written = fs::read_dir(displaced_ref).map_or(0, |d| d.count());
            drop(held);
            written
        });
        fx.switch_to(&a, false).unwrap();
        other.join().unwrap()
    });
    assert_eq!(
        written_while_held, 0,
        "the file was written under another process's lock"
    );
    assert_eq!(fx.displaced(), [STRAY_API_KEY.as_bytes()]);
}

#[test]
fn the_listing_takes_no_lock() {
    // §6.3: it reads the store and the directory only. A reader that waited on the held lock
    // would time out after 5 s.
    let fx = three_entries();
    let _held = hold_lock(&fx);
    let started = Instant::now();
    assert_eq!(fx.engine.displaced().unwrap().entries.len(), 3);
    assert!(started.elapsed() < Duration::from_secs(1));
}
```

Append to `crates/tagteam-engine/tests/log_lines.rs` (Task 7's file, whose tests take turns):

```rust
#[test]
fn a_purge_logs_each_entry_it_deleted_by_its_id() {
    // §14.2 and Decision 10: a purge is a state change a user may need to reconstruct. Its
    // line names the entry's own ID, never the identity the entry was attributed to.
    let _serial = one_at_a_time();
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.login("stranger@x.co", "rt-s");
    fx.switch_to(&a, true).unwrap();
    let id = fx.engine.displaced().unwrap().entries[0].id.clone();
    let (_, logs) = capture_logs(|| fx.engine.purge_displaced(&[id.clone()]).unwrap());
    let line = one(&logs, "deleted a displaced credential");
    assert_eq!(field(line, "displaced"), Some(id.as_str()), "{line}");
    assert!(logs.iter().all(|l| !l.contains("stranger")), "{logs:#?}");
}
```

In `crates/tagteam-engine/src/error.rs`'s `kind_is_pinned_for_every_variant`, after
`(EngineError::NoSuchAccount("x".into()), "no-such-account"),`, insert:

```rust
            (
                EngineError::NoSuchDisplaced("x".into()),
                "no-such-displaced",
            ),
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-engine --test displaced`, `cargo test -p tagteam-engine --test log_lines`
and `cargo test -p tagteam-engine --lib error::tests`
Expected: compile errors:
- `unresolved import `tagteam_engine::displace::DISPLACED_LOCK``;
- `no method named `purge_displaced` found for struct `Engine``;
- `no variant or associated item named `NoSuchDisplaced` found for enum `EngineError``.

- [ ] **Step 3: Implement**

**`crates/tagteam-engine/src/error.rs`.** After the variant (line 63):

```rust
    #[error("no account matches {0:?}")]
    NoSuchAccount(String),
```

insert:

```rust
    /// §6.3: an ID `displaced --purge` was given that names no entry, or that is not a
    /// displaced ID at all (Decision 12).
    #[error("no displaced credential matches {0:?}; `tagteam displaced` lists them")]
    NoSuchDisplaced(String),
```

and in `kind`, after `EngineError::NoSuchAccount(_) => "no-such-account",`, insert:

```rust
            EngineError::NoSuchDisplaced(_) => "no-such-displaced",
```

**`crates/tagteam-engine/src/displace.rs`.** Replace the `std` and `tagteam_provider` imports:

```rust
use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;
use tagteam_core::{Fingerprint, ProviderId};
use tagteam_provider::Env;
use tagteam_provider::atomic::{ensure_private_dir, write_atomic_private};
```

with:

```rust
use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;
use tagteam_core::{Fingerprint, ProviderId};
use tagteam_provider::atomic::{ensure_private_dir, write_atomic_private};
use tagteam_provider::{Cancel, Env, FlockGuard};
```

After `pub struct DisplacedList { … }`, before `/// `^[0-9]{1,19}-…` (Decision 12).`, insert:

```rust
/// The displaced lock (§5, §6.3), relative to the data directory.
pub const DISPLACED_LOCK: &str = "locks/displaced.lock";
/// How long the writer and a purge wait for it (§6.3).
const DISPLACED_WAIT: Duration = Duration::from_secs(5);

/// Takes the displaced lock: a leaf `flock` (§4.3), held around one entry's file and row and
/// nothing else. Its wait is no cancellation point (Decision 11). `displace` runs inside the
/// switch's and the gate's critical spans (§14.1), so it waits on a fresh token that nothing
/// sets; a purge's deletion takes milliseconds.
fn lock_displaced(env: &Env) -> Result<FlockGuard, EngineError> {
    Ok(FlockGuard::lock(
        &env.data_dir().join(DISPLACED_LOCK),
        DISPLACED_WAIT,
        &Cancel::new(),
    )?)
}

/// Deletes `path` itself, never what a symlink there points to, and verifies that it is gone.
/// An absent path counts as deleted.
fn remove_verified(path: &Path) -> Result<(), EngineError> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e.into()),
        _ => {}
    }
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
        Ok(_) => Err(EngineError::Io(io::Error::other(format!(
            "{} is still there after it was deleted",
            path.display()
        )))),
    }
}

```

Inside `impl Engine`, after `displaced_account`'s closing brace, insert:

```rust

    /// The entries `ids` names, each once, in the order given (§6.3). Every ID is checked
    /// before anything is deleted. Each must be a displaced ID (Decision 12), which is checked
    /// before any path is built from it, and must name a file or a row. All the forms are
    /// checked before any presence. The first ID that fails is `NoSuchDisplaced`. This reads
    /// only, under no lock, and creates nothing (§5).
    pub fn known_displaced(&self, ids: &[String]) -> Result<Vec<String>, EngineError> {
        if let Some(bad) = ids.iter().find(|id| !is_displaced_id(id)) {
            return Err(EngineError::NoSuchDisplaced(bad.clone()));
        }
        let mut present = displaced_files(&displaced_dir(&self.env))?;
        if let Some(store) = self.existing_store()? {
            present.extend(store.displaced_rows()?.into_iter().map(|r| r.id));
        }
        let mut known: Vec<String> = Vec::with_capacity(ids.len());
        for id in ids {
            if !present.contains(id) {
                return Err(EngineError::NoSuchDisplaced(id.clone()));
            }
            if !known.contains(id) {
                known.push(id.clone());
            }
        }
        Ok(known)
    }

    /// `displaced --purge` (§6.3): deletes the entries `ids` names, once each, after
    /// `known_displaced` has checked every one, so an unknown ID deletes nothing. Each
    /// deletion holds the displaced lock: the file first, verified gone, then the row. Returns
    /// the IDs, each once, in the order given.
    pub fn purge_displaced(&self, ids: &[String]) -> Result<Vec<String>, EngineError> {
        let ids = self.known_displaced(ids)?;
        let dir = displaced_dir(&self.env);
        for id in &ids {
            {
                let _lock = lock_displaced(&self.env)?;
                remove_verified(&dir.join(format!("{id}.json")))?;
                // The store is opened under the lock. A writer that created the store while
                // this purge waited has finished its row by then.
                if let Some(store) = self.existing_store()? {
                    store.delete_displaced(id)?;
                }
            }
            // Once the lock is released (§4.3): the entry's own ID (§14.2).
            tracing::info!(displaced = %id, "deleted a displaced credential");
        }
        Ok(ids)
    }
```

Checkpoint: run `cargo test -p tagteam-engine --test displaced`. Every test passes except
`the_writer_holds_the_displaced_lock_around_the_file_and_its_row`. It fails with
`assertion `left == right` failed: the file was written under another process's lock`,
`left: 1`, `right: 0`, because the writer does not take the lock yet.

Now replace `displace` (from its doc comment `/// Stashes live credential bytes that are about to
be overwritten (§6.3).` to its closing brace) with:

```rust
/// Stashes live credential bytes that are about to be overwritten (§6.3). Forensic and
/// write-only: always a plain 0600 file, never a Keychain item. The file is written first and
/// its row second, so a failed insert leaves a file with no row, never a row that names nothing.
/// Both are written under the displaced lock, so a purge never deletes a file whose row is
/// still to be inserted. The lock is a leaf (§4.3): the row is a SQLite write, and nothing else
/// is taken while the lock is held, so the entry is logged once it is released.
pub(crate) fn displace(
    engine: &Engine,
    provider: &ProviderId,
    bytes: &[u8],
    fp: Option<&Fingerprint>,
    reason: &str,
    identity: Option<&Value>,
) -> Result<String, EngineError> {
    let env = engine.env();
    let dir = displaced_dir(env);
    let now = engine.now_ms();
    let id = new_id(now, fp);
    {
        let _lock = lock_displaced(env)?;
        ensure_private_dir(&dir)?;
        write_atomic_private(&dir.join(format!("{id}.json")), bytes, 0o600)?;
        engine.store()?.insert_displaced(&DisplacedRow {
            id: id.clone(),
            provider: provider.clone(),
            at: now,
            reason: reason.to_owned(),
            fingerprint: fp.map(|f| f.as_str().to_owned()).unwrap_or_default(),
            identity: identity.cloned(),
        })?;
    }
    // The entry's own ID, never the identity it was attributed to (§14.2).
    tracing::info!(
        provider = %provider,
        displaced = %id,
        reason,
        "saved a credential to displaced/"
    );
    Ok(id)
}
```

- [ ] **Step 4: Run them and see them pass**

Run: `cargo test -p tagteam-engine --test displaced`, `cargo test -p tagteam-engine --test log_lines`
and `cargo test -p tagteam-engine --lib`. Expected: PASS. `log_lines`' displacement test still
finds `displace`'s line, now logged after the lock.

Run: `cargo test -p tagteam-engine --features test-hooks`. Expected: PASS. Every displacement
in the switch, gate and recovery suites now takes and releases the lock. Each fixture has its
own data directory, so tests running in parallel never contend for it.

- [ ] **Step 5: Format, lint, commit**

```
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p tagteam-engine --features test-hooks
git add crates/tagteam-engine/src/displace.rs crates/tagteam-engine/src/error.rs \
  crates/tagteam-engine/tests/displaced.rs crates/tagteam-engine/tests/log_lines.rs
git commit -m "Purge displaced credentials under the displaced lock, which the writer now holds too"
```

---

#### Part D: a displaced row names only the live login's own secret (L467)

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-engine/tests/displaced.rs`, replace `use common::{Fx, STRAY_API_KEY};` with:

```rust
use common::{API_KEY, Fx, STRAY_API_KEY, crashed_switch, write_target_credential};
```

insert after `use serde_json::{Value, json};`:

```rust
use tagteam_cc::ItemKind;
```

replace `use tagteam_provider::FlockGuard;` with:

```rust
use tagteam_provider::{FlockGuard, Keychain};
```

and append at the end of the file:

```rust
/// Every entry with its file's bytes, newest first.
fn with_bytes(fx: &Fx) -> Vec<(DisplacedEntry, Vec<u8>)> {
    let DisplacedList { dir, entries } = fx.engine.displaced().unwrap();
    entries
        .into_iter()
        .map(|e| {
            let bytes = fs::read(dir.join(format!("{}.json", e.id))).unwrap();
            (e, bytes)
        })
        .collect()
}

#[test]
fn a_stray_key_a_switch_clears_is_displaced_with_no_identity() {
    // §6.3, Decision 13. The managed key sits beside b's OAuth login, on the other auth axis
    // (§9.4 step 7). It is not b's, so its row names no identity, and so no account.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b"); // live: b
    fx.put_managed_key(STRAY_API_KEY.as_bytes());
    fx.switch_to(&a, false).unwrap();
    let saved = with_bytes(&fx);
    assert_eq!(saved.len(), 1);
    let (entry, bytes) = &saved[0];
    assert_eq!(bytes.as_slice(), STRAY_API_KEY.as_bytes());
    assert_eq!((entry.identity.as_ref(), entry.account), (None, None));
    assert_eq!(entry.reason.as_deref(), Some("displaced-live-login"));
}

#[test]
fn a_forced_switch_names_the_live_login_on_its_own_secret_only() {
    // §9.4 step 2 displaces both axes. The stranger's OAuth credential is the live login's own,
    // so its row keeps the live identity. The key beside it gets none.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.login("stranger@x.co", "rt-s");
    fx.put_managed_key(STRAY_API_KEY.as_bytes());
    fx.switch_to(&a, true).unwrap();
    let saved = with_bytes(&fx);
    assert_eq!(saved.len(), 2);
    let identity_of = |secret: &[u8]| {
        saved
            .iter()
            .find(|(_, bytes)| bytes.as_slice() == secret)
            .map(|(e, _)| e.identity.clone())
            .expect("displaced")
    };
    let login = Fx::credential_json("stranger@x.co", "rt-s")
        .to_string()
        .into_bytes();
    assert_eq!(
        identity_of(&login),
        Some(Fx::oauth_account("stranger@x.co"))
    );
    assert_eq!(identity_of(STRAY_API_KEY.as_bytes()), None);
    assert!(
        saved.iter().all(|(e, _)| {
            e.account.is_none() && e.reason.as_deref() == Some("forced-activation")
        })
    );
}

#[test]
fn forward_recovery_displaces_a_stray_key_with_no_identity() {
    // §9.6 applies §9.4 step 7's rule before clearing the other axis. The key written between
    // the journal row and the crash is saved, attributed to no one.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    crashed_switch(&fx, &b, &a);
    write_target_credential(&fx, &a);
    fx.put_managed_key(STRAY_API_KEY.as_bytes());
    drop(fx.engine.mutation_guard().unwrap());
    let saved = with_bytes(&fx);
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].1.as_slice(), STRAY_API_KEY.as_bytes());
    assert_eq!(saved[0].0.identity, None);
}

#[test]
fn a_stray_login_beside_an_api_key_account_whose_key_is_gone_gets_no_identity() {
    // Decision 13. The live account is an API-key account, so its own secret is on the
    // managed-key axis, and that key is gone. The OAuth login in the credential entry is not
    // its own: its row names no identity, never the API-key account's. A forced switch saves
    // it at §9.4 step 2, an unforced one at step 7.
    for (force, reason) in [(true, "forced-activation"), (false, "displaced-live-login")] {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let k = fx.add_api_key(API_KEY);
        fx.switch_to(&k, false).unwrap();
        let (svc, acct) = fx.live_item(ItemKind::ManagedKey);
        fx.kc.delete(&svc, &acct).unwrap();
        let stray = Fx::credential_json("stray@x.co", "rt-stray")
            .to_string()
            .into_bytes();
        fx.set_live_credential(&stray);
        fx.switch_to(&a, force).unwrap();
        let saved = with_bytes(&fx);
        assert_eq!(saved.len(), 1, "force={force}");
        let (entry, bytes) = &saved[0];
        assert_eq!(bytes, &stray, "force={force}");
        assert_eq!(
            (entry.identity.as_ref(), entry.account),
            (None, None),
            "force={force}"
        );
        assert_eq!(entry.reason.as_deref(), Some(reason));
    }
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-engine --test displaced`
Expected: four FAILs. Each names the outgoing or live login where it should name no one:
- `a_stray_key_a_switch_clears_is_displaced_with_no_identity`: `left: (Some(Object
  {"emailAddress": String("b@x.co"), …}), Some(2))`, `right: (None, None)`. The stray key is
  even mapped to b's position.
- `a_forced_switch_names_the_live_login_on_its_own_secret_only`: on
  `identity_of(STRAY_API_KEY.as_bytes())`, `left: Some(Object {"emailAddress":
  String("stranger@x.co"), …})`, `right: None`.
- `forward_recovery_displaces_a_stray_key_with_no_identity`: `left: Some(Object
  {"emailAddress": String("b@x.co"), …})`, `right: None`.
- `a_stray_login_beside_an_api_key_account_whose_key_is_gone_gets_no_identity`: `force=true`,
  `left: (Some(Object {"emailAddress": String("api-key-2@token.local"), …}), Some(2))`,
  `right: (None, None)`. The stray OAuth login is filed under the API-key account, at its
  position.

- [ ] **Step 3: Implement**

**`crates/tagteam-engine/src/switch.rs`.** After `fn unless_forced` (it ends at line 243, before
`/// What a row says about a login,`), insert:

```rust

/// §6.3, Decision 13: the live identity names only the live login's own secret,
/// `own_secret`. A secret on the other auth axis is never attributed to the login beside it,
/// so its row names no identity.
fn attributed<'i>(
    live_identity: Option<&'i Identity>,
    own_secret: Option<&[u8]>,
    bytes: &[u8],
) -> Option<&'i Identity> {
    live_identity.filter(|_| own_secret == Some(bytes))
}
```

In `transact`, immediately before the comment
`// What steps 2 and 4 settle, and the vaults below: step 7's rule never saves it again.`
(line 1125), insert:

```rust
        // The live login's own secret, the only one the live identity names when a secret is
        // displaced (§6.3, Decision 13): the live secret on a stored account's own axis, or the
        // credential entry's for an unmanaged login, which `oauthAccount` names. Unlike step 6's
        // `from_secret`, it never falls back to the other axis.
        let own_secret = match &outgoing {
            Some(o) => Axis::of(p, &o.kind).live_secret(&live),
            None => Axis::Entry.live_secret(&live),
        };
```

In the direct branch, replace:

```rust
                        let saved = self.displace_live(
                            p,
                            provider,
                            &bytes,
                            reason,
                            live_identity.as_ref(),
                            &mut warnings,
                        );
```

with:

```rust
                        let identity =
                            attributed(live_identity.as_ref(), own_secret.as_deref(), &bytes);
                        let saved = self.displace_live(
                            p,
                            provider,
                            &bytes,
                            reason,
                            identity,
                            &mut warnings,
                        );
```

In the step-7 loop, replace:

```rust
                self.save_unheld(
                    p,
                    provider,
                    bytes,
                    &mut held,
                    req.force,
                    live_identity.as_ref(),
                    &mut warnings,
                )?;
```

with:

```rust
                self.save_unheld(
                    p,
                    provider,
                    bytes,
                    &mut held,
                    req.force,
                    live_identity.as_ref(),
                    own_secret.as_deref(),
                    &mut warnings,
                )?;
```

In the `before_fallback` closure, replace:

```rust
            self.save_unheld(
                p,
                provider,
                bytes,
                &mut held,
                req.force,
                live_identity.as_ref(),
                &mut warnings,
            )
            .map_err(|e| {
```

with:

```rust
            self.save_unheld(
                p,
                provider,
                bytes,
                &mut held,
                req.force,
                live_identity.as_ref(),
                own_secret.as_deref(),
                &mut warnings,
            )
            .map_err(|e| {
```

Step 6 and its `from_secret` stay as they are: the journal's `from_fp` keeps the fallback to the
entry, which §9.6 recovery reads.

Replace `save_unheld` (from its doc comment `/// §9.4 step 7: saves `bytes`, a live entry a change
is about to overwrite or delete,` to its closing brace) with:

```rust
    /// §9.4 step 7: saves `bytes`, a live entry a change is about to overwrite or delete, unless
    /// it holds nothing account-scoped or a generation `held` already keeps. Either way the
    /// generation is held from then on, so no entry of it is saved twice. A failed save aborts,
    /// except under `force` (B.5). The row names `live_identity` only when `bytes` is
    /// `own_secret`, the live login's own (§6.3, Decision 13). A secret on the other auth axis
    /// gets no identity.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn save_unheld(
        &self,
        p: &dyn Provider,
        provider: &ProviderId,
        bytes: &[u8],
        held: &mut Held,
        force: bool,
        live_identity: Option<&Identity>,
        own_secret: Option<&[u8]>,
        warnings: &mut Vec<String>,
    ) -> Result<(), EngineError> {
        match p.fingerprint(bytes) {
            Some(fp) if held.insert(fp.as_str()) => {
                let saved = self.displace_live(
                    p,
                    provider,
                    bytes,
                    "displaced-live-login",
                    attributed(live_identity, own_secret, bytes),
                    warnings,
                );
                unless_forced(saved, force, warnings)
            }
            _ => Ok(()),
        }
    }
```

**`crates/tagteam-engine/src/active.rs`.** `publish` names no identity, and so has no own secret.
Both of its `save_unheld` calls pass `None` for each. Replace:

```rust
                if let Err(e) = self.save_unheld(
                    p,
                    &row.provider,
                    bytes,
                    &mut held,
                    false,
                    None,
                    &mut warnings,
                ) {
```

with:

```rust
                if let Err(e) = self.save_unheld(
                    p,
                    &row.provider,
                    bytes,
                    &mut held,
                    false,
                    None,
                    None,
                    &mut warnings,
                ) {
```

and replace:

```rust
                self.save_unheld(
                    p,
                    &row.provider,
                    bytes,
                    &mut held,
                    false,
                    None,
                    &mut warnings,
                )
```

with:

```rust
                self.save_unheld(
                    p,
                    &row.provider,
                    bytes,
                    &mut held,
                    false,
                    None,
                    None,
                    &mut warnings,
                )
```

**`crates/tagteam-engine/src/recover.rs`.** In `finish_forward`, delete the line (229):

```rust
        let live_identity = p.live_identity(&self.env).present();
```

and replace the call:

```rust
            self.save_unheld(
                p,
                &row.provider,
                bytes,
                &mut held,
                false,
                live_identity.as_ref(),
                &mut warnings,
            )?;
```

with:

```rust
            // Every entry cleared here sits on the other auth axis, and §9.6 never takes the
            // live identity as evidence of whose it is: attributed to no one (§6.3).
            self.save_unheld(
                p,
                &row.provider,
                bytes,
                &mut held,
                false,
                None,
                None,
                &mut warnings,
            )?;
```

- [ ] **Step 4: Run them and see them pass**

Run: `cargo test -p tagteam-engine --test displaced`. Expected: PASS.

Run: `cargo test -p tagteam-engine --features test-hooks`. Expected: PASS. No existing test reads
a displaced row's identity. `switch.rs`, `recover.rs`, `destroyed.rs` and `active.rs` compare
file contents and counts, which do not change.

- [ ] **Step 5: Format, lint, commit**

```
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p tagteam-engine --features test-hooks
git add crates/tagteam-engine/src/switch.rs crates/tagteam-engine/src/active.rs \
  crates/tagteam-engine/src/recover.rs crates/tagteam-engine/tests/displaced.rs
git commit -m "Name the live identity on a displaced row only for the live login's own secret"
```

---

### Task 9: `tagteam displaced`

> **Re-sync (M3a, M4a, other M5a tasks).**
> - **The `Command` variant.** `Command::Displaced` goes after Task 3's `Command::Config`, which
>   follows `Command::Statusline`; Task 10's `Completions` follows it. M3a Task 10 changes
>   `Switch`, which sits earlier.
> - **`mod` and `use` lines.** `lib.rs`'s `mod displaced_cmd;` goes after Task 3's
>   `mod config_cmd;`, and `app.rs`'s `use crate::{…}` line gains `displaced_cmd` after Task 3's
>   `config_cmd`.
> - **M3a Task 6 adds `fn command_name`,** an exhaustive `match` over `Command`. Add the arm
>   `Command::Displaced { .. } => "displaced",` (`Command::Displaced => "displaced",` after Part
>   A).
> - **M3a Task 6 makes a prompt a cancellation point.** In Part B's `App::displaced`, replace
>   ```rust
>             if !self.io.prompter.confirm(&question, false) {
>                 return Err(cancelled());
>             }
>   ```
>   with
>   ```rust
>             let confirmed = self.io.prompter.confirm(&question, false);
>             self.after_prompt()?;
>             if !confirmed {
>                 return Err(cancelled());
>             }
>   ```
> - **`KIND_NEEDS_CONFIRMATION`** goes next to M3a's `KIND_INTERRUPTED`, after
>   `KIND_UNSUPPORTED`.
> - **M4a Task 8** restructures `run`. `dispatch` and the `App` methods are unchanged, and the
>   engine methods refuse nothing in a run shell.

§6.3: "**`tagteam displaced [--json]`** lists the entries, newest first, joining rows and files:
the ID, provider, time, reason, identity (and the position of the managed account it names, if
any) and the fingerprint's first 12 hex digits. A file with no row is listed as `unrecorded`, with
the time its name carries; a row whose file is gone is listed as `file missing`. It never prints a
credential … `identity` and `account` are null when unknown; `file` is `present` or `missing`.
**`tagteam displaced --purge ID... [--yes]`** deletes each named entry … Every ID is checked
before anything is deleted; an unknown one is an error that names it. It asks for confirmation on
a terminal. Without a terminal, or with `--json`, it requires `--yes` and otherwise fails with
`needs-confirmation`." §13.1 lists `displaced [--purge ID... [--yes]]`. This task adds the
command and renders it in a new `displaced_cmd` module. The listing comes first (Part A), and
the purge with its confirmation second (Part B). Part C extends Task 7's redaction pin to `displaced` and its purge.

**Readings of the spec this task commits to:**
- **WHEN is UTC ISO 8601 to the second,** the same string as the JSON's `at`. This differs from
  `list`'s relative AGE.
  - An entry is a forensic record, and restoring one starts with matching it to the switch that
    made it, in the log. The log is stamped in UTC (§14.2).
  - An age keeps moving, and loses its precision for an entry months old.
  - A local time would need a time-zone library tagteam does not carry.
- **The columns are ID, WHEN, PROVIDER (the provider ID), REASON, IDENTITY, ACCOUNT (`#3`) and
  FP.**
  - IDENTITY is the email, with ` [org]` when the identity names an organization, as `list`
    names an account. It is read through the entry's provider, so the CLI names no identity
    field. `unknown` when there is none.
  - A missing value is `—`, `render::MISSING`.
  - FP comes from the recorded fingerprint, so a file with no row shows `—` there, even though
    its ID carries the same 12 digits.
  - The state (`unrecorded`, `file missing`) trails the row, after the last column, as `list`
    appends its notes. A row whose file is gone still records who the credential belonged to,
    and the IDENTITY cell keeps showing it. §6.3's "listed as `unrecorded`" and "listed as
    `file missing`" read either way.
- **"The listing names the file":** each file is `<dir>/<ID>.json`. The footer names the
  directory and that pattern, and each row names its ID.
- **`--provider` does not narrow the listing.** §6.3 lists every entry, and §13.1's narrowing
  names only `list`, `status` and `doctor`. An unregistered `--provider` still fails, as it does
  for every command.
- **`--yes` without `--purge` is a usage error (exit 2), raised by the app.** clap's `requires`
  would give the same exit code, but the CLI's argument scrubbing (`without_argument_values`)
  would drop `--purge`'s name from that message.
- **The question counts the distinct entries and pluralizes:** "Delete 1 displaced credential?
  It cannot be recovered." and "Delete 3 displaced credentials? They cannot be recovered."
  The default is no. A no, or an end of input, is `cancelled` (exit 1), as for every declined
  confirmation.
- **Under `--json`, `--yes` is required even on a terminal.**
- **`displaced` touches no Keychain item,** so it runs no lock check (`touches_keychain` is
  unchanged).

**Files:**
- Modify: `crates/tagteam/src/cli.rs` (`Command::Displaced`, after Task 3's `Command::Config`)
- Create: `crates/tagteam/src/displaced_cmd.rs`
- Modify: `crates/tagteam/src/lib.rs` (`mod displaced_cmd;` after Task 3's `mod config_cmd;`)
- Modify: `crates/tagteam/src/render.rs` (`width` and `pad` become `pub(crate)`, 205–211)
- Modify: `crates/tagteam/src/app.rs`:
  - the `crate::` import, line 28 (Task 3's);
  - consts, 44 and 56;
  - `dispatch`, after Task 3's `Command::Config` arm;
  - new `App::displaced` after `history`, 868.
- Test: `crates/tagteam/src/displaced_cmd.rs` (unit), `crates/tagteam/tests/displaced_cli.rs`
  (new), `crates/tagteam/tests/app.rs`, `crates/tagteam/tests/logging.rs` (Task 7's redaction pin;
  Part C)

**Interfaces:**
- Consumes:
  - Task 8:
    - `tagteam_engine::displace::{DisplacedEntry, DisplacedList}`
    - `Engine::displaced(&self) -> Result<DisplacedList, EngineError>`
    - `Engine::known_displaced(&self, &[String]) -> Result<Vec<String>, EngineError>`
    - `Engine::purge_displaced(&self, &[String]) -> Result<Vec<String>, EngineError>`
    - `EngineError::NoSuchDisplaced` (`no-such-displaced`)
  - Existing:
    - `Prompter::confirm(&mut self, &str, default_yes: bool) -> bool`
    - `App::{can_prompt, print}`, `cancelled()`, `Failure::{Usage, Message}`
    - `render::MISSING`
    - `tagteam_cc::usage::format_iso8601(i64) -> String`
    - `Engine::provider`, `Provider::parse_identity`
- Produces:
  - `cli::Command::Displaced { purge: Vec<String>, yes: bool }`
  - `displaced_cmd::human(list: &DisplacedList, identity: &dyn Fn(&DisplacedEntry) -> Option<String>) -> String`
  - `displaced_cmd::json(list: &DisplacedList) -> Value`
  - `displaced_cmd::identity(engine: &Engine, e: &DisplacedEntry) -> Option<String>`
  - `displaced_cmd::purge_question(n: usize) -> String`
  - `displaced_cmd::purged_human(deleted: &[String]) -> String`
  - `displaced_cmd::purged_json(deleted: &[String]) -> Value`
  - `render::width(&str) -> usize` and `render::pad(&str, usize) -> String`, now `pub(crate)`
  - in `app.rs`: `const KIND_NEEDS_CONFIRMATION: &str = "needs-confirmation"`, plus the
    messages `YES_WITHOUT_PURGE` and `PURGE_NEEDS_YES`

---

#### Part A: the listing

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam/src/lib.rs`, after Task 3's `mod config_cmd;`, add `mod displaced_cmd;`. Create
`crates/tagteam/src/displaced_cmd.rs` holding only this test module for now. Step 3 writes the
code above it.

```rust
#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;
    use tagteam_core::ProviderId;

    use super::*;

    const NEWEST: &str = "1790000300-0123456789ab-aaaaaa";
    const MIDDLE: &str = "1790000250-fedcba987654-bbbbbb";
    const OLDEST: &str = "1790000200-00112233aabb-cccccc";

    /// The fingerprint a recorded entry carries: its ID's 12 hex digits, then zeros.
    fn fingerprint(id: &str) -> String {
        format!("sha256:{}{}", &id[11..23], "0".repeat(52))
    }

    fn recorded(
        id: &str,
        at_ms: i64,
        email: &str,
        account: Option<u32>,
        file_present: bool,
    ) -> DisplacedEntry {
        DisplacedEntry {
            id: id.into(),
            provider: Some(ProviderId::new("claude-code")),
            at_ms,
            reason: Some("displaced-live-login".into()),
            fingerprint: Some(fingerprint(id)),
            identity: Some(json!({"emailAddress": email})),
            account,
            file_present,
            recorded: true,
        }
    }

    /// A row and its file naming b (#2), a file with no row, and a row whose file is gone.
    fn sample() -> DisplacedList {
        DisplacedList {
            dir: PathBuf::from("/h/.local/share/tagteam/displaced"),
            entries: vec![
                recorded(NEWEST, 1_790_000_300_500, "b@x.co", Some(2), true),
                DisplacedEntry {
                    id: MIDDLE.into(),
                    provider: None,
                    at_ms: 1_790_000_250_000,
                    reason: None,
                    fingerprint: None,
                    identity: None,
                    account: None,
                    file_present: true,
                    recorded: false,
                },
                recorded(OLDEST, 1_790_000_200_000, "stranger@x.co", None, false),
            ],
        }
    }

    /// The identity's email: what the app's provider-backed `identity` reads from a CC identity.
    fn email(e: &DisplacedEntry) -> Option<String> {
        e.identity.as_ref()?["emailAddress"]
            .as_str()
            .map(str::to_owned)
    }

    #[test]
    fn the_listing_is_a_table_newest_first_then_the_directory() {
        assert_eq!(
            human(&sample(), &email),
            concat!(
                "ID                              WHEN                  PROVIDER     REASON                IDENTITY       ACCOUNT  FP\n",
                "1790000300-0123456789ab-aaaaaa  2026-09-21T14:18:20Z  claude-code  displaced-live-login  b@x.co         #2       0123456789ab\n",
                "1790000250-fedcba987654-bbbbbb  2026-09-21T14:17:30Z  —            —                     unknown        —        —             unrecorded\n",
                "1790000200-00112233aabb-cccccc  2026-09-21T14:16:40Z  claude-code  displaced-live-login  stranger@x.co  —        00112233aabb  file missing\n",
                "\n",
                "The files are /h/.local/share/tagteam/displaced/<ID>.json; tagteam never reads one back, so restoring one is manual.\n",
            )
        );
    }

    #[test]
    fn the_json_is_section_6_3_s_object() {
        assert_eq!(
            json(&sample()),
            json!({
                "schemaVersion": 1,
                "dir": "/h/.local/share/tagteam/displaced",
                "displaced": [
                    {"id": NEWEST, "provider": "claude-code", "at": "2026-09-21T14:18:20Z",
                     "reason": "displaced-live-login", "fingerprint": fingerprint(NEWEST),
                     "identity": {"emailAddress": "b@x.co"}, "account": 2, "file": "present",
                     "recorded": true},
                    {"id": MIDDLE, "provider": null, "at": "2026-09-21T14:17:30Z", "reason": null,
                     "fingerprint": null, "identity": null, "account": null, "file": "present",
                     "recorded": false},
                    {"id": OLDEST, "provider": "claude-code", "at": "2026-09-21T14:16:40Z",
                     "reason": "displaced-live-login", "fingerprint": fingerprint(OLDEST),
                     "identity": {"emailAddress": "stranger@x.co"}, "account": null,
                     "file": "missing", "recorded": true},
                ]
            })
        );
        // In §6.3's field order, which `preserve_order` keeps.
        let text = json(&sample()).to_string();
        assert!(
            text.starts_with(&format!(
                r#"{{"schemaVersion":1,"dir":"/h/.local/share/tagteam/displaced","displaced":[{{"id":"{NEWEST}","provider":"claude-code","at":"#
            )),
            "{text}"
        );
    }

    #[test]
    fn an_empty_listing_says_so() {
        let empty = DisplacedList {
            dir: PathBuf::from("/d"),
            entries: Vec::new(),
        };
        assert_eq!(human(&empty, &email), "No displaced credentials.\n");
        assert_eq!(
            json(&empty),
            json!({"schemaVersion": 1, "dir": "/d", "displaced": []})
        );
    }
}
```

Create `crates/tagteam/tests/displaced_cli.rs`:

```rust
//! `tagteam displaced` through the binary (§6.3), with Review Focus 3. Needs
//! `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{cmd, login, seed_home};
use serde_json::{Value, json};
use tagteam_cc::usage::format_iso8601;
use tagteam_provider::{Env, FileKeychain};

fn displaced_dir(root: &Path) -> PathBuf {
    Env::for_test(root).data_dir().join("displaced")
}

fn listing(root: &Path) -> Value {
    let out = cmd(root)
        .args(["displaced", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    serde_json::from_slice(&out).unwrap()
}

/// `a@x.co` stored at position 1, then a stranger's login that `switch 1 --force` displaced
/// (§9.4 step 2). This gives one real entry, as a user meets one. Returns its ID.
fn forced_over_a_stranger(root: &Path) -> String {
    let env = Env::for_test(root);
    let kc = FileKeychain::new(root.join("keychain"));
    seed_home(&env);
    login(&env, &kc, "a@x.co", "", "rt-a");
    cmd(root).arg("add").assert().success();
    login(&env, &kc, "stranger@x.co", "", "rt-s");
    cmd(root)
        .args(["switch", "1", "--force"])
        .assert()
        .success();
    listing(root)["displaced"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// The second an ID carries, as the listing prints it.
fn when(id: &str) -> String {
    format_iso8601(id.split('-').next().unwrap().parse().unwrap())
}

/// The fingerprint's 12 hex digits an ID carries.
fn fp12(id: &str) -> &str {
    id.split('-').nth(1).unwrap()
}

#[test]
fn the_listing_joins_the_row_and_its_file() {
    let d = tempfile::tempdir().unwrap();
    let id = forced_over_a_stranger(d.path());
    let v = listing(d.path());
    let fingerprint = v["displaced"][0]["fingerprint"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        fingerprint.starts_with(&format!("sha256:{}", fp12(&id))),
        "{fingerprint}"
    );
    assert_eq!(
        v,
        json!({
            "schemaVersion": 1,
            "dir": displaced_dir(d.path()).to_str().unwrap(),
            "displaced": [{
                "id": id, "provider": "claude-code", "at": when(&id), "reason": "forced-activation",
                "fingerprint": fingerprint,
                "identity": {"emailAddress": "stranger@x.co", "organizationUuid": "",
                             "accountUuid": "uuid-stranger@x.co-"},
                "account": null, "file": "present", "recorded": true,
            }]
        })
    );
    assert!(displaced_dir(d.path()).join(format!("{id}.json")).is_file());
}

#[test]
fn the_human_listing_names_each_column_and_the_directory() {
    let d = tempfile::tempdir().unwrap();
    let id = forced_over_a_stranger(d.path());
    let out = cmd(d.path())
        .arg("displaced")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        String::from_utf8(out).unwrap(),
        format!(
            "ID                              WHEN                  PROVIDER     REASON             IDENTITY       ACCOUNT  FP\n\
             {id}  {}  claude-code  forced-activation  stranger@x.co  —        {}\n\
             \n\
             The files are {}/<ID>.json; tagteam never reads one back, so restoring one is manual.\n",
            when(&id),
            fp12(&id),
            displaced_dir(d.path()).display()
        )
    );
}

#[test]
fn a_fresh_machine_lists_nothing_and_creates_nothing() {
    let d = tempfile::tempdir().unwrap();
    let home = d.path().join("home");
    fs::create_dir_all(&home).unwrap();
    cmd(d.path())
        .arg("displaced")
        .assert()
        .success()
        .stdout("No displaced credentials.\n");
    assert_eq!(
        listing(d.path()),
        json!({"schemaVersion": 1, "dir": displaced_dir(d.path()).to_str().unwrap(), "displaced": []})
    );
    assert!(
        fs::read_dir(&home).unwrap().next().is_none(),
        "HOME must stay empty"
    );
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam --lib displaced_cmd`
Expected: compile errors in `displaced_cmd::tests`: `cannot find type `DisplacedEntry` in this
scope`, `cannot find type `DisplacedList` in this scope`, and `cannot find function `human` in
this scope`.

Run: `cargo test -p tagteam --features test-support --test displaced_cli`
Expected: all three tests FAIL with `Unexpected failure` and `code=2`. clap rejects `displaced`
with `error: unrecognized subcommand`; the argument scrubbing drops the name. The module's compile
errors do not block this run, because integration tests build the library without its unit
tests.

- [ ] **Step 3: Implement**

**`crates/tagteam/src/render.rs`.** Replace `fn width(s: &str) -> usize {` with
`pub(crate) fn width(s: &str) -> usize {` and `fn pad(s: &str, w: usize) -> String {` with
`pub(crate) fn pad(s: &str, w: usize) -> String {` (lines 205 and 209).

**`crates/tagteam/src/displaced_cmd.rs`.** Above the test module, write:

```rust
//! `tagteam displaced` (§6.3): the listing, for a person and as JSON. It never prints a
//! credential.

use serde_json::{Value, json};
use tagteam_cc::usage::format_iso8601;
use tagteam_core::ProviderId;
use tagteam_engine::Engine;
use tagteam_engine::displace::{DisplacedEntry, DisplacedList};

use crate::render::{self, MISSING};

const EMPTY: &str = "No displaced credentials.\n";
const HEADS: [&str; 7] = [
    "ID", "WHEN", "PROVIDER", "REASON", "IDENTITY", "ACCOUNT", "FP",
];
/// An entry that names no identity, or one this build cannot read.
const UNKNOWN: &str = "unknown";
/// A file with no row, and a row with no file (§6.3).
const UNRECORDED: &str = "unrecorded";
const FILE_MISSING: &str = "file missing";

/// When an entry was displaced, in UTC to the second. This is the JSON's `at`, on the log's
/// clock (§14.2), so an entry can be matched to the switch that made it.
fn when(e: &DisplacedEntry) -> String {
    format_iso8601(e.at_ms.div_euclid(1000))
}

/// A fingerprint's first 12 hex digits (§6.3), as file names carry them.
fn fp12(fingerprint: &str) -> &str {
    let hex = fingerprint.strip_prefix("sha256:").unwrap_or(fingerprint);
    hex.get(..12).unwrap_or(hex)
}

/// The identity `e` names, as `list` names an account: its email, with its organization in
/// brackets. It is read through the entry's provider, so the CLI names no provider's identity
/// fields. `None` when the entry names no identity, or none this build can read.
pub(crate) fn identity(engine: &Engine, e: &DisplacedEntry) -> Option<String> {
    let provider = engine.provider(e.provider.as_ref()?).ok()?;
    let identity = provider.parse_identity(e.identity.as_ref()?).ok()?;
    let email = identity.email.unwrap_or(identity.label);
    Some(match identity.org_name {
        Some(org) => format!("{email} [{org}]"),
        None => email,
    })
}

/// One entry's cells, in `HEADS`' order, and its state when it has no row or no file.
fn cells(
    e: &DisplacedEntry,
    identity: &dyn Fn(&DisplacedEntry) -> Option<String>,
) -> ([String; 7], Option<&'static str>) {
    let or_missing = |s: Option<String>| s.unwrap_or_else(|| MISSING.to_owned());
    let cells = [
        e.id.clone(),
        when(e),
        or_missing(e.provider.as_ref().map(|p| p.as_str().to_owned())),
        or_missing(e.reason.clone()),
        identity(e).unwrap_or_else(|| UNKNOWN.to_owned()),
        or_missing(e.account.map(|n| format!("#{n}"))),
        or_missing(e.fingerprint.as_deref().map(|f| fp12(f).to_owned())),
    ];
    let state = if !e.recorded {
        Some(UNRECORDED)
    } else if !e.file_present {
        Some(FILE_MISSING)
    } else {
        None
    };
    (cells, state)
}

/// §6.3's listing for a person: one row per entry, newest first, aligned by display width,
/// with a trailing state for an entry that has no row or no file. The directory the files
/// are in comes last. `identity` names the identity an entry carries.
pub(crate) fn human(
    list: &DisplacedList,
    identity: &dyn Fn(&DisplacedEntry) -> Option<String>,
) -> String {
    if list.entries.is_empty() {
        return EMPTY.into();
    }
    let rows: Vec<([String; 7], Option<&str>)> =
        list.entries.iter().map(|e| cells(e, identity)).collect();
    let widths: Vec<usize> = HEADS
        .iter()
        .enumerate()
        .map(|(i, head)| {
            rows.iter()
                .map(|(c, _)| render::width(&c[i]))
                .fold(render::width(head), usize::max)
        })
        .collect();
    let line = |cells: &[&str], state: Option<&str>| {
        let mut s = cells
            .iter()
            .zip(&widths)
            .map(|(c, w)| render::pad(c, *w))
            .collect::<Vec<_>>()
            .join("  ");
        if let Some(state) = state {
            s.push_str("  ");
            s.push_str(state);
        }
        format!("{}\n", s.trim_end())
    };
    let mut out = line(&HEADS, None);
    for (c, state) in &rows {
        let c: Vec<&str> = c.iter().map(String::as_str).collect();
        out.push_str(&line(&c, *state));
    }
    out.push_str(&format!(
        "\nThe files are {}/<ID>.json; tagteam never reads one back, so restoring one is manual.\n",
        list.dir.display()
    ));
    out
}

/// §6.3's object: `{schemaVersion, dir, displaced: [{id, provider, at, reason, fingerprint,
/// identity, account, file, recorded}]}`. `at` is in ISO 8601 UTC, and whatever is not known
/// is null.
pub(crate) fn json(list: &DisplacedList) -> Value {
    let displaced: Vec<Value> = list
        .entries
        .iter()
        .map(|e| {
            json!({
                "id": e.id,
                "provider": e.provider.as_ref().map(ProviderId::as_str),
                "at": when(e),
                "reason": e.reason,
                "fingerprint": e.fingerprint,
                "identity": e.identity,
                "account": e.account,
                "file": if e.file_present { "present" } else { "missing" },
                "recorded": e.recorded,
            })
        })
        .collect();
    json!({"schemaVersion": 1, "dir": list.dir.to_string_lossy(), "displaced": displaced})
}
```

**`crates/tagteam/src/cli.rs`.** After Task 3's `Config { … },` variant, the last variant of
`Command`, insert:

```rust
    /// List credentials tagteam set aside rather than overwrite
    ///
    /// A switch saves a live login it would otherwise overwrite, and any other credential that
    /// was not tagteam's to keep, as a file in tagteam's data directory. tagteam never reads one
    /// back: restoring one is manual. --json prints {schemaVersion, dir, displaced: [{id,
    /// provider, at, reason, fingerprint, identity, account, file, recorded}]}, with times in ISO
    /// 8601 UTC.
    Displaced,
```

**`crates/tagteam/src/app.rs`.** Replace Task 3's
`use crate::{config_cmd, history, render, root_guard, statusline};` with:

```rust
use crate::{config_cmd, displaced_cmd, history, render, root_guard, statusline};
```

In `dispatch`, after Task 3's arm `Command::Config { action } => self.config(action)?,`, insert:

```rust
            Command::Displaced => self.displaced()?,
```

After `history` (its closing brace is just above `/// The live login's account, for a command
whose ACCOUNT defaults to it.`), insert:

```rust

    /// §6.3's listing: every displaced entry, newest first, and the directory the files are in.
    fn displaced(&mut self) -> Result<(), Failure> {
        let list = self.engine.displaced()?;
        let human = displaced_cmd::human(&list, &|e| displaced_cmd::identity(&self.engine, e));
        self.print(&human, displaced_cmd::json(&list));
        Ok(())
    }
```

- [ ] **Step 4: Run them and see them pass**

Run: `cargo test -p tagteam --lib displaced_cmd` (three tests) and
`cargo test -p tagteam --features test-support --test displaced_cli` (three tests).
Expected: PASS.

- [ ] **Step 5: Format, lint, commit**

```
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p tagteam --features test-support
git add crates/tagteam/src/cli.rs crates/tagteam/src/displaced_cmd.rs crates/tagteam/src/lib.rs \
  crates/tagteam/src/render.rs crates/tagteam/src/app.rs crates/tagteam/tests/displaced_cli.rs
git commit -m "List displaced credentials with tagteam displaced"
```

---

#### Part B: `--purge` and its confirmation

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam/src/displaced_cmd.rs`'s test module, after `an_empty_listing_says_so`, add:

```rust
    #[test]
    fn the_question_counts_the_entries() {
        assert_eq!(
            purge_question(1),
            "Delete 1 displaced credential? It cannot be recovered."
        );
        assert_eq!(
            purge_question(3),
            "Delete 3 displaced credentials? They cannot be recovered."
        );
    }

    #[test]
    fn a_purge_reports_each_deleted_id() {
        let deleted = [NEWEST.to_owned(), OLDEST.to_owned()];
        assert_eq!(
            purged_human(&deleted),
            format!("Deleted {NEWEST}.\nDeleted {OLDEST}.\n")
        );
        assert_eq!(
            purged_json(&deleted),
            json!({"schemaVersion": 1, "ok": true, "deleted": [NEWEST, OLDEST]})
        );
    }
```

In `crates/tagteam/tests/displaced_cli.rs`, append:

```rust
const NEEDS_YES: &str =
    "displaced credentials cannot be recovered once deleted; pass --yes to delete them";
const YES_ALONE: &str = "--yes confirms --purge; name the entries to delete with --purge ID...";

#[test]
fn purge_with_yes_deletes_the_file_and_the_row() {
    let d = tempfile::tempdir().unwrap();
    let id = forced_over_a_stranger(d.path());
    cmd(d.path())
        .args(["displaced", "--purge", &id, "--yes"])
        .assert()
        .success()
        .stdout(format!("Deleted {id}.\n"));
    assert!(!displaced_dir(d.path()).join(format!("{id}.json")).exists());
    cmd(d.path())
        .arg("displaced")
        .assert()
        .success()
        .stdout("No displaced credentials.\n");
}

#[test]
fn without_a_terminal_or_under_json_a_purge_needs_yes() {
    let d = tempfile::tempdir().unwrap();
    let id = forced_over_a_stranger(d.path());
    let file = displaced_dir(d.path()).join(format!("{id}.json"));
    cmd(d.path())
        .args(["displaced", "--purge", &id])
        .assert()
        .code(1)
        .stdout("")
        .stderr(format!("tagteam: {NEEDS_YES}\n"));
    let out = cmd(d.path())
        .args(["displaced", "--purge", &id, "--json"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        serde_json::from_slice::<Value>(&out).unwrap(),
        json!({"schemaVersion": 1, "error": {"type": "needs-confirmation", "message": NEEDS_YES}})
    );
    assert!(file.exists());
    let out = cmd(d.path())
        .args(["displaced", "--purge", &id, "--yes", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        serde_json::from_slice::<Value>(&out).unwrap(),
        json!({"schemaVersion": 1, "ok": true, "deleted": [id]})
    );
    assert!(!file.exists());
}

#[test]
fn an_id_that_names_no_entry_is_refused_and_nothing_is_deleted() {
    // Review Focus 3: refused before any path is built, even beside a valid ID. A file whose
    // name differs only in case exists, and is still no entry.
    let d = tempfile::tempdir().unwrap();
    let id = forced_over_a_stranger(d.path());
    let upper = format!("{}-{}-AAAAAA", id.split('-').next().unwrap(), fp12(&id));
    fs::write(
        displaced_dir(d.path()).join(format!("{upper}.json")),
        "not an entry",
    )
    .unwrap();
    for bad in ["../../tagteam.db", "x", "", upper.as_str()] {
        for ids in [[id.as_str(), bad], [bad, id.as_str()]] {
            let out = cmd(d.path())
                .args(["displaced", "--json", "--yes", "--purge"])
                .args(ids)
                .assert()
                .code(1)
                .get_output()
                .stdout
                .clone();
            assert_eq!(
                serde_json::from_slice::<Value>(&out).unwrap(),
                json!({"schemaVersion": 1, "error": {"type": "no-such-displaced",
                       "message": format!("no displaced credential matches {bad:?}; `tagteam displaced` lists them")}}),
                "{ids:?}"
            );
        }
    }
    cmd(d.path())
        .args(["displaced", "--purge", "../../tagteam.db", "--yes"])
        .assert()
        .code(1)
        .stdout("")
        .stderr("tagteam: no displaced credential matches \"../../tagteam.db\"; `tagteam displaced` lists them\n");
    for name in [&id, &upper] {
        assert!(
            displaced_dir(d.path())
                .join(format!("{name}.json"))
                .exists(),
            "{name}"
        );
    }
    assert!(
        Env::for_test(d.path())
            .data_dir()
            .join("tagteam.db")
            .exists()
    );
    assert_eq!(listing(d.path())["displaced"][0]["id"], json!(id));
}

#[test]
fn yes_without_purge_is_a_usage_error() {
    let d = tempfile::tempdir().unwrap();
    cmd(d.path())
        .args(["displaced", "--yes"])
        .assert()
        .code(2)
        .stdout("")
        .stderr(format!("tagteam: {YES_ALONE}\n"));
    let out = cmd(d.path())
        .args(["displaced", "--yes", "--json"])
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        serde_json::from_slice::<Value>(&out).unwrap(),
        json!({"schemaVersion": 1, "error": {"type": "usage", "message": YES_ALONE}})
    );
    // `--purge` takes at least one ID.
    cmd(d.path())
        .args(["displaced", "--purge"])
        .assert()
        .code(2);
}
```

In `crates/tagteam/tests/app.rs`, replace `use std::path::Path;` with
`use std::path::{Path, PathBuf};`, and append at the end of the file:

```rust
const DELETE_ONE: &str = "Delete 1 displaced credential? It cannot be recovered.";
const NEEDS_YES: &str =
    "displaced credentials cannot be recovered once deleted; pass --yes to delete them";

/// `a@x.co` stored, and a stranger's login that `switch 1 --force` displaced (§9.4 step 2).
/// Returns the entry's ID and its file.
fn with_a_displaced_login() -> (H, String, PathBuf) {
    let h = with_unmanaged_login();
    h.ok(&["switch", "1", "--force"]);
    let v: Value = serde_json::from_str(&h.ok(&["displaced", "--json"])).unwrap();
    let id = v["displaced"][0]["id"].as_str().unwrap().to_owned();
    let file = h
        .env
        .data_dir()
        .join("displaced")
        .join(format!("{id}.json"));
    (h, id, file)
}

#[test]
fn purging_asks_on_a_terminal_and_deletes_only_on_yes() {
    // §6.3: the question defaults to no, so Enter keeps the entry, and so does an explicit no.
    let (h, id, file) = with_a_displaced_login();
    for answer in ["n", ""] {
        let mut declined = Scripted::answering(&[answer]);
        let (code, out, err) = h.run(&["displaced", "--purge", id.as_str()], &mut declined);
        assert_eq!(
            (code, out.as_str(), err.as_str()),
            (1, "", "tagteam: cancelled\n"),
            "{answer:?}"
        );
        assert_eq!(declined.asked, [DELETE_ONE]);
        assert!(file.exists(), "{answer:?}");
    }
    let mut yes = Scripted::answering(&["y"]);
    let (code, out, err) = h.run(&["displaced", "--purge", id.as_str()], &mut yes);
    assert_eq!(
        (code, out, err),
        (0, format!("Deleted {id}.\n"), String::new())
    );
    assert_eq!(yes.asked, [DELETE_ONE]);
    assert!(!file.exists());
    assert_eq!(h.ok(&["displaced"]), "No displaced credentials.\n");
}

#[test]
fn purging_never_asks_off_a_terminal_or_under_json() {
    // Review Focus 2's rule for every prompt: a caller that cannot answer is never asked.
    let (h, id, file) = with_a_displaced_login();
    let (code, out, err) = h.run(
        &["displaced", "--purge", id.as_str()],
        &mut Scripted::none(),
    );
    assert_eq!((code, out.as_str()), (1, ""));
    assert_eq!(err, format!("tagteam: {NEEDS_YES}\n"));
    // `--json` never prompts, even on a terminal: Scripted panics on any prompt it has no answer for.
    let (code, out, _) = h.run(
        &["displaced", "--purge", id.as_str(), "--json"],
        &mut Scripted::answering(&[]),
    );
    assert_eq!(code, 1);
    assert_eq!(
        serde_json::from_str::<Value>(&out).unwrap(),
        json!({"schemaVersion": 1, "error": {"type": "needs-confirmation", "message": NEEDS_YES}})
    );
    assert!(file.exists());
    // `--yes` needs nobody to answer.
    let (code, out, _) = h.run(
        &["displaced", "--purge", id.as_str(), "--yes", "--json"],
        &mut Scripted::answering(&[]),
    );
    assert_eq!(code, 0);
    assert_eq!(
        serde_json::from_str::<Value>(&out).unwrap(),
        json!({"schemaVersion": 1, "ok": true, "deleted": [id]})
    );
    assert!(!file.exists());
}

#[test]
fn an_unknown_id_is_refused_before_anyone_is_asked() {
    // §6.3: every ID is checked first. Scripted panics on any prompt it has no answer for.
    let (h, id, file) = with_a_displaced_login();
    let (code, out, err) = h.run(
        &["displaced", "--purge", id.as_str(), "../../tagteam.db"],
        &mut Scripted::answering(&[]),
    );
    assert_eq!((code, out.as_str()), (1, ""));
    assert_eq!(
        err,
        "tagteam: no displaced credential matches \"../../tagteam.db\"; `tagteam displaced` lists them\n"
    );
    assert!(file.exists());
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam --lib displaced_cmd`
Expected: compile errors: `cannot find function `purge_question` in this scope`, and the same
for `purged_human` and `purged_json`.

Run `cargo test --no-fail-fast -p tagteam --features test-support --test displaced_cli --test app`.
Integration tests build the library without its unit tests, so the errors above do not block
them. Expected:
- The four new `displaced_cli` tests FAIL. clap exits 2 with `error: unexpected argument
  found` (the scrubbing drops which one). For `yes_without_purge_is_a_usage_error` that stderr
  is not `tagteam: --yes confirms …`.
- The three new `app` tests panic in `H::run`'s `Cli::try_parse_from(argv).unwrap()`.

- [ ] **Step 3: Implement**

**`crates/tagteam/src/displaced_cmd.rs`.** After `pub(crate) fn json`, and before the test module,
add:

```rust

/// The question `--purge` asks on a terminal (§6.3), for `n` entries. Its default is no.
pub(crate) fn purge_question(n: usize) -> String {
    if n == 1 {
        "Delete 1 displaced credential? It cannot be recovered.".into()
    } else {
        format!("Delete {n} displaced credentials? They cannot be recovered.")
    }
}

/// One line per deleted entry.
pub(crate) fn purged_human(deleted: &[String]) -> String {
    deleted
        .iter()
        .map(|id| format!("Deleted {id}.\n"))
        .collect()
}

/// `{schemaVersion, ok, deleted: [id]}`.
pub(crate) fn purged_json(deleted: &[String]) -> Value {
    json!({"schemaVersion": 1, "ok": true, "deleted": deleted})
}
```

Replace the module doc (its two `//!` lines) with:

```rust
//! `tagteam displaced` (§6.3): the listing, for a person and as JSON, and `--purge`'s
//! results. It never prints a credential.
```

**`crates/tagteam/src/cli.rs`.** Replace the Part A variant (from `/// List credentials tagteam
set aside rather than overwrite` to `Displaced,`) with:

```rust
    /// List credentials tagteam set aside rather than overwrite, or delete them
    ///
    /// A switch saves a live login it would otherwise overwrite, and any other credential that
    /// was not tagteam's to keep, as a file in tagteam's data directory. tagteam never reads one
    /// back: restoring one is manual. --purge deletes the entries named, after a confirmation
    /// or with --yes. --json prints {schemaVersion, dir, displaced: [{id, provider, at, reason,
    /// fingerprint, identity, account, file, recorded}]}, with times in ISO 8601 UTC, and for a
    /// purge {schemaVersion, ok, deleted: [id]}.
    Displaced {
        /// Delete these entries, each file and its row; they cannot be recovered
        #[arg(long, num_args = 1.., value_name = "ID")]
        purge: Vec<String>,
        /// Delete without asking
        #[arg(long)]
        yes: bool,
    },
```

**`crates/tagteam/src/app.rs`.** After `const KIND_UNSUPPORTED: &str = "unsupported";` (line 44),
insert:

```rust
/// A deletion that needs a person's yes, or `--yes` (§6.3).
const KIND_NEEDS_CONFIRMATION: &str = "needs-confirmation";
```

After `const STATUSLINE_UNDER_JSON: &str = "statusline prints a line of text; run it without
--json";` (line 56), insert:

```rust
const YES_WITHOUT_PURGE: &str =
    "--yes confirms --purge; name the entries to delete with --purge ID...";
const PURGE_NEEDS_YES: &str =
    "displaced credentials cannot be recovered once deleted; pass --yes to delete them";
```

In `dispatch`, replace `Command::Displaced => self.displaced()?,` with:

```rust
            Command::Displaced { purge, yes } => self.displaced(purge, yes)?,
```

Replace Part A's `App::displaced` (from `/// §6.3's listing: every displaced entry, newest first,
and the directory the files are in.` to its closing brace) with:

```rust
    /// §6.3: the listing, or `--purge`'s deletions once confirmed. Every ID is checked before
    /// the question, so an unknown one fails without asking. A person is asked on a terminal.
    /// Anywhere else, and under `--json`, `--yes` is required.
    fn displaced(&mut self, purge: Vec<String>, yes: bool) -> Result<(), Failure> {
        if purge.is_empty() {
            if yes {
                return Err(Failure::Usage(YES_WITHOUT_PURGE.into()));
            }
            let list = self.engine.displaced()?;
            let human = displaced_cmd::human(&list, &|e| displaced_cmd::identity(&self.engine, e));
            self.print(&human, displaced_cmd::json(&list));
            return Ok(());
        }
        let ids = self.engine.known_displaced(&purge)?;
        if !yes {
            if !self.can_prompt() {
                return Err(Failure::Message(
                    KIND_NEEDS_CONFIRMATION,
                    PURGE_NEEDS_YES.into(),
                ));
            }
            let question = displaced_cmd::purge_question(ids.len());
            if !self.io.prompter.confirm(&question, false) {
                return Err(cancelled());
            }
        }
        let deleted = self.engine.purge_displaced(&ids)?;
        self.print(
            &displaced_cmd::purged_human(&deleted),
            displaced_cmd::purged_json(&deleted),
        );
        Ok(())
    }
```

- [ ] **Step 4: Run the tests and see them pass**

Run: `cargo test -p tagteam --lib displaced_cmd` (five tests),
`cargo test -p tagteam --features test-support --test displaced_cli` (seven tests) and
`cargo test -p tagteam --test app`. Expected: PASS.

Run: `cargo test -p tagteam --features test-support`. Expected: PASS.

- [ ] **Step 5: Format, lint, commit**

```
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p tagteam --features test-support
git add crates/tagteam/src/cli.rs crates/tagteam/src/displaced_cmd.rs crates/tagteam/src/app.rs \
  crates/tagteam/tests/displaced_cli.rs crates/tagteam/tests/app.rs
git commit -m "Purge displaced credentials after a confirmation, or with --yes"
```

---

#### Part C: the redaction pin covers `displaced`

Task 7's `every_command_at_trace_leaves_no_identity_or_secret_in_the_log` (§15.3 "Logs", B.69)
runs every command at TRACE, but `displaced` did not exist then. Its flow also displaces nothing,
so a bare `displaced` there would list no entry. This part gives it one: a stranger's login, with
the fixture's organization, that `switch 1 --force` displaces (§9.4 step 2). Its row names the
stranger and the organization, `displaced` lists them on stdout, and `--purge` deletes the entry.
No log line may name either.

- [ ] **Step 1: Extend the pin**

In `crates/tagteam/tests/logging.rs`, replace:

```rust
const SETUP_TOKEN: &str = "sk-ant-oat01-zqsetup-Mx6kPw2zRq9vTs4nLy7j";
/// No line holds one of these whole.
const IDENTITIES: [&str; 5] = [ALPHA_EMAIL, BRAVO_EMAIL, KEY_EMAIL, SETUP_EMAIL, ORG_NAME];
/// No line holds 13 consecutive characters of one of these.
const SECRETS: [&str; 10] = [
```

with:

```rust
const SETUP_TOKEN: &str = "sk-ant-oat01-zqsetup-Mx6kPw2zRq9vTs4nLy7j";
const STRANGER_EMAIL: &str = "zq-stranger-7736@redact.test";
const STRANGER_RT: &str = "zqrt-stranger-Qw4xKp9zRm2vTn7s";
const STRANGER_AT: &str = "zqat-stranger-Hv8kWq3zPx6mRt2n";
/// No line holds one of these whole.
const IDENTITIES: [&str; 6] = [
    ALPHA_EMAIL,
    BRAVO_EMAIL,
    KEY_EMAIL,
    SETUP_EMAIL,
    STRANGER_EMAIL,
    ORG_NAME,
];
/// No line holds 13 consecutive characters of one of these.
const SECRETS: [&str; 12] = [
    STRANGER_RT,
    STRANGER_AT,
```

and, in the test, insert after `run(&["config", "unset", "ui.color"]);`:

```rust
    // A stranger's login that a forced switch displaces (§9.4 step 2): its row names the
    // stranger and the organization, which `displaced` lists and `--purge` deletes.
    login_as(root, STRANGER_EMAIL, STRANGER_RT, STRANGER_AT);
    run(&["switch", "1", "--force"]);
    run(&["displaced"]);
    let shown = traced(root, &server)
        .args(["displaced", "--json"])
        .output()
        .unwrap();
    assert!(shown.status.success());
    let shown: Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(
        shown["displaced"][0]["identity"]["emailAddress"],
        STRANGER_EMAIL
    );
    let id = shown["displaced"][0]["id"].as_str().unwrap().to_owned();
    run(&["displaced", "--purge", &id, "--yes"]);
```

- [ ] **Step 2: Run it and see it pass**

Run: `cargo test -p tagteam --features test-support --test logging every_command`
Expected: PASS. The pin is a guard: `displaced` and the purge log the entry's ID, never the
identity its row holds. It fails only if a later change logs one, for example an `Identity` or a
row's JSON.

- [ ] **Step 3: Commit**

```
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
git add crates/tagteam/tests/logging.rs
git commit -m "Pin that displaced and its purge log no identity"
```

---

### Task 10: `tagteam completions bash|zsh|fish`

> **Re-sync.**
> - **M3a Task 6:** the branch goes into `run_command`, beside `statusline`'s. It returns `Ended::Code(EXIT_USAGE)` after the `--json` refusal and `Ended::Code(0)` after the script. `command_name` gains `Command::Completions { .. } => "completions",`.
> - **M4a Task 8:** move the branch to just after the `RunShell::Unreadable` refusal and before the provider flag is resolved. §12.8 refuses every command but `statusline` under an unreadable marker, `completions` included. `build_registry` and `locate` read only the marker, so the command still touches no store, no Keychain and no settings.
> - **M4a Task 8's `build_registry`:** strengthen `completion_offers_only_providers_this_build_registers` to compare the ids of `build_registry(&ctx).all()` with `cli::COMPLETED_PROVIDERS` exactly, in both directions.
> - **Task 9's `Command::Displaced`** precedes this task's variant. `Completions` is the last variant of `Command`, and its `dispatch` arm the last arm.
> - **`clap_complete`** is a new crate, so the first build fetches it into `~/.cargo` and writes it into `Cargo.lock`.

§13.7: "`tagteam completions bash|zsh|fish` prints a completion script for that shell on stdout, generated by `clap_complete` from the command definitions, for the user or a package manager to install. The script is static: commands, flags, provider IDs, settings keys (§6.4) and the other fixed values complete. Account references do not, since completing them would read the store. The command needs no store and no Keychain." §15.2: "`tagteam` (CLI): snapshot tests … of each shell's completion script." Decision 14: the `config` KEY argument "accepts any string but advertises the registry's keys". This task adds `clap_complete`, the `completions` command and its `CompletionShell`, and the fish addition (Reading 7). It gives `--provider` a parser that, like Task 3's `ConfigKeyParser`, accepts any string and advertises this build's provider ids. The command is answered in `app::run` before any engine, settings or store exists.

**Readings of the spec this task commits to:**
1. **Provider ids complete for `--provider`** from `cli::COMPLETED_PROVIDERS`, which Task 3 introduced for `provider.<id>.` keys. `--provider` still accepts any string, so an unknown id still reaches `unknown-provider`. `hide_possible_values` keeps `--help` as it was. A unit test pins that every offered id is one this build registers.
2. **The script is built in memory, then written.** `clap_complete::generate` panics (`failed to write completion file`) when its writer fails. Writing a finished buffer and ignoring the error makes a closed stdout exit 0 without a panic, as `list` does (`a_closed_stdout_is_not_a_panic`).
3. **`--json` is a usage error:** exit 2, `{"schemaVersion":1,"error":{"type":"usage","message":"completions prints a script; run it without --json"}}` on stdout, nothing on stderr, as for `statusline --json`.
4. **No settings warnings.** The command is answered before the settings are read, so even a corrupt `config.toml` draws no warning.
5. **Only the three shells.** `clap_complete::aot::Shell` also has Elvish and PowerShell. `CompletionShell` lists only bash, zsh and fish, which §13.7 names, so `completions powershell` is clap's usage error.
6. **Tests: what each script offers, not its bytes.** §15.2 asks for snapshot tests of each shell's script, and the repository has no `insta`. What a snapshot would guard is a script that quietly stops offering something; an exact golden file would mostly pin `clap_complete`'s own bytes, and need re-blessing at every new command and every `clap_complete` release. These tests guard the first with expectations derived from the same definitions the scripts come from. `insta` stays M5b's to add for the JSON shapes, if it wants it. For each shell, the test walks `Cli::command()` for every subcommand name, visible alias and long flag, and takes every `KEYS` name, every `provider.claude-code.<key>` spelling, the provider id and the shell names. Each must appear in the script as a whole word (a long flag as `--name`, or `-l name` in fish), and two runs must give the same bytes. bash's script is also sourced and its completion function called, which proves real completions of a key, a prefixed key, a provider and a shell. That runs on macOS's bash 3.2 and on Linux. zsh and fish have no such harness, so the fish addition gets a test of its own (Reading 7).
7. **fish completes positional values too, and only in their place.** `clap_complete`'s fish generator emits `complete` lines only for options, flags and subcommands (4.6.11's `aot/shells/fish.rs`), so `tagteam config get <TAB>` and `tagteam completions <TAB>` would offer nothing in fish, against §13.7's "settings keys … complete". `CompletionShell::script` appends one fish `complete` line per positional argument with fixed values, derived from the same `clap::Command` tree. That is part of the generation §13.7 names.
   - **The condition.** fish's `__fish_seen_subcommand_from` cannot be it, because it stays true once the positional is typed: `config set ui.color <TAB>` would offer the keys again for VALUE, and `completions bash <TAB>` the shells. So the script also gets one helper, `__fish_tagteam_words`. It lists the words before the cursor (`commandline -opc`, less the first), skipping options and the word after each option that takes a value. That option list comes from the same tree, global ones included, so `-p/--provider P` shifts nothing. A line's condition holds only while those words, joined by spaces, are exactly its subcommands, then one word for each positional before it: `string match -qr '^config set$'`. A level with visible aliases is `(name|alias)`, and an earlier positional is `\S+`. Today's tree has neither: the four lines are `config get`, `config set`, `config unset` and `completions`, and the test pins exactly those, so a new line fails it until the test covers the new one.
   - **The form.** The lines follow `clap_complete`'s own, `complete -c tagteam -n "…" -f -a "…"`. No fish runs in CI, so `fish -n` cannot check them. The test parses the helper's option list and each line's pattern, and works out what the lines offer on sample command lines as fish would: before the positional, once it is given, and with options and a global option's value around it.
   - **An option still waiting for its value.** At `config get --provider <TAB>` the word being completed is the provider, not the key. The helper then lists one more, empty, word, which no line's pattern matches, so no positional is offered there.
   - **The helper's limits,** none of which changes an offer on a valid command line today. One option list serves every subcommand, so no spelling may take a value in one command and none in another; the test pins that. Only the first value of `--purge ID...` is skipped, and `displaced` has no positional. A word starting with `-` that clap takes as a positional (after `--`, or as `config set`'s VALUE) is skipped as an option. No key or shell starts with `-`, and nothing follows VALUE.

**Files:**
- Modify: `Cargo.toml` (`[workspace.dependencies]`, after `clap`)
- Modify: `crates/tagteam/Cargo.toml` (`[dependencies]`, after `clap.workspace = true`)
- Modify: `Cargo.lock` (gains `clap_complete`, written by the first build)
- Modify: `crates/tagteam/src/cli.rs` (imports; `Cli.provider`'s attribute; `Command::Completions` as the last variant; `ProviderParser`, `CompletionShell`, `fish_positionals`, `options_taking_values` and `fish_positional_lines` at the end of the file)
- Modify: `crates/tagteam/src/app.rs` (the `*_UNDER_JSON` constants, :56; `run`'s fast paths, after the `statusline` branch, :303-305; `dispatch`'s last arm; one unit test)
- Test: `crates/tagteam/tests/completions.rs` (new)
- Test: `crates/tagteam/tests/cli.rs` (`a_closed_stdout_is_not_a_panic`, :170-188)
- Test: `crates/tagteam/tests/logging.rs` (Task 7's redaction pin; Steps 6–8)

**Interfaces:**
- Consumes:
  - Task 2: `tagteam_engine::settings::KEYS`.
  - Task 3: `cli::{ConfigKeyParser, COMPLETED_PROVIDERS}`, `Command::Config`, `build_engine(ctx: Context, flag: Option<&ProviderId>)`.
  - Existing: `fail`, `KIND_USAGE`, `EXIT_USAGE`, `Io.out`, `common::cmd`, `Cli`.
- Produces:
  - `Command::Completions { shell: CompletionShell }`.
  - `#[derive(Clone, Copy, ValueEnum)] pub enum CompletionShell { Bash, Zsh, Fish }`, with `pub fn script(self) -> Vec<u8>`.
  - `#[derive(Clone)] pub struct ProviderParser` (`TypedValueParser<Value = String>`), on `Cli.provider`.

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam/tests/completions.rs`:

```rust
//! `tagteam completions bash|zsh|fish` through the binary (§13.7). Needs
//! `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use clap::CommandFactory;
use common::cmd;
use serde_json::{Value, json};
use tagteam::cli::Cli;
use tagteam_engine::settings::KEYS;

const SHELLS: [&str; 3] = ["bash", "zsh", "fish"];

/// `tagteam completions <shell>`'s script. It says nothing on stderr.
fn script(root: &Path, shell: &str) -> String {
    let out = cmd(root)
        .args(["completions", shell])
        .assert()
        .success()
        .stderr("")
        .get_output()
        .stdout
        .clone();
    String::from_utf8(out).unwrap()
}

/// The words of `script`: its runs of letters, digits, `.`, `-` and `_`. A name a script offers
/// is one of them, whatever the shell's quoting around it.
fn words(script: &str) -> BTreeSet<&str> {
    script
        .split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')))
        .filter(|w| !w.is_empty())
        .collect()
}

/// Every subcommand name and visible alias under `cmd`, and every long flag.
fn definitions(cmd: &clap::Command, names: &mut BTreeSet<String>, flags: &mut BTreeSet<String>) {
    flags.extend(
        cmd.get_arguments()
            .filter_map(|a| a.get_long())
            .map(str::to_owned),
    );
    for sub in cmd.get_subcommands() {
        names.extend(
            sub.get_name_and_visible_aliases()
                .into_iter()
                .map(str::to_owned),
        );
        definitions(sub, names, flags);
    }
}

#[test]
fn each_shell_s_script_offers_every_command_flag_key_and_provider() {
    // §13.7: commands, flags, provider IDs, settings keys (§6.4) and the other fixed values
    // complete. The expectations come from the same definitions the scripts do.
    let d = tempfile::tempdir().unwrap();
    let (mut names, mut flags) = (BTreeSet::new(), BTreeSet::new());
    definitions(&Cli::command(), &mut names, &mut flags);
    assert!(names.contains("config") && names.contains("get") && names.contains("ls"));
    let mut values: Vec<String> = KEYS.iter().map(|k| k.name.to_owned()).collect();
    values.extend(
        KEYS.iter()
            .filter(|k| k.per_provider)
            .map(|k| format!("provider.claude-code.{}", k.name)),
    );
    values.extend(["claude-code", "bash", "zsh", "fish", "tagteam"].map(str::to_owned));
    for shell in SHELLS {
        let text = script(d.path(), shell);
        assert_eq!(
            text,
            script(d.path(), shell),
            "{shell}: the same script every time"
        );
        let words = words(&text);
        let missing: Vec<&String> = names
            .iter()
            .chain(&values)
            .filter(|w| !words.contains(w.as_str()))
            .collect();
        assert!(missing.is_empty(), "{shell} offers none of {missing:?}");
        // bash and zsh spell a flag `--name`; fish declares it `-l name`.
        let missing: Vec<&String> = flags
            .iter()
            .filter(|f| match shell {
                "fish" => !text.contains(&format!("-l {f}")),
                _ => !words.contains(format!("--{f}").as_str()),
            })
            .collect();
        assert!(
            missing.is_empty(),
            "{shell} offers none of the flags {missing:?}"
        );
    }
}

#[test]
fn bash_completes_a_config_key_a_provider_and_a_shell() {
    // The bash script, sourced, as bash's completion calls it: `_tagteam <command> <word> <previous>`.
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("tagteam.bash");
    fs::write(&path, script(d.path(), "bash")).unwrap();
    let complete = |words: &[&str]| -> Vec<String> {
        let line = words.join(" ");
        let out = std::process::Command::new("bash")
            .arg("-c")
            .arg(
                "source \"$1\"; shift; COMP_WORDS=(\"$@\"); COMP_CWORD=$(($# - 1)); \
                 _tagteam tagteam \"${COMP_WORDS[COMP_CWORD]}\" \"${COMP_WORDS[COMP_CWORD-1]}\"; \
                 printf '%s\\n' \"${COMPREPLY[@]}\"",
            )
            .arg("bash")
            .arg(&path)
            .args(words)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{line}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .collect()
    };
    let autoswitch: Vec<String> = KEYS
        .iter()
        .filter(|k| k.name.starts_with("auto"))
        .map(|k| k.name.to_owned())
        .collect();
    assert_eq!(complete(&["tagteam", "config", "get", "auto"]), autoswitch);
    assert_eq!(
        complete(&["tagteam", "config", "get", "provider.claude-code.run"]),
        ["provider.claude-code.run.share_extra"]
    );
    assert_eq!(complete(&["tagteam", "--provider", ""]), ["claude-code"]);
    let shells: Vec<String> = complete(&["tagteam", "completions", ""])
        .into_iter()
        .filter(|w| !w.starts_with('-'))
        .collect();
    assert_eq!(shells, ["bash", "zsh", "fish"], "and the flags");
}

/// How each line the fish script adds for a positional starts, up to its pattern's words.
const FISH_POSITIONAL: &str =
    "complete -c tagteam -n \"__fish_tagteam_words | string join ' ' | string match -qr '^";

/// The lines tagteam adds to the fish script, as fish reads them.
struct Fish<'s> {
    /// The options `__fish_tagteam_words` skips the next word after.
    valued: BTreeSet<&'s str>,
    /// Each positional line's pattern, its words joined by spaces, and the values it offers.
    lines: Vec<(&'s str, BTreeSet<&'s str>)>,
}

impl<'s> Fish<'s> {
    fn parse(script: &'s str) -> Self {
        let helper = script
            .split_once("function __fish_tagteam_words\n")
            .expect("the helper is defined")
            .1;
        let valued = helper
            .lines()
            .find_map(|l| l.trim().strip_prefix("else if contains -- $word "))
            .expect("the helper names the options that take a value")
            .split(' ')
            .collect();
        let lines = script
            .lines()
            .filter_map(|l| l.strip_prefix(FISH_POSITIONAL))
            .map(|rest| {
                let (pattern, values) = rest.split_once("\\$'\" -f -a \"").unwrap();
                (pattern, values.trim_end_matches('"').split(' ').collect())
            })
            .collect();
        Fish { valued, lines }
    }

    /// What these lines offer for the word after `line`. As the helper does, this keeps the
    /// words after the first that are neither an option nor the word after one in `valued`. A
    /// line applies when its pattern has as many words, each matching the kept word in its place:
    /// a name, one of `(a|b)`, or any word for `\S+`.
    fn offers(&self, line: &str) -> BTreeSet<&'s str> {
        let mut words = Vec::new();
        let mut skip = false;
        for word in line.split(' ').skip(1) {
            if skip {
                skip = false;
            } else if self.valued.contains(word) {
                skip = true;
            } else if !(word.len() > 1 && word.starts_with('-')) {
                words.push(word);
            }
        }
        // The helper's empty word for an option still waiting for its value.
        if skip {
            words.push("");
        }
        self.lines
            .iter()
            .filter(|(pattern, _)| {
                let parts: Vec<&str> = pattern.split(' ').collect();
                parts.len() == words.len()
                    && parts.iter().zip(&words).all(|(part, word)| {
                        match part.strip_prefix('(').and_then(|p| p.strip_suffix(')')) {
                            Some(names) => names.split('|').any(|n| n == *word),
                            None => (*part == r"\S+" && !word.is_empty()) || part == word,
                        }
                    })
            })
            .flat_map(|(_, values)| values.iter().copied())
            .collect()
    }
}

/// Every spelling of an option under `cmd`, by whether it takes a value.
fn options(cmd: &clap::Command, valued: &mut BTreeSet<String>, flags: &mut BTreeSet<String>) {
    for arg in cmd.get_arguments().filter(|a| !a.is_positional()) {
        let shorts = arg.get_short_and_visible_aliases().unwrap_or_default();
        let longs = arg.get_long_and_visible_aliases().unwrap_or_default();
        let spellings = shorts
            .into_iter()
            .map(|s| format!("-{s}"))
            .chain(longs.into_iter().map(|l| format!("--{l}")));
        if arg.get_action().takes_values() {
            valued.extend(spellings);
        } else {
            flags.extend(spellings);
        }
    }
    for sub in cmd.get_subcommands() {
        options(sub, valued, flags);
    }
}

#[test]
fn fish_completes_a_positional_only_in_its_place() {
    // clap_complete's fish script completes no positional argument: tagteam adds a line for
    // each one with fixed values. A line applies only while the words before the cursor, less
    // options and their values, are exactly its subcommands. No fish runs here, so the test
    // reads the lines as fish would.
    let d = tempfile::tempdir().unwrap();
    let script = script(d.path(), "fish");
    let fish = Fish::parse(&script);
    // The helper skips the value of every option that takes one, global ones included. One
    // list serves every subcommand, since no spelling takes a value in one and none in another.
    let (mut valued, mut flags) = (BTreeSet::new(), BTreeSet::new());
    options(&Cli::command(), &mut valued, &mut flags);
    assert!(valued.contains("-p") && valued.contains("--provider"));
    assert!(valued.is_disjoint(&flags), "{valued:?} {flags:?}");
    assert_eq!(fish.valued, valued.iter().map(String::as_str).collect());
    // Each line names its exact word path.
    let patterns: Vec<&str> = fish.lines.iter().map(|(pattern, _)| *pattern).collect();
    assert_eq!(
        patterns,
        ["config get", "config set", "config unset", "completions"]
    );
    let mut names: BTreeSet<String> = KEYS.iter().map(|k| k.name.to_owned()).collect();
    names.extend(
        KEYS.iter()
            .filter(|k| k.per_provider)
            .map(|k| format!("provider.claude-code.{}", k.name)),
    );
    let keys: BTreeSet<&str> = names.iter().map(String::as_str).collect();
    let shells = BTreeSet::from(["bash", "zsh", "fish"]);
    let nothing = BTreeSet::new();
    for (line, offered) in [
        ("tagteam config get", &keys),
        ("tagteam config set", &keys),
        ("tagteam config unset", &keys),
        ("tagteam completions", &shells),
        // Options anywhere, and a global option's value, shift nothing.
        ("tagteam -p claude-code config get", &keys),
        ("tagteam config --provider claude-code --json set", &keys),
        // An option still waiting for its value is completed as that value, never a key.
        ("tagteam config get --provider", &nothing),
        ("tagteam completions -p", &nothing),
        ("tagteam --provider=claude-code completions", &shells),
        // Once the positional is given, it is offered no more.
        ("tagteam config set ui.color", &nothing),
        ("tagteam config unset autoswitch.models", &nothing),
        ("tagteam completions bash", &nothing),
        // Nor before its command, nor under another one.
        ("tagteam", &nothing),
        ("tagteam config", &nothing),
        ("tagteam -p config get", &nothing),
        ("tagteam help config get", &nothing),
    ] {
        assert_eq!(&fish.offers(line), offered, "{line}");
    }
}

#[test]
fn completions_reads_no_settings_and_creates_nothing() {
    // §13.7: no store and no Keychain. A corrupt config.toml draws no warning: the script never
    // reads the settings.
    let d = tempfile::tempdir().unwrap();
    let config = d.path().join("home/.config/tagteam");
    fs::create_dir_all(&config).unwrap();
    fs::write(config.join("config.toml"), "[autoswitch\n").unwrap();
    fs::create_dir_all(d.path().join("keychain")).unwrap();
    fs::write(d.path().join("keychain/LOCKED"), "").unwrap();
    for shell in SHELLS {
        assert!(!script(d.path(), shell).is_empty(), "{shell}");
    }
    let listed = |dir: &Path| -> Vec<String> {
        fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect()
    };
    assert_eq!(listed(&d.path().join("home")), [".config"]);
    assert_eq!(listed(&d.path().join("home/.config")), ["tagteam"]);
    assert_eq!(listed(&config), ["config.toml"]);
    assert_eq!(listed(&d.path().join("keychain")), ["LOCKED"]);
}

#[test]
fn completions_under_json_or_for_another_shell_is_a_usage_error() {
    let d = tempfile::tempdir().unwrap();
    let out = cmd(d.path())
        .args(["completions", "bash", "--json"])
        .assert()
        .code(2)
        .stderr("")
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        serde_json::from_slice::<Value>(&out).unwrap(),
        json!({"schemaVersion": 1, "error": {"type": "usage",
               "message": "completions prints a script; run it without --json"}})
    );
    cmd(d.path())
        .args(["completions", "powershell"])
        .assert()
        .code(2)
        .stdout("");
    cmd(d.path()).arg("completions").assert().code(2).stdout("");
}
```

In `crates/tagteam/tests/cli.rs`, in `a_closed_stdout_is_not_a_panic`, replace

```rust
    let cases: [(&[&str], i32); 3] = [
        (&["frobnicate", "--json"], 2),
        (&["list", "--json"], 0),
        (&["list"], 0),
    ];
```

with

```rust
    let cases: [(&[&str], i32); 4] = [
        (&["frobnicate", "--json"], 2),
        (&["list", "--json"], 0),
        (&["list"], 0),
        (&["completions", "bash"], 0),
    ];
```

In `crates/tagteam/src/app.rs`'s `mod tests`, insert before `fn an_unreadable_active_flag_after_a_commit_is_inactive_not_an_error` (and its `#[test]`):

```rust
    #[test]
    fn completion_offers_only_providers_this_build_registers() {
        // §13.7: the provider IDs completion offers are static; each must be one this build has.
        let dir = tempfile::tempdir().unwrap();
        let ctx = Context {
            env: Env::for_test(dir.path()),
            keychain: Arc::new(tagteam_provider::FakeKeychain::new()),
            platform: Platform::MacOs,
            api_base: None,
            stdout_terminal: false,
            no_color_env: false,
            force_color_env: false,
        };
        let (engine, _) = build_engine(ctx, None);
        for id in crate::cli::COMPLETED_PROVIDERS {
            assert!(engine.provider(&ProviderId::new(*id)).is_ok(), "{id}");
        }
    }
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test --no-fail-fast -p tagteam --features test-support --test completions --test cli`
Expected: the five tests in `completions.rs` and `a_closed_stdout_is_not_a_panic` fail. `completions` is not a subcommand yet, so each run exits 2 where 0 is expected. Under `--json` it prints clap's usage error, not this command's message.

Run: `cargo test -p tagteam --lib completion_offers`
Expected: PASS already. It is a guard: Task 3 put `COMPLETED_PROVIDERS` and `build_engine` in place, and it fails only if completion ever offers a provider this build lacks.

- [ ] **Step 3: Implement**

In `Cargo.toml`, after the `clap = …` line of `[workspace.dependencies]`, add:

```toml
# `tagteam completions` (§13.7): static scripts generated from the command definitions.
clap_complete = "4"
```

In `crates/tagteam/Cargo.toml`, after `clap.workspace = true`, add:

```toml
clap_complete.workspace = true
```

`clap_complete` 4.6 needs `clap` 4.6.6 or later (the lock has 4.6.7) and Rust 1.85. The first build writes it into `Cargo.lock`.

In `crates/tagteam/src/cli.rs`, replace the imports Task 3 wrote,

```rust
use std::ffi::OsStr;

use clap::builder::{PossibleValue, StringValueParser, TypedValueParser};
use clap::{Parser, Subcommand};
use tagteam_core::CLAUDE_CODE;
use tagteam_engine::settings::KEYS;
```

with

```rust
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::io::Write;

use clap::builder::{PossibleValue, StringValueParser, TypedValueParser};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::aot::{Shell, generate};
use tagteam_core::CLAUDE_CODE;
use tagteam_engine::settings::KEYS;
```

replace `Cli.provider`'s attribute line,

```rust
    #[arg(short = 'p', long, global = true, value_name = "PROVIDER")]
```

with

```rust
    #[arg(
        short = 'p',
        long,
        global = true,
        value_name = "PROVIDER",
        value_parser = ProviderParser,
        hide_possible_values = true
    )]
```

add after Task 9's `Displaced { … },` variant, as the last variant of `Command`:

```rust
    /// Print a completion script for bash, zsh or fish
    Completions { shell: CompletionShell },
```

and append at the end of the file:

```rust
/// `--provider`. Any UTF-8 string parses, so a provider this build lacks still reaches the
/// engine's `unknown-provider`, while completion offers the providers it has (§13.7).
#[derive(Clone)]
pub struct ProviderParser;

impl TypedValueParser for ProviderParser {
    type Value = String;

    fn parse_ref(
        &self,
        cmd: &clap::Command,
        arg: Option<&clap::Arg>,
        value: &OsStr,
    ) -> Result<String, clap::Error> {
        StringValueParser::new().parse_ref(cmd, arg, value)
    }

    fn possible_values(&self) -> Option<Box<dyn Iterator<Item = PossibleValue> + '_>> {
        Some(Box::new(
            COMPLETED_PROVIDERS.iter().map(|p| PossibleValue::new(*p)),
        ))
    }
}

/// The shells `tagteam completions` writes a script for (§13.7).
#[derive(Clone, Copy, ValueEnum)]
pub enum CompletionShell {
    Bash,
    Zsh,
    Fish,
}

impl CompletionShell {
    /// The script, generated by `clap_complete` from these definitions (§13.7). It is static:
    /// commands, flags, provider IDs, settings keys and the other fixed values complete, and
    /// account references do not. It is built in memory, so a closed stdout is the caller's
    /// write error rather than a panic inside the generator.
    pub fn script(self) -> Vec<u8> {
        let shell = match self {
            CompletionShell::Bash => Shell::Bash,
            CompletionShell::Zsh => Shell::Zsh,
            CompletionShell::Fish => Shell::Fish,
        };
        let mut cmd = Cli::command();
        let mut script = Vec::new();
        generate(shell, &mut cmd, "tagteam", &mut script);
        if let CompletionShell::Fish = self {
            fish_positionals(&cmd, &mut script);
        }
        script
    }
}

/// clap_complete's fish script completes options and subcommands, but no positional argument's
/// values, so `config get <KEY>` and `completions <SHELL>` would offer nothing. This appends,
/// from the same definitions, a `complete` line for each positional with fixed values, and the
/// helper their conditions read. `__fish_tagteam_words` lists the words before the cursor that
/// are neither an option nor the word after an option that takes a value, global ones included.
/// A line applies only while those words are exactly the subcommands that lead to its
/// positional, then one word for each positional before it, so it offers nothing once its
/// positional is given.
fn fish_positionals(cmd: &clap::Command, script: &mut Vec<u8>) {
    let mut valued = BTreeSet::new();
    options_taking_values(cmd, &mut valued);
    let valued = valued.into_iter().collect::<Vec<_>>().join(" ");
    let _ = write!(
        script,
        r#"
# The words before the cursor that are neither an option nor an option's value: the
# subcommands given, then the positional arguments given.
function __fish_tagteam_words
    set -l words (commandline -opc)
    set -e words[1]
    set -l skip 0
    for word in $words
        if test $skip = 1
            set skip 0
        else if contains -- $word {valued}
            set skip 1
        else if not string match -q -- '-?*' $word
            echo $word
        end
    end
    # An option still waiting for its value: the word being completed is that value, so an
    # empty word makes every line's pattern fail.
    if test $skip = 1
        echo ''
    end
end

"#
    );
    fish_positional_lines(cmd, &mut Vec::new(), script);
}

/// Every spelling of an option under `cmd` that takes a value, such as `-p` and `--provider`.
fn options_taking_values(cmd: &clap::Command, valued: &mut BTreeSet<String>) {
    for arg in cmd
        .get_arguments()
        .filter(|a| !a.is_positional() && a.get_action().takes_values())
    {
        let shorts = arg.get_short_and_visible_aliases().unwrap_or_default();
        let longs = arg.get_long_and_visible_aliases().unwrap_or_default();
        valued.extend(shorts.into_iter().map(|s| format!("-{s}")));
        valued.extend(longs.into_iter().map(|l| format!("--{l}")));
    }
    for sub in cmd.get_subcommands() {
        options_taking_values(sub, valued);
    }
}

/// A `complete` line for each positional under `cmd` with fixed values. `path` holds the
/// patterns of the subcommands that lead there: a name, or `(name|alias)`.
fn fish_positional_lines(cmd: &clap::Command, path: &mut Vec<String>, script: &mut Vec<u8>) {
    for sub in cmd.get_subcommands() {
        let names = sub.get_name_and_visible_aliases();
        path.push(match names.as_slice() {
            [name] => (*name).to_owned(),
            _ => format!("({})", names.join("|")),
        });
        for (before, arg) in sub.get_positionals().enumerate() {
            let values: Vec<String> = arg
                .get_possible_values()
                .into_iter()
                .filter(|v| !v.is_hide_set())
                .map(|v| v.get_name().to_owned())
                .collect();
            if values.is_empty() {
                continue;
            }
            let words: Vec<&str> = path
                .iter()
                .map(String::as_str)
                .chain(std::iter::repeat_n(r"\S+", before))
                .collect();
            let condition = format!(
                r"__fish_tagteam_words | string join ' ' | string match -qr '^{}\$'",
                words.join(" ")
            );
            let _ = writeln!(
                script,
                "complete -c tagteam -n \"{condition}\" -f -a \"{}\"",
                values.join(" ")
            );
        }
        fish_positional_lines(sub, path, script);
        path.pop();
    }
}
```

In `crates/tagteam/src/app.rs`, directly after

```rust
const STATUSLINE_UNDER_JSON: &str = "statusline prints a line of text; run it without --json";
```

and before Task 9's `YES_WITHOUT_PURGE`, add

```rust
const COMPLETIONS_UNDER_JSON: &str = "completions prints a script; run it without --json";
```

In `run`, directly after the `statusline` branch (`if let Some(Command::Statusline { print_config }) = &cli.command { … }`), add:

```rust
    // §13.7: a script from the command definitions alone, before anything is built. It reads no
    // settings, no store and no Keychain, so it warns about nothing and creates nothing.
    if let Some(Command::Completions { shell }) = &cli.command {
        if json {
            fail(io, true, KIND_USAGE, COMPLETIONS_UNDER_JSON);
            return EXIT_USAGE;
        }
        let _ = io.out.write_all(&shell.script());
        return 0;
    }
```

and at the end of `dispatch`'s `match`, after Task 9's arm
`Command::Displaced { purge, yes } => self.displaced(purge, yes)?,`:

```rust
            Command::Completions { .. } => unreachable!("run answers completions before dispatch"),
```

- [ ] **Step 4: Run the tests and see them pass**

Run: `cargo test -p tagteam --features test-support --test completions --test cli` — Expected: PASS (5 and 15 tests).
Run: `cargo test -p tagteam --features test-support` — Expected: PASS.
Run: `cargo test -p tagteam` — Expected: PASS (no binary tests without the feature; the unit tests run).

- [ ] **Step 5: Format, lint, commit**

```
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p tagteam --features test-support
git add Cargo.toml Cargo.lock crates/tagteam/Cargo.toml crates/tagteam/src/cli.rs crates/tagteam/src/app.rs crates/tagteam/tests/completions.rs crates/tagteam/tests/cli.rs
git commit -m "Print bash, zsh and fish completion scripts, with the settings keys and provider ids"
```

**The redaction pin covers `completions`.** Task 7's
`every_command_at_trace_leaves_no_identity_or_secret_in_the_log` runs every command at TRACE
(§15.3 "Logs"), and `completions` did not exist then.

- [ ] **Step 6: Extend the pin**

In `crates/tagteam/tests/logging.rs`, in that test, insert after Task 9's
`run(&["displaced", "--purge", &id, "--yes"]);`:

```rust
    for shell in ["bash", "zsh", "fish"] {
        run(&["completions", shell]);
    }
```

- [ ] **Step 7: Run it and see it pass**

Run: `cargo test -p tagteam --features test-support --test logging every_command`
Expected: PASS. `completions` reads nothing and logs nothing; the pin guards that it stays so.

- [ ] **Step 8: Commit**

```
cargo fmt --all && cargo fmt --all --check
git add crates/tagteam/tests/logging.rs
git commit -m "Pin that completions logs no identity"
```

---

### Task 11: Final verification

The whole branch is verified. `statusline` keeps §1.1's budget with the logging subscriber now
installed at the process boundary (Task 6), and the release binary carries no test hook. The
plan's status stays as the design-record rules want it while the merge request is open.

**Files:**
- Check: `docs/superpowers/plans/2026-10-01-tagteam-m5a-config-logging-displaced.md` (its
  `**Status:**` line).

**Interfaces:**
- Consumes: everything Tasks 1–10 produced.
- Produces: nothing new.

**Spec:**
- §1.1: `tagteam statusline` completes in ≤ 10 ms p95, and `tagteam list` in ≤ 50 ms p95 when no
  usage fetch is due, both on Apple Silicon.
- §14.2: a command that logs nothing never opens the log, so `statusline` stays within §1.1's
  budget.
- Global design-record rules: `In progress` while work or review is active, with the MR
  reference; `Implemented` with the MR in the final pre-merge commit.

- [ ] **Step 1: Format, lint and test everything**

Run:
```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --features tagteam/test-support --no-fail-fast 2>&1 | tee "$TMPDIR/m5a-test.log" | grep -E '^test result:|FAILED|panicked'
awk '/^test result:/ {p += $4; f += $6; i += $8} END {printf "%d/%d passed, %d ignored\n", p, p + f, i}' "$TMPDIR/m5a-test.log"
cargo test --workspace --features tagteam/test-support --no-fail-fast -- --ignored 2>&1 | tee "$TMPDIR/m5a-ignored.log" | grep -E '^test result:|FAILED|panicked'
awk '/^test result:/ {p += $4; f += $6} END {printf "%d/%d passed\n", p, p + f}' "$TMPDIR/m5a-ignored.log"
cargo test -p tagteam --lib
```

Expected:
- `fmt --check` prints nothing, and both clippy runs finish with no warning.
- Every `test result:` line says `ok`, and no `FAILED` or `panicked` line appears.
- The first `awk` prints `N/N passed, M ignored`, with the two numbers before the slash equal.
  Report that line as the total.
- The `--ignored` run passes every ignored test, and its `awk` prints `K/K passed`.
- `cargo test -p tagteam --lib` runs without features, so it exercises the release branch of the
  test-override checks.

Check that the redaction pin runs every command, the ones Tasks 9 and 10 appended included:

```bash
grep -c -E 'run\(&\["(displaced|completions)"' crates/tagteam/tests/logging.rs
```

Expected: `3`: `displaced`, `displaced --purge` and `completions` (§15.3 "Logs").

On a Mac, run the real Keychain tests:

```bash
cargo test -p tagteam-provider --features real_keychain --test real_keychain
```

Expected: PASS, with no GUI prompt. They drive `/usr/bin/security` against a throwaway keychain
file, which needs the Security daemons. An agent runs them sandboxed first, and unsandboxed only
when that run fails with sandbox evidence: `Operation not permitted`, or a `securityd` or
`SecKeychain` error. A person runs them in a plain terminal.

- [ ] **Step 2: The timing budgets, with the logging subscriber installed**

Run the timing tests, one at a time, on an otherwise idle machine:

```bash
cargo test --release -p tagteam --features test-support --test perf -- --ignored --nocapture --test-threads=1
```

Expected: PASS for every test the file holds. `statusline` p95 ≤ 10 ms in each of its cases, and
`list` p95 ≤ 50 ms with nothing due. The subscriber is set up for every command now (Task 6), and
the log file opens only on an event (Task 5), so neither budget moves.

One failure gets one re-run. A second failure is a real miss of the budget: stop and report the
measured p95.

Run, against a fixture HOME with no state directory:

```bash
d=$(mktemp -d) && mkdir -p "$d/home" && env -i HOME="$d/home" PATH="$PATH" target/release/tagteam statusline </dev/null; test ! -e "$d/home/.local/state/tagteam" && echo no-log
```

Expected: `no-log` (§14.2: `statusline` never opens the log). `env -i` drops every `XDG_*`,
`CLAUDE_*` and `TAGTEAM_*` variable of the shell, so no real state is read.

- [ ] **Step 3: Build the release binary and check it carries no test hooks**

```bash
cargo build --release -p tagteam
grep -a -c TAGTEAM_TEST_ target/release/tagteam ; test $? -eq 1
```

Expected: the build succeeds, and `grep` prints `0` and exits 1, so the final `test` exits 0.
The keychain-directory, platform and API-base overrides exist only under `test-support`.

- [ ] **Step 4: A smoke run of the new commands against a throwaway HOME**

```bash
d=$(mktemp -d) && mkdir -p "$d/home"
t() { env -i HOME="$d/home" PATH="$PATH" target/release/tagteam "$@"; }
t config path
t config set autoswitch.threshold 85
t config get autoswitch.threshold
t config list --json | head -c 200; echo
t displaced
t completions zsh | head -3
env -i PATH="$PATH" target/release/tagteam list; echo "exit $?"
```

Expected:
- `config path` prints `$d/home/.config/tagteam/config.toml`.
- `set` reports the write, and `get` prints `85`.
- `list --json` starts with `{"schemaVersion":1,"path":`.
- `displaced` says there are none.
- `completions zsh` starts with zsh's `#compdef tagteam` line.
- With `HOME` unset, `list` prints the `HOME` error and `exit 1` (§5).

`env -i` keeps the shell's `XDG_*`, `CLAUDE_*` and `TAGTEAM_*` variables out. None of these
touches the login keychain: `config`, `displaced` and `completions` touch no
Keychain item, and `list` with no store spawns no `security` (Appendix A.3).

- [ ] **Step 5: The plan's status**

Run: `grep -n '^\*\*Status:\*\*' docs/superpowers/plans/2026-10-01-tagteam-m5a-config-logging-displaced.md`

Expected: `3:**Status:** In progress`. The Execution notes set it when execution started, and it
stays so while the work and its reviews are active.
- If it still says `Approved`, set it to `**Status:** In progress` and commit that alone:
  `git commit -m "Mark the M5a plan in progress"`.
- Once the Draft MR is open, record it on the same line, `**Status:** In progress — <MR URL>`, in
  one commit: `git commit -m "Record the M5a merge request on the plan"`.
- The final pre-merge commit, after the pre-merge review, sets `**Status:** Implemented — <MR
  URL>`.

The spec stays `In progress` until M5b. Opening the MR and the pre-merge review follow the
global workflow and are not steps of this plan.
