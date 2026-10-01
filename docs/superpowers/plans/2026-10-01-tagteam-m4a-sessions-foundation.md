# tagteam M4a — Sessions Foundation Implementation Plan

**Status:** Approved

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Everything `tagteam run` stands on, apart from `run` itself:
- **Default-home epoch.** The default home gets an activation epoch, so no automatic capture undoes an explicit replacement.
- **Run-shell detection.** A tagteam command inside a session finds its run shell from the profile marker, and then sees the default home, never the profile.
- **Session-owned accounts.** These are detected from reservations and session records, and the refresh gate, `switch`, `remove`, usage collection and the views all honour them.
- **Lazy capture.** A quiescent profile's provenance is applied: a rotation is captured, and a conflict refuses.
- **Profile links.** They sync by allowlist.
- **Keychain naming.** It follows Claude Code 2.1.286, which drops the read fallbacks.

`run`'s launch, exit handling, bootstrap and validation (M4b) build on these.

**Architecture:**
- **Pure decisions in `tagteam-core`.** The provenance table (§12.5) and the stale-live-store rule in the outgoing classification (§9.4 step 4) are pure functions of fingerprints and flags.
- **Generic profile machinery in `tagteam-provider`.**
  - The marker, seed and links files.
  - The session-record parser and liveness, behind a new `ProcessProbe` port (§4.2).
  - The non-creating lock probe.
  - The share-entry matcher.
  - Twelve new `Provider` methods supply the agent-specific facts.
- **Claude Code's facts in `tagteam-cc`.** These are the profile credential read, the spelling, the share policy, the outer home and the profile item's deletion. `FakeAgent` implements the same methods with deliberately different shapes.
- **Policy in `tagteam-engine`.**
  - Run-shell detection and the effective environment.
  - Session state, provenance and lazy capture.
  - The session rules in the gate, `switch`, `remove` and `add`.
  - The session-owned usage branch.
  - Link sync.
  - The activation epoch in the store.
- **The CLI renders.** `list` and `status` show sessions. `statusline` resolves its provider and account from the run shell. Every command refuses inside a run shell whose marker is unreadable, except `statusline`.

**Tech Stack:** Rust (edition 2024), rusqlite (bundled), serde_json (`preserve_order`, `arbitrary_precision`), libc, `unicode-normalization` (already in `tagteam-cc`), clap 4, thiserror, tracing. Tests use tempfile, assert_cmd, `FakeKeychain`, `FakeClock`, `ScriptedHttp`, the new `FakeProcessProbe`, and `MockServer`.

**Spec:** `docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md`, signed off at `089b873` (M4 amendments `1e79bb9`, storage-write marking `59d5104`, rollback restore `f686839`, M5's amendments `089b873`, whose §10.3 delete order Task 9's `remove_locked` follows). Decision 16 amends §12.6; Michael signed it off on 2026-10-01, and Task 17 writes it into the spec. Read these before starting any task:
- §2, §3, §4.2, §4.3, §4.5, §5, §6.1, §6.2;
- §7.2, §7.3, §7.5, §8.1;
- §9.2, §9.4, §9.6, §10.1, §10.3;
- §12 (all of it), §13.1, §13.2, §13.5, §14.1, §15;
- Appendix A, Appendix B 50 and 57–63.

Section numbers below refer to that spec.

## Execution notes

- **Execution waits for M3a to merge into `main`.** M3a (`docs/superpowers/plans/2026-10-01-tagteam-m3a-signals-strategies.md`, on `m3-auto-switch`) changes almost every engine file this plan touches. The first execution step rebases `m4-run-sessions` onto that `main` and re-syncs this plan with M3a's final code: names, line numbers and the interfaces listed under "M3a interfaces this plan builds on". The re-sync is recorded under "Execution rulings" below before Task 1 starts.
- When execution starts, set this plan's `**Status:**` to `In progress` in one commit. The spec stays `In progress` until M5.
- **Run this plan in the `m4-run-sessions` worktree** (`~/Code/tagteam-m4-run-sessions`, Michael's `wt` worktree).
- Feature flags used by tests (unchanged): `tagteam-provider/file-keychain`, `tagteam-provider/mock-server`, `tagteam-engine/test-hooks`, `tagteam-cc/test-hooks`, and `tagteam/test-support`, which enables all of them. Engine tests that need hooks run with `cargo test -p tagteam-engine --features test-hooks`.
- Clippy must pass both with `--features tagteam/test-support` and with no features. Hook-only test code is gated on `test-hooks`.
- Every task runs `cargo fmt --all` before `cargo fmt --all --check`.
- Tests never reach the real network, the real HOME, or the login keychain (§15.1). Profile fixtures live under the fixture's `data_dir()/sessions/`. Session records in engine tests are judged by `FakeProcessProbe`, never by real pids.
- Test helpers that two integration-test files share live in `tests/common/mod.rs`. The canonical profile fixtures are Task 8's (`Fx::{profile_dir, write_marker, make_profile, shell_env, engine_located}`) and Task 9's (`Fx.process`, `LSTART`, `record_json`, `Fx::{write_seed, set_profile_credential, item_for_spelling, profile_item, hold_reservation, plant_record, live_record, dead_record}`). Later tasks add to them only what those lack, each once, in the earliest task that needs it.
- **Tasks 6 and 9 run as a non-root user.** Task 9's `a_directory_that_cannot_be_listed_counts_as_owned` relies on an unlistable `0o000` directory, as `block_rescue`'s tests already do, and Task 6's real-process tests probe the test process itself (`kill`, its start time and its arguments).
- **The Linux liveness code** (`SystemProcessProbe`'s `btime`, clock ticks, `comm` and `cmdline` branches, and the two real-process tests on Linux) is first run by CI's ubuntu job. The local Docker recipe (a non-root user with a passwd entry, `HOME` outside `/tmp`, the `docker` call unpiped) reproduces it before pushing.
- **Spellings are canonical.** On macOS, `/var/folders` canonicalizes to `/private/var/folders`, so a profile's spelling and its marker's `configDir` need not start with the fixture's `data_dir()`. Tests compare against `canonical_profile_path`, never the raw fixture path.
- **A note for M4b.** A defined-but-empty outer `CLAUDE_CONFIG_DIR` is recorded as `""`. M4b restores the outer home before it `exec`s plain `claude`, so it decides whether restoring that `""` counts as "exporting one" (Appendix A.1: "tagteam treats an empty value as unset, never exports one"); the record keeps `""`, so either choice stays open. A canonical profile path that is not UTF-8 has a lossy spelling (`profile_spelling`), so M4b's bootstrap should refuse it rather than export the lossy text. M4b also prints `SyncReport.warnings` on stderr and maps `ProfileSplit` to the launch's refusal (§12.5 step 3).

### M3a interfaces this plan builds on

When execution starts, these names come from M3a's merged code. The task text below is written against `main` before M3a, and re-sync swaps them in:
- `Env.cancel: Cancel`. `Env` construction sites gain M4a's `vars` beside it.
- `EngineError::Interrupted(i32)` and `EngineError::signal()`. M4a's new variants join the same `kind()` table.
- `LockError::Interrupted`. M4a's lock probe never waits, so it has no cancellation point.
- `switch.rs`'s usage strategies (`SwitchTarget::Usage`, `switch_planned`, `UsageStrategy`). Task 11's session-owned candidate filter applies to every candidate list, so rotation, `best` and `next-available` all skip session-owned accounts (§9.3).
- `collect.rs`'s cancellation points: before reserving (in `collect_one`) and before sending. Task 12 splits `send` into `send` and `send_credential`; M3a's "before sending" point belongs in `send_credential`, which the session branch shares, so the branch needs none of its own. `collect_usage`'s `CollectMode` destructuring gains `Scheduled`; Task 12's role computation sits after `live_accounts` either way.
- `live.rs`'s file-mode pin (M3a Task 7) and its storage-write lock task. Task 5's removal of the Keychain fallbacks edits the same functions: re-apply its single-item reads and the `report_item`/`remove_item` renames inside M3a's versions. Task 5 adds no lock call.
- **Whole-function replacements** in Tasks 8–11 may land on code M3a has changed: `app::run` (signal setup), `refresh_stored` (cancellation points), and `plan`, `freshen`, `rederive` and `candidate_order` (usage strategies). Re-apply each task's delta onto M3a's version; Task 11 lists what changes for the usage strategies. `rederive`'s session re-check is an exhaustive `match` on `req.target`, so `SwitchTarget::Usage` fails to compile until it is given the rotation's `Replan`. Task 9's guard in `add_live` and `add_token`, and Task 11's insertion in `transact`, are anchored edits, so that Tasks 2, 3 and 5 can rewrite the rest of those functions first.
- M3a's new settings keys join `every_key_is_read_from_a_full_file` beside Task 13's `share_extra`.

## Milestones

| Milestone | Scope |
|---|---|
| M1, M2a, M2b | Implemented (`main` at `3c1f458`) |
| M3a, M3b | Signals, cancellation and usage strategies; auto-switch (`m3-auto-switch`) |
| **M4a (this plan)** | Activation epoch and replacement evidence; Keychain naming without fallbacks; session records and liveness; profile files; the `Provider` session methods; run-shell detection and the outer home; session state and the destructive guard; provenance and lazy capture; the session rules in the gate and `switch`; the session-owned usage fetch; link sync; `list`, `status` and `statusline` in and around sessions; `FakeAgent` sessions |
| M4b | `tagteam run`: launch reservations, launch and exit handling, bootstrap and validation, seed and merge-back, the environment scrub, signals, `map`/`unmap`/`shell-init` |
| M5 | Export/import, `doctor`, `config` (writes), `displaced`, `purge`, `completions`, logging to file, `cargo xtask compat`, release |

**Deliberately absent from M4a, and why that is safe:**
- **No launch reservations are created.** Nothing in M4a starts a session, so the only profiles are test fixtures and, after M4b, real ones. M4a can only test reservations (§12.5), and its probe never creates a file.
- **No bootstrap.** A profile without a seed file is treated as not bootstrapped: provenance does not apply, and nothing is captured from it.
- **No `import`, no `purge`.** They are M5. The replacement evidence is wired into `add` and `add-token`, the two replacers that exist today, and `import` takes the same store call when M5 adds it.
- **`auto` (M3b) is not in this plan.** Task 9's `Engine::session_state` and Task 11's candidate filter are the interfaces M3b's tick uses to skip session-owned candidates.
- **A per-home `active_accounts`.** `active_accounts` is per provider, not per home, so an `add` run under a `CLAUDE_CONFIG_DIR` other than the default home records that account as current while the default home may still hold its old lineage. Run-shell detection (Task 8) restores the outer home for every command inside a session, so this needs the user to point `CLAUDE_CONFIG_DIR` elsewhere by hand. It is a spec gap, noted and not planned.

## Decisions

Rulings made while planning. Each names what it would cost if wrong.

1. **The migration is a ladder.** `schema.sql` stays the frozen version 1 DDL, and a `MIGRATION_V2` constant holds the `ALTER TABLE` statements and the backfill. A fresh file runs both steps; a version 1 file runs only the second. `SCHEMA_VERSION` becomes 2. Cost if wrong: one more migration pattern to unpick, before any release ships.
2. **The activation epoch lives on `active_accounts`, not on `accounts`.** `AccountRow` gains no field. `Store::activation` reads it, and `Store::live_store_stale(row)` answers §12.5's question. `set_active` and `commit_switch` take the epoch explicitly, and deleting an account clears an orphaned epoch in the same transaction. Cost if wrong: a column move.
3. **Replacement evidence is a store responsibility.** `begin_replacement` takes `live_names_account` (computed by the engine from the live identity, a file read with no Keychain) and writes `active_accounts` inside its own transaction. `LoginMeta.from_live` travels in `replacing_meta`, so `finish_replacement` records the new activation epoch atomically for `add`, and a reconciliation of a dead `add` does too. A new account's activation is still written after `commit_login`, in its own write: no replacement can stale-mark a new account, so that write needs no atomicity. Cost if wrong: one more transaction boundary.
4. **The stale-live-store rule is a pure fact.** `OutgoingFacts` gains `live_store_stale`, and `decide_outgoing` turns a capture into `Displace` when it is set. The class is unchanged, so logs still say `OursRotated` or `Unresolved`. The switch and recovery share it. Cost if wrong: one test literal in `classify.rs`; every other literal spreads `..facts()`.
5. **`Env` gains `vars`, provider-owned variables a registry asked for.** Claude Code keeps its two dedicated fields. `FakeAgent` reads `FAKEAGENT_HOME` from `vars`, and the CLI captures every provider's `session_dir_var()`, plus `CLAUDECODE`, at startup: in `Context::from_process`, at the process boundary, so an in-process test carries only the `vars` it sets. Cost if wrong: one map in `Env`.
6. **Run-shell detection and the effective `Env` happen once, at construction.** `tagteam_engine::session::detect_run_shell(env, registry)` returns the `RunShell` and the effective `Env`, with the outer home applied through the marker provider's `apply_outer_home`. `EngineConfig` carries both, so the about 45 `self.env` sites need no edit. `Env::inside_run_shell` is removed. Cost if wrong: one construction path to change.
7. **Session records go through a `ProcessProbe` port** (§4.2 lists it): `SystemProcessProbe` in production and `FakeProcessProbe` in tests. `EngineConfig` gains `process: Arc<dyn ProcessProbe>`. Liveness from `ps lstart` text is a match within ±1 s (Decision 16 says when a mismatch means a recycled pid), with no date crate: civil-to-epoch arithmetic. Cost if wrong: a port signature.
8. **Session state is computed on each call and never cached.** It costs one directory read and one non-blocking `flock` per reservation, plus one parse per record. `list` computes it for every account that has a profile directory. Cost if wrong: a cache, if a perf test ever misses (`list` stays under 50 ms, checked by the existing perf test).
9. **An unreadable profile credential stops automatic decisions without guessing.** The gate returns `Transient { kind: "profile-unreadable" }` and sends nothing. A switch to the account fails as `UnreadableAccount`. The session-owned usage fetch reports `keychain_unavailable`. Cost if wrong: an account that needs `doctor` (M5) instead of switching on a guess.
10. **Kinds.** `SessionOwned` gives `session-owned`, `ProfileConflict` gives `profile-conflict`, `RunShellUnreadable` gives `run-shell-unreadable` and `ProfileSplit` gives `profile-split`. The two new `last_error` tokens are `live-replaced` (§7.5 `Replaced`) and `profile-drifted` (§8.1 identity drift); both fall through to `usageStatus: unavailable`. Cost if wrong: renamed stable strings, before any release.
11. **The `list` session column appears only when some row is session-owned**, so the common layout stays byte-identical. It is one character wide (`▶`), placed after the position. In a run shell, the session's own row also gets the trailing note `this`. That is how §13.1's "`▶ this`" renders while staying aligned. Cost if wrong: a layout tweak.
12. **`remove` deletes a profile even when its marker is unreadable.** It deletes the hashed item for the profile's current canonical spelling, then the directory, with a warning that an item under an older spelling may remain. Refusing would leave an account that cannot be removed. Cost if wrong: one orphaned Keychain item, which `doctor` reports in M5.
13. **The statusline's provider order is** `--provider`, the run shell's marker, any provider that says the process was invoked by it (`Provider::invoked_by`), then `default_provider` (§13.5). With one statusline-capable provider registered, all four agree today.
14. **Link sync is engine-generic.** The provider supplies the policy: the source home, the allowlist, the must-share entries and the known-private patterns. `run.share_extra` adds names. Each `*` in a private pattern matches any run of characters. Cost if wrong: a richer matcher.
15. **`DoomedEntry.on_fallback` and `read_services` are removed.** With one Keychain item per spelling, `on_fallback` was constantly false, and its filters were dead code. Cost if wrong: none, the behaviour is unchanged.
16. **An `lstart` mismatch on its own is not proof of recycling** (amends §12.6; signed off by Michael on 2026-10-01). A record whose `procStart` is `lstart` text and whose pid exists is judged by the start-time match within ±1 s. A mismatch counts as recycled only when the process also does not mention the provider's launch command. A mismatch on a process that mentions it counts as live, and so does one whose `mentions` is unknown.
    - **Why:** on Linux the start time is computed from `/proc/stat` `btime`, which moves with the wall clock, and procps may compute its own `lstart` from the uptime clock. After a suspend or a clock step, a live session could otherwise read as recycled, which is the unsafe direction (§12.6: anything undetermined counts as live).
    - **Cost if wrong:** an account stays session-owned while an unrelated `claude` process holds its old pid. That is the safe direction.
    - Task 6's `record_is_live` and its tests implement it. Task 17 adds the spec edit to §12.6.
17. **`statusline` does not compute session state.** `AccountView.in_session` is computed for `list`, `status` and account-command results. The statusline's view leaves it `false`. That keeps the statusline away from profile directories, and it shows nothing session-related. Cost if wrong: one more file read per statusline call.
18. **Rulings taken inside the tasks**, one line each; the task named records the reasoning:
    - `add_token` refuses while the live identity is unreadable: guessing either way could erase another account's stale mark, or skip this one's (Task 2).
    - An `Activation` with no epoch is not stale (Task 1).
    - The stale displacement warning wins over the "no refresh token" one (Task 3).
    - The live login's account stays on the active collection path even when a session owns it, §12.8 (Task 12).
    - An absent or unreadable profile identity is `profile-drifted` (Task 12).
    - Marker problems in the session-owned fetch are `keychain-unavailable` (Task 12).
    - The sync's source home comes from the marker's `outer` (Task 13).
    - The unknown-entry scan covers the source home only (Task 13).
    - A join refuses a must-share split, and creates missing must-share sources (Task 13).
    - `status` shows the session line only for the marker's provider (Task 14).
    - `Engine::statusline` answers `NoLogin` under an unreadable marker (Task 15).
    - `remove`'s spelling warning stays a WARN log, and `doctor` (M5) reports orphaned items (Task 9).
    - A seed write failure after a capture stops the gate with an error (Task 10).

## Global Constraints

Every task's requirements include these. Values are copied from the spec.

- Platforms: macOS and Linux only (§1.2). Rust edition 2024, MSRV ≥ 1.85 (§16).
- Keychain access only through `/usr/bin/security` (§4.4, Appendix A.3). Profile Keychain items are named from the **recorded** spelling, never a re-derived one (§12.2, Appendix A.2): `"Claude Code-credentials-" + hex(sha256(NFC(spelling)))[..8]`.
- `~/.claude.json` and other CC files are never re-serialized; only the §9.5 splice writes them. Profile files that tagteam owns (`.tagteam-*`) are written with the atomic writer, mode 0600, and their directories are created 0700 (§5).
- Every read is tri-state (`Read<T>`); unreadable is never absent (§4.3, B.1). A malformed session record counts as **unreadable**, and an unreadable record makes the account session-owned (§10.3, §12.6).
- Anything about liveness that cannot be determined counts as live (§12.6).
- A capture from a profile requires the profile to be quiescent, its identity not drifted, and its provenance to say it rotated. It is never decided by expiry (§6.2, §12.5, B.52).
- A capture from the live store never takes a stale-marked live store (§6.2, §12.5, B.50).
- `statusline` does no network and no Keychain access, and never builds the `Http` adapter (§13.5). It stays within 10 ms p95.
- Log lines identify accounts by position or ID, never by email (§4.4, B.35).
- `--json` stdout is exactly one object; warnings go to stderr (B.36).
- Stable strings, verbatim:
  - **Error kinds:** `session-owned`, `profile-conflict`, `run-shell-unreadable`, `profile-split`, `inside-run-shell` (existing).
  - **`last_error` tokens:** `live-replaced`, `profile-drifted`, `token-expired`, `keychain-unavailable`, `no-access-token`.
  - **JSON fields:** `inSession`, `session`.
  - **Files:** `.tagteam-profile.json`, `.tagteam-seed.json`, `.tagteam-links.json`, `.tagteam-launch/`.
  - **Marker JSON:** `{"format": "tagteam-profile", "version": 1, "provider", "accountId", "configDir", "outer"}`.

## Review Focus

The five input classes or failure modes most likely to bite a user, which no task's main tests already exercise. Each has a test in its owning task:

1. **A run shell whose environment differs from the outer one.** For example, Claude's Bash tool runs with `XDG_DATA_HOME` changed, or the profile path is reached through a symlink. Detection is by marker alone, so it still works, and the gate never refreshes the default login's token from inside it. (Task 8)
2. **A session record that SIGKILL left behind, whose pid the OS has since recycled to an unrelated process.** The `lstart` mismatch, on a process that does not mention the launch command, makes it dead, so the account is not stuck session-owned forever (Decision 16). A record whose `procStart` is missing falls back to cswap's heuristic. (Task 6)
3. **`add-token` over the live account, then its access token expires.** §7.5 returns `Replaced` and the vault keeps the replacement. A later `switch` away displaces the old live lineage instead of capturing it. (Tasks 2–4)
4. **`statusline` inside a run shell whose account was removed meanwhile, or whose marker is corrupt.** It prints nothing, exits 0 and touches no Keychain item. Every other command names the marker and refuses. (Tasks 8, 15)
5. **A `~/.claude` entry tagteam does not know** (a new CC feature directory), and a shared file that CC has replaced by a regular file in the profile. The unknown entry stays private and is noted once. The split file warns, or refuses for `history.jsonl`, and neither copy is touched. (Task 13)

---

## File Structure

```
crates/tagteam-core/
  src/provenance.rs              NEW   ProvenanceVerdict, provenance() (Task 10)
  src/classify.rs                MOD   OutgoingFacts.live_store_stale (Task 3)
  src/lib.rs                     MOD   pub mod provenance (Task 10)
crates/tagteam-provider/
  src/env.rs                     MOD   Env.vars, Env::var, Env::capture_vars (Task 7); inside_run_shell removed (Task 8)
  src/liveness.rs                NEW   SessionRecord, RecordEntry, read_session_records, parse_lstart,
                                       ProcessProbe, SystemProcessProbe, FakeProcessProbe, record_is_live (Task 6)
  src/process.rs                 MOD   start_of becomes pub(crate) (Task 6)
  src/flock.rs                   MOD   LockProbe, probe_lock (Task 6)
  src/profile.rs                 NEW   profile_path, ProfileMarker, Seed, LinksRecord, RunShell,
                                       launch_reservations, entry_matches, canonical_profile_path (Task 7)
  src/provider.rs                MOD   DoomedEntry.on_fallback removed (Task 5); SharePolicy, MustShare, EntryKind,
                                       the twelve Provider session methods (Task 7); IdentitySurface.create_only (Task 13)
  src/security.rs                MOD   a test comment (Task 17)
  src/lib.rs                     MOD   modules and re-exports (Tasks 6, 7)
crates/tagteam-cc/
  src/naming.rs                  MOD   read_services removed (Task 5)
  src/live.rs                    MOD   single-item reads, snapshot, restore, doomed (Task 5); delete_items (Task 7)
  src/provider.rs                MOD   keychain_items (Task 5); the session methods (Task 7); create_only (Task 13)
  src/session.rs                 NEW   CC's share policy, outer home, profile reads and deletion (Task 7)
  src/lib.rs                     MOD   exports (Tasks 5, 7)
  tests/session.rs               NEW   the session methods (Task 7)
crates/tagteam-fake/
  src/paths.rs                   MOD   FAKEAGENT_HOME (Task 7)
  src/provider.rs                MOD   doomed (Task 5); sessions capability and methods (Task 7); create_only (Task 13)
crates/tagteam-engine/
  src/store/schema.sql           (unchanged: frozen v1)
  src/store/mod.rs               MOD   MIGRATION_V2 ladder, Activation, set_active/commit_switch epochs,
                                       live_store_stale, JournalRow.to_epoch (Task 1); LoginMeta.from_live,
                                       begin_replacement evidence, finish_replacement activation (Task 2)
  src/engine.rs                  MOD   unresolved_journal's to_epoch (Task 1); EngineConfig.run_shell,
                                       refuse_inside_run_shell (Task 8); EngineConfig.process (Task 9)
  src/testutil.rs, src/lazy_http.rs  MOD   the new EngineConfig fields (Tasks 8, 9)
  src/error.rs                   MOD   RunShellUnreadable (Task 8), SessionOwned (Task 9), ProfileConflict (Task 11),
                                       ProfileSplit (Task 13)
  src/session.rs                 NEW   detect_run_shell (Task 8); SessionState, Engine::session_state (Task 9)
  src/provenance.rs              NEW   ProfileCheck, identity_drifted, Engine::apply_provenance (Task 10)
  src/profiles.rs                NEW   SyncReport, Engine::sync_profile_links (Task 13)
  src/lifecycle.rs               MOD   add_live's epoch (Task 1); replacement evidence (Task 2);
                                       destructive guard, profile removal (Task 9)
  src/switch.rs                  MOD   commit epoch, journal to_epoch (Task 1); stale displace (Task 3);
                                       the on_fallback filter (Task 5); session rules (Task 11)
  src/recover.rs                 MOD   commit epoch (Task 1); the row's to_epoch, stale displace (Task 3)
  src/active.rs                  MOD   ActiveOutcome::Replaced (Task 4); the on_fallback filter (Task 5)
  src/refresh.rs                 MOD   owner_of uses session state (Task 9); step-3 provenance (Task 10)
  src/collect.rs                 MOD   Replaced mapping (Task 4); the session-owned branch (Task 12)
  src/views.rs                   MOD   live-replaced row (Task 4); profile-drifted row (Task 12);
                                       AccountView.in_session, ShellAccount (Task 14); statusline in a run shell (Task 15)
  src/settings.rs                MOD   run.share_extra (Task 13)
  src/lib.rs                     MOD   modules (Tasks 8, 10, 13)
  tests/common/mod.rs            MOD   activation helpers (Task 1); replacement helpers (Task 2); inert items (Task 5);
                                       profile fixtures (Task 8); Fx.process, seeds, records, reservations (Task 9);
                                       set_profile_identity, profile_credential (Task 12); make_profile_for and the
                                       create-only comparison (Task 13); FakeFx::engine_located (Task 15)
  tests/run_shell.rs             NEW   (Task 8)
  tests/session.rs               NEW   (Task 9; Task 11 appends)
  tests/provenance.rs            NEW   (Task 10; Task 11 appends)
  tests/session_usage.rs         NEW   (Task 12)
  tests/profiles.rs              NEW   (Task 13)
  tests/session_views.rs         NEW   (Task 14; Task 15 appends)
  tests/fake_sessions.rs         NEW   (Task 16)
  tests/*.rs                     MOD   existing tests each task's Files list names
crates/tagteam/
  Cargo.toml                     MOD   tagteam-fake as a dev-dependency (Task 15)
  src/app.rs                     MOD   Context::from_process captures, registry first, locate, unreadable-marker
                                       refusal (Task 8); EngineConfig.process (Task 9); status session (Task 14);
                                       run_statusline (Task 15)
  src/statusline.rs              MOD   located engine (Task 8); EngineConfig.process (Task 9);
                                       resolve_provider, run-shell account (Task 15)
  src/render.rs                  MOD   inSession, ▶ column, status session (Task 14)
  tests/common/mod.rs            MOD   cc_profile, hold_launch (Task 14)
  tests/run_shell_cli.rs         NEW   (Task 8; Tasks 14 and 15 append a module each)
  tests/perf.rs                  MOD   statusline in a run shell (Task 15)
docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md  MOD  §12.6 (Decision 16); Appendix A.1, A.3 probe facts (Task 17)
```

---

## Interface Contract

Every task implements exactly these names and signatures. A task that finds one unworkable stops and reports it rather than inventing a variant, because other tasks' code is written against this list.

### `tagteam-core`

**`src/provenance.rs`** (Task 10), `pub mod provenance;` in `lib.rs`, re-exported at the crate root:

```rust
/// §12.5's table, as a pure function of three generation fingerprints and the stale mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProvenanceVerdict {
    /// P = V. `reseed`: the seed differs from V and moves to it.
    InStep { reseed: bool },
    /// P ≠ V, V = S, not stale-marked: the profile rotated; capture P, and the seed becomes P.
    Capture,
    /// P ≠ V, P = S: the vault moved on; P is older and may be consumed. Never captured.
    VaultMovedOn,
    /// Stale-marked and P ≠ V (with P ≠ S): the explicit replacement wins. Never captured.
    ReplacementWins,
    /// P ≠ V, P ≠ S, V ≠ S, not stale-marked: both moved in an unknown order.
    Conflict,
}

/// `p`, `v`, `seed` are generation fingerprints (`sha256:…`, §2). Rows are checked in the
/// spec's order: P = V first, then P = S, then V = S, then the stale mark.
pub fn provenance(p: &str, v: &str, seed: &str, stale_marked: bool) -> ProvenanceVerdict;
```

(V = S with the stale mark set is `ReplacementWins`: a replacement never loses to a capture.)

**`src/classify.rs`** (Task 3): `OutgoingFacts` gains

```rust
    /// §9.4 step 4 / §12.5: the live store is stale-marked for the outgoing account, so a
    /// capture would undo an explicit replacement. Turns a capture into `Displace`.
    pub live_store_stale: bool,
```

`decide_outgoing` keeps the class and returns `OutgoingAction::Displace` for any class whose action would be `CaptureToVault` when `live_store_stale` is true.

### `tagteam-provider`

**`src/env.rs`** (Tasks 7, 8):

```rust
pub struct Env {
    // …existing fields…
    /// Provider-owned variables the registry asked for (§4.5 `session_dir_var`, `CLAUDECODE`),
    /// captured by `capture_vars`. Empty in `for_test`.
    pub vars: std::collections::BTreeMap<String, std::ffi::OsString>,
}
impl Env {
    /// Reads each named variable from the process environment into `vars` (absent: not inserted).
    pub fn capture_vars(&mut self, names: &[&str]);
    pub fn var(&self, name: &str) -> Option<&std::ffi::OsStr>;
}
// `Env::inside_run_shell` is removed (Task 8).
```

**`src/flock.rs`** (Task 6):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockProbe { Missing, Free, Held }
/// Tests `path` with a non-blocking `flock`, opening it read-only and never creating it.
pub fn probe_lock(path: &std::path::Path) -> std::io::Result<LockProbe>;
```

**`src/liveness.rs`** (Task 6), `pub mod liveness;`, re-exported:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRecord {
    pub pid: u32,
    pub proc_start: Option<String>,
    pub started_at_ms: Option<i64>,
    pub kind: Option<String>,
}
/// §12.6: non-object JSON, a missing or non-integer or out-of-range `pid`, depth > 32, or
/// invalid UTF-8 is `Err(detail)`; unknown fields are ignored.
pub fn parse_session_record(bytes: &[u8]) -> Result<SessionRecord, String>;

#[derive(Debug, Clone, PartialEq)]
pub enum RecordEntry { Record(SessionRecord), Unreadable { path: std::path::PathBuf, detail: String } }
/// Every `*.json` in `dir`, in name order. A missing `dir` is `Present(vec![])`; a `dir` that
/// cannot be listed is `Unreadable`.
pub fn read_session_records(dir: &std::path::Path) -> Read<Vec<RecordEntry>>;

/// `ps -o lstart=` with `LC_ALL=C TZ=UTC` (`Thu Oct  1 12:34:56 2026`) to epoch seconds.
/// The weekday must be one of the seven names, and is not checked against the date.
pub fn parse_lstart(s: &str) -> Option<i64>;

pub trait ProcessProbe: Send + Sync {
    /// `kill(pid, 0)`: `Some(true)` on success or `EPERM`, `Some(false)` on `ESRCH`, `None` otherwise.
    fn exists(&self, pid: u32) -> Option<bool>;
    /// The process's start time in epoch seconds.
    fn start_time_s(&self, pid: u32) -> Option<i64>;
    /// Linux `/proc/<pid>/stat` field 22, for the all-digit legacy `procStart`.
    fn start_ticks(&self, pid: u32) -> Option<u64>;
    /// Whether the executable name or the arguments contain `needle`.
    fn mentions(&self, pid: u32, needle: &str) -> Option<bool>;
}
pub struct SystemProcessProbe;
impl ProcessProbe for SystemProcessProbe { /* proc_pidinfo + KERN_PROCARGS2 / /proc */ }

/// Tests only (no feature gate: it touches nothing). Unknown pids are `exists: Some(false)`.
#[derive(Default)]
pub struct FakeProcessProbe { /* Mutex<BTreeMap<u32, FakeProcess>> */ }
#[derive(Debug, Clone, Default)]
pub struct FakeProcess {
    pub exists: Option<bool>, pub start_time_s: Option<i64>, pub start_ticks: Option<u64>,
    pub mentions_launch: Option<bool>,
}
impl FakeProcessProbe {
    pub fn new() -> Self;
    pub fn set(&self, pid: u32, p: FakeProcess);
}

/// §12.6: live if the pid exists and still belongs to the record's writer. `launch_command`
/// is the provider's (`claude`), used by the heuristic for an absent `procStart` and, under
/// Decision 16, to judge an `lstart` mismatch (recycled only if the process does not mention it).
pub fn record_is_live(probe: &dyn ProcessProbe, r: &SessionRecord, launch_command: &str) -> bool;
```

**`src/profile.rs`** (Task 7), `pub mod profile;`, re-exported:

```rust
pub const MARKER_FILE: &str = ".tagteam-profile.json";
pub const SEED_FILE: &str = ".tagteam-seed.json";
pub const LINKS_FILE: &str = ".tagteam-links.json";
pub const LAUNCH_DIR: &str = ".tagteam-launch";

/// `<data_dir>/sessions/<id>` (§5).
pub fn profile_path(env: &Env, id: &AccountId) -> std::path::PathBuf;
/// `realpath` of an existing profile directory (§12.2 "One spelling", before the provider's
/// own normalization).
pub fn canonical_profile_path(profile: &std::path::Path) -> std::io::Result<std::path::PathBuf>;

#[derive(Debug, Clone, PartialEq)]
pub struct ProfileMarker {
    pub provider: ProviderId,
    pub account_id: AccountId,
    /// The exported spelling (§12.2).
    pub config_dir: String,
    /// The provider's record of the outer home (§4.5 `outer_home`).
    pub outer: serde_json::Value,
}
impl ProfileMarker {
    /// `Absent`: no marker file. `Unreadable`: present but not a valid marker (§12.8).
    pub fn read(profile: &std::path::Path) -> Read<ProfileMarker>;
    /// Atomic, 0600; creates `profile` 0700 if absent.
    pub fn write(&self, profile: &std::path::Path) -> std::io::Result<()>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seed {
    /// The login epoch the profile was bootstrapped under (§12.5).
    pub login_epoch: i64,
    /// The generation fingerprint the profile last agreed on with the vault.
    pub seed_fp: String,
    /// Set by M4b's per-launch check when validation reported `invalid` (§12.3).
    pub needs_bootstrap: bool,
}
impl Seed {
    pub fn read(profile: &std::path::Path) -> Read<Seed>;
    pub fn write(&self, profile: &std::path::Path) -> std::io::Result<()>;
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LinksRecord {
    /// Entry name → the fully resolved source the link points at, for every link tagteam made.
    pub links: std::collections::BTreeMap<String, std::path::PathBuf>,
    /// Unknown entries already noted once (§12.2).
    pub noted_unknown: std::collections::BTreeSet<String>,
}
impl LinksRecord {
    pub fn read(profile: &std::path::Path) -> Read<LinksRecord>;   // Absent → caller uses Default
    pub fn write(&self, profile: &std::path::Path) -> std::io::Result<()>;
}

/// `<profile>/.tagteam-launch/*.lock`, each probed with `probe_lock`. A missing directory is
/// `Present(vec![])`.
pub fn launch_reservations(profile: &std::path::Path) -> Read<Vec<(std::path::PathBuf, LockProbe)>>;

/// A share-list entry against a directory entry name: exact, or with each `*` matching any run
/// of characters, possibly empty (`*.lock`, `daemon*`, `.*_auth_refresh-*`).
pub fn entry_matches(pattern: &str, name: &str) -> bool;

/// §12.8: where a process stands.
#[derive(Debug, Clone, PartialEq)]
pub enum RunShell {
    Outside,
    Inside { profile: std::path::PathBuf, marker: ProfileMarker },
    Unreadable { marker: std::path::PathBuf, detail: String },
}
```

**`src/provider.rs`** (Tasks 5, 7, 13):

```rust
// Task 5: `DoomedEntry` loses `on_fallback`; it is `{ pub bytes: Read<Vec<u8>> }`.

// Task 13: IdentitySurface gains
    /// Entries tagteam may create, empty, only when absent (§3's create-only row).
    pub create_only: Vec<PathBuf>,

// Task 7:
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind { Dir, File }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MustShare { pub name: &'static str, pub kind: EntryKind }

/// §12.2: what a profile shares with the outer home.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharePolicy {
    /// The outer home's config dir (CC: the resolved default config home).
    pub source: PathBuf,
    /// Linked when present in `source`.
    pub shared: Vec<&'static str>,
    /// Created empty in `source` when absent, then linked; a real copy in a profile refuses.
    pub must_share: Vec<MustShare>,
    /// Never linked; patterns for `entry_matches`.
    pub private: Vec<&'static str>,
}

pub trait Provider: Send + Sync {
    // …existing methods…
    /// §4.5 "Parallel sessions" (Task 7).
    fn launch_command(&self) -> &'static str;
    /// The variable that names a profile (CC: `CLAUDE_CONFIG_DIR`); `None` without sessions.
    fn session_dir_var(&self) -> Option<&'static str>;
    /// The directory `env` points this provider at, if set and non-empty.
    fn session_dir(&self, env: &Env) -> Option<PathBuf>;
    /// §12.2: the record of the home `env` resolves to, stored as the marker's `outer`.
    fn outer_home(&self, env: &Env) -> Value;
    /// §12.8: `env` with this provider's home variables restored from `outer`.
    fn apply_outer_home(&self, env: &Env, outer: &Value) -> Result<Env, ProviderError>;
    /// §12.2: the exported spelling for a canonical profile path (CC: NFC of the path).
    fn profile_spelling(&self, canonical: &Path) -> String;
    /// §12.2 (Task 7 returns it; Task 13 consumes it).
    fn share_policy(&self, env: &Env) -> SharePolicy;
    /// Where the profile's session records live (CC: `<profile>/sessions`).
    fn session_records_dir(&self, profile: &Path) -> PathBuf;
    /// §8.1, §12.5: the profile's credential, read as the agent reads it, for `spelling`.
    fn read_profile_credential(&self, env: &Env, spelling: &str) -> Read<Credential>;
    /// §12.5 "Identity drift": the profile's login identity (CC: its `.claude.json` `oauthAccount`).
    fn profile_identity(&self, env: &Env, spelling: &str) -> Read<Identity>;
    /// §10.3: deletes the agent-owned credential items for `spelling` and verifies them gone
    /// (CC macOS: the hashed Keychain item; otherwise nothing outside the directory).
    fn delete_profile_credential(&self, env: &Env, spelling: &str) -> Result<(), ProviderError>;
    /// §13.5: whether `env` is a process this agent started (CC: `CLAUDECODE` or `CLAUDE_CONFIG_DIR`).
    fn invoked_by(&self, env: &Env) -> bool;
}
```

`SharePolicy`, `MustShare` and `EntryKind` are re-exported at the crate root.

### `tagteam-cc`

**`src/naming.rs`** (Task 5): `read_services` is removed, and `keychain_service` is the one item per axis. `lib.rs` stops exporting `read_services`.

**`src/session.rs`** (Task 7), crate-private helpers the provider methods call:

```rust
pub(crate) const CC_SHARED: &[&str] = &[
    "CLAUDE.md", "settings.json", "keybindings.json",
    "agents", "commands", "skills", "plugins", "hooks", "output-styles", "themes", "rules",
    "workflows", "file-history", "paste-cache", "shell-snapshots", "session-env",
];
pub(crate) const CC_MUST_SHARE: &[(&str, EntryKind)] = &[("projects", EntryKind::Dir), ("history.jsonl", EntryKind::File)];
pub(crate) const CC_PRIVATE: &[&str] = &[
    ".credentials.json", ".claude.json", ".claude-*-oauth.json", ".config.json",
    "sessions", "ide", "jobs", "daemon", "daemon.*", "daemon-auth-cooldown", "daemon-auth-status.json",
    "backups", "cache", "mcp-needs-auth-cache.json", "stats-cache.json",
    "policy-limits.json*", "remote-settings.json", "remote-settings-consent.json",
    "remote-settings-helper-consent", ".session_ingress_token", "hfi-auth.json",
    "state", "seed-admin", "bridge-spawn", "chrome", "debug", "feedback", "routines",
    "settings.local.json", ".last-cleanup", ".cc-writes", ".device-keys.json",
    "*.lock", "*.lock.owner", ".storage-write", ".*_auth_refresh-*", ".tagteam-*",
];
/// `outer` is `{"CLAUDE_CONFIG_DIR": <string>|null, "CLAUDE_SECURESTORAGE_CONFIG_DIR": <string>|null}`:
/// null means undefined; "" is kept (defined-but-empty, Appendix A.1).
pub(crate) fn outer_home(env: &Env) -> Value;
pub(crate) fn apply_outer_home(env: &Env, outer: &Value) -> Result<Env, ProviderError>;
/// The profile `Env`: `claude_config_dir = Some(spelling)`, `claude_securestorage_config_dir = None`.
pub(crate) fn profile_env(env: &Env, spelling: &str) -> Env;
```

`ClaudeCode` implements the session methods with these helpers:
- `launch_command` is `"claude"`, and `session_dir_var` is `Some("CLAUDE_CONFIG_DIR")`.
- `read_profile_credential` is `LiveStore::read_credential(&profile_env, &CcPaths::resolve(&profile_env))`.
- `profile_identity` is `config::live_identity(&CcPaths::resolve(&profile_env))`.
- `delete_profile_credential` uses `LiveStore`'s remove path on the OAuth and managed-key items of `profile_env`, verified with the existence probe. On Linux it is a no-op.
- `invoked_by` is `env.var("CLAUDECODE") == Some("1") || session_dir(env).is_some()`.

### `tagteam-fake`

**`src/paths.rs`** (Task 7): `FakePaths::resolve` uses `env.var("FAKEAGENT_HOME")` when set and non-empty, else `<home>/.fakeagent`. Its sessions are deliberately unlike CC's:
- `capabilities().sessions` is `true`, `launch_command` is `"fakeagent"`, and `session_dir_var` is `Some("FAKEAGENT_HOME")`.
- `share_policy`: shared `["notes", "prefs.json"]`, must-share `[("journal.log", File)]`, private `["credential.json", "identity.json", "procs", ".live.lock", ".tagteam-*"]`.
- Session records live in `<profile>/procs`.
- `outer_home` is `{"FAKEAGENT_HOME": <string>|null}`.
- `profile_spelling` is the path as is.
- `read_profile_credential` reads `<spelling>/credential.json`.
- `profile_identity` reads `<spelling>/identity.json`.
- `delete_profile_credential` is a no-op.
- `invoked_by` is `false`.

### `tagteam-engine`

**`src/store/mod.rs`** (Tasks 1, 2):

```rust
const SCHEMA_VERSION: i64 = 2;
const MIGRATION_V2: &str = "ALTER TABLE active_accounts ADD COLUMN login_epoch INTEGER;
ALTER TABLE switch_journal ADD COLUMN to_epoch INTEGER;
UPDATE active_accounts SET login_epoch =
  (SELECT login_epoch FROM accounts WHERE accounts.id = active_accounts.account_id)
  WHERE account_id IS NOT NULL;";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activation { pub account: AccountId, pub epoch: Option<i64> }

impl Store {
    pub fn activation(&self, provider: &ProviderId) -> Result<Option<Activation>, StoreError>;
    /// `epoch` is stored with `id`; `id = None` stores `None` for both.
    pub fn set_active(&self, provider: &ProviderId, id: Option<&AccountId>, epoch: Option<i64>) -> Result<(), StoreError>;
    pub fn commit_switch(&self, provider: &ProviderId, to: &AccountId, epoch: i64, event: &EventRow) -> Result<(), StoreError>;
    /// §12.5: `active_accounts` names `row` with an epoch other than `row.login_epoch`.
    pub fn live_store_stale(&self, row: &AccountRow) -> Result<bool, StoreError>;
    /// §12.5 step 1 plus the default home's evidence.
    pub fn begin_replacement(&self, id: &AccountId, fp: &str, meta: &LoginMeta<'_>, live_names_account: bool) -> Result<(), StoreError>;
    // finish_replacement: unchanged signature; also records the activation epoch when
    // replacing_meta says `from_live` and `active_accounts` still names the account.
    // delete_account: also NULLs an orphaned epoch.
}
pub struct LoginMeta<'a> { /* existing */ pub from_live: bool }
pub struct JournalRow { /* existing */ pub to_epoch: Option<i64> }
```

**`src/error.rs`** (Tasks 8, 9, 11, 13), each variant pinned in `kind_is_pinned_for_every_variant`:

```rust
    #[error("position {position} ({label}) is in use by a `tagteam run` session; exit that session first")]
    SessionOwned { position: u32, label: String },          // "session-owned"
    #[error("position {position} ({label})'s session profile and the vault both moved since they last agreed; log in again with `tagteam add` to resolve it")]
    ProfileConflict { position: u32, label: String },       // "profile-conflict"
    #[error("the run-shell marker {} cannot be read ({detail})", marker.display())]
    RunShellUnreadable { marker: PathBuf, detail: String }, // "run-shell-unreadable"
    #[error("{} is a real copy where {} should be linked; merge the two by hand, then remove the copy", profile.display(), shared.display())]
    ProfileSplit { profile: PathBuf, shared: PathBuf },     // "profile-split"
```

`ProfileSplit`'s second field is `shared`, not `source`: thiserror treats a field named `source` as the error's source, which a `PathBuf` cannot be.

**`src/engine.rs`** (Tasks 8, 9):

```rust
pub struct EngineConfig {
    // …existing fields…
    /// §4.2's process port, for session records (Task 9).
    pub process: Arc<dyn ProcessProbe>,
    /// §12.8, from `detect_run_shell`; `env` is already the effective (outer) environment.
    pub run_shell: RunShell,
}
impl Engine {
    pub fn run_shell(&self) -> &RunShell;
}
// refuse_inside_run_shell: Outside → Ok; Inside → InsideRunShell; Unreadable → RunShellUnreadable.
```

**`src/session.rs`** (Tasks 8, 9), `pub mod session;`:

```rust
/// §12.8: the first registered provider with sessions whose `session_dir(env)` holds a marker
/// decides. Returns the run shell and the effective `Env` (the marker provider's
/// `apply_outer_home`, or `env` unchanged when `Outside` or `Unreadable`). The provider whose
/// variable found the marker must be the provider the marker names; a mismatch is `Unreadable`.
pub fn detect_run_shell(env: &Env, registry: &ProviderRegistry) -> (RunShell, Env);

#[derive(Debug, Clone, PartialEq)]
pub enum SessionState {
    NoProfile,
    Quiescent { profile: PathBuf },
    Owned { profile: PathBuf },
    /// A reservation or a record could not be read: counts as owned (§10.3, §12.6).
    Unreadable { profile: PathBuf, detail: String },
}
impl SessionState {
    pub fn owned(&self) -> bool;            // Owned | Unreadable
    pub fn profile(&self) -> Option<&Path>;
}
impl Engine {
    /// §12.5: reservations (any `Held`) and session records (`record_is_live`, any `Unreadable`).
    pub fn session_state(&self, p: &dyn Provider, row: &AccountRow) -> Result<SessionState, EngineError>;
}
```

**`src/provenance.rs`** (Task 10), `pub mod provenance;`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileCheck {
    /// No profile, not quiescent, no seed (never bootstrapped), or its identity drifted.
    NotApplicable,
    InStep,
    Captured,
    VaultMovedOn,
    ReplacementWins,
    Conflict,
    /// The seed, the marker, the profile credential or the profile identity could not be read,
    /// or the marker is missing beside a seed, or names another account or provider (Decision 9).
    Unreadable(String),
}
impl Engine {
    /// §12.5 under `lock` (the account's): reads the seed, the marker's spelling and the
    /// profile credential; applies `tagteam_core::provenance`; captures through
    /// `persist_generation` and moves the seed on `Capture`; reseeds on `InStep { reseed: true }`.
    pub(crate) fn apply_provenance(&self, p: &dyn Provider, row: &AccountRow, lock: &AccountLock) -> Result<ProfileCheck, EngineError>;
}
/// §12.5 "Identity drift", the one definition (Task 12's `collect.rs` calls it too): the email
/// differs (the label, when an identity has none), or the organization does when both name one.
pub(crate) fn identity_drifted(identity: &Identity, row: &AccountRow) -> bool;
```

**`src/profiles.rs`** (Task 13), `pub mod profiles;`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SyncReport { pub created: Vec<String>, pub removed: Vec<String>, pub warnings: Vec<String> }
impl Engine {
    /// §12.2's sync. `joining`: a session already runs, so only missing links are created.
    /// It takes no lock: its caller (M4b's launch) holds `MutationGuard` and the account lock.
    pub fn sync_profile_links(&self, p: &dyn Provider, profile: &Path, joining: bool) -> Result<SyncReport, EngineError>;
}
```

**`src/active.rs`** (Task 4): `ActiveOutcome::Replaced` (§7.5 step 2), returned before any reconciliation, and before any lock beyond what step 2 already holds.

**`src/views.rs`** (Tasks 14, 15):

```rust
pub struct AccountView { /* existing */ pub in_session: bool }
#[derive(Debug, Clone)]
pub enum ShellAccount { NotInShell, Managed(AccountRow), Unmanaged }
impl Engine {
    pub fn shell_account(&self) -> Result<ShellAccount, EngineError>;
}
// Engine::statusline: in a run shell, the marker's account by id (no live-identity cache, no
// `.claude.json` parse); `Unmanaged` → StatuslineView::NoLogin, and so does an unreadable marker.
```

`in_session` is computed for `list` and `status` (`account_view_of`) and for account-command results (`account_view`). `account_view_with` takes it as an explicit `in_session: bool` parameter and never computes it; the statusline's path (`with_pace = false`) passes `false` (Decision 17).

**`src/settings.rs`** (Task 13): `Settings.share_extra: Vec<String>` (`run.share_extra`, the provider's table first; names that are empty, `.` or `..`, or contain `/` are dropped with a warning).

**`src/collect.rs`** (Tasks 4, 12): `ActiveOutcome::Replaced` → `failed("live-replaced")`, plus a warning naming `tagteam switch <N> --force`. A session-owned account takes a third branch, `Collection::session`, with no lock, no refresh and no retry (§8.1).

**`src/switch.rs`** (Task 11): `Engine::switch_candidate(&self, p: &dyn Provider, row: &AccountRow) -> Result<bool, EngineError>` (crate-private) is `is_candidate(row)` and not session-owned; every candidate list goes through it. When the gate answers `Owned(Session)`, `freshen` maps a direct target → `SessionOwned`; a rotation re-plans (§9.3: every strategy skips session-owned candidates). The gate's `Conflict` → `ProfileConflict`.

### `tagteam` (CLI)

- `Context::from_process` calls `env.capture_vars` with `CLAUDECODE` plus every production provider's `session_dir_var()` (`app::session_vars(&build_registry(&ctx))`). That happens at the process boundary, never in `app::run`, so in-process tests carry exactly the `vars` they set (§15.1) (Task 8).
- `app::run` builds the registry before the engine and calls `app::locate(env, &registry)`, which only detects (`detect_run_shell`); it never captures. On `RunShell::Unreadable` it fails every command except `statusline` with `run-shell-unreadable` (Task 8).
- `render::row_json` adds `inSession: true` when set. `render::status_json` and `status_human` add the session's account in a run shell (Task 14).
- `statusline::engine` resolves the provider in Decision 13's order through a new `statusline::resolve_provider` (Task 15). From Task 15 on it detects the run shell itself (`detect_run_shell`, over its walled registry) rather than through `app::locate`, and returns `(Engine, ProviderId, Arc<LazyHttp>, Arc<NoKeychain>)`.

---

## Tasks

| # | Task | Depends on |
|---|---|---|
| 1 | Store v2: migration ladder, activation epoch, journal `to_epoch` | — |
| 2 | Replacement evidence and `add`'s activation epoch | 1 |
| 3 | Switch and recovery: record the epoch, displace a stale live store | 1, 2 |
| 4 | §7.5 `Replaced` and its usage mapping | 1, 2, 3 |
| 5 | Keychain naming: one item per spelling | — |
| 6 | Session records, `lstart` liveness, `ProcessProbe`, the lock probe | — |
| 7 | Profile files and the `Provider` session methods (CC, FakeAgent) | 5, 6 |
| 8 | Run-shell detection, the outer home, refusals (L312) | 7 |
| 9 | Session state, `Owned(Session)`, the destructive guard, profile removal | 6, 7, 8 |
| 10 | Provenance and lazy capture in the gate | 9 |
| 11 | `switch`'s session rules and refusal kinds (K17) | 9, 10 |
| 12 | The session-owned usage fetch | 8, 9, 10 |
| 13 | Link sync by allowlist, `run.share_extra`, create-only surface | 7, 8 |
| 14 | `list` and `status`: `inSession`, `▶`, the session's account | 8, 9, 13 |
| 15 | `statusline`: provider order and the run shell's account | 8, 13, 14 |
| 16 | `FakeAgent` sessions across the engine; the negative surface test | 7–14 |
| 17 | Spec probe facts; final verification | all |

---

### Task 1: Store v2: migration ladder, activation epoch, journal `to_epoch`

The default home gets the activation epoch §12.5 judges it by: `active_accounts.login_epoch`,
the account's `login_epoch` when tagteam made it live. The switch journal carries the target's
epoch, so a forward recovery can record it. Both are new columns, so the store moves to schema
version 2. Today's `migrate` applies one blob when the version is below `SCHEMA_VERSION`. It
becomes a ladder (Decision 1): `schema.sql` stays the frozen version 1 DDL, and `MIGRATION_V2`
holds the `ALTER TABLE` statements and the backfill.

**Readings of the spec this task commits to:**
- **The ladder is an array.** `MIGRATIONS = [SCHEMA_V1, MIGRATION_V2]`, and step `n` takes a
  file from version `n` to `n + 1`. A compile-time assertion ties `SCHEMA_VERSION` to its
  length, so a future step cannot be added without the version bump. The alternative was one
  `if version < n` per step. It reads the same for two steps, but nothing stops the next step
  from forgetting the bump.
- **Both columns are written on every upsert.** `SET_ACTIVE_SQL` sets `account_id` and
  `login_epoch` on conflict. Today it sets only `account_id`, so an epoch left over from the
  previous account would stale-mark the next one.
- **An epoch-less activation is no evidence.** `set_active(p, Some(id), None)` is allowed (the
  contract's signature), and no production caller makes it. `live_store_stale` reads it as not
  stale, the same way the migration takes the live store as current.
- **`delete_account` clears the orphan after the delete.** `ON DELETE SET NULL` clears
  `account_id`, and an FK action cannot clear a second column. The same transaction then runs
  `UPDATE active_accounts SET login_epoch = NULL WHERE account_id IS NULL`, which states §6.1's
  "NULL only with `account_id`" directly.
- **The signature changes force the values at their call sites, so this task writes the final
  ones:**
  - the switch's commit passes `target.login_epoch` (§9.4 step 9);
  - the journal row carries `Some(target.login_epoch)` (§9.4 step 6);
  - `add` records `Some(account.login_epoch)` (§10.1).
  
  Recovery's commit passes `to.login_epoch` for now. That is §9.6's fallback for a row
  without `to_epoch`. Task 3 makes recovery read the row's own epoch, with its own failing test.

**Files:**
- Modify: `crates/tagteam-engine/src/store/mod.rs`:
  - 23–27, the schema constants: `MIGRATION_V2`, `MIGRATIONS`, `SCHEMA_VERSION = 2`
  - 101–114, `JournalRow.to_epoch`
  - after 123, a new `Activation`
  - 125–168, `journal_to_json` and `journal_from_json`
  - 198–201, `SET_ACTIVE_SQL`
  - 244–264, `journal_from_row`
  - 328–337, `set_active_on`
  - 459–490, `migrate`
  - 841–853, `delete_account`
  - 855–874, `active` and `set_active`, plus the new `activation` and `live_store_stale`
  - 893–907, `commit_switch`
  - 942–964, `insert_journal`
- Modify: `crates/tagteam-engine/src/switch.rs`. In `transact`, the journal literal at 1205–1221 gets `to_epoch`. In `apply`, the `commit_switch` call at 1493 gets the epoch.
- Modify: `crates/tagteam-engine/src/recover.rs`. In `finish_forward`, the `commit_switch` call at 285 gets the epoch.
- Modify: `crates/tagteam-engine/src/lifecycle.rs`. In `add_live`, the `set_active` call at 498 gets the epoch.
- Modify: `crates/tagteam-engine/src/engine.rs`. The test helper `tests::unresolved_journal` at 354–366 gets `to_epoch: None`.
- Modify: `crates/tagteam-engine/tests/common/mod.rs`. `crash_row` at 148–162 gets `to_epoch`. New fixture methods: `Fx::activation` and `Fx::live_store_stale`.
- Test: `crates/tagteam-engine/tests/store.rs`, `crates/tagteam-engine/tests/switch.rs`, `crates/tagteam-engine/tests/add.rs`
- Modify (only to compile against the new signatures):
  - `crates/tagteam-engine/tests/switch.rs:60`
  - `crates/tagteam-engine/tests/collect.rs:1283`
  - `crates/tagteam-engine/tests/recover.rs:559`
  - `crates/tagteam-engine/tests/switch_rollback.rs:60`
  - `crates/tagteam-engine/tests/manage.rs:130`

**Interfaces:**
- Consumes: existing `Store`, `AccountRow`, `JournalRow`, `EventRow`, `set_active_on`, `user_version`, `set_wal_mode`, `TransactionBehavior::Immediate`.
- Produces (Interface Contract, `store/mod.rs`):
  - `const SCHEMA_VERSION: i64 = 2`
  - `const MIGRATION_V2: &str`
  - `pub struct Activation { pub account: AccountId, pub epoch: Option<i64> }`
  - `Store::activation(&self, provider: &ProviderId) -> Result<Option<Activation>, StoreError>`
  - `Store::set_active(&self, provider: &ProviderId, id: Option<&AccountId>, epoch: Option<i64>) -> Result<(), StoreError>`
  - `Store::commit_switch(&self, provider: &ProviderId, to: &AccountId, epoch: i64, event: &EventRow) -> Result<(), StoreError>`
  - `Store::live_store_stale(&self, row: &AccountRow) -> Result<bool, StoreError>`
  - `JournalRow.to_epoch: Option<i64>`
  - `delete_account` NULLs an orphaned epoch.
- Produces (fixtures):
  - `Fx::activation(&self) -> Option<Activation>`
  - `Fx::live_store_stale(&self, id: &AccountId) -> bool`
  - `crash_row` now sets `to_epoch: Some(<to's login_epoch>)`, as the switch's step 6 writes it.

**Spec:**
- §6.1: `active_accounts.login_epoch` is the activation epoch, "NULL only with `account_id`", and `switch_journal.to_epoch` is the target's `login_epoch` when the row was written. Migrations are embedded, forward-only and tracked by `PRAGMA user_version`.
- §12.5, "The default home's activation epoch":
  - The live store is stale-marked when `active_accounts` names the account with an epoch other than its `login_epoch`.
  - The migration fills each named account's current `login_epoch`.
- §9.4 step 6: the journal row records the target's `login_epoch`.
- §9.4 step 9: the commit sets the active account and its activation epoch in one transaction.
- §10.1 step 4: `add` records the account's current `login_epoch` as the activation epoch.
- §9.6: a row written before `to_epoch` falls back to `to_id`'s current `login_epoch`, so the column is not backfilled.

- [ ] **Step 1: Write the failing tests**

Existing tests this task changes, with their new expected values:
- `tests/store.rs`:
  - `opening_migrates_once_and_open_existing_never_creates`: `schema_version()` is `2` at lines 26 and 29 (was `1`).
  - `a_newer_schema_version_is_refused_and_the_database_is_untouched`: the hypothetical newer file is version **3**. This touches the comment at 215 ("schema v3"), `PRAGMA user_version = 3;` at 221, `Err(StoreError::UnsupportedSchema(3))` at 226 and `assert_eq!(version, 3)` at 234. With `SCHEMA_VERSION = 2`, a version 2 file is no longer newer.
  - `concurrent_first_opens_all_succeed`: `2` at line 309 (was `1`).
  - `every_open_path_leaves_the_store_in_wal_mode`: the comment at 254 reads "(v2)" (cosmetic).
  - `commit_switch_is_one_transaction_and_delete_clears_active`: `j` gets `to_epoch: Some(4)`, so the two round trips the test already makes cover the column and the `prior` snapshot. The commit becomes `s.commit_switch(&cc(), &b, 0, &ev)`.
  - `a_malformed_prior_journal_snapshot_is_reported_not_silently_dropped`: the literal at 418 gets `to_epoch: None`.
  - `the_database_and_its_sidecars_are_created_0600`: `s.set_active(&cc(), None, None)` at 533.
- `tests/switch.rs`, `make_fresh_machine` (line 60): `.set_active(&fx.provider(), None, None)`.
- `tests/collect.rs`, `hooks::a_stale_active_record_does_not_outrank_the_live_login` (line 1283, compiled only with `test-hooks`): `.set_active(&fx.provider(), Some(&b), Some(0))`. b's epoch is 0.
- `tests/recover.rs`, `a_forced_switch_killed_before_landing_leaves_the_superseded_row` (literal at 559): add `to_epoch: Some(store.account(&b).unwrap().unwrap().login_epoch),` after `to_fp`. That is what the forced switch's own step 6 would write.
- `tests/switch_rollback.rs`, `undecidable_row` (line 60): add `to_epoch: None,` after `to_fp`.
- `tests/manage.rs`, `metadata_commands_proceed_through_an_interrupted_switch_but_remove_refuses` (line 130): add `to_epoch: None,` after `to_fp`.
- `src/engine.rs`, `tests::unresolved_journal` (line 355): add `to_epoch: None,` after `to_fp`.

Two literals only spread `..crash_row(..)` or `..j.clone()`: `tests/recover.rs:215` and `tests/store.rs:159`. They need no edit. `tests/oracle.rs:206`, `tests/freshen.rs:529` and `tests/recover.rs:488` only copy rows.

In `crates/tagteam-engine/tests/common/mod.rs`, change the store import to:

```rust
use tagteam_engine::store::{Activation, JournalRow, LoginMeta, NewAccount, Store};
```

replace `crash_row` with:

```rust
/// The row a switch from `from` to `to` writes at step 6, held by a process that has died. It
/// journals the target's `login_epoch`, as the switch does (§9.4 step 6).
pub fn crash_row(fx: &Fx, from: &AccountId, to: &AccountId) -> JournalRow {
    let store = fx.engine.store().unwrap();
    let from_row = store.account(from).unwrap().unwrap();
    let to_row = store.account(to).unwrap().unwrap();
    JournalRow {
        provider: fx.provider(),
        holder: dead_holder(),
        from_id: Some(from.clone()),
        to_id: to.clone(),
        from_fp: Some(vault_fp(fx, from)),
        from_identity: Some(from_row.identity_json),
        to_fp: vault_fp(fx, to),
        to_epoch: Some(to_row.login_epoch),
        started_at: 1,
        prior: None,
    }
}
```

and add to the main `impl Fx` block, after `quarantine`:

```rust
    /// The provider's active account and its activation epoch (§12.5).
    pub fn activation(&self) -> Option<Activation> {
        self.engine
            .store()
            .unwrap()
            .activation(&self.provider())
            .unwrap()
    }

    /// Whether the live store is stale-marked for `id` (§12.5).
    pub fn live_store_stale(&self, id: &AccountId) -> bool {
        let store = self.engine.store().unwrap();
        let row = store.account(id).unwrap().unwrap();
        store.live_store_stale(&row).unwrap()
    }
```

In `crates/tagteam-engine/tests/store.rs`, change the store import to:

```rust
use tagteam_engine::store::{
    Activation, EventRow, JournalRow, LoginMeta, NewAccount, Store, StoreError,
};
```

and append:

```rust
/// A `switch` event to `to`, as `commit_switch` records one.
fn switch_event(to: &AccountId) -> EventRow {
    EventRow {
        at: 6,
        provider: cc(),
        kind: "switch".into(),
        from_id: None,
        to_id: Some(to.clone()),
        trigger: Some("manual".into()),
        source: "cli".into(),
        detail: None,
    }
}

/// The provider's `active_accounts` row as stored, read through an independent connection.
fn active_row(path: &std::path::Path, provider: &str) -> Option<(Option<String>, Option<i64>)> {
    use rusqlite::OptionalExtension;
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row(
            "SELECT account_id, login_epoch FROM active_accounts WHERE provider = ?1",
            [provider],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .unwrap()
}

#[test]
fn a_version_1_store_is_migrated_with_its_activation_epochs_filled() {
    // Decision 1: `schema.sql` is the frozen v1 DDL, so a v1 file runs only the v2 step. The
    // backfill takes the live store as current (§12.5): each named account's activation epoch
    // is its current `login_epoch`. A journal row keeps a NULL `to_epoch` (§9.6).
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("t.db");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(include_str!("../src/store/schema.sql"))
            .unwrap();
        conn.execute_batch(
            "INSERT INTO accounts
               (id, provider, position, identity_key, label, kind, identity_json, login_epoch, added_at)
               VALUES ('a', 'claude-code', 1, 'a@x.co', 'a@x.co', 'oauth', '{}', 3, 1),
                      ('b', 'claude-code', 2, 'b@x.co', 'b@x.co', 'oauth', '{}', 0, 1);
             INSERT INTO active_accounts (provider, account_id)
               VALUES ('claude-code', 'a'), ('fake-agent', NULL);
             INSERT INTO switch_journal
               (provider, holder_pid, holder_start, from_id, to_id, to_fp, started_at)
               VALUES ('claude-code', 1, 2, 'a', 'b', 'sha256:b', 5);
             PRAGMA user_version = 1;",
        )
        .unwrap();
    }

    let s = Store::open(&path).unwrap();

    assert_eq!(s.schema_version().unwrap(), 2);
    let a = AccountId::from_string("a");
    assert_eq!(
        s.activation(&cc()).unwrap(),
        Some(Activation {
            account: a.clone(),
            epoch: Some(3)
        })
    );
    assert!(
        !s.live_store_stale(&s.account(&a).unwrap().unwrap()).unwrap(),
        "the live store is taken as current"
    );
    assert_eq!(s.activation(&ProviderId::new("fake-agent")).unwrap(), None);
    assert_eq!(s.journal(&cc()).unwrap().unwrap().to_epoch, None);
    drop(s);
    assert_eq!(
        active_row(&path, "fake-agent"),
        Some((None, None)),
        "NULL only with account_id"
    );
    assert_eq!(journal_mode(&path), "wal");
    // Reopening runs nothing more: the file is already at the build's version.
    assert_eq!(Store::open(&path).unwrap().schema_version().unwrap(), 2);
}

#[test]
fn the_activation_epoch_moves_with_the_active_account() {
    // Both columns are written on every upsert: an epoch left from the previous account would
    // stale-mark the next one (§12.5).
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("t.db");
    let s = Store::open(&path).unwrap();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let b = add(&s, &cc(), "b", "b@x.co", 2);
    let on = |account: &AccountId, epoch| {
        Some(Activation {
            account: account.clone(),
            epoch,
        })
    };
    assert_eq!(s.activation(&cc()).unwrap(), None);

    s.set_active(&cc(), Some(&a), Some(5)).unwrap();
    assert_eq!(s.activation(&cc()).unwrap(), on(&a, Some(5)));
    s.set_active(&cc(), Some(&b), None).unwrap();
    assert_eq!(
        s.activation(&cc()).unwrap(),
        on(&b, None),
        "a's epoch is not kept for b"
    );
    s.commit_switch(&cc(), &a, 2, &switch_event(&a)).unwrap();
    assert_eq!(s.activation(&cc()).unwrap(), on(&a, Some(2)));
    assert_eq!(s.active(&cc()).unwrap(), Some(a.clone()), "`active` is the id alone");

    // No account, no epoch, whatever the caller passes.
    s.set_active(&cc(), None, Some(9)).unwrap();
    assert_eq!(s.activation(&cc()).unwrap(), None);
    assert_eq!(active_row(&path, "claude-code"), Some((None, None)));
}

#[test]
fn deleting_the_active_account_clears_its_epoch_too() {
    // `ON DELETE SET NULL` clears `account_id` only; §6.1 allows a NULL epoch only with it.
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("t.db");
    let s = Store::open(&path).unwrap();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    s.set_active(&cc(), Some(&a), Some(4)).unwrap();

    s.delete_account(&a).unwrap();

    assert_eq!(s.activation(&cc()).unwrap(), None);
    assert_eq!(active_row(&path, "claude-code"), Some((None, None)));
}

#[test]
fn the_live_store_is_stale_only_when_the_active_account_s_epoch_moved() {
    // §12.5: stale-marked when `active_accounts` names the account with an activation epoch
    // other than its `login_epoch`. A row with no epoch is no evidence either way.
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("t.db");
    let s = Store::open(&path).unwrap();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let b = add(&s, &cc(), "b", "b@x.co", 2);
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute("UPDATE accounts SET login_epoch = 2 WHERE id = 'a'", [])
        .unwrap();
    let stale = |id: Option<&AccountId>, epoch: Option<i64>| {
        s.set_active(&cc(), id, epoch).unwrap();
        s.live_store_stale(&s.account(&a).unwrap().unwrap()).unwrap()
    };

    assert!(!stale(None, None), "no active account");
    assert!(!stale(Some(&b), Some(0)), "another account is active");
    assert!(!stale(Some(&a), Some(2)), "activated at its current epoch");
    assert!(stale(Some(&a), Some(1)), "activated before a replacement");
    assert!(!stale(Some(&a), None), "no epoch recorded");
}

#[test]
fn a_prior_snapshot_from_before_to_epoch_reads_as_none() {
    // §9.6: a row written before the column has no epoch of its own. Its `prior` snapshot has
    // no `to_epoch` key at all, and still parses.
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("t.db");
    let s = Store::open(&path).unwrap();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let b = add(&s, &cc(), "b", "b@x.co", 2);
    s.insert_journal(&JournalRow {
        provider: cc(),
        holder: ProcessStamp { pid: 1, start: 2 },
        from_id: Some(a),
        to_id: b,
        from_fp: None,
        from_identity: None,
        to_fp: "sha256:b".into(),
        to_epoch: Some(1),
        started_at: 5,
        prior: None,
    })
    .unwrap();
    drop(s);
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "UPDATE switch_journal SET prior = ?1 WHERE provider = 'claude-code'",
            [r#"{"provider":"claude-code","holder_pid":1,"holder_start":2,"from_id":"a","to_id":"b","from_fp":null,"from_identity":null,"to_fp":"sha256:b","started_at":4,"prior":null}"#],
        )
        .unwrap();

    let row = Store::open(&path).unwrap().journal(&cc()).unwrap().unwrap();

    assert_eq!(row.to_epoch, Some(1));
    assert_eq!(row.prior.unwrap().to_epoch, None);
}
```

In `crates/tagteam-engine/tests/switch.rs`, change the store import to
`use tagteam_engine::store::{Activation, NewAccount};`, and append:

```rust
/// Sets `id`'s `login_epoch` directly, as explicit replacements since it was added would have.
fn set_login_epoch(fx: &Fx, id: &AccountId, epoch: i64) {
    rusqlite::Connection::open(fx.env.data_dir().join("tagteam.db"))
        .unwrap()
        .execute(
            "UPDATE accounts SET login_epoch = ?2 WHERE id = ?1",
            rusqlite::params![id.as_str(), epoch],
        )
        .unwrap();
}

#[test]
fn a_switch_records_the_target_s_login_epoch_as_the_activation_epoch() {
    // §9.4 step 9: the active account and its activation epoch move in the commit.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b"); // live: b
    set_login_epoch(&fx, &a, 3);

    switch(&fx, to(&a), false).unwrap();

    assert_eq!(
        fx.activation(),
        Some(Activation {
            account: a.clone(),
            epoch: Some(3)
        })
    );
    assert!(!fx.live_store_stale(&a));
}

#[cfg(feature = "test-hooks")]
#[test]
fn the_journal_row_carries_the_target_s_login_epoch() {
    // §9.4 step 6: the row names the target's `login_epoch`, which a forward recovery records
    // (§9.6). Read by a second engine while the row exists, between the journal and the commit.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    set_login_epoch(&fx, &a, 3);
    let seen: Arc<Mutex<Option<tagteam_engine::store::JournalRow>>> = Arc::default();
    let other = fx.engine_with_env(fx.env.clone());
    let (slot, provider) = (seen.clone(), fx.provider());
    fx.engine.on_point(
        "after-journal",
        Box::new(move || {
            *slot.lock().unwrap() = other.store().unwrap().journal(&provider).unwrap();
        }),
    );

    switch(&fx, to(&a), false).unwrap();

    let row = seen.lock().unwrap().clone().expect("the row was journaled");
    assert_eq!((row.to_id, row.to_epoch), (a, Some(3)));
}
```

In `crates/tagteam-engine/tests/add.rs`, add `use tagteam_engine::store::Activation;` and
append:

```rust
#[test]
fn add_records_the_account_s_login_epoch_as_its_activation_epoch() {
    // §10.1 step 4: the live store now holds exactly what the vault holds.
    let fx = Fx::new();
    fx.login("me@work.co", "rt-1");
    let first = fx.engine.add_live(add_opts(&fx)).unwrap().account;
    assert_eq!(
        fx.activation(),
        Some(Activation {
            account: first.id.clone(),
            epoch: Some(0)
        })
    );

    fx.rotate_live("rt-2");
    let again = fx.engine.add_live(add_opts(&fx)).unwrap().account;

    assert_eq!(again.login_epoch, 1);
    assert_eq!(
        fx.activation(),
        Some(Activation {
            account: first.id.clone(),
            epoch: Some(1)
        })
    );
    assert!(!fx.live_store_stale(&first.id));
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-engine --test store`
Expected: compile errors in `tests/common/mod.rs` and `tests/store.rs`:
- cannot find type `Activation` in `tagteam_engine::store`;
- struct `JournalRow` has no field named `to_epoch`;
- no method named `activation` (and `live_store_stale`) found for `Store`;
- `set_active` takes 2 arguments but 3 were supplied;
- `commit_switch` takes 3 arguments but 4 were supplied.

Every engine integration-test binary shares `common`, so `--test switch` and `--test add` fail
the same way.

- [ ] **Step 3: Implement**

In `crates/tagteam-engine/src/store/mod.rs`, replace lines 23–27 (`SCHEMA_V1` and
`SCHEMA_VERSION`) with:

```rust
/// Version 1, frozen: never edited, so a fresh file and an upgraded one end in the same schema.
const SCHEMA_V1: &str = include_str!("schema.sql");

/// Version 2 (§6.1, §12.5): the activation epoch on `active_accounts`, filled with each named
/// account's current `login_epoch` (the live store is taken as current, the best evidence there
/// is), and the target's epoch on the switch journal, left NULL on a row written before it
/// (§9.6 falls back to the target's current epoch).
const MIGRATION_V2: &str = "ALTER TABLE active_accounts ADD COLUMN login_epoch INTEGER;
ALTER TABLE switch_journal ADD COLUMN to_epoch INTEGER;
UPDATE active_accounts SET login_epoch =
  (SELECT login_epoch FROM accounts WHERE accounts.id = active_accounts.account_id)
  WHERE account_id IS NOT NULL;";

/// The migration ladder: step `n` takes a file from version `n` to `n + 1`. A fresh file runs
/// every step, and an older one only the steps above its version.
const MIGRATIONS: [&str; 2] = [SCHEMA_V1, MIGRATION_V2];

/// The `PRAGMA user_version` this build knows how to read and write. A stored version above
/// this is a store written by a newer tagteam; `migrate` refuses it rather than guessing.
const SCHEMA_VERSION: i64 = 2;

const _: () = assert!(MIGRATIONS.len() as i64 == SCHEMA_VERSION);
```

Replace the `JournalRow` struct (101–114) with:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct JournalRow {
    pub provider: ProviderId,
    pub holder: ProcessStamp,
    pub from_id: Option<AccountId>,
    pub to_id: AccountId,
    pub from_fp: Option<String>,
    pub from_identity: Option<Value>,
    pub to_fp: String,
    /// The target's `login_epoch` when the row was written (§9.4 step 6): the activation epoch
    /// a forward finish records (§9.6). `None` on a row written before the column existed.
    pub to_epoch: Option<i64>,
    pub started_at: i64,
    /// The unresolved row a forced switch superseded. If the forced switch never lands, this
    /// is what recovery or rollback puts back, so the unresolved state is never forgotten.
    pub prior: Option<Box<JournalRow>>,
}
```

After `LoginMeta` (line 123), add:

```rust
/// The store's record of the default home's live login (§12.5): the account tagteam made live,
/// and that account's `login_epoch` when it did, or from before a replacement superseded the
/// live login. `epoch` is `None` only for a row written without one; the migration filled every
/// named account's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activation {
    pub account: AccountId,
    pub epoch: Option<i64>,
}
```

Replace `journal_to_json` and `journal_from_json` (125–168) with:

```rust
fn journal_to_json(j: &JournalRow) -> Value {
    json!({
        "provider": j.provider.as_str(),
        "holder_pid": j.holder.pid,
        "holder_start": j.holder.start,
        "from_id": j.from_id.as_ref().map(AccountId::as_str),
        "to_id": j.to_id.as_str(),
        "from_fp": j.from_fp,
        "from_identity": j.from_identity,
        "to_fp": j.to_fp,
        "to_epoch": j.to_epoch,
        "started_at": j.started_at,
        "prior": j.prior.as_deref().map(journal_to_json),
    })
}

/// The inverse of `journal_to_json`. Returns an error rather than silently dropping a
/// malformed `prior` snapshot: a caller (rollback or recovery) that got `None` back would
/// delete the undecidable row instead of restoring it (§9.6). A snapshot written before
/// `to_epoch` existed has no such key, and reads as `None`.
fn journal_from_json(v: &Value) -> rusqlite::Result<JournalRow> {
    fn malformed() -> rusqlite::Error {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            "malformed journal snapshot".into(),
        )
    }
    Ok(JournalRow {
        provider: ProviderId::new(v["provider"].as_str().ok_or_else(malformed)?),
        holder: ProcessStamp {
            pid: v["holder_pid"].as_u64().ok_or_else(malformed)? as u32,
            start: v["holder_start"].as_u64().ok_or_else(malformed)?,
        },
        from_id: v["from_id"].as_str().map(AccountId::from_string),
        to_id: AccountId::from_string(v["to_id"].as_str().ok_or_else(malformed)?),
        from_fp: v["from_fp"].as_str().map(str::to_owned),
        from_identity: Some(v["from_identity"].clone()).filter(|x| !x.is_null()),
        to_fp: v["to_fp"].as_str().ok_or_else(malformed)?.to_owned(),
        to_epoch: v["to_epoch"].as_i64(),
        started_at: v["started_at"].as_i64().ok_or_else(malformed)?,
        prior: match v.get("prior") {
            Some(p) if !p.is_null() => Some(Box::new(journal_from_json(p)?)),
            _ => None,
        },
    })
}
```

Replace `SET_ACTIVE_SQL` (198–201) with:

```rust
/// Upserts the provider's active account and its activation epoch, both columns on conflict:
/// an epoch left from the previous account would stale-mark the next one (§12.5). Shared by
/// every writer of the row, each inside its own larger transaction or alone.
const SET_ACTIVE_SQL: &str = "INSERT INTO active_accounts (provider, account_id, login_epoch) \
    VALUES (?1, ?2, ?3) \
    ON CONFLICT(provider) DO UPDATE SET account_id = excluded.account_id, \
    login_epoch = excluded.login_epoch";
```

Replace `journal_from_row` (244–264) with:

```rust
fn journal_from_row(r: &Row<'_>) -> rusqlite::Result<JournalRow> {
    Ok(JournalRow {
        provider: ProviderId::new(r.get::<_, String>("provider")?),
        holder: ProcessStamp {
            pid: r.get("holder_pid")?,
            start: r.get::<_, i64>("holder_start")? as u64,
        },
        from_id: r
            .get::<_, Option<String>>("from_id")?
            .map(AccountId::from_string),
        to_id: AccountId::from_string(r.get::<_, String>("to_id")?),
        from_fp: r.get("from_fp")?,
        from_identity: json_col(r, "from_identity")?,
        to_fp: r.get("to_fp")?,
        to_epoch: r.get("to_epoch")?,
        started_at: r.get("started_at")?,
        prior: match json_col(r, "prior")? {
            Some(v) => Some(Box::new(journal_from_json(&v)?)),
            None => None,
        },
    })
}
```

Replace `set_active_on` (328–337) with:

```rust
/// `SET_ACTIVE_SQL` on any connection-like handle. No account means no epoch, whatever the
/// caller passes (§6.1: NULL only with `account_id`).
fn set_active_on(
    c: &Connection,
    provider: &ProviderId,
    id: Option<&AccountId>,
    epoch: Option<i64>,
) -> rusqlite::Result<usize> {
    c.execute(
        SET_ACTIVE_SQL,
        params![provider.as_str(), id.map(AccountId::as_str), id.and(epoch)],
    )
}
```

Replace `migrate` (459–490) with:

```rust
    /// Applies each migration step above the file's version exactly once (`MIGRATIONS`). A
    /// stored version newer than `SCHEMA_VERSION` is refused outright, before anything else
    /// runs against the connection — including switching it to WAL, which is why that pragma is
    /// set here and not in `connect`: a file this build refuses must be left exactly as it was
    /// found. The check-and-apply itself runs in one `IMMEDIATE` transaction, re-reading the
    /// version under the write lock it grants — both to guard against a concurrent racing
    /// first open (§6.1), and, re-checked again, against a concurrent newer binary upgrading
    /// the file between this function's first read and the moment it takes the lock. Every step
    /// and the version bump commit together, so a file is never left between two versions. WAL
    /// mode is set only once every such check has passed and, when a step ran, only after that
    /// transaction has committed — WAL can't be switched from inside a transaction, and a file
    /// this build ends up refusing must never have been touched at all.
    fn migrate(&self) -> Result<(), StoreError> {
        let mut c = self.lock();
        let version = user_version(&c)?;
        if version > SCHEMA_VERSION {
            return Err(StoreError::UnsupportedSchema(version));
        }
        if version < SCHEMA_VERSION {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let version = user_version(&tx)?;
            if version > SCHEMA_VERSION {
                return Err(StoreError::UnsupportedSchema(version));
            }
            if version < SCHEMA_VERSION {
                let done = usize::try_from(version).unwrap_or(0);
                for step in &MIGRATIONS[done..] {
                    tx.execute_batch(step)?;
                }
                tx.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))?;
            }
            tx.commit()?;
        }
        set_wal_mode(&c)?;
        Ok(())
    }
```

Replace `delete_account` (841–853) with:

```rust
    /// Deletes the account; its usage rows cascade, and its `usage:<id>` lease row (which has
    /// no foreign key) goes in the same transaction. `ON DELETE SET NULL` clears an
    /// `active_accounts` row that named it, but cannot clear a second column, so the orphaned
    /// activation epoch is cleared here too (§6.1: NULL only with `account_id`).
    pub fn delete_account(&self, id: &AccountId) -> Result<(), StoreError> {
        let mut c = self.lock();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("DELETE FROM accounts WHERE id = ?1", [id.as_str()])?;
        tx.execute(
            "DELETE FROM leases WHERE name = ?1",
            [usage::lease_name(id)],
        )?;
        tx.execute(
            "UPDATE active_accounts SET login_epoch = NULL WHERE account_id IS NULL",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }
```

Keep `active` (855–865) as it is. Replace `set_active` (867–874) with these three methods:

```rust
    /// The provider's active account and its activation epoch (§12.5); `None` when no account
    /// is recorded (none was ever set, or it was removed).
    pub fn activation(&self, provider: &ProviderId) -> Result<Option<Activation>, StoreError> {
        let c = self.lock();
        let row: Option<(Option<String>, Option<i64>)> = c
            .query_row(
                "SELECT account_id, login_epoch FROM active_accounts WHERE provider = ?1",
                [provider.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(row.and_then(|(id, epoch)| {
            id.map(|id| Activation {
                account: AccountId::from_string(id),
                epoch,
            })
        }))
    }

    /// `epoch` is stored with `id`; `id = None` stores `None` for both.
    pub fn set_active(
        &self,
        provider: &ProviderId,
        id: Option<&AccountId>,
        epoch: Option<i64>,
    ) -> Result<(), StoreError> {
        set_active_on(&self.lock(), provider, id, epoch)?;
        Ok(())
    }

    /// §12.5: `active_accounts` names `row` with an epoch other than `row.login_epoch`. An
    /// explicit replacement then superseded the lineage the live store holds, which is never
    /// captured. A row with no epoch is no evidence, and reads as current, as the migration
    /// takes it.
    pub fn live_store_stale(&self, row: &AccountRow) -> Result<bool, StoreError> {
        Ok(matches!(
            self.activation(&row.provider)?,
            Some(Activation { account, epoch: Some(epoch) })
                if account == row.id && epoch != row.login_epoch
        ))
    }
```

Replace `commit_switch` (893–907) with:

```rust
    /// §9.4 step 9: the active account, its activation epoch (the target's `login_epoch`, which
    /// cannot move while its account lock is held), the event and the journal row move
    /// together.
    pub fn commit_switch(
        &self,
        provider: &ProviderId,
        to: &AccountId,
        epoch: i64,
        event: &EventRow,
    ) -> Result<(), StoreError> {
        let mut c = self.lock();
        let tx = c.transaction()?;
        set_active_on(&tx, provider, Some(to), Some(epoch))?;
        Self::insert_event_on(&tx, event)?;
        tx.execute(DELETE_JOURNAL_SQL, [provider.as_str()])?;
        tx.commit()?;
        Ok(())
    }
```

Replace `insert_journal` (942–964) with:

```rust
    /// Writes the provider's journal row, atomically replacing any row already there: a
    /// forced switch settles an undecidable row this way without a gap in which neither
    /// row exists (§9.6).
    pub fn insert_journal(&self, j: &JournalRow) -> Result<(), StoreError> {
        self.lock().execute(
            "INSERT OR REPLACE INTO switch_journal \
             (provider, holder_pid, holder_start, from_id, to_id, from_fp, from_identity, to_fp, \
             to_epoch, started_at, prior) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                j.provider.as_str(),
                j.holder.pid,
                j.holder.start as i64,
                j.from_id.as_ref().map(AccountId::as_str),
                j.to_id.as_str(),
                j.from_fp,
                j.from_identity.as_ref().map(Value::to_string),
                j.to_fp,
                j.to_epoch,
                j.started_at,
                j.prior.as_deref().map(|p| journal_to_json(p).to_string()),
            ],
        )?;
        Ok(())
    }
```

`schema.sql` is not touched. The journal readers use `SELECT *` (lines 926, 935) and read by
column name, so the appended column needs no column list.

Call sites, all in `crates/tagteam-engine/src/`:

- `switch.rs`, `Engine::transact`, the journal literal: replace

  ```rust
              to_fp: p
                  .fingerprint(&target_secret)
                  .map(|f| f.as_str().to_owned())
                  .unwrap_or_default(),
              started_at: self.now_ms(),
  ```

  with

  ```rust
              to_fp: p
                  .fingerprint(&target_secret)
                  .map(|f| f.as_str().to_owned())
                  .unwrap_or_default(),
              // §9.4 step 6: read under the target's account lock, so it cannot move before
              // the commit; a forward recovery records it (§9.6).
              to_epoch: Some(target.login_epoch),
              started_at: self.now_ms(),
  ```

- `switch.rs`, `Engine::apply`: replace

  ```rust
          tx.store.commit_switch(
              &req.provider,
              &target.id,
              &EventRow {
  ```

  with

  ```rust
          tx.store.commit_switch(
              &req.provider,
              &target.id,
              target.login_epoch,
              &EventRow {
  ```

- `recover.rs`, `Engine::finish_forward`: replace

  ```rust
          store.commit_switch(
              &row.provider,
              &to.id,
              &EventRow {
  ```

  with

  ```rust
          store.commit_switch(
              &row.provider,
              &to.id,
              to.login_epoch,
              &EventRow {
  ```

  Task 3 replaces `to.login_epoch` with `row.to_epoch.unwrap_or(to.login_epoch)`.

- `lifecycle.rs`, `Engine::add_live`: replace
  `store.set_active(&opts.provider, Some(&account.id))?;` with
  `store.set_active(&opts.provider, Some(&account.id), Some(account.login_epoch))?;`.

- `engine.rs`, `tests::unresolved_journal`: add `to_epoch: None,` after `to_fp: "sha256:stale".into(),`.

`Store::active` keeps its signature. Its callers are unchanged: `switch.rs:535`,
`collect.rs:105` and `:741`, `views.rs:396`, and the tests.

- [ ] **Step 4: Run them and see them pass**, then the crate's whole suite

Run: `cargo test -p tagteam-engine --test store --test add --test switch`
Expected: PASS, including the seven new tests and the six changed ones in `tests/store.rs`.
The eighth new test is gated on `test-hooks` and runs below.

Run: `cargo test -p tagteam-engine --features test-hooks`
Expected: PASS. This includes `the_journal_row_carries_the_target_s_login_epoch` and the
`collect.rs` `hooks` module. Recovery still commits `to.login_epoch`. Every `crash_row` journals
that same epoch, so no recovery test moves.

Run: `cargo test --workspace --features tagteam/test-support`
Expected: PASS.

- [ ] **Step 5: Format and lint**

`cargo fmt --all && cargo fmt --all --check`, and `cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings`, and `cargo clippy --workspace --all-targets -- -D warnings`

The gated test in `tests/switch.rs` names `tagteam_engine::store::JournalRow` by path, so the
no-feature clippy run sees no unused import.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-engine/src/store/mod.rs crates/tagteam-engine/src/switch.rs \
  crates/tagteam-engine/src/recover.rs crates/tagteam-engine/src/lifecycle.rs \
  crates/tagteam-engine/src/engine.rs crates/tagteam-engine/tests/common/mod.rs \
  crates/tagteam-engine/tests/store.rs crates/tagteam-engine/tests/switch.rs \
  crates/tagteam-engine/tests/add.rs crates/tagteam-engine/tests/collect.rs \
  crates/tagteam-engine/tests/recover.rs crates/tagteam-engine/tests/switch_rollback.rs \
  crates/tagteam-engine/tests/manage.rs
git commit -m "Migrate the store to v2 with the activation epoch and the journal's target epoch"
```

---

### Task 2: Replacement evidence and `add`'s activation epoch

An explicit replacement must stale-mark the live store as soon as it begins, whoever activated
the account and whatever `active_accounts` held (§12.5 "A replacement records its own
evidence"). `begin_replacement` cannot see the live identity. Decision 3 makes the engine read
it, a file read with no Keychain, and pass it as `live_names_account`. The store then decides
the `active_accounts` half from its own table, inside the same transaction.

`add` is the one replacer whose new login is the live one. `LoginMeta.from_live` travels in
`replacing_meta`, so `finish_replacement` records the new epoch atomically (§10.1). So does a
reconciliation of an `add` that died, since `reconcile_replacement` only calls
`finish_replacement`.

**Readings of the spec this task commits to:**
- **The evidence rule, row by row** (in `begin_replacement`'s transaction, after the
  increment):
  - If `live_names_account` is set or `active_accounts` names the account, and the row does not
    already name it with an epoch, the row becomes `(account, epoch before the increment)`.
  - Otherwise the row is left as it is.
  
  A row that already names the account with an epoch stays as it is. That epoch is either the
  current one, which the increment now makes stale, or an older replacement's, which is still
  stale.
- **A rollback needs no undo.** The evidence holds the pre-increment epoch, which
  `rollback_replacement` restores, so the live store is current again.
- **`finish_replacement` records the new epoch only while `active_accounts` still names the
  account.** `begin_replacement` always made it name the account for an `add`. If a switch
  committed another account after a replacer died, that switch's record is newer, and the
  reconciliation leaves it alone. This is what "as `add` would have" amounts to once a later
  switch has run. For the in-process `add`, the two readings are the same. See Contract
  problems.
- **Metadata without `from_live`**, recorded by a build before this field existed, is not from
  the live login.
- **`add_token` reads the live identity under the account lock**, just before
  `commit_login`.
  - `Present`: it names the account when the identity keys are equal.
  - `Absent`: it does not.
  - `Unreadable`: `add_token` refuses with `EngineError::Unreadable`, before anything is
    written. Either guess can lose a replacement's protection:
    - counting it as naming the account can overwrite another account's stale activation
      evidence, so a later switch away captures that account's superseded live lineage;
    - counting it as not naming the account can leave this account's own superseded lineage
      unmarked.

    Nothing is decided on what cannot be read (B.1).
- **`add_live` keeps its separate `set_active` after `commit_login`.** It is the only write for
  a new account (Decision 3: no replacement can stale-mark an account that did not exist yet).
  For a replacement it writes the same value `finish_replacement` already recorded.
- **Review Focus 3's `add-token` half** is tested here with a live setup-token account. That is
  the only kind `add-token` can replace in place while it is live (§10.2 refuses a different
  kind under the same email). Task 4 explains why its §7.5 half needs an OAuth replacement.

**Files:**
- Modify: `crates/tagteam-engine/src/store/mod.rs`:
  - 116–123, `LoginMeta.from_live`
  - 642–683, `begin_replacement`
  - 685–731, `finish_replacement`
- Modify: `crates/tagteam-engine/src/lifecycle.rs`:
  - after 162, a new private `LoginSource`
  - 286–388, `commit_login`'s signature and its replacement branch (326–332)
  - 485–498, `add_live`'s `commit_login` call and the comment on `set_active`
  - 553–585, `add_token`, which reads the live identity and passes it on
- Modify: `crates/tagteam-engine/tests/common/mod.rs`:
  - 607–637, `Fx::begin_replacement`
  - new `Fx::begin_replacement_with`, `Fx::live_names`, `Fx::replace_login` and the private `Fx::fixture_vault`
- Modify: `crates/tagteam-engine/tests/engine_basics.rs`. In `a_pending_replacement_is_reconciled_by_the_next_lock_holder`: the `LoginMeta` literal at 57, and the `begin_replacement` calls at 65 and 79.
- Test: `crates/tagteam-engine/tests/store.rs`, `crates/tagteam-engine/tests/add.rs`

**Interfaces:**
- Consumes:
  - Task 1: `Activation`, `Store::activation`, `Store::live_store_stale`, `set_active_on(c, provider, id, epoch)`, `Fx::activation`, `Fx::live_store_stale`.
  - Existing: `Engine::lock_account`, `Engine::mutation_guard`, `Vault::store`, `Provider::live_identity`, `Provider::identity_key`.
- Produces (Interface Contract):
  - `LoginMeta<'a> { …, pub from_live: bool }`
  - `Store::begin_replacement(&self, id: &AccountId, fp: &str, meta: &LoginMeta<'_>, live_names_account: bool) -> Result<(), StoreError>`
  - `finish_replacement`, with its signature unchanged, records the activation epoch for a `from_live` replacement.
- Produces (fixtures):
  - `Fx::begin_replacement(&self, id: &AccountId, new_bytes: &[u8], identity_json: &Value, kind: &str)`, which now models a died `add`
  - `Fx::begin_replacement_with(&self, id: &AccountId, new_bytes: &[u8], identity_json: &Value, kind: &str, from_live: bool)`
  - `Fx::live_names(&self, id: &AccountId) -> bool`
  - `Fx::replace_login(&self, id: &AccountId, new_bytes: &[u8], kind: &str)`

**Spec:**
- §12.5 "Explicit replacements are recoverable", step 1:
  - `replacing_meta` records "whether it was taken from the live login, as `add` takes it".
  - When the live identity or `active_accounts` names the account, the same transaction records the default home's evidence.
- §12.5 "A replacement records its own evidence": the row is set to the account with the pre-increment epoch, unless it already names it with an epoch. "`add` … records the new epoch in its last transaction".
- §12.5, reconciliation: "A replacement taken from the live login also records the account's `login_epoch` as the activation epoch, as `add` would have".
- §10.1 step 4: the activation is "written in the same transaction as the account's last store write (for a replacement, the one that clears `replacing_fp`)".
- §15.2, "Replacement evidence":
  - over the live account, the live store is stale-marked whatever `active_accounts` held, including another account while the live identity named this one;
  - an `add` that dies part-way is reconciled with its activation epoch recorded.

- [ ] **Step 1: Write the failing tests**

Existing tests this task changes, with their new values. The calls gain `, false` and the
literals gain `from_live: false`. None of them names the live login, so their expected values
are unchanged:
- `tests/store.rs`:
  - `replacement_markers_move_the_epoch`: the `meta` closure at 105, and the calls at 111 and 129.
  - `finish_replacement_refuses_metadata_missing_required_fields`: 321 and 327.
  - `begin_replacement_is_guarded_against_a_second_start`: 356, 362 and 365.
  - `rollback_after_rollback_leaves_the_epoch_at_its_start`: 377 and 383.
  - `a_missing_account_is_reported_by_both_replacement_entry_points`: 395 and 402.
- `tests/engine_basics.rs`, `a_pending_replacement_is_reconciled_by_the_next_lock_holder`: 57, 65 and 79.

The tests that call `Fx::begin_replacement` keep passing unchanged. Their comments each describe
an `add` that died, which the helper now models (`from_live`, with the live identity naming the
account):
- `tests/add.rs:409` `add_rechecks_the_account_uuid_after_a_pending_replacement_lands`;
- `tests/add.rs:442` `add_token_rechecks_the_kind_after_a_pending_replacement_lands`;
- `tests/add.rs:460` `a_landed_replacement_can_make_add_token_valid`;
- `tests/switch.rs:259` `a_replacement_finished_by_the_lock_is_switched_away_from_with_its_new_kind`.

The last one depends on it. Its reconciliation records b's new epoch, so b's rotated `rt-b2` is
still captured on the way out. With `from_live: false`, Task 3's rule would displace it.

In `crates/tagteam-engine/tests/common/mod.rs`, replace `Fx::begin_replacement` with:

```rust
    /// A pending replacement whose vault write landed but whose last transaction never did:
    /// an `add` that died (§12.5), which `finish_replacement`/`rollback_replacement` exist for.
    /// `identity_json` is the raw `oauthAccount`-shaped object the replacement claims to be.
    pub fn begin_replacement(
        &self,
        id: &AccountId,
        new_bytes: &[u8],
        identity_json: &Value,
        kind: &str,
    ) {
        self.begin_replacement_with(id, new_bytes, identity_json, kind, true);
    }

    /// `begin_replacement` for either kind of replacer. With `from_live`, it is an `add`: its
    /// login is the live one, so the live identity names the account by construction. Without,
    /// it is a replacer that does not take the live login (`add-token`, M5's `import`), whose
    /// evidence comes from the live identity as `add_token` reads it (`Fx::live_names`).
    pub fn begin_replacement_with(
        &self,
        id: &AccountId,
        new_bytes: &[u8],
        identity_json: &Value,
        kind: &str,
        from_live: bool,
    ) {
        let identity = self.cc.parse_identity(identity_json).unwrap();
        let identity_key = self.cc.identity_key(&identity);
        self.put_vault(id, new_bytes);
        let meta = LoginMeta {
            identity_key: identity_key.as_str(),
            identity: &identity,
            kind,
            login_expires_at: None,
            from_live,
        };
        let live_names_account = from_live || self.live_names(id);
        self.engine
            .store()
            .unwrap()
            .begin_replacement(
                id,
                self.cc.fingerprint(new_bytes).unwrap().as_str(),
                &meta,
                live_names_account,
            )
            .unwrap();
    }

    /// Whether the live identity names `id`, as `add_token` decides it (§12.5). An identity
    /// that cannot be read has no answer, and `add_token` refuses it, so a fixture that reaches
    /// one is a broken test.
    pub fn live_names(&self, id: &AccountId) -> bool {
        let row = self.engine.store().unwrap().account(id).unwrap().unwrap();
        match self.cc.live_identity(&self.env) {
            Read::Present(live) => self.cc.identity_key(&live).as_str() == row.identity_key,
            Read::Absent => false,
            Read::Unreadable(e) => panic!("fixture: the live identity cannot be read: {e:?}"),
        }
    }

    /// An explicit replacement of `id`'s login with `new_bytes` by a replacer that does not
    /// take the live login, carried out as `add_token` does it and M5's `import` will
    /// (§12.5 steps 1–3):
    /// - under the mutation lock and the account lock;
    /// - the marker with its evidence;
    /// - the vault write, after which the old generation is `.prev`;
    /// - then the marker cleared.
    ///
    /// The identity stays the account's own. Claude Code keeps whatever it holds.
    pub fn replace_login(&self, id: &AccountId, new_bytes: &[u8], kind: &str) {
        let _guard = self.engine.mutation_guard().unwrap();
        let lock = self.engine.lock_account(id).unwrap();
        let store = self.engine.store().unwrap();
        let row = store.account(id).unwrap().unwrap();
        let identity = self.cc.parse_identity(&row.identity_json).unwrap();
        let meta = LoginMeta {
            identity_key: &row.identity_key,
            identity: &identity,
            kind,
            login_expires_at: self.cc.login_expires_at(new_bytes),
            from_live: false,
        };
        let fp = self.cc.fingerprint(new_bytes).unwrap();
        store
            .begin_replacement(id, fp.as_str(), &meta, self.live_names(id))
            .unwrap();
        self.fixture_vault()
            .store(&lock, new_bytes, &|b| self.cc.fingerprint(b))
            .unwrap();
        store.finish_replacement(id).unwrap();
    }

    /// A vault over the fixture's own backend: the Keychain on macOS, files on Linux, as the
    /// fixture's engine has it.
    fn fixture_vault(&self) -> Vault {
        match self.platform {
            Platform::MacOs => self.keychain_vault(),
            Platform::Linux => Vault::new(Box::new(FileVault::new(
                self.env.data_dir().join("vault"),
            ))),
        }
    }
```

In `crates/tagteam-engine/tests/store.rs`, append:

```rust
/// A replacement of `a@x.co`'s login with `identity`, as `begin_replacement` records it.
fn login_meta(identity: &Identity, from_live: bool) -> LoginMeta<'_> {
    LoginMeta {
        identity_key: "a@x.co\n",
        identity,
        kind: "oauth",
        login_expires_at: None,
        from_live,
    }
}

/// `a` (epoch 0) and `b` stored, the provider's active row set to `active`, then a replacement
/// of `a` begun with `live_names_account`: what the active row holds afterwards.
fn evidence_after(
    active: Option<(&str, Option<i64>)>,
    live_names_account: bool,
) -> Option<Activation> {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("t.db")).unwrap();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    add(&s, &cc(), "b", "b@x.co", 2);
    if let Some((id, epoch)) = active {
        s.set_active(&cc(), Some(&AccountId::from_string(id)), epoch)
            .unwrap();
    }
    let incoming = identity("a@x.co");
    s.begin_replacement(&a, "sha256:x", &login_meta(&incoming, false), live_names_account)
        .unwrap();
    assert_eq!(s.account(&a).unwrap().unwrap().login_epoch, 1);
    s.activation(&cc()).unwrap()
}

#[test]
fn a_replacement_records_the_default_home_s_evidence() {
    // §12.5 "A replacement records its own evidence", row by row. `a` starts at epoch 0, so
    // the evidence is 0 and the replacement moves `a` to 1: stale-marked from here on.
    let on = |id: &str, epoch| {
        Some(Activation {
            account: AccountId::from_string(id),
            epoch,
        })
    };
    // The live identity names a: the row becomes a, whatever it held, including another
    // account (§15.2).
    assert_eq!(evidence_after(None, true), on("a", Some(0)));
    assert_eq!(evidence_after(Some(("b", Some(0))), true), on("a", Some(0)));
    // The row names a without an epoch: one is recorded, live identity or not.
    assert_eq!(evidence_after(Some(("a", None)), false), on("a", Some(0)));
    // Neither names a: nothing is recorded.
    assert_eq!(evidence_after(None, false), None);
    assert_eq!(evidence_after(Some(("b", Some(0))), false), on("b", Some(0)));
}

#[test]
fn a_row_that_already_names_the_account_with_an_epoch_is_kept() {
    // An earlier replacement left the row at 0 while a moved to 1. A second one keeps it at 0,
    // the lineage the live store actually holds, rather than moving the mark to 1.
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("t.db")).unwrap();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    s.set_active(&cc(), Some(&a), Some(0)).unwrap();
    let incoming = identity("a@x.co");
    s.begin_replacement(&a, "sha256:x", &login_meta(&incoming, false), false)
        .unwrap();
    s.finish_replacement(&a).unwrap();

    s.begin_replacement(&a, "sha256:y", &login_meta(&incoming, false), true)
        .unwrap();

    let row = s.account(&a).unwrap().unwrap();
    assert_eq!(row.login_epoch, 2);
    assert_eq!(
        s.activation(&cc()).unwrap(),
        Some(Activation {
            account: a.clone(),
            epoch: Some(0)
        })
    );
    assert!(s.live_store_stale(&row).unwrap());
}

#[test]
fn a_rolled_back_replacement_leaves_its_evidence_current() {
    // The evidence holds the epoch from before the increment, which a rollback restores: a
    // replacement that never landed stale-marks nothing.
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("t.db")).unwrap();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let incoming = identity("a@x.co");
    s.begin_replacement(&a, "sha256:x", &login_meta(&incoming, false), true)
        .unwrap();
    assert!(s.live_store_stale(&s.account(&a).unwrap().unwrap()).unwrap());

    s.rollback_replacement(&a).unwrap();

    let row = s.account(&a).unwrap().unwrap();
    assert_eq!(row.login_epoch, 0);
    assert!(!s.live_store_stale(&row).unwrap());
}

#[test]
fn finishing_a_replacement_taken_from_the_live_login_records_its_epoch() {
    // §10.1, §12.5: `add`'s new login is the live one, so its last transaction makes the live
    // store current again. Any other replacer's leaves it stale-marked.
    for from_live in [true, false] {
        let d = tempfile::tempdir().unwrap();
        let s = Store::open(&d.path().join("t.db")).unwrap();
        let a = add(&s, &cc(), "a", "a@x.co", 1);
        s.set_active(&cc(), Some(&a), Some(0)).unwrap();
        let incoming = identity("a@x.co");
        s.begin_replacement(&a, "sha256:x", &login_meta(&incoming, from_live), true)
            .unwrap();

        s.finish_replacement(&a).unwrap();

        let want = if from_live { 1 } else { 0 };
        assert_eq!(
            s.activation(&cc()).unwrap(),
            Some(Activation {
                account: a.clone(),
                epoch: Some(want)
            }),
            "from_live={from_live}"
        );
        assert_eq!(
            s.live_store_stale(&s.account(&a).unwrap().unwrap()).unwrap(),
            !from_live
        );
    }
}

#[test]
fn a_finish_never_overwrites_a_switch_committed_since_the_replacer_died() {
    // An `add` over a died after its vault write; a switch to b committed before anyone
    // reconciled a. That record is newer than the one the `add` would have written.
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("t.db")).unwrap();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let b = add(&s, &cc(), "b", "b@x.co", 2);
    let incoming = identity("a@x.co");
    s.begin_replacement(&a, "sha256:x", &login_meta(&incoming, true), true)
        .unwrap();
    s.commit_switch(&cc(), &b, 0, &switch_event(&b)).unwrap();

    s.finish_replacement(&a).unwrap();

    assert_eq!(
        s.activation(&cc()).unwrap(),
        Some(Activation {
            account: b,
            epoch: Some(0)
        })
    );
}

#[test]
fn replacement_metadata_without_from_live_is_not_from_the_live_login() {
    // A marker recorded before the field existed.
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("t.db");
    let s = Store::open(&path).unwrap();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    s.set_active(&cc(), Some(&a), Some(0)).unwrap();
    let incoming = identity("a@x.co");
    s.begin_replacement(&a, "sha256:x", &login_meta(&incoming, true), true)
        .unwrap();
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "UPDATE accounts SET replacing_meta = ?1 WHERE id = 'a'",
            [r#"{"identity_key":"a@x.co\n","label":"a@x.co","email":"a@x.co","org_uuid":"","kind":"oauth","identity_json":{"emailAddress":"a@x.co"}}"#],
        )
        .unwrap();

    s.finish_replacement(&a).unwrap();

    assert_eq!(
        s.activation(&cc()).unwrap(),
        Some(Activation {
            account: a,
            epoch: Some(0)
        })
    );
}
```

The raw string `r#"…\n…"#` keeps the two characters `\n`, a JSON escape, so `identity_key`
parses as `"a@x.co\n"`, the key `add` gives `a`.

In `crates/tagteam-engine/tests/add.rs`, append:

```rust
/// `add-token` with a setup token for `s@x.co`.
fn setup_token(fx: &Fx, token: &str) -> AddTokenOptions {
    AddTokenOptions {
        email: Some("s@x.co".into()),
        ..token_opts(fx, token)
    }
}

#[test]
fn a_dead_add_is_reconciled_with_its_activation_epoch() {
    // §12.5, §15.2: an `add` that wrote the vault and died is finished by the next lock holder,
    // and its login was the live one, so the activation epoch moves with it (§10.1). The live
    // store is current again, and what Claude Code rotates next is still captured.
    let fx = Fx::new();
    let b = fx.add("b@work.co", "rt-b");
    let a = fx.add("me@work.co", "rt-1"); // live
    fx.login("me@work.co", "rt-2"); // a new lineage, as `claude /login` leaves it
    fx.begin_replacement(
        &a,
        &common::credential("me@work.co", "rt-2"),
        &Fx::oauth_account("me@work.co"),
        "oauth",
    );
    assert!(
        fx.live_store_stale(&a),
        "stale-marked from the moment the replacement began"
    );

    drop(fx.engine.lock_account(&a).unwrap());

    let row = fx.engine.store().unwrap().account(&a).unwrap().unwrap();
    assert_eq!((row.login_epoch, row.replacing_fp), (1, None));
    assert_eq!(
        fx.activation(),
        Some(Activation {
            account: a.clone(),
            epoch: Some(1)
        })
    );
    fx.rotate_live("rt-3");
    fx.switch_to(&b, false).unwrap();
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-3"));
}

#[test]
fn add_token_over_the_live_account_stale_marks_the_live_store_whatever_the_row_held() {
    // §12.5, §15.2: the live identity names s while the store's record names b, as after a
    // login made outside tagteam. The replacement's first transaction records s at its old
    // epoch; `add-token` writes the vault only, so Claude Code keeps the old token.
    let fx = Fx::new();
    let b = fx.add("b@x.co", "rt-b");
    let s = fx
        .engine
        .add_token(setup_token(&fx, "sk-ant-oat01-first"))
        .unwrap()
        .account
        .id;
    fx.switch_to(&s, false).unwrap(); // the live login: s
    fx.engine
        .store()
        .unwrap()
        .set_active(&fx.provider(), Some(&b), Some(0))
        .unwrap();

    let out = fx
        .engine
        .add_token(setup_token(&fx, "sk-ant-oat01-second"))
        .unwrap();

    assert_eq!((out.created, out.account.login_epoch), (false, 1));
    assert_eq!(
        fx.activation(),
        Some(Activation {
            account: s.clone(),
            epoch: Some(0)
        })
    );
    assert!(fx.live_store_stale(&s));
    assert_eq!(
        fx.live_credential().unwrap()["claudeAiOauth"]["accessToken"],
        "sk-ant-oat01-first"
    );
}

#[test]
fn add_token_over_an_account_the_live_login_does_not_name_records_no_evidence() {
    let fx = Fx::new();
    let s = fx
        .engine
        .add_token(setup_token(&fx, "sk-ant-oat01-first"))
        .unwrap()
        .account
        .id;
    let b = fx.add("b@x.co", "rt-b"); // live, and the store's record

    fx.engine
        .add_token(setup_token(&fx, "sk-ant-oat01-second"))
        .unwrap();

    assert_eq!(
        fx.activation(),
        Some(Activation {
            account: b,
            epoch: Some(0)
        })
    );
    assert!(!fx.live_store_stale(&s));
}

#[test]
fn add_token_refuses_while_the_live_identity_cannot_be_read() {
    // B.1: an identity that cannot be read decides nothing. Either guess could cost a
    // replacement its protection: counting it as naming s would overwrite b's activation
    // evidence (a later switch away from b would then capture b's superseded lineage), and
    // counting it as not naming s could leave s's own superseded lineage unmarked.
    let fx = Fx::new();
    let s = fx
        .engine
        .add_token(setup_token(&fx, "sk-ant-oat01-first"))
        .unwrap()
        .account
        .id;
    fx.add("b@x.co", "rt-b");
    // An unpaired surrogate: the file splices, but the value does not parse.
    let unreadable = common::CLAUDE_JSON.replacen(
        '{',
        r#"{"oauthAccount": {"emailAddress": "b\ud800@x.co"},"#,
        1,
    );
    std::fs::write(fx.paths().global_config, &unreadable).unwrap();
    assert!(matches!(
        fx.cc.live_identity(&fx.env),
        tagteam_provider::Read::Unreadable(_)
    ));

    let before = fx.activation();
    let err = fx
        .engine
        .add_token(setup_token(&fx, "sk-ant-oat01-second"))
        .unwrap_err();

    assert_eq!(err.kind(), "unreadable", "{err}");
    // Nothing was written: the evidence still names b, and s keeps its first login.
    assert_eq!(fx.activation(), before);
    let row = fx.engine.store().unwrap().account(&s).unwrap().unwrap();
    assert_eq!((row.login_epoch, row.replacing_fp), (0, None));
    assert!(
        String::from_utf8(fx.vault_bytes(&s).unwrap())
            .unwrap()
            .contains("sk-ant-oat01-first")
    );
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-engine --test store --test add`
Expected: compile errors in `tests/common/mod.rs` and `tests/store.rs`:
- struct `LoginMeta` has no field named `from_live`;
- `begin_replacement` takes 3 arguments but 4 were supplied.

With the new arguments dropped, these would compile against Task 1's store and fail for the
right reason:
- `a_replacement_records_the_default_home_s_evidence` fails on its first assertion: the row is still `None`, since no evidence is written.
- `finishing_a_replacement_taken_from_the_live_login_records_its_epoch` fails for `from_live=true`: the epoch stays `Some(0)`.
- `a_dead_add_is_reconciled_with_its_activation_epoch` fails on the activation: `Some(0)`, not `Some(1)`.
- `add_token_over_the_live_account_…` fails: the row still names b.
- `add_token_refuses_while_the_live_identity_cannot_be_read` fails: `add_token` succeeds,
  where it must refuse.

`a_rolled_back_replacement_leaves_its_evidence_current`, `a_row_that_already_names_…`,
`a_finish_never_overwrites_…`, `replacement_metadata_without_from_live_…` and
`add_token_over_an_account_the_live_login_does_not_name_records_no_evidence` are guards. They
fail here only by not compiling, and must pass once the rule is in.

- [ ] **Step 3: Implement**

In `crates/tagteam-engine/src/store/mod.rs`, replace `LoginMeta` (116–123) with:

```rust
/// The metadata an explicit replacement installs with its credential (§12.5). It is recorded
/// with the marker, so the next lock holder can finish a replacement whose vault write landed.
pub struct LoginMeta<'a> {
    pub identity_key: &'a str,
    pub identity: &'a Identity,
    pub kind: &'a str,
    pub login_expires_at: Option<i64>,
    /// The login was taken from the live store (`add`, §10.1). Once it lands, the live store
    /// holds exactly the vault's generation, so finishing records the new epoch as the
    /// activation epoch (§12.5). Recorded in `replacing_meta`, so a reconciliation does too.
    pub from_live: bool,
}
```

Replace `begin_replacement` (642–683) with:

```rust
    /// Marks the start of a replacement (§12.5 step 1): bumps the epoch and records both the
    /// incoming fingerprint and the metadata to install once it lands. Guarded so a second
    /// `begin` on an already-pending account is refused rather than clobbering the first.
    ///
    /// The same transaction records the default home's evidence. When `live_names_account`
    /// (the engine read the live identity) or `active_accounts` names the account, and the row
    /// does not already name it with an epoch, the row is set to the account at the epoch it
    /// had before the increment. The live store is then stale-marked whoever activated it and
    /// whatever the row held before; for a login taken from the live store, `finish_replacement`
    /// lifts the mark. A rollback restores that same epoch, so it needs no undo here.
    pub fn begin_replacement(
        &self,
        id: &AccountId,
        fp: &str,
        meta: &LoginMeta<'_>,
        live_names_account: bool,
    ) -> Result<(), StoreError> {
        let meta = json!({
            "identity_key": meta.identity_key,
            "label": meta.identity.label,
            "email": meta.identity.email,
            "org_uuid": meta.identity.org_uuid,
            "org_name": meta.identity.org_name,
            "account_uuid": meta.identity.account_uuid,
            "kind": meta.kind,
            "identity_json": meta.identity.raw,
            "login_expires_at": meta.login_expires_at,
            "from_live": meta.from_live,
        });
        let mut c = self.lock();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let n = tx.execute(
            "UPDATE accounts SET login_epoch = login_epoch + 1, replacing_fp = ?2, replacing_meta = ?3 \
             WHERE id = ?1 AND replacing_fp IS NULL",
            params![id.as_str(), fp, meta.to_string()],
        )?;
        if n == 0 {
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM accounts WHERE id = ?1)",
                [id.as_str()],
                |r| r.get(0),
            )?;
            return Err(if exists {
                StoreError::ReplacementPending
            } else {
                StoreError::NoSuchAccount
            });
        }
        let (provider, epoch): (String, i64) = tx.query_row(
            "SELECT provider, login_epoch FROM accounts WHERE id = ?1",
            [id.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let provider = ProviderId::new(provider);
        let active: Option<(Option<String>, Option<i64>)> = tx
            .query_row(
                "SELECT account_id, login_epoch FROM active_accounts WHERE provider = ?1",
                [provider.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (names, with_epoch) = match &active {
            Some((Some(named), recorded)) if named == id.as_str() => (true, recorded.is_some()),
            _ => (false, false),
        };
        if (live_names_account || names) && !with_epoch {
            set_active_on(&tx, &provider, Some(id), Some(epoch - 1))?;
        }
        tx.commit()?;
        Ok(())
    }
```

Replace `finish_replacement` (685–731) with:

```rust
    /// The replacement landed: installs its recorded metadata, clears any quarantine and the
    /// marker, all in one transaction. A missing `identity_key`, `label` or `kind` in the
    /// recorded metadata means the account and its marker are left exactly as they were
    /// (§12.5) rather than installing an empty identity.
    ///
    /// A login taken from the live store (`from_live`, `add`'s) also records the account's new
    /// `login_epoch` as the activation epoch here (§10.1, §12.5): the live store holds exactly
    /// the vault's generation. Only while `active_accounts` still names the account:
    /// `begin_replacement` made it so, and a switch that committed another account after a
    /// replacer died is the newer record. Metadata without `from_live` is not from the live
    /// store.
    pub fn finish_replacement(&self, id: &AccountId) -> Result<(), StoreError> {
        let mut c = self.lock();
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let row: Option<Option<String>> = tx
            .query_row(
                "SELECT replacing_meta FROM accounts WHERE id = ?1",
                [id.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        let meta = match row {
            None => return Err(StoreError::NoSuchAccount),
            Some(meta) => meta,
        };
        if let Some(m) = meta {
            let v: Value =
                serde_json::from_str(&m).map_err(|e| StoreError::Corrupt(e.to_string()))?;
            let missing = |field: &str| {
                StoreError::Corrupt(format!("replacement metadata is missing its {field} field"))
            };
            let identity_key = v["identity_key"]
                .as_str()
                .ok_or_else(|| missing("identity_key"))?;
            let label = v["label"].as_str().ok_or_else(|| missing("label"))?;
            let kind = v["kind"].as_str().ok_or_else(|| missing("kind"))?;
            let identity = Identity {
                label: label.to_owned(),
                email: v["email"].as_str().map(str::to_owned),
                org_uuid: v["org_uuid"].as_str().unwrap_or_default().to_owned(),
                org_name: v["org_name"].as_str().map(str::to_owned),
                account_uuid: v["account_uuid"].as_str().map(str::to_owned),
                raw: v["identity_json"].clone(),
            };
            let login_expires_at = v["login_expires_at"].as_i64();
            apply_login(&tx, id, identity_key, &identity, kind, login_expires_at)?;
            if v["from_live"].as_bool().unwrap_or(false) {
                tx.execute(
                    "UPDATE active_accounts SET login_epoch = \
                     (SELECT login_epoch FROM accounts WHERE id = ?1) WHERE account_id = ?1",
                    [id.as_str()],
                )?;
            }
        }
        tx.execute(
            "UPDATE accounts SET replacing_fp = NULL, replacing_meta = NULL WHERE id = ?1",
            [id.as_str()],
        )?;
        tx.commit()?;
        Ok(())
    }
```

`rollback_replacement` is unchanged. `Engine::reconcile_replacement` (`engine.rs:244–263`) is
unchanged: it calls `finish_replacement`, which reads `from_live` from `replacing_meta`, so a
dead `add` reconciled under any later account lock records its epoch too.

In `crates/tagteam-engine/src/lifecycle.rs`, add after `struct Prepared` (line 162):

```rust
/// Where a login comes from, for the evidence an explicit replacement records (§12.5).
#[derive(Clone, Copy)]
struct LoginSource {
    /// Taken from the live store (`add`): finishing the replacement records its new epoch as
    /// the activation epoch (§10.1).
    from_live: bool,
    /// The live identity names the account (§12.5 "A replacement records its own evidence").
    live_names_account: bool,
}
```

In `Engine::commit_login`, change the doc comment and signature to:

```rust
    /// Writes the login under the locks `prep` names. The replacement is persisted before the
    /// occupant it displaces is removed, so a failure never loses the occupant. `source` is the
    /// evidence a replacement records for the default home (§12.5).
    #[allow(clippy::too_many_arguments)]
    fn commit_login(
        &self,
        store: &Store,
        p: &dyn Provider,
        provider: &ProviderId,
        prep: &Prepared,
        locks: &[AccountLock],
        identity: &Identity,
        kind: &str,
        secret: &[u8],
        position: Option<u32>,
        alias: Option<&str>,
        source: LoginSource,
    ) -> Result<(AccountRow, bool), EngineError> {
```

and in its replacement branch replace

```rust
                let meta = LoginMeta {
                    identity_key: key.as_str(),
                    identity,
                    kind,
                    login_expires_at: p.login_expires_at(secret),
                };
                store.begin_replacement(&row.id, &new_fp, &meta)?;
```

with

```rust
                let meta = LoginMeta {
                    identity_key: key.as_str(),
                    identity,
                    kind,
                    login_expires_at: p.login_expires_at(secret),
                    from_live: source.from_live,
                };
                // §12.5 step 1, with the default home's evidence in the same transaction.
                store.begin_replacement(&row.id, &new_fp, &meta, source.live_names_account)?;
```

The rest of `commit_login` is unchanged. The new-account branch records no evidence.

In `Engine::add_live`, replace

```rust
        let (account, created) = self.commit_login(
            &store,
            p.as_ref(),
            &opts.provider,
            &prep,
            &accounts,
            &identity,
            &kind,
            now.bytes(),
            opts.position,
            alias.as_deref(),
        )?;
        drop(live_locks);
        store.set_active(&opts.provider, Some(&account.id), Some(account.login_epoch))?;
```

with

```rust
        let (account, created) = self.commit_login(
            &store,
            p.as_ref(),
            &opts.provider,
            &prep,
            &accounts,
            &identity,
            &kind,
            now.bytes(),
            opts.position,
            alias.as_deref(),
            // `add`'s login is the live one, verified just above under the live locks.
            LoginSource {
                from_live: true,
                live_names_account: true,
            },
        )?;
        drop(live_locks);
        // §10.1: a new account's activation, in a write of its own (no replacement can
        // stale-mark an account that did not exist). A replacement's last transaction has
        // already recorded this same epoch.
        store.set_active(&opts.provider, Some(&account.id), Some(account.login_epoch))?;
```

In `Engine::add_token`, replace everything from `let accounts = self.lock_prepared(&prep)?;`
(line 553) to the end of the function with:

```rust
        let accounts = self.lock_prepared(&prep)?;
        // The different-kind collision (§10.2) is decided only now: taking the account lock
        // may have finished a pending replacement and changed the account's kind. The uuid
        // conflict check is re-run here too, for the same reason (Task 18's review, item 1) —
        // a no-op today since a token identity never claims a uuid, but it keeps the two
        // rechecks in one place rather than only one of them surviving the next change.
        let current = store.account(&prep.id)?;
        check_identity_conflict(current.as_ref(), claimed_uuid, &identity.label)?;
        if let Some(existing) = &current {
            if existing.kind != kind {
                return Err(EngineError::InvalidInput(format!(
                    "{} is already stored as a {} account",
                    identity.label, existing.kind
                )));
            }
        }
        // §12.5: whether the live identity names this account, read now that its lock is
        // held: a file read, with no Keychain. One that cannot be read leaves the evidence
        // undecidable, and either guess can cost a replacement its protection, so nothing is
        // written (B.1).
        let live_names_account = match p.live_identity(&self.env) {
            Read::Present(live) => p.identity_key(&live) == p.identity_key(&identity),
            Read::Absent => false,
            Read::Unreadable(e) => return Err(EngineError::Unreadable(e)),
        };
        let (account, created) = self.commit_login(
            &store,
            p.as_ref(),
            &opts.provider,
            &prep,
            &accounts,
            &identity,
            &kind,
            &secret,
            opts.position,
            alias.as_deref(),
            LoginSource {
                from_live: false,
                live_names_account,
            },
        )?;
        Ok(AddOutcome {
            account,
            created,
            notices: vec![],
        })
    }
```

In `crates/tagteam-engine/tests/engine_basics.rs`, `a_pending_replacement_is_reconciled_by_the_next_lock_holder`:
- add `from_live: false,` to the `LoginMeta` literal (line 57);
- change the two calls to `.begin_replacement(&id, "sha256:never-written", &meta, false)` (line 65) and `store.begin_replacement(&id, fp.as_str(), &meta, false)` (line 79).

- [ ] **Step 4: Run them and see them pass**, then the crate's whole suite

Run: `cargo test -p tagteam-engine --test store --test add --test engine_basics --test switch`
Expected: PASS, including the ten new tests and
`a_replacement_finished_by_the_lock_is_switched_away_from_with_its_new_kind`.

Run: `cargo test -p tagteam-engine --features test-hooks`
Expected: PASS.

Run: `cargo test --workspace --features tagteam/test-support`
Expected: PASS.

- [ ] **Step 5: Format and lint**

`cargo fmt --all && cargo fmt --all --check`, and `cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings`, and `cargo clippy --workspace --all-targets -- -D warnings`

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-engine/src/store/mod.rs crates/tagteam-engine/src/lifecycle.rs \
  crates/tagteam-engine/tests/common/mod.rs crates/tagteam-engine/tests/store.rs \
  crates/tagteam-engine/tests/add.rs crates/tagteam-engine/tests/engine_basics.rs
git commit -m "Record a replacement's evidence for the default home and add's activation epoch"
```

---

### Task 3: Switch and recovery: record the epoch, displace a stale live store

A live store that is stale-marked holds a lineage an explicit replacement superseded. The
switch's outgoing capture and recovery's capture would put that lineage back over the
replacement. §9.4 step 4 bounds both captures: such a credential is displaced instead.
Decision 4 makes the bound a pure fact. `OutgoingFacts.live_store_stale` turns a capture into
`Displace` in `decide_outgoing`. The class stays, so the logs still say `OursRotated` or
`Unresolved`, and the switch and recovery share the rule.

The epoch-recording half of this task's title landed in Task 1, because changing
`commit_switch`'s and `set_active`'s signatures, and adding `JournalRow.to_epoch`, forced a
value at every production call site. Task 1 writes the final ones, and this task does not
repeat them:
- the switch's commit records `target.login_epoch`;
- its journal row carries `Some(target.login_epoch)`;
- `add_live` records `Some(account.login_epoch)`.

Recovery still commits `to.login_epoch`. This task makes it record the row's own epoch
(§9.6), so a replacement that landed on the target since the row was written leaves the live
store stale-marked.

**Readings of the spec this task commits to:**
- **Only a capture is bounded.** `Ours`, `Superseded` and `Wiped` capture nothing, so the stale
  mark leaves them as they are. `Foreign` is displaced already.
- **The third warning wins over the refresh-token one.** A credential that is both stale and
  lacks a refresh token gets the stale warning: the replacement is the reason it never belongs
  in the vault.
- **Recovery displaces through its existing path.** `capture_rotated_outgoing` returns
  `false`, and `finish_forward` saves the entry with `save_unheld`, which logs "the previous
  live credential was saved as displaced/…" at WARN. Recovery prints no warnings of its own.
- **Review Focus 3's switch half** is tested with `Fx::replace_login` (Task 2), which runs the
  store calls `add_token` runs, over an OAuth login. Task 4 says why `add-token` itself cannot
  produce this state.

**Files:**
- Modify: `crates/tagteam-core/src/classify.rs`:
  - 10–26, `OutgoingFacts.live_store_stale`
  - 45–68, `decide_outgoing`
  - 75–85, the test helper `facts()`, plus a new test
- Modify: `crates/tagteam-engine/src/switch.rs`. In `settle_outgoing`: the facts literal at 1331–1342, and the third warning in the `Displace` arm at 1371–1394.
- Modify: `crates/tagteam-engine/src/recover.rs`:
  - `finish_forward`, the epoch in the commit at 285
  - `capture_rotated_outgoing`, the doc at 302–311 and the facts literal at 338–348
- Test: `crates/tagteam-core/src/classify.rs` (unit), `crates/tagteam-engine/tests/switch.rs`, `crates/tagteam-engine/tests/recover.rs`

**Interfaces:**
- Consumes:
  - Task 1: `Store::live_store_stale`, `Store::commit_switch(.., epoch, ..)`, `JournalRow.to_epoch`, `crash_row`'s `to_epoch`, `Fx::activation`, `Fx::live_store_stale`; and the switch's commit epoch, its journal row's `to_epoch` and `add_live`'s epoch, which Task 1 already writes (not repeated here).
  - Task 2: `Fx::replace_login`, `Fx::begin_replacement_with`.
- Produces:
  - `OutgoingFacts.live_store_stale: bool` (Interface Contract, `classify.rs`).
  - The `Displace` warning "the live credential predates position {n}'s replacement, so it did not replace the stored one; it was saved as displaced/{id}.json".
  - Recovery's commit epoch `row.to_epoch.unwrap_or(to.login_epoch)`.

**Spec:**
- §9.4 step 4: "a live credential whose activation epoch is stale (§12.5) never replaces the replacement that made it stale … displaced instead".
- §9.6, forward finish: the activation epoch recorded is the row's `to_epoch`, falling back to `to_id`'s current `login_epoch`, "so a replacement that landed on `to_id` since leaves the live store stale-marked". The capture of a cleared entry is classified as §9.4 step 4 would classify it.
- §12.5, the default home's activation epoch:
  - "A stale-marked live store is never captured: the switch's outgoing capture and recovery's capture displace it instead".
  - "`tagteam switch <N> --force` re-activates the vault generation … and its commit records the current epoch".
- §15.2, "Activation epoch":
  - an `import` over the live account is never undone by the switch's capture or recovery;
  - `switch <N> --force` clears the stale mark;
  - a forward recovery after a replacement leaves the live store stale-marked.

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-core/src/classify.rs`'s test module, replace `facts()` with:

```rust
    fn facts() -> OutgoingFacts {
        OutgoingFacts {
            bytes_equal_vault: false,
            fp_equal_vault: false,
            equals_vault_prev: false,
            wiped: false,
            tokenless: false,
            oracle: OracleVerdict::Unavailable,
            lacks_refresh_over_complete: false,
            live_store_stale: false,
        }
    }
```

Every other literal in the module spreads `..facts()`, so `facts()` is the only one that changes
(Decision 4). Append:

```rust
    #[test]
    fn a_stale_marked_live_store_is_displaced_never_captured() {
        // §9.4 step 4, §12.5: the class stays, so the log still says which capture it was.
        for (oracle, class) in [
            (OracleVerdict::ThisAccount, OutgoingClass::OursRotated),
            (OracleVerdict::Unavailable, OutgoingClass::Unresolved),
        ] {
            let f = OutgoingFacts {
                oracle,
                live_store_stale: true,
                ..facts()
            };
            assert_eq!(
                decide_outgoing(&f),
                (class, OutgoingAction::Displace),
                "{oracle:?}"
            );
        }
        // The rows that capture nothing are as they were.
        let stale = OutgoingFacts {
            live_store_stale: true,
            ..facts()
        };
        for (f, want) in [
            (
                OutgoingFacts {
                    fp_equal_vault: true,
                    ..stale
                },
                (OutgoingClass::Ours, OutgoingAction::Nothing),
            ),
            (
                OutgoingFacts {
                    equals_vault_prev: true,
                    ..stale
                },
                (OutgoingClass::Superseded, OutgoingAction::Nothing),
            ),
            (
                OutgoingFacts {
                    wiped: true,
                    ..stale
                },
                (OutgoingClass::Wiped, OutgoingAction::Nothing),
            ),
            (
                OutgoingFacts {
                    oracle: OracleVerdict::OtherIdentity,
                    ..stale
                },
                (OutgoingClass::Foreign, OutgoingAction::Displace),
            ),
        ] {
            assert_eq!(decide_outgoing(&f), want);
        }
    }
```

In `crates/tagteam-engine/tests/switch.rs`, add `credential` to the `common` import, and append:

```rust
/// The warning a displaced stale live store leaves, up to the displaced file's name.
const STALE_DISPLACED: &str = "the live credential predates position 2's replacement, so it did not replace the stored one; it was saved as displaced/";

#[test]
fn switching_away_from_a_replaced_live_login_displaces_it_instead_of_capturing() {
    // §9.4 step 4, §12.5, Review Focus 3: b's login was replaced while Claude Code kept the old
    // one and went on refreshing it. Attributed or not, that lineage is displaced.
    for attributed in [false, true] {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let b = fx.add("b@x.co", "rt-b"); // live, position 2
        fx.replace_login(&b, &credential("b@x.co", "rt-b-new"), "oauth");
        fx.rotate_live("rt-b2");
        if attributed {
            let owner = fx.cc.parse_identity(&Fx::oauth_account("b@x.co")).unwrap();
            fx.oracle.set(Some(owner));
        }

        let out = switch(&fx, to(&a), false).unwrap();

        assert_eq!(
            fx.vault_refresh_token(&b).as_deref(),
            Some("rt-b-new"),
            "attributed={attributed}: the replacement stays"
        );
        let displaced = fx.displaced();
        assert_eq!(displaced.len(), 1, "attributed={attributed}");
        assert!(String::from_utf8_lossy(&displaced[0]).contains("rt-b2"));
        assert!(
            out.warnings.iter().any(|w| w.starts_with(STALE_DISPLACED)),
            "{:?}",
            out.warnings
        );
        assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
    }
}

#[test]
fn a_self_switch_over_a_replaced_live_login_activates_the_replacement() {
    // §9.2: a self-switch whose live credential diverged runs a full switch once the oracle
    // attributes it to the account. Its step 4 displaces the old lineage instead of capturing
    // it back, so the replacement is what gets activated, and the mark is cleared.
    let fx = Fx::new();
    fx.add("b@x.co", "rt-b");
    let a = fx.add("a@x.co", "rt-a"); // live, position 2
    fx.replace_login(&a, &credential("a@x.co", "rt-a-new"), "oauth");
    fx.rotate_live("rt-a2");
    let owner = fx.cc.parse_identity(&Fx::oauth_account("a@x.co")).unwrap();
    fx.oracle.set(Some(owner));

    let out = switch(&fx, to(&a), false).unwrap();

    assert_eq!(out.reason, SwitchReason::Activated);
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a-new"));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a-new"));
    assert!(
        out.warnings.iter().any(|w| w.starts_with(STALE_DISPLACED)),
        "{:?}",
        out.warnings
    );
    assert_eq!(
        fx.activation(),
        Some(Activation {
            account: a.clone(),
            epoch: Some(1)
        })
    );
}

#[test]
fn a_forced_switch_to_a_replaced_live_login_clears_the_stale_mark() {
    // §12.5: `tagteam switch <N> --force` re-activates the vault's generation, as a forced
    // self-switch does (§9.2), and its commit records the current epoch. Claude Code's old
    // lineage is displaced, not lost.
    let fx = Fx::new();
    fx.add("b@x.co", "rt-b");
    let a = fx.add("a@x.co", "rt-a");
    fx.replace_login(&a, &credential("a@x.co", "rt-a-new"), "oauth");
    fx.rotate_live("rt-a2");
    assert!(fx.live_store_stale(&a));

    let out = switch(&fx, to(&a), true).unwrap();

    assert!(out.switched);
    assert!(!fx.live_store_stale(&a));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a-new"));
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a-new"));
    let displaced = fx.displaced();
    assert_eq!(displaced.len(), 1);
    assert!(String::from_utf8_lossy(&displaced[0]).contains("rt-a2"));
}
```

In `crates/tagteam-engine/tests/recover.rs`, add `credential` to the `common` import, change
`use tagteam_engine::store::JournalRow;` to `use tagteam_engine::store::{Activation, JournalRow};`,
and append:

```rust
#[test]
fn forward_recovery_never_captures_a_replaced_live_login() {
    // §9.6, §12.5: a's login was replaced while Claude Code kept the old one; then a switch to
    // k died after writing the key, and Claude Code rotated its copy of the old lineage. The
    // oracle attributes that rotation to a, yet capturing it would undo the replacement: it
    // is displaced.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let k = fx.add_api_key(API_KEY); // leaves a live
    fx.replace_login(&a, &credential("a@x.co", "rt-a-new"), "oauth");
    crashed_switch(&fx, &a, &k);
    fx.put_managed_key(API_KEY.as_bytes());
    fx.rotate_live("rt-a-rotated-by-cc");
    oracle_says(&fx, "a@x.co");

    any_mutation(&fx, &a);

    assert_journal_cleared(&fx);
    assert_eq!(active(&fx), Some(k));
    assert_eq!(
        fx.vault_refresh_token(&a).as_deref(),
        Some("rt-a-new"),
        "the replacement stays"
    );
    let displaced = fx.displaced();
    assert_eq!(displaced.len(), 1);
    assert!(String::from_utf8_lossy(&displaced[0]).contains("rt-a-rotated-by-cc"));
}

#[test]
fn forward_recovery_records_the_row_s_epoch_so_a_later_replacement_stays_stale_marked() {
    // §9.6: a forward finish records the row's `to_epoch`. A replacement that landed on a
    // since the row was written then leaves the live store stale-marked. A row from before the
    // column falls back to a's current epoch.
    for (to_epoch, stale) in [(Some(0), true), (None, false)] {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let b = fx.add("b@x.co", "rt-b"); // live: b
        let row = JournalRow {
            to_epoch,
            ..crash_row(&fx, &b, &a)
        };
        fx.engine.store().unwrap().insert_journal(&row).unwrap();
        write_target_credential(&fx, &a); // the switch landed a's credential, then died
        // Then a replacement of a's login wrote the vault and its replacer died; recovery's
        // own account lock finishes it before recovering the row.
        fx.begin_replacement_with(
            &a,
            &credential("a@x.co", "rt-a-new"),
            &Fx::oauth_account("a@x.co"),
            "oauth",
            false,
        );

        any_mutation(&fx, &a);

        assert_journal_cleared(&fx);
        let want = if stale { 0 } else { 1 };
        assert_eq!(
            fx.activation(),
            Some(Activation {
                account: a.clone(),
                epoch: Some(want)
            }),
            "to_epoch={to_epoch:?}"
        );
        assert_eq!(fx.live_store_stale(&a), stale, "to_epoch={to_epoch:?}");
    }
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-core classify`
Expected: compile error: struct `OutgoingFacts` has no field named `live_store_stale`.

Run: `cargo test -p tagteam-engine --test switch --test recover`
Expected: FAIL:
- `switching_away_from_a_replaced_live_login_displaces_it_instead_of_capturing`: b's vault holds `rt-b2`, the captured old lineage, not `rt-b-new`.
- `a_self_switch_over_a_replaced_live_login_activates_the_replacement`: a's vault holds `rt-a2`.
- `forward_recovery_never_captures_a_replaced_live_login`: a's vault holds `rt-a-rotated-by-cc`.
- `forward_recovery_records_the_row_s_epoch_so_a_later_replacement_stays_stale_marked`, `to_epoch=Some(0)`: the activation epoch is `Some(1)`, from `to.login_epoch`.

`a_forced_switch_to_a_replaced_live_login_clears_the_stale_mark` already passes: Task 1's commit
records the epoch. It pins §12.5's last rule.

- [ ] **Step 3: Implement**

In `crates/tagteam-core/src/classify.rs`, add to `OutgoingFacts`, after
`lacks_refresh_over_complete`:

```rust
    /// §9.4 step 4 / §12.5: the live store is stale-marked for the outgoing account, so a
    /// capture would undo an explicit replacement. Turns a capture into `Displace`.
    pub live_store_stale: bool,
```

and replace `decide_outgoing` with:

```rust
/// The §9.4 step 4 table, `Superseded` included, with §6.2's bounds on an automatic capture. A
/// capture never replaces a refresh token with a credential that lacks one, and never takes a
/// stale-marked live store (§12.5), which would undo the replacement that marked it. Either is
/// displaced instead, under its class. A credential with no token at all is never captured
/// either: it is left alone like a wiped blob, and the vault keeps its generation.
pub fn decide_outgoing(f: &OutgoingFacts) -> (OutgoingClass, OutgoingAction) {
    if f.bytes_equal_vault || f.fp_equal_vault {
        return (OutgoingClass::Ours, OutgoingAction::Nothing);
    }
    if f.equals_vault_prev {
        return (OutgoingClass::Superseded, OutgoingAction::Nothing);
    }
    if f.wiped || f.tokenless {
        return (OutgoingClass::Wiped, OutgoingAction::Nothing);
    }
    let (class, backfill_uuid) = match f.oracle {
        OracleVerdict::ThisAccount => (OutgoingClass::OursRotated, true),
        OracleVerdict::OtherIdentity => return (OutgoingClass::Foreign, OutgoingAction::Displace),
        OracleVerdict::Unavailable => (OutgoingClass::Unresolved, false),
    };
    if f.lacks_refresh_over_complete || f.live_store_stale {
        (class, OutgoingAction::Displace)
    } else {
        (class, OutgoingAction::CaptureToVault { backfill_uuid })
    }
}
```

In `crates/tagteam-engine/src/switch.rs`, `Engine::settle_outgoing`, replace

```rust
            lacks_refresh_over_complete: !p.has_refresh_token(&bytes)
                && vault.as_deref().is_some_and(|v| p.has_refresh_token(v)),
        };
```

with

```rust
            lacks_refresh_over_complete: !p.has_refresh_token(&bytes)
                && vault.as_deref().is_some_and(|v| p.has_refresh_token(v)),
            // `out` was read under its account lock, so its epoch cannot move until the commit.
            live_store_stale: store.live_store_stale(out)?,
        };
```

and in the same function replace

```rust
                warnings.push(if class == OutgoingClass::Foreign {
                    format!(
                        "the live credential did not belong to position {}; it was saved as displaced/{id}.json",
                        out.position
                    )
                } else {
                    format!(
                        "the live credential had no refresh token, so it did not replace position {}'s stored one; it was saved as displaced/{id}.json",
                        out.position
                    )
                });
```

with

```rust
                warnings.push(if class == OutgoingClass::Foreign {
                    format!(
                        "the live credential did not belong to position {}; it was saved as displaced/{id}.json",
                        out.position
                    )
                } else if facts.live_store_stale {
                    format!(
                        "the live credential predates position {}'s replacement, so it did not replace the stored one; it was saved as displaced/{id}.json",
                        out.position
                    )
                } else {
                    format!(
                        "the live credential had no refresh token, so it did not replace position {}'s stored one; it was saved as displaced/{id}.json",
                        out.position
                    )
                });
```

`facts` is `Copy` and still in scope: `decide_outgoing(&facts)` borrowed it.

In `crates/tagteam-engine/src/recover.rs`, `Engine::finish_forward`, replace

```rust
        store.commit_switch(
            &row.provider,
            &to.id,
            to.login_epoch,
            &EventRow {
```

with

```rust
        // §9.6: the epoch the row journaled, so a replacement that landed on the target since
        // leaves the live store stale-marked. A row written before the column falls back to the
        // target's current epoch.
        store.commit_switch(
            &row.provider,
            &to.id,
            row.to_epoch.unwrap_or(to.login_epoch),
            &EventRow {
```

In `Engine::capture_rotated_outgoing`, change the doc comment's sentence "and §6.2's
refresh-token bound applies through `decide_outgoing`." to "and §6.2's refresh-token bound and
§12.5's stale mark apply through `decide_outgoing`.", then replace

```rust
            lacks_refresh_over_complete: !p.has_refresh_token(bytes)
                && vault.as_deref().is_some_and(|v| p.has_refresh_token(v)),
        };
```

with

```rust
            lacks_refresh_over_complete: !p.has_refresh_token(bytes)
                && vault.as_deref().is_some_and(|v| p.has_refresh_token(v)),
            // Recovery holds `from`'s account lock, so its epoch cannot move under this read.
            live_store_stale: store.live_store_stale(from)?,
        };
```

Those are the only two `OutgoingFacts` literals outside `classify.rs`: `switch.rs:1331` and
`recover.rs:338`.

- [ ] **Step 4: Run them and see them pass**, then the crate's whole suite

Run: `cargo test -p tagteam-core`
Expected: PASS.

Run: `cargo test -p tagteam-engine --test switch --test recover`
Expected: PASS, including the five new tests. The capture tests that existed before still
capture, since nothing in them is stale-marked:
- `a_rotated_outgoing_credential_is_captured_before_switching`;
- `a_resolved_self_switch_activates_the_generation_it_captured`;
- `forward_recovery_captures_a_rotated_outgoing_token_the_oracle_attributes`;
- `a_replacement_finished_by_the_lock_is_switched_away_from_with_its_new_kind`.

Run: `cargo test -p tagteam-engine --features test-hooks`
Expected: PASS.

Run: `cargo test --workspace --features tagteam/test-support`
Expected: PASS.

- [ ] **Step 5: Format and lint**

`cargo fmt --all && cargo fmt --all --check`, and `cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings`, and `cargo clippy --workspace --all-targets -- -D warnings`

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-core/src/classify.rs crates/tagteam-engine/src/switch.rs \
  crates/tagteam-engine/src/recover.rs crates/tagteam-engine/tests/switch.rs \
  crates/tagteam-engine/tests/recover.rs
git commit -m "Displace a stale live store instead of capturing it, and recover with the journaled epoch"
```

---

### Task 4: §7.5 `Replaced` and its usage mapping

The active-token refresh is the last place a stale-marked live store could be adopted. §7.5
step 3's "any other full token pair" row writes the live credential into the vault, and step 5
refreshes it. Step 2 now stops first, with `ActiveOutcome::Replaced`. The check comes right
after the `LiveMoved` check, under the guard, the account lock and CC's credential locks. It
comes before `active_quarantine` and `reconcile_active`, so nothing is read further, sent or
written. The usage collector reports `Replaced` as the `last_error` token `live-replaced`
(Decision 10: `unavailable`), with a warning that names the forced switch which activates the
replacement.

**Review Focus 3, and how it is tested.** The focus line reads: "`add-token` over the live
account, then its access token expires." §7.5 and the stale displacement need an *OAuth* live
login whose account was replaced while Claude Code kept the old lineage. In M4a no command
produces one:
- **`add-token` can replace only an account of its own kind.** §10.2 refuses a different kind
  under the same email. Over a live setup-token account, it does stale-mark the live store,
  which Task 2 tests. But a setup token never refreshes: `collect.rs` keeps a non-refreshable
  kind away from §7.5, and `active_login` refuses one. Its live copy also equals the vault's
  `.prev`, so a switch away classifies it `Superseded` and captures nothing anyway.
- **`add` never leaves a stale live store.** Its login *is* the live one, so its last
  transaction records the new epoch (§10.1). A dead `add` is reconciled with that epoch too
  (Task 2). An `add` of a new lineage for the live account is the negative case.
- **M5's `import` is the real replacer of a live OAuth account.** It takes exactly
  `add_token`'s store calls: the evidence, the vault write and the marker cleared.
  `Fx::replace_login` (Task 2) runs those calls, with the evidence computed from the live
  identity as `add_token` computes it.

So the end-to-end test replaces the live OAuth account with `Fx::replace_login`. It then lets
CC rotate its copy, and walks the rest of the focus line through the real commands: §7.5, a
usage collection, a switch away and a switch back.

**Files:**
- Modify: `crates/tagteam-engine/src/active.rs`:
  - 39–62, `ActiveOutcome::Replaced`
  - 120–128, the step 2 check in `refresh_active`
- Modify: `crates/tagteam-engine/src/collect.rs`:
  - 395–436, `Collection::refresh_live`
  - after 680, a new `Collection::warn_replaced`
- Modify: `crates/tagteam-engine/src/views.rs`. A test row only, in `tests::usage_status_follows_the_table_row_by_row` at 785–795. `usage_status` already maps an unknown token to `Unavailable`.
- Test: `crates/tagteam-engine/tests/active.rs`, `crates/tagteam-engine/tests/collect_active.rs`

**Interfaces:**
- Consumes:
  - Task 1: `Store::live_store_stale`, `Activation`, `Fx::activation`, `Fx::live_store_stale`.
  - Task 2: `Fx::replace_login`.
  - Task 3: the stale displacement on a switch away.
- Produces:
  - `ActiveOutcome::Replaced` (Interface Contract, `active.rs`).
  - The `last_error` token `live-replaced`.
  - The collect warning "{label} (position {n})'s login was replaced while Claude Code kept the old one; run `tagteam switch {n} --force` to activate the replacement" (Interface Contract, `collect.rs`).

**Spec:**
- §7.5 step 2: if the account's activation epoch is stale, stop. "Nothing is read further or sent. The outcome is `Replaced`; a usage fetch reports it as `unavailable`, with a warning to run `tagteam switch <N> --force`."
- §8.1, active account: any outcome other than the usable four or `Dead` "reports `unavailable` with a warning".
- §12.5: "the active-token refresh neither adopts nor refreshes it (§7.5 `Replaced`). CC goes on using and refreshing it."
- Decision 10: `live-replaced` falls through to `usageStatus: unavailable`.
- Review Focus 3: `Replaced`, the vault keeps the replacement, and a later switch away displaces the old lineage.

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-engine/tests/active.rs`, change the `common` import to:

```rust
use common::{
    Fx, block_rescue, failed, prev_refresh_token, quarantine_of, rescue_files, token_requests,
    unblock_rescue,
};
```

add `use tagteam_engine::store::Activation;`, and append:

```rust
#[test]
fn a_stale_marked_live_store_is_replaced_never_adopted_or_refreshed() {
    // §7.5 step 2: a's login was replaced while Claude Code kept the old one and rotated it.
    // Step 3 would adopt that rotation over the replacement and step 5 refresh it; neither
    // happens, for either trigger, and nothing is written.
    let fx = Fx::new();
    fx.add("b@x.co", "rt-b");
    let a = fx.add("a@x.co", "rt-a");
    fx.replace_login(&a, &cred("a@x.co", "rt-new"), "oauth");
    fx.rotate_live("rt-a2");
    expire_live(&fx);
    fx.script_refresh(Some("rt-x")); // what a refresh would spend, if one were sent
    let live = bytes(&fx.live_credential().unwrap());
    let before = fx.kc.items();

    for trigger in [
        ActiveTrigger::Expired,
        ActiveTrigger::Rejected {
            access_fp: fx.cc.access_fingerprint(&live).unwrap().as_str().to_owned(),
        },
    ] {
        assert_eq!(
            active(&fx, trigger.clone()).unwrap(),
            ActiveOutcome::Replaced,
            "{trigger:?}"
        );
    }

    assert_eq!(token_requests(&fx), 0);
    assert_eq!(fx.kc.items(), before, "neither the vault nor the live store was written");
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-new"));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a2"));
}

#[test]
fn a_replacement_of_the_live_login_is_never_undone() {
    // Review Focus 3, end to end. a's login was replaced while it was live (as `import` does
    // it, Task 4's note), and Claude Code went on refreshing its own copy.
    let fx = Fx::new();
    let b = fx.add("b@x.co", "rt-b");
    let a = fx.add("a@x.co", "rt-a"); // live, position 2
    fx.replace_login(&a, &cred("a@x.co", "rt-new"), "oauth");
    fx.rotate_live("rt-a2");
    expire_live(&fx);

    // §7.5: Replaced, and the vault keeps the replacement.
    assert_eq!(
        active(&fx, ActiveTrigger::Expired).unwrap(),
        ActiveOutcome::Replaced
    );
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-new"));

    // §8.1: the usage fetch reports it, with the command that activates the replacement.
    let report = fx.collect(&[&a]);
    assert_eq!(report.outcomes, [(a.clone(), failed("live-replaced"))]);
    assert!(report.warnings[0].contains("`tagteam switch 2 --force`"));
    assert_eq!(token_requests(&fx), 0);

    // §9.4 step 4: switching away displaces Claude Code's old lineage instead of capturing it.
    let out = fx.switch_to(&b, false).unwrap();
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-new"));
    assert!(
        out.warnings
            .iter()
            .any(|w| w.starts_with("the live credential predates position 2's replacement")),
        "{:?}",
        out.warnings
    );
    assert!(
        fx.displaced()
            .iter()
            .any(|d| String::from_utf8_lossy(d).contains("rt-a2"))
    );

    // Switching back activates the replacement, and the commit clears the mark: §7.5 refreshes
    // that lineage as usual.
    fx.switch_to(&a, false).unwrap();
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-new"));
    assert_eq!(
        fx.activation(),
        Some(Activation {
            account: a.clone(),
            epoch: Some(1)
        })
    );
    expire_live(&fx);
    fx.script_refresh(Some("rt-new2"));
    assert_eq!(
        active(&fx, ActiveTrigger::Expired).unwrap(),
        ActiveOutcome::Refreshed
    );
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-new2"));
}
```

In `crates/tagteam-engine/tests/collect_active.rs`, append:

```rust
#[test]
fn a_replaced_live_login_reports_live_replaced_and_names_the_forced_switch() {
    // §7.5 step 2, §8.1, Decision 10: §7.5 stops with `Replaced`, so nothing is refreshed or
    // fetched; the failure is `live-replaced` (unavailable), its slot goes back, and one
    // warning says how to activate the replacement.
    let fx = Fx::new();
    fx.add("b@x.co", "rt-b");
    let a = fx.add("a@x.co", "rt-a"); // live, position 2
    fx.replace_login(&a, &credential("a@x.co", "rt-new"), "oauth");
    fx.rotate_live("rt-a2");
    expire_live(&fx);
    fx.script_refresh(Some("rt-x"));
    fx.script_usage(200, usage_fixture());

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), failed("live-replaced"))]);
    assert_eq!(
        report.warnings,
        ["a@x.co (position 2)'s login was replaced while Claude Code kept the old one; run `tagteam switch 2 --force` to activate the replacement"]
    );
    assert!(fx.http.requests().is_empty(), "nothing refreshed or fetched");
    assert_eq!(usage_requests(&fx), 0, "nothing was sent: the slot went back");
    assert_eq!(
        fx.usage_state(&a).unwrap().last_error.as_deref(),
        Some("live-replaced")
    );
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-new"));
}
```

In `crates/tagteam-engine/src/views.rs`, `tests::usage_status_follows_the_table_row_by_row`,
add `("live-replaced", Unavailable),` to the `for (error, want) in [ … ]` list, after
`("refresh-failed", Unavailable),`.

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-engine --test active`
Expected: compile error: no variant named `Replaced` found for enum `ActiveOutcome`.

Run: `cargo test -p tagteam-engine --test collect_active`
Expected: FAIL. In `a_replaced_live_login_reports_live_replaced_and_names_the_forced_switch`,
the outcome is `Recorded`, not `Failed { kind: "live-replaced" }`: §7.5 adopted `rt-a2` into
the vault, refreshed it with the scripted reply, and the fetch went on.

Run: `cargo test -p tagteam-engine --lib views`
Expected: PASS already. The new `live-replaced` row pins Decision 10's fall-through, which must
keep holding.

- [ ] **Step 3: Implement**

In `crates/tagteam-engine/src/active.rs`, add to `ActiveOutcome`, after `Unpersisted`:

```rust
    /// §7.5 step 2: the account's activation epoch is stale (§12.5). An explicit command
    /// replaced its login while Claude Code kept the old one, and adopting or refreshing that
    /// lineage would undo the replacement. Nothing was read further, sent or written; Claude
    /// Code goes on refreshing its own copy until `tagteam switch <N> --force`.
    Replaced,
```

In `Engine::refresh_active`, replace

```rust
            // Step 2: the same account, read fresh, now that CC cannot rotate it.
            let (row, live) = self.active_login(p, provider)?;
            if &row.id != lock.id() {
                return Err(EngineError::LiveMoved);
            }
            // §7.4: a quarantined generation is never sent again.
```

with

```rust
            // Step 2: the same account, read fresh, now that CC cannot rotate it.
            let (row, live) = self.active_login(p, provider)?;
            if &row.id != lock.id() {
                return Err(EngineError::LiveMoved);
            }
            // §12.5: a replacement superseded the lineage the live store holds. `row` was read
            // under the account lock, so its epoch cannot move before this returns.
            if self.store()?.live_store_stale(&row)? {
                return Ok(ActiveOutcome::Replaced);
            }
            // §7.4: a quarantined generation is never sent again.
```

In `crates/tagteam-engine/src/collect.rs`, replace `Collection::refresh_live` (its doc comment
at 395–403 and the function at 404–436) with:

```rust
    /// §7.5, then the live token it leaves, read fresh. `Refreshed`, `PersistedNotPublished`,
    /// `PublishedOnly` and `NotNeeded` go on; `send` still refuses a token that is expired or
    /// refused. `Dead` has quarantined the account (`relogin_required`). `Replaced` is
    /// `live-replaced` (Decision 10: `unavailable`), with a warning naming the forced switch
    /// that activates the replacement (§7.5 step 2). Any other outcome, and any error, is a
    /// failure with a warning, never a command error (M2a Task 16's carry-over), except a live
    /// credential the oracle gives to another identity, which has a status of its own. A kind
    /// that does not refresh never reaches §7.5, which would refuse it: its expired token is
    /// `token-expired`, and its refused one `http-401`, as on the inactive path (Decision 11: a
    /// refusal is an ordinary failure).
    fn refresh_live(&mut self, trigger: ActiveTrigger) -> Result<Vec<u8>, Stop> {
        if !self.p.kind_traits(&self.row.kind).refreshable {
            return Err(failed(match trigger {
                ActiveTrigger::Rejected { .. } => "http-401",
                ActiveTrigger::Expired => "token-expired",
            }));
        }
        match self.engine.refresh_active(&self.row.provider, trigger) {
            Ok(
                ActiveOutcome::NotNeeded { .. }
                | ActiveOutcome::Refreshed
                | ActiveOutcome::PersistedNotPublished
                | ActiveOutcome::PublishedOnly,
            ) => self.live_bytes(),
            Ok(ActiveOutcome::Dead(_)) => Err(failed("refresh-failed")),
            Ok(ActiveOutcome::Replaced) => {
                self.warn_replaced();
                Err(failed("live-replaced"))
            }
            Ok(ActiveOutcome::Unpersisted) => {
                self.warn_lost();
                Err(failed("refresh-failed"))
            }
            Ok(ActiveOutcome::Systemic(detail)) => {
                self.warn_refresh(&detail);
                Err(failed("refresh-failed"))
            }
            Ok(ActiveOutcome::Transient { kind }) => {
                self.warn_refresh(&kind);
                Err(failed("refresh-failed"))
            }
            Err(EngineError::ForeignLiveCredential { .. }) => Err(failed("foreign-credential")),
            Err(e) => {
                self.warn_refresh(&e);
                Err(failed("refresh-failed"))
            }
        }
    }
```

and add after `warn_lost`:

```rust
    /// §7.5 step 2: the live store holds a lineage an explicit replacement superseded. Names
    /// the account by label and position, never a token, and the command that activates the
    /// replacement.
    fn warn_replaced(&mut self) {
        self.warnings.push(format!(
            "{label} (position {n})'s login was replaced while Claude Code kept the old one; run `tagteam switch {n} --force` to activate the replacement",
            label = self.row.label,
            n = self.row.position
        ));
    }
```

The failure ends the fetch before sending, so `record` gives back its slot through the path
every pre-send failure takes (§8.3). `views.rs::usage_status` needs no change: `live-replaced`
reaches its `Some(_) => UsageStatus::Unavailable` arm. `collect.rs:413` is the only `match` on
`ActiveOutcome` outside `active.rs`. The CLI matches none.

- [ ] **Step 4: Run them and see them pass**, then the crate's whole suite

Run: `cargo test -p tagteam-engine --test active --test collect_active`
Expected: PASS, including the three new tests.

Run: `cargo test -p tagteam-engine --lib views`
Expected: PASS.

Run: `cargo test -p tagteam-engine --features test-hooks`
Expected: PASS. No existing `active.rs` or `collect_active.rs` test stale-marks its live store:
each adds its accounts with `add`, which records the current epoch.

Run: `cargo test --workspace --features tagteam/test-support`
Expected: PASS.

- [ ] **Step 5: Format and lint**

`cargo fmt --all && cargo fmt --all --check`, and `cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings`, and `cargo clippy --workspace --all-targets -- -D warnings`

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-engine/src/active.rs crates/tagteam-engine/src/collect.rs \
  crates/tagteam-engine/src/views.rs crates/tagteam-engine/tests/active.rs \
  crates/tagteam-engine/tests/collect_active.rs
git commit -m "Stop the active-token refresh on a replaced live login and report it as live-replaced"
```

---

### Task 5: Keychain naming: one item per spelling

Appendix A.2 (*2.1.286*): "**One item, no fallback.** Every read, write and delete uses that one
service." Claude Code no longer falls back to the unsuffixed item for an explicitly set
`CLAUDE_CONFIG_DIR=~/.claude`, nor to `hash(readlink target)` for a symlinked config dir, so every
spelling of a directory names its own item. tagteam still does both: `naming::read_services`
returns up to three services per axis, and every `LiveStore` path (the read, `doomed`, the file
fallback's report and delete, the API-key strip, `snapshot`, `restore`) and the identity surface
iterate over them. Reading those items now makes tagteam wrong in both directions. It can take a
stale unsuffixed item for the live credential when the suffixed one is absent. And it rewrites,
strips and deletes items that CC no longer reads, which belong to another spelling. This task
removes the fallbacks: every caller uses the single `keychain_service(env, kind)`.

**Readings of the spec this task commits to:**
- **The former fallback items are inert.** tagteam never reads, writes, clears, snapshots or
  restores an item named for another spelling: not the unsuffixed items under an explicit
  `~/.claude`, and not a symlink target's items. `doctor` reports them (M5). Nothing here
  deletes them, because another spelling may still be in use.
- **The identity surface lists one item per axis.** A former fallback item therefore falls under
  §15.3's byte-for-byte rule, and the invariant test proves that no command touches it.
- **`DoomedEntry.on_fallback` goes (Decision 15).** With one item per axis, the item is destroyed
  by the write itself, never only by a fallback, so `on_fallback` was always `false` and the two
  engine filters on it were dead code. `before_fallback` stays: a Keychain refusal still reports
  the item to it before deleting it, and since §9.4 step 7 has already saved and held those bytes,
  nothing is saved twice.
- **`Snapshot` keeps its item vectors** (empty off macOS, one entry on it) rather than becoming
  `Option`s, so `restore`'s ordering loop and its abort accounting stay exactly as they are. The
  larger rewrite to `Option<ItemSnapshot>` was rejected: M3a's storage-write lock edits the same
  functions, and the smaller diff re-syncs more easily. The cost is a vector that never holds two.
- `read_managed_key` keeps its current behaviour of no retry. Only the OAuth read retries
  (Appendix A.3, "Active reads").

**M3a re-sync.** M3a Task 7 (the file-mode pin) and the storage-write lock task edit
`write_credential_entry`, `write_managed_key`, `clear_managed_key`, `snapshot` and `restore`.
Make this task's single-item reads and the `report_item`/`remove_item` renames inside M3a's
versions of those functions. This task adds and removes no lock call.

**Files:**
- Modify: `crates/tagteam-cc/src/naming.rs` (remove `read_services`, lines 47–78, and the
  `use std::path::{Path, PathBuf};` import, line 2; rewrite two unit tests, lines 230–264)
- Modify: `crates/tagteam-cc/src/lib.rs` (line 13: stop exporting `read_services`)
- Modify: `crates/tagteam-cc/src/live.rs` (import, line 16; `Extent` docs, lines 44–55;
  `Snapshot` doc, lines 57–58; delete `find_first`, lines 201–212; `read_credential`, 214–230;
  `read_managed_key`, 232–248; `doomed`, 250–289; `report_items` → `report_item`, 291–306;
  `remove_items` → `remove_item`, 308–324; their callers in `write_credential_entry`, 337–376,
  `write_managed_key`, 486 and 497, and `clear_managed_key`, 501–514;
  `clear_credential_account_keys`, 378–410; `snapshot`, 516–539)
- Modify: `crates/tagteam-cc/src/provider.rs` (import, line 21; `keychain_items`, lines 79–90)
- Modify: `crates/tagteam-provider/src/provider.rs` (`DoomedEntry`, lines 101–111; the
  `ShadowingItem` doc comment, line 304; the `DoomedEntry` literal in
  `tests::debug_output_never_contains_secret_bytes`, lines 799–802)
- Modify: `crates/tagteam-fake/src/provider.rs` (`FakeAgent::doomed`, lines 306–321)
- Modify: `crates/tagteam-engine/src/switch.rs` (`Engine::transact`, lines 1173–1179: the step-7
  comment and the `on_fallback` filter)
- Modify: `crates/tagteam-engine/src/active.rs` (`Engine::publish`, line 527: the
  `on_fallback` filter)
- Test: `crates/tagteam-cc/tests/provider.rs`, `crates/tagteam-cc/tests/live_store.rs`,
  `crates/tagteam-fake/tests/provider.rs`, `crates/tagteam-engine/tests/common/mod.rs`,
  `crates/tagteam-engine/tests/switch.rs`, `crates/tagteam-engine/tests/destroyed.rs`,
  `crates/tagteam-engine/tests/recover.rs`, `crates/tagteam-engine/tests/invariant.rs`

**Interfaces:**
- Consumes:
  - `tagteam_cc::naming::{keychain_service(env: &Env, kind: ItemKind) -> String, keychain_account(env: &Env) -> String, ItemKind}`
  - `FakeKeychain::{put, get, set_unreadable, set_fail_write, set_fail_delete}`
  - The engine fixture: `Fx::with(Platform, adjust)`, `Fx::{add, add_api_key, put_managed_key, managed_key, live_credential, live_email, displaced}`,
    `common::{crashed_switch, write_target_credential, assert_journal_cleared, journal, splice_config_key}`,
    and `invariant.rs`'s `run_every_command_on`
- Produces:
  - `tagteam_provider::DoomedEntry { pub bytes: Read<Vec<u8>> }` (Interface Contract, Task 5)
  - `tagteam_cc`'s root exports `ItemKind, keychain_account, keychain_service`; `read_services`
    no longer exists
  - `LiveStore`'s private `fn report_item(&self, env: &Env, kind: ItemKind, before_fallback: BeforeFallback<'_>) -> Result<(), ProviderError>`
    and `fn remove_item(&self, env: &Env, kind: ItemKind, fence: Fence<'_>) -> Result<(), ProviderError>`.
    Task 7's `LiveStore::delete_items` calls `remove_item`.
  - In `crates/tagteam-engine/tests/common/mod.rs`: `INERT_ITEM`, `INERT_MANAGED_ITEM`,
    `Fx::with_explicit_default_config_dir() -> Fx`,
    `Fx::put_inert_items(&self) -> (Vec<u8>, Vec<u8>)` and
    `Fx::inert_items(&self) -> (Option<Vec<u8>>, Option<Vec<u8>>)`, which replace the fallback
    helpers

**Spec:**
- Appendix A.2 "One item, no fallback": every read, write and delete uses the one service, and
  items left under the former fallback names are inert.
- §12.2 "One spelling": CC names a profile's item from exactly the exported string and tries no
  other spelling.
- Appendix A.3 "File fallback": the old item is deleted and verified `Absent` before the
  activation commits.
- §9.4 step 7: every live entry a change destroys is named first and saved unless held.
- §15.3: outside the identity surface, every byte stays identical.

**What happens to every test that depends on a fallback item** (preflight C §2, checked against
the code):

| Test | File | Verdict |
|---|---|---|
| `an_explicit_default_config_dir_reads_suffixed_then_plain` | `cc/src/naming.rs` | Rewritten as `an_explicit_default_config_dir_names_only_its_suffixed_items` |
| `a_symlinked_config_dir_is_also_read_by_its_target` | `cc/src/naming.rs` | Rewritten as `a_symlinked_config_dir_is_named_by_the_link_s_spelling_alone` |
| `plant_every_entry`, `secrets_by_place` (helpers) | `cc/tests/provider.rs` | Rewritten: one planted item per axis, plus the two inert items, which `secrets_by_place` also reports |
| `doomed_names_everything_each_change_destroys` | `cc/tests/provider.rs` | Rewritten: no entry is conditional any more, so every destroyed or reported secret must be named; the inert items are never named and never touched |
| `doomed_reports_an_entry_it_cannot_read` | `cc/tests/provider.rs` | Rewritten as `doomed_reports_an_unreadable_item_and_never_reads_an_inert_one` |
| `identity_surface_lists_every_macos_credential_and_managed_key_service` | `cc/tests/provider.rs` | Rewritten as `identity_surface_lists_the_one_item_per_axis_of_the_exported_spelling` |
| `an_unreadable_primary_item_is_never_skipped_for_a_fallback` | `cc/tests/live_store.rs` | Rewritten as `an_explicit_default_config_dir_reads_only_its_suffixed_item`; its unreadable-item half stays, on the one item |
| `every_fallback_item_is_cleared_snapshotted_and_restored` | `cc/tests/live_store.rs` | Rewritten as `an_explicit_default_config_dir_clears_snapshots_and_restores_only_its_suffixed_item` |
| `restore_continues_past_a_non_lock_failure_and_skips_an_already_matching_entry` | `cc/tests/live_store.rs` | Rewritten on the default Env, same name: the managed-key item's restore fails, the OAuth item already matches, and the credentials file is still restored last |
| `file_fallback_verifies_every_fallback_item_is_gone_including_the_plain_one` | `cc/tests/live_store.rs` | Rewritten as `a_file_fallback_reports_and_deletes_only_the_suffixed_item_of_an_explicit_default_config_dir`. The one item's verification stays in `file_fallback_requires_the_shadowing_item_to_be_gone`, unchanged |
| `FALLBACK_ITEM`, `FALLBACK_MANAGED_ITEM`, `Fx::{with_fallback_items, put_fallback_item, fallback_item, put_fallback_managed_item, fallback_managed_item}` | `engine/tests/common/mod.rs` | Replaced by `INERT_ITEM`, `INERT_MANAGED_ITEM`, `Fx::{with_explicit_default_config_dir, put_inert_items, inert_items}` |
| `an_api_key_switch_saves_a_fallback_keychain_item_before_stripping_it` | `engine/tests/switch.rs` | Rewritten as `no_switch_reads_or_touches_an_inert_former_fallback_item` |
| `a_fallback_item_the_vault_already_holds_is_not_displaced` | `engine/tests/switch.rs` | Deleted: no switch destroys a fallback item any more, so there is no displacement to hold back. The `.prev` capture it also checked is `a_stale_mirror_the_vault_already_holds_is_not_displaced` |
| `an_unreadable_fallback_keychain_item_aborts_before_anything_is_written` | `engine/tests/switch.rs` | Rewritten as `an_unreadable_inert_item_never_blocks_a_switch`. An unreadable suffixed item still aborts, in `unsafe_live_reads_abort_without_changing_anything`, unchanged |
| `a_fallback_item` | `engine/tests/destroyed.rs` | Deleted: that kind of entry no longer exists. Its OAuth half is `the_primary_item` and `switch.rs`'s `a_stray_oauth_login_is_displaced_before_an_api_key_switch_strips_it`; its managed-key half is `switch.rs`'s `a_stray_managed_key_is_displaced_before_an_oauth_switch_clears_it` |
| `a_fallback_item_the_oauth_file_fallback_deletes` | `engine/tests/destroyed.rs` | Deleted: a file fallback now deletes only the item step 7 has already named and saved (`the_primary_item`) |
| `a_fallback_item_the_api_key_fallback_deletes` | `engine/tests/destroyed.rs` | Deleted, for the same reason on the managed-key axis |
| `recovery_saves_what_clearing_the_managed_key_axis_destroys` | `engine/tests/destroyed.rs` | Rewritten on `Fx::new()`, same name: the planted secret is the managed-key item itself |
| `forward_recovery_to_an_api_key_saves_a_fallback_keychain_item_before_stripping_it` | `engine/tests/recover.rs` | Rewritten as `forward_recovery_to_an_api_key_leaves_an_inert_former_fallback_item_alone`. Preflight C §2 names `forward_recovery_never_displaces_a_stale_mirror_the_vault_holds` (lines 310–326) here; that test uses no fallback item and is unchanged |
| `fallback_keychain_items_stay_within_the_surface` | `engine/tests/invariant.rs` | Rewritten as `inert_former_fallback_items_stay_byte_identical` |

Three more existing tests change mechanically in Step 3:
- Two name the removed field. `tagteam-provider/src/provider.rs`'s
  `tests::debug_output_never_contains_secret_bytes` loses `on_fallback: true` from its
  `DoomedEntry` literal. `tagteam-fake/tests/provider.rs`'s
  `a_write_keeps_the_machines_device_key_and_undoes_exactly` loses its
  `assert!(!doomed[0].on_fallback);` line.
- `cc/tests/live_store.rs`'s `a_counting_fence_stops_the_deletes_in_remove_items` (line 1009) is
  renamed `a_counting_fence_stops_the_delete_in_remove_item`, after the function it pins. Its
  body is unchanged.

- [ ] **Step 1: Write the failing tests**

**`crates/tagteam-cc/src/naming.rs`**, in `mod tests`: replace
`an_explicit_default_config_dir_reads_suffixed_then_plain` and
`a_symlinked_config_dir_is_also_read_by_its_target` (lines 230–264) with:

```rust
    #[test]
    fn an_explicit_default_config_dir_names_only_its_suffixed_items() {
        // Appendix A.2 (2.1.286): no fallback to the unsuffixed items for `~/.claude`.
        let mut e = env();
        e.claude_config_dir = Some("/home/tester/.claude".into());
        assert_eq!(
            keychain_service(&e, ItemKind::OAuth),
            "Claude Code-credentials-b2e2cf9d"
        );
        assert_eq!(
            keychain_service(&e, ItemKind::ManagedKey),
            "Claude Code-b2e2cf9d"
        );
    }

    #[test]
    fn a_symlinked_config_dir_is_named_by_the_link_s_spelling_alone() {
        let d = tempfile::tempdir().unwrap();
        let target = d.path().join("real");
        std::fs::create_dir(&target).unwrap();
        let link = d.path().join("link");
        std::os::unix::fs::symlink("real", &link).unwrap();
        let mut e = env();
        e.claude_config_dir = Some(link.clone().into_os_string());
        let hash = |p: &Path| {
            hex::encode(Sha256::digest(p.to_str().unwrap().as_bytes()).as_slice())[..8].to_owned()
        };
        assert_eq!(
            keychain_service(&e, ItemKind::OAuth),
            format!("Claude Code-credentials-{}", hash(&link))
        );
        assert_ne!(hash(&link), hash(&target));
    }
```

Both pin naming that does not change (`keychain_service` already returns these names), so they
pass before and after. The behaviour change is pinned by the store tests below.

**`crates/tagteam-cc/tests/provider.rs`**: change the import on line 10 to

```rust
use tagteam_cc::{CcPaths, ItemKind, keychain_account, keychain_service};
```

Replace everything from the `plant_every_entry` doc comment (line 153) through the end of
`doomed_reports_an_entry_it_cannot_read` (line 322) with:

```rust
/// The unsuffixed items an explicit `CLAUDE_CONFIG_DIR=~/.claude` fell back to before Claude
/// Code 2.1.286. Inert now (Appendix A.2): nothing reads, writes or clears them.
const INERT_OAUTH: &str = "Claude Code-credentials";
const INERT_MANAGED: &str = "Claude Code";

/// `"<prefix>-" + hex(sha256(dir))[..8]`: Appendix A.2's name, computed here independently of
/// `keychain_service`.
fn hashed(prefix: &str, dir: &str) -> String {
    use sha2::{Digest, Sha256};
    format!(
        "{prefix}-{}",
        &hex::encode(Sha256::digest(dir.as_bytes()).as_slice())[..8]
    )
}

/// Every entry a change can destroy, planted with a distinct secret, under an explicit
/// `CLAUDE_CONFIG_DIR=~/.claude`, beside the two inert former fallback items (Appendix A.2).
fn plant_every_entry(f: &Fx) -> Env {
    let mut env = f.env.clone();
    env.claude_config_dir = Some(env.home.join(".claude").into_os_string());
    let paths = CcPaths::resolve(&env);
    let acct = keychain_account(&env);
    let entry = json!({"claudeAiOauth": {"refreshToken": "rt-item"}, "mcpOAuth": {"m": 1}});
    f.kc.put(
        &keychain_service(&env, ItemKind::OAuth),
        &acct,
        entry.to_string().as_bytes(),
    );
    let inert = json!({"claudeAiOauth": {"refreshToken": "rt-inert"}, "mcpOAuth": {"m": 2}});
    f.kc.put(INERT_OAUTH, &acct, inert.to_string().as_bytes());
    let file = json!({"claudeAiOauth": {"refreshToken": "rt-file"}});
    fs::write(&paths.credentials_file, file.to_string()).unwrap();
    f.kc.put(
        &keychain_service(&env, ItemKind::ManagedKey),
        &acct,
        b"sk-ant-api03-item",
    );
    f.kc.put(INERT_MANAGED, &acct, b"sk-ant-api03-inert");
    let config = json!({"primaryApiKey": "sk-ant-api03-plain"});
    fs::write(&paths.global_config, config.to_string()).unwrap();
    env
}

/// Every secret the planted entries hold now, by where it is, the inert items included.
fn secrets_by_place(f: &Fx, env: &Env) -> Vec<(String, Vec<u8>)> {
    let paths = CcPaths::resolve(env);
    let acct = keychain_account(env);
    let mut out: Vec<(String, Vec<u8>)> = [
        keychain_service(env, ItemKind::OAuth),
        keychain_service(env, ItemKind::ManagedKey),
        INERT_OAUTH.to_owned(),
        INERT_MANAGED.to_owned(),
    ]
    .into_iter()
    .filter_map(|svc| f.kc.get(&svc, &acct).map(|b| (svc, b)))
    .collect();
    if let Ok(b) = fs::read(&paths.credentials_file) {
        out.push(("credentials file".into(), b));
    }
    if let Read::Present(Some(Value::String(k))) =
        tagteam_cc::config::get_key(&paths.global_config, "primaryApiKey")
    {
        out.push(("primaryApiKey".into(), k.into_bytes()));
    }
    out
}

#[test]
fn doomed_names_everything_each_change_destroys() {
    use tagteam_provider::LiveChange;
    let api_key = "sk-ant-api03-target-key-abcdefghijklmn";
    // (change, how the Keychain treats the write: 0 takes it, 1 refuses it, 2 was pinned to
    // the file by an earlier refusal)
    let cases: [(LiveChange, u8); 8] = [
        (LiveChange::Write("oauth"), 0),
        (LiveChange::Write("oauth"), 1),
        (LiveChange::Write("oauth"), 2),
        (LiveChange::Write("api_key"), 0),
        (LiveChange::Write("api_key"), 1),
        (LiveChange::ClearOther("oauth"), 0),
        (LiveChange::ClearOther("api_key"), 0),
        (LiveChange::ClearOther("setup_token"), 0),
    ];
    for (change, keychain) in cases {
        let f = fx();
        let env = plant_every_entry(&f);
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
        let acct = keychain_account(env);
        let inert_before = (f.kc.get(INERT_OAUTH, &acct), f.kc.get(INERT_MANAGED, &acct));
        let before = secrets_by_place(f, env);
        let doomed = f.cc.doomed(env, &locks, change);
        let planned: Vec<Vec<u8>> = doomed
            .iter()
            .filter_map(|d| d.bytes.clone().present())
            .collect();
        let (refused, pinned) = (keychain == 1, keychain == 2);
        let mut reported: Vec<Vec<u8>> = Vec::new();
        match change {
            LiveChange::Write(kind) => {
                let item = if kind == "api_key" {
                    ItemKind::ManagedKey
                } else {
                    ItemKind::OAuth
                };
                f.kc.set_fail_write(&keychain_service(env, item), refused);
                let login = if kind == "api_key" {
                    StoredLogin {
                        kind: kind.into(),
                        secret: api_key.as_bytes().to_vec(),
                        identity: f.cc.token_identity("api-key-9@token.local"),
                    }
                } else {
                    target(f, "t@x.co", "rt-target")
                };
                let mut record = |b: &[u8]| {
                    reported.push(b.to_vec());
                    Ok(())
                };
                f.cc.write_credential(env, &locks, &login, &mut record)
                    .unwrap();
            }
            LiveChange::ClearOther(kind) => {
                f.cc.clear_other_axis(env, &locks, kind).unwrap();
            }
        }
        let after = secrets_by_place(f, env);
        let case = format!("{change:?}, keychain={keychain}");
        for (place, bytes) in &before {
            if !after.contains(&(place.clone(), bytes.clone())) {
                assert!(
                    planned.contains(bytes),
                    "{case}: {place} was destroyed without being named"
                );
            }
        }
        for r in &reported {
            assert!(
                planned.contains(r),
                "{case}: a fallback reported an entry the plan did not name"
            );
        }
        assert_eq!(
            (f.kc.get(INERT_OAUTH, &acct), f.kc.get(INERT_MANAGED, &acct)),
            inert_before,
            "{case}: an inert former fallback item was touched"
        );
        for inert in [&inert_before.0, &inert_before.1].into_iter().flatten() {
            assert!(!planned.contains(inert), "{case}: an inert item was named");
        }
        if !refused && !pinned {
            assert!(reported.is_empty(), "{case}: nothing falls back");
        }
    }
}

#[test]
fn doomed_reports_an_unreadable_item_and_never_reads_an_inert_one() {
    let f = fx();
    let env = plant_every_entry(&f);
    let acct = keychain_account(&env);
    f.kc.set_unreadable(INERT_OAUTH, &acct, true);
    f.kc.set_unreadable(INERT_MANAGED, &acct, true);
    let g = MutationGuard::acquire(&env, Duration::from_secs(1)).unwrap();
    let locks = f.cc.lock_live(&env, &g).unwrap();
    let change = tagteam_provider::LiveChange::Write("oauth");
    let doomed = f.cc.doomed(&env, &locks, change);
    assert!(
        doomed
            .iter()
            .all(|d| !matches!(d.bytes, Read::Unreadable(_))),
        "an inert item is never read: {doomed:?}"
    );
    f.kc.set_unreadable(&keychain_service(&env, ItemKind::OAuth), &acct, true);
    let doomed = f.cc.doomed(&env, &locks, change);
    assert!(
        doomed
            .iter()
            .any(|d| matches!(d.bytes, Read::Unreadable(_))),
        "{doomed:?}"
    );
}
```

Replace `identity_surface_lists_every_macos_credential_and_managed_key_service` (lines 570–596)
with:

```rust
#[test]
fn identity_surface_lists_the_one_item_per_axis_of_the_exported_spelling() {
    // Appendix A.2 (2.1.286): a symlinked config dir is named by the link's spelling alone,
    // never also by its target's.
    let f = fx();
    let target_dir = f.env.home.join("real-profile");
    fs::create_dir_all(&target_dir).unwrap();
    let link = f.env.home.join("link-profile");
    std::os::unix::fs::symlink(&target_dir, &link).unwrap();
    let mut env = f.env.clone();
    env.claude_config_dir = Some(link.clone().into_os_string());

    let s = f.cc.identity_surface(&env);
    let acct = keychain_account(&env);
    let spelling = link.to_str().unwrap();
    assert_eq!(
        s.credential_items,
        vec![(hashed("Claude Code-credentials", spelling), acct.clone())]
    );
    assert_eq!(s.owned_items, vec![(hashed("Claude Code", spelling), acct)]);
}
```

(`sha2` and `hex` are `tagteam-cc` dependencies, so the test crate can use them.)

**`crates/tagteam-cc/tests/live_store.rs`**: change the import on line 8 to

```rust
use tagteam_cc::{CcPaths, ItemKind, config, keychain_account, keychain_service};
```

After `save_nothing` (line 17), add:

```rust
/// The unsuffixed items an explicit `CLAUDE_CONFIG_DIR=~/.claude` fell back to before Claude
/// Code 2.1.286. Inert now (Appendix A.2): nothing reads, writes or clears them.
const INERT_OAUTH: &str = "Claude Code-credentials";
const INERT_MANAGED: &str = "Claude Code";

/// A fixture with an explicit `CLAUDE_CONFIG_DIR=~/.claude`.
fn explicit_default() -> Fx {
    fx_with(|e| e.claude_config_dir = Some(e.home.join(".claude").into_os_string()))
}
```

Replace `an_unreadable_primary_item_is_never_skipped_for_a_fallback` (lines 252–279) with these
two tests:

```rust
#[test]
fn an_explicit_default_config_dir_reads_only_its_suffixed_item() {
    // Appendix A.2 (2.1.286): `CLAUDE_CONFIG_DIR=~/.claude` names the suffixed items alone. The
    // unsuffixed ones are never read, whatever they hold and whether or not they can be read.
    let f = explicit_default();
    let s = store(&f, Platform::MacOs);
    let acct = keychain_account(&f.env);
    let (oauth, managed) = (
        keychain_service(&f.env, ItemKind::OAuth),
        keychain_service(&f.env, ItemKind::ManagedKey),
    );
    assert_ne!(
        (oauth.as_str(), managed.as_str()),
        (INERT_OAUTH, INERT_MANAGED)
    );
    f.kc.put(INERT_OAUTH, &acct, b"inert");
    f.kc.put(INERT_MANAGED, &acct, b"sk-ant-api03-inert");
    for unreadable in [false, true] {
        f.kc.set_unreadable(INERT_OAUTH, &acct, unreadable);
        f.kc.set_unreadable(INERT_MANAGED, &acct, unreadable);
        assert!(
            matches!(s.read_credential(&f.env, &f.paths), Read::Absent),
            "unreadable={unreadable}"
        );
        assert!(
            matches!(s.read_managed_key(&f.env, &f.paths), Read::Absent),
            "unreadable={unreadable}"
        );
    }
    f.kc.put(&oauth, &acct, b"suffixed");
    f.kc.put(&managed, &acct, b"sk-ant-api03-suffixed");
    assert_eq!(
        s.read_credential(&f.env, &f.paths)
            .present()
            .unwrap()
            .bytes(),
        b"suffixed"
    );
    assert_eq!(
        s.read_managed_key(&f.env, &f.paths).present().unwrap(),
        b"sk-ant-api03-suffixed"
    );
    // The suffixed item is the only authority: unreadable is unreadable, and the file covers
    // it only as a degraded read.
    f.kc.set_unreadable(&oauth, &acct, true);
    f.kc.set_unreadable(&managed, &acct, true);
    assert!(matches!(
        s.read_credential(&f.env, &f.paths),
        Read::Unreadable(_)
    ));
    assert!(matches!(
        s.read_managed_key(&f.env, &f.paths),
        Read::Unreadable(_)
    ));
    fs::write(&f.paths.credentials_file, "file").unwrap();
    assert_eq!(
        s.read_credential(&f.env, &f.paths)
            .present()
            .unwrap()
            .provenance(),
        Provenance::Degraded
    );
}

#[test]
fn a_symlinked_config_dir_is_read_and_cleared_only_under_the_link_s_spelling() {
    // Appendix A.2 (2.1.286): no `hash(readlink target)` fallback.
    let f = fx();
    let real = f.env.home.join("real-profile");
    fs::create_dir_all(&real).unwrap();
    let link = f.env.home.join("link-profile");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let mut by_link = f.env.clone();
    by_link.claude_config_dir = Some(link.into_os_string());
    let mut by_target = f.env.clone();
    by_target.claude_config_dir = Some(real.into_os_string());
    let s = store(&f, Platform::MacOs);
    let acct = keychain_account(&f.env);
    let link_item = keychain_service(&by_link, ItemKind::OAuth);
    let target_item = keychain_service(&by_target, ItemKind::OAuth);
    assert_ne!(link_item, target_item);
    let paths = CcPaths::resolve(&by_link);
    let target_entry = br#"{"claudeAiOauth":{"refreshToken":"target"}}"#;
    f.kc.put(&target_item, &acct, target_entry);
    assert!(
        matches!(s.read_credential(&by_link, &paths), Read::Absent),
        "the target's item is never read through the link"
    );
    f.kc.put(
        &link_item,
        &acct,
        br#"{"claudeAiOauth":{"refreshToken":"link"}}"#,
    );
    assert_eq!(
        s.read_credential(&by_link, &paths)
            .present()
            .unwrap()
            .bytes(),
        br#"{"claudeAiOauth":{"refreshToken":"link"}}"#
    );
    s.clear_credential_account_keys(&by_link, &paths, &open)
        .unwrap();
    assert!(
        f.kc.get(&link_item, &acct).is_none(),
        "cleared under the link's item"
    );
    assert_eq!(
        f.kc.get(&target_item, &acct).unwrap(),
        target_entry,
        "the target's item is left alone"
    );
}
```

Replace `every_fallback_item_is_cleared_snapshotted_and_restored` (lines 434–456) with:

```rust
#[test]
fn an_explicit_default_config_dir_clears_snapshots_and_restores_only_its_suffixed_item() {
    let f = explicit_default();
    let s = store(&f, Platform::MacOs);
    let acct = keychain_account(&f.env);
    let oauth = keychain_service(&f.env, ItemKind::OAuth);
    let entry = br#"{"claudeAiOauth":{"refreshToken":"suffixed"},"mcpOAuth":{"m":1}}"#;
    let inert = br#"{"claudeAiOauth":{"refreshToken":"plain"}}"#;
    f.kc.put(&oauth, &acct, entry);
    f.kc.put(INERT_OAUTH, &acct, inert);
    f.kc.set_unreadable(INERT_OAUTH, &acct, true);
    let snap = s
        .snapshot(&f.env, &f.paths)
        .expect("an unreadable inert item never blocks a snapshot");
    f.kc.set_unreadable(INERT_OAUTH, &acct, false);
    s.clear_credential_account_keys(&f.env, &f.paths, &open)
        .unwrap();
    assert_eq!(
        json_of(&f.kc.get(&oauth, &acct).unwrap()),
        json!({"mcpOAuth": {"m": 1}})
    );
    assert_eq!(
        f.kc.get(INERT_OAUTH, &acct).unwrap(),
        inert,
        "the inert item is never cleared"
    );
    f.kc.put(INERT_OAUTH, &acct, b"changed meanwhile");
    s.restore(&f.env, &f.paths, &snap, &open).unwrap();
    assert_eq!(f.kc.get(&oauth, &acct).unwrap(), entry);
    assert_eq!(
        f.kc.get(INERT_OAUTH, &acct).unwrap(),
        b"changed meanwhile",
        "nor snapshotted and restored"
    );
}
```

Replace `restore_continues_past_a_non_lock_failure_and_skips_an_already_matching_entry` (lines
891–952) with:

```rust
#[test]
fn restore_continues_past_a_non_lock_failure_and_skips_an_already_matching_entry() {
    let f = fx();
    let recording = Arc::new(RecordingKeychain::new(f.kc.clone()));
    let s = LiveStore::new(recording.clone(), Platform::MacOs).with_retry_delay(Duration::ZERO);
    let acct = keychain_account(&f.env);
    let oauth = keychain_service(&f.env, ItemKind::OAuth);
    let managed = keychain_service(&f.env, ItemKind::ManagedKey);

    // `restore` puts back the managed-key item first, then the OAuth item, then the
    // credentials file. The managed-key item's upsert fails; the file, restored last, proves
    // the restore does not stop there. The OAuth item already matches its snapshot and must
    // receive no write at all, proving the skip.
    f.kc.put(&managed, &acct, b"orig-managed");
    f.kc.put(&oauth, &acct, b"unchanged-oauth");
    fs::write(&f.paths.credentials_file, "orig-file").unwrap();
    let snap = s.snapshot(&f.env, &f.paths).unwrap();

    f.kc.put(&managed, &acct, b"target-managed");
    fs::write(&f.paths.credentials_file, "target-file").unwrap();
    f.kc.set_fail_write(&managed, true);

    match s.restore(&f.env, &f.paths, &snap, &open) {
        Err(ProviderError::Incomplete { failed }) => assert_eq!(failed, vec![managed.clone()]),
        other => panic!("expected Incomplete naming {managed}, got {other:?}"),
    }
    assert_eq!(
        f.kc.get(&managed, &acct).unwrap(),
        b"target-managed",
        "the failed restore must leave the target value in place, not corrupt it"
    );
    assert_eq!(
        fs::read(&f.paths.credentials_file).unwrap(),
        b"orig-file",
        "the file must still be restored after the item failed"
    );
    assert_eq!(f.kc.get(&oauth, &acct).unwrap(), b"unchanged-oauth");

    let calls = recording.calls();
    assert!(
        calls
            .iter()
            .any(|(svc, op)| svc == &managed && *op == "upsert")
    );
    assert!(
        !calls.iter().any(|(svc, _)| svc == &oauth),
        "an entry that already matched its snapshot must receive no write at all: {calls:?}"
    );
}
```

Replace `file_fallback_verifies_every_fallback_item_is_gone_including_the_plain_one` (lines
1131–1153) with:

```rust
#[test]
fn a_file_fallback_reports_and_deletes_only_the_suffixed_item_of_an_explicit_default_config_dir() {
    let f = explicit_default();
    let s = store(&f, Platform::MacOs);
    let acct = keychain_account(&f.env);
    let oauth = keychain_service(&f.env, ItemKind::OAuth);
    f.kc.put(&oauth, &acct, b"old");
    f.kc.put(INERT_OAUTH, &acct, b"inert");
    f.kc.set_fail_write(&oauth, true);
    let mut reported: Vec<Vec<u8>> = Vec::new();
    let mut record = |b: &[u8]| {
        reported.push(b.to_vec());
        Ok(())
    };
    assert_eq!(
        s.write_credential_entry(&f.env, &f.paths, b"new", &open, &mut record)
            .unwrap(),
        SecretStore::Fallback(f.paths.credentials_file.clone())
    );
    assert_eq!(reported, vec![b"old".to_vec()]);
    assert!(f.kc.get(&oauth, &acct).is_none());
    assert_eq!(f.kc.get(INERT_OAUTH, &acct).unwrap(), b"inert");
}
```

**`crates/tagteam-engine/tests/common/mod.rs`**: replace the two `FALLBACK_*` constants (lines
104–107) with:

```rust
/// The unsuffixed OAuth item an explicit `CLAUDE_CONFIG_DIR=~/.claude` fell back to before
/// Claude Code 2.1.286. Inert since (Appendix A.2): nothing may read, write or clear it.
pub const INERT_ITEM: &str = "Claude Code-credentials";
/// The unsuffixed managed-key item, inert in the same way.
pub const INERT_MANAGED_ITEM: &str = "Claude Code";
```

and replace `Fx::with_fallback_items` through `Fx::fallback_managed_item` (lines 233–259) with:

```rust
    /// A macOS fixture with an explicit `CLAUDE_CONFIG_DIR=~/.claude`. Claude Code names only
    /// the suffixed items for it (Appendix A.2), so `INERT_ITEM` and `INERT_MANAGED_ITEM` are
    /// another spelling's items, which no command may touch.
    pub fn with_explicit_default_config_dir() -> Self {
        Self::with(Platform::MacOs, |e| {
            e.claude_config_dir = Some(e.home.join(".claude").into_os_string())
        })
    }

    /// Plants both inert items with secrets no vault holds; returns what each holds.
    pub fn put_inert_items(&self) -> (Vec<u8>, Vec<u8>) {
        let acct = keychain_account(&self.env);
        let oauth = Self::credential_json("old@x.co", "rt-inert")
            .to_string()
            .into_bytes();
        let managed = STRAY_API_KEY.as_bytes().to_vec();
        self.kc.put(INERT_ITEM, &acct, &oauth);
        self.kc.put(INERT_MANAGED_ITEM, &acct, &managed);
        (oauth, managed)
    }

    /// What the two inert items hold now.
    pub fn inert_items(&self) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
        let acct = keychain_account(&self.env);
        (
            self.kc.get(INERT_ITEM, &acct),
            self.kc.get(INERT_MANAGED_ITEM, &acct),
        )
    }
```

**`crates/tagteam-engine/tests/switch.rs`**: replace
`an_api_key_switch_saves_a_fallback_keychain_item_before_stripping_it` (lines 550–572) with:

```rust
#[test]
fn no_switch_reads_or_touches_an_inert_former_fallback_item() {
    // Appendix A.2 (2.1.286): under an explicit CLAUDE_CONFIG_DIR=~/.claude, CC names only the
    // suffixed items. The unsuffixed ones belong to another spelling: an OAuth switch, an
    // API-key switch and the switch back neither save, strip nor delete them.
    let fx = Fx::with_explicit_default_config_dir();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let k = fx.add_api_key(API_KEY);
    let (oauth, managed) = fx.put_inert_items();
    for (step, target) in [("to a", &a), ("to k", &k), ("back to a", &a)] {
        switch(&fx, to(target), false).unwrap();
        assert_eq!(displaced_files(&fx), 0, "{step}");
        assert_eq!(
            fx.inert_items(),
            (Some(oauth.clone()), Some(managed.clone())),
            "{step}"
        );
    }
}
```

Delete `a_fallback_item_the_vault_already_holds_is_not_displaced` (lines 595–607), and replace
`an_unreadable_fallback_keychain_item_aborts_before_anything_is_written` (lines 609–622) with:

```rust
#[test]
fn an_unreadable_inert_item_never_blocks_a_switch() {
    // It was a fallback that §9.4 step 3 had to read before step 7 could clear it; now no
    // switch reads it at all (Appendix A.2).
    let fx = Fx::with_explicit_default_config_dir();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let k = fx.add_api_key(API_KEY);
    fx.put_inert_items();
    let acct = keychain_account(&fx.env);
    fx.kc.set_unreadable(common::INERT_ITEM, &acct, true);
    fx.kc
        .set_unreadable(common::INERT_MANAGED_ITEM, &acct, true);
    switch(&fx, to(&k), false).unwrap();
    switch(&fx, to(&a), false).unwrap();
    assert!(common::journal(&fx).is_none());
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
}
```

**`crates/tagteam-engine/tests/destroyed.rs`**: delete `a_fallback_item` (lines 60–93),
`a_fallback_item_the_oauth_file_fallback_deletes` (lines 138–164) and
`a_fallback_item_the_api_key_fallback_deletes` (lines 166–194). Remove the two imports they
alone used, `use tagteam_cc::ItemKind;` and `use tagteam_provider::SecretStore;` (lines 12–13).
Replace `recovery_saves_what_clearing_the_managed_key_axis_destroys` (lines 196–229) with:

```rust
#[test]
fn recovery_saves_what_clearing_the_managed_key_axis_destroys() {
    // §9.6 finishing forward to OAuth clears the managed-key axis: the managed-key item and a
    // hidden `primaryApiKey` go, and are saved first unless a vault holds them.
    for held in [false, true] {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let k = fx.add_api_key(API_KEY);
        fx.switch_to(&k, false).unwrap();
        crashed_switch(&fx, &k, &a);
        write_target_credential(&fx, &a);
        let planted = if held { API_KEY } else { STRAY_API_KEY };
        fx.put_managed_key(planted.as_bytes());
        splice_config_key(
            &fx.paths().global_config,
            "primaryApiKey",
            &json!(if held { API_KEY } else { OTHER_API_KEY }),
        );
        fx.engine.set_disabled(&a, false).unwrap(); // a mutation: it recovers the row
        assert_journal_cleared(&fx);
        let want = if held {
            vec![]
        } else {
            vec![
                STRAY_API_KEY.as_bytes().to_vec(),
                OTHER_API_KEY.as_bytes().to_vec(),
            ]
        };
        let mut got = fx.displaced();
        got.sort();
        assert_eq!(got, want, "held={held}");
        assert_eq!(fx.managed_key(), None);
    }
}
```

**`crates/tagteam-engine/tests/recover.rs`**: replace
`forward_recovery_to_an_api_key_saves_a_fallback_keychain_item_before_stripping_it` (lines
327–345) with:

```rust
#[test]
fn forward_recovery_to_an_api_key_leaves_an_inert_former_fallback_item_alone() {
    // Appendix A.2 (2.1.286): stripping the credential entry strips the suffixed item only.
    let fx = Fx::with_explicit_default_config_dir();
    let a = fx.add("a@x.co", "rt-a");
    let k = fx.add_api_key(API_KEY);
    crashed_switch(&fx, &a, &k);
    fx.put_managed_key(API_KEY.as_bytes());
    let (oauth, managed) = fx.put_inert_items();
    any_mutation(&fx, &a);
    assert_journal_cleared(&fx);
    assert!(fx.displaced().is_empty());
    assert_eq!(fx.inert_items(), (Some(oauth), Some(managed)));
    assert_eq!(
        fx.live_credential(),
        Some(serde_json::json!({"mcpOAuth": {"srv": {"token": "machine-shared"}}})),
        "the suffixed item was stripped"
    );
}
```

**`crates/tagteam-engine/tests/invariant.rs`**: replace
`fallback_keychain_items_stay_within_the_surface` (lines 359–372) with:

```rust
#[test]
fn inert_former_fallback_items_stay_byte_identical() {
    // An explicit CLAUDE_CONFIG_DIR=~/.claude names only the suffixed items (Appendix A.2), so
    // the surface lists those alone, and the unsuffixed items fall under the byte-for-byte
    // rule: any command that read-modified-wrote one would be flagged.
    let fx = Fx::with_explicit_default_config_dir();
    fx.put_inert_items();
    run_every_command_on(&fx);
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-cc`
Expected: FAIL, seven tests, each because the old code reads, reports or clears an item under
another spelling:
- `live_store.rs`:
  - `an_explicit_default_config_dir_reads_only_its_suffixed_item` fails on
    `unreadable=false`: the read falls back to the unsuffixed item.
  - `a_symlinked_config_dir_is_read_and_cleared_only_under_the_link_s_spelling` fails with
    "the target's item is never read through the link".
  - `an_explicit_default_config_dir_clears_snapshots_and_restores_only_its_suffixed_item`
    panics at "an unreadable inert item never blocks a snapshot": the snapshot reads the
    unreadable unsuffixed item and refuses.
  - `a_file_fallback_reports_and_deletes_only_the_suffixed_item_of_an_explicit_default_config_dir`
    fails because `reported` also holds the inert item's bytes.
- `provider.rs`:
  - `doomed_names_everything_each_change_destroys` fails with
    `Write("oauth"), keychain=0: an inert former fallback item was touched`: clearing the
    managed-key axis deletes the unsuffixed managed item.
  - `doomed_reports_an_unreadable_item_and_never_reads_an_inert_one` fails with "an inert item
    is never read".
  - `identity_surface_lists_the_one_item_per_axis_of_the_exported_spelling` fails because the
    surface lists two items (the link's and the target's).

The two `naming.rs` tests and the rewritten `restore_continues_…` already pass. They pin
behaviour that does not change, and must keep passing.

Run: `cargo test -p tagteam-engine --features test-hooks --test switch --test destroyed --test recover --test invariant`
Expected: FAIL, four tests:
- `switch.rs`:
  - `no_switch_reads_or_touches_an_inert_former_fallback_item` fails at `"to a"` with 1
    displaced file: activating OAuth clears the unsuffixed managed item after saving it.
  - `an_unreadable_inert_item_never_blocks_a_switch` panics on `Unreadable(… "rc 36 …")`.
- `recover.rs`: `forward_recovery_to_an_api_key_leaves_an_inert_former_fallback_item_alone`
  fails on `fx.displaced().is_empty()`.
- `invariant.rs`: `inert_former_fallback_items_stay_byte_identical` panics at `add a` with
  `LiveApiKey`: the managed-key read falls back to the unsuffixed item and sees an API key.

`destroyed.rs` passes: its rewritten test plants the item the old code already reads.

- [ ] **Step 3: Read, write and clear the one item**

**`crates/tagteam-cc/src/naming.rs`**: delete `read_services` (lines 47–78, with its doc
comment) and the `use std::path::{Path, PathBuf};` line (line 2), which only it used. The test
module keeps its own `use std::path::{Path, PathBuf};`.

**`crates/tagteam-cc/src/lib.rs`**, line 13:

```rust
pub use naming::{ItemKind, keychain_account, keychain_service};
```

**`crates/tagteam-cc/src/live.rs`**: change the import on line 16 to

```rust
use crate::naming::{ItemKind, keychain_account, keychain_service};
```

Replace the `Extent` enum and the `Snapshot` doc comment with:

```rust
/// How much of one auth axis a change destroys (§9.4 step 7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Extent {
    /// Left alone.
    None,
    /// Written: the axis's one Keychain item (Appendix A.2), and the file or `primaryApiKey`
    /// behind it.
    Written,
    /// Cleared: the same entries, deleted or stripped to their machine-shared keys.
    Cleared,
}

/// The exact prior state of every entry a switch may write: each axis's one Keychain item
/// (Appendix A.2; none off macOS) and the two files. It holds secrets, so it has no `Debug`.
#[derive(Clone)]
pub struct Snapshot {
    oauth_items: Vec<ItemSnapshot>,
    managed_items: Vec<ItemSnapshot>,
    credentials_file: Option<Vec<u8>>,
    global_config: Option<Vec<u8>>,
}
```

Delete `find_first` (lines 201–212). Replace `read_credential` and `read_managed_key` with:

```rust
    /// Keychain first, retried twice 300 ms apart; the file covers an absent item. A failed
    /// Keychain read covered by the file is `Degraded` (§4.3). The one item `env` names is the
    /// only one read (Appendix A.2): no other spelling's item is ever consulted.
    pub fn read_credential(&self, env: &Env, paths: &CcPaths) -> Read<Credential> {
        if !self.mac() {
            return read_bytes(&paths.credentials_file).map(Credential::fresh);
        }
        let svc = keychain_service(env, ItemKind::OAuth);
        let acct = keychain_account(env);
        match self.retrying(|| self.keychain.find(&svc, &acct)) {
            Read::Present(b) => Read::Present(Credential::fresh(b)),
            Read::Absent => read_bytes(&paths.credentials_file).map(Credential::fresh),
            Read::Unreadable(e) => match read_bytes(&paths.credentials_file) {
                Read::Present(b) => Read::Present(Credential::degraded(b)),
                _ => Read::Unreadable(e),
            },
        }
    }

    /// The Keychain item first; then `primaryApiKey` in the config. An empty item stays
    /// `Present("")`, since a Keychain timeout can look like that. An empty `primaryApiKey` is
    /// what a successful file read really found, and it names no key, so it reads as absent;
    /// both write paths remove it anyway.
    pub fn read_managed_key(&self, env: &Env, paths: &CcPaths) -> Read<Vec<u8>> {
        if self.mac() {
            match self.keychain.find(
                &keychain_service(env, ItemKind::ManagedKey),
                &keychain_account(env),
            ) {
                Read::Present(v) => return Read::Present(v),
                Read::Unreadable(e) => return Read::Unreadable(e),
                Read::Absent => {}
            }
        }
        read_primary_api_key(paths)
    }
```

Replace `doomed`, `report_items` and `remove_items` (lines 250–324) with:

```rust
    /// Every entry holding secrets that a change of these extents destroys, read now (§9.4
    /// step 7): on each axis its Keychain item, then the plaintext entry behind it
    /// (`.credentials.json`, `primaryApiKey`). An absent entry is listed as `Absent`.
    pub fn doomed(
        &self,
        env: &Env,
        paths: &CcPaths,
        entry: Extent,
        managed: Extent,
    ) -> Vec<DoomedEntry> {
        let mut out = Vec::new();
        for (kind, extent) in [(ItemKind::OAuth, entry), (ItemKind::ManagedKey, managed)] {
            if extent == Extent::None {
                continue;
            }
            if self.mac() {
                let (svc, acct) = (keychain_service(env, kind), keychain_account(env));
                out.push(DoomedEntry {
                    bytes: self.retrying(|| self.keychain.find(&svc, &acct)),
                });
            }
            let plain = match kind {
                ItemKind::OAuth => read_bytes(&paths.credentials_file),
                ItemKind::ManagedKey => read_primary_api_key(paths),
            };
            out.push(DoomedEntry { bytes: plain });
        }
        out
    }

    /// Hands `before_fallback` the current bytes of `kind`'s item, before a fallback deletes it.
    fn report_item(
        &self,
        env: &Env,
        kind: ItemKind,
        before_fallback: BeforeFallback<'_>,
    ) -> Result<(), ProviderError> {
        let found = self
            .keychain
            .find(&keychain_service(env, kind), &keychain_account(env));
        if let Some(bytes) = present_or_err(found)? {
            before_fallback(&bytes)?;
        }
        Ok(())
    }

    /// Deletes `kind`'s item and verifies it gone with the existence probe (Appendix A.3).
    fn remove_item(
        &self,
        env: &Env,
        kind: ItemKind,
        fence: Fence<'_>,
    ) -> Result<(), ProviderError> {
        let (svc, acct) = (keychain_service(env, kind), keychain_account(env));
        fence()?;
        let _ = self.keychain.delete(&svc, &acct);
        if !matches!(self.keychain.exists(&svc, &acct), Read::Absent) {
            return Err(ProviderError::ShadowingItem(svc));
        }
        Ok(())
    }
```

Replace `write_credential_entry` (its doc comment and its two calls change):

```rust
    /// Appendix A.3 write, including the verified file fallback, which first reports the item
    /// it will delete to `before_fallback`. Returns where this write put the credential:
    /// a file mirrored for hot reload does not make it a file store.
    pub fn write_credential_entry(
        &self,
        env: &Env,
        paths: &CcPaths,
        bytes: &[u8],
        fence: Fence<'_>,
        before_fallback: BeforeFallback<'_>,
    ) -> Result<SecretStore, ProviderError> {
        if !self.mac() {
            self.write_file(paths, bytes, fence)?;
            return Ok(SecretStore::File(paths.credentials_file.clone()));
        }
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
                Err(e) => tracing::warn!(
                    "keychain write failed, falling back to the credentials file: {e}"
                ),
            }
        }
        self.report_item(env, ItemKind::OAuth, before_fallback)?;
        self.write_file(paths, bytes, fence)?;
        self.remove_item(env, ItemKind::OAuth, fence)?;
        self.file_mode_pinned.store(true, Ordering::SeqCst);
        Ok(SecretStore::Fallback(paths.credentials_file.clone()))
    }
```

Replace `clear_credential_account_keys`:

```rust
    /// API-key activation: keep only the machine-shared keys of the credential item and the
    /// credentials file; delete either when none remain (§9.4 step 7).
    pub fn clear_credential_account_keys(
        &self,
        env: &Env,
        paths: &CcPaths,
        fence: Fence<'_>,
    ) -> Result<(), ProviderError> {
        if self.mac() {
            let (svc, acct) = (
                keychain_service(env, ItemKind::OAuth),
                keychain_account(env),
            );
            if let Some(b) = present_or_err(self.keychain.find(&svc, &acct))? {
                let kept = keep_shared(&b)?;
                fence()?;
                match kept {
                    Some(k) => self.keychain.upsert(&svc, &acct, &k)?,
                    None => self.keychain.delete(&svc, &acct)?,
                }
            }
        }
        if let Some(b) = present_or_err(read_bytes(&paths.credentials_file))? {
            let kept = keep_shared(&b)?;
            match kept {
                Some(k) => write_atomic_private_with(&paths.credentials_file, &k, 0o600, fence)?,
                None => {
                    fence()?;
                    remove_if_present(&paths.credentials_file)?
                }
            }
        }
        Ok(())
    }
```

In `write_managed_key`, change the doc comment's "and every managed-key item is removed and
verified gone" to "and the managed-key item is removed and verified gone", then replace

```rust
            self.report_items(env, ItemKind::ManagedKey, before_fallback)?;
```

with

```rust
            self.report_item(env, ItemKind::ManagedKey, before_fallback)?;
```

and

```rust
        self.remove_items(env, ItemKind::ManagedKey, fence)?;
        Ok(SecretStore::Fallback(paths.global_config.clone()))
```

with

```rust
        self.remove_item(env, ItemKind::ManagedKey, fence)?;
        Ok(SecretStore::Fallback(paths.global_config.clone()))
```

Replace `clear_managed_key` and `snapshot`:

```rust
    /// Writing OAuth clears the managed key: the managed-key item is deleted (verified) and
    /// `primaryApiKey` is dropped. `approved` is kept (B.10).
    pub fn clear_managed_key(
        &self,
        env: &Env,
        paths: &CcPaths,
        fence: Fence<'_>,
    ) -> Result<(), ProviderError> {
        if self.mac() {
            self.remove_item(env, ItemKind::ManagedKey, fence)?;
        }
        config::splice_key(&paths.global_config, "primaryApiKey", None, fence)?;
        Ok(())
    }
```

```rust
    /// Refuses when any entry a switch may overwrite cannot be read.
    pub fn snapshot(&self, env: &Env, paths: &CcPaths) -> Result<Snapshot, ProviderError> {
        let acct = keychain_account(env);
        let items = |kind| -> Result<Vec<ItemSnapshot>, ProviderError> {
            if !self.mac() {
                return Ok(vec![]);
            }
            let svc = keychain_service(env, kind);
            let value = present_or_err(self.keychain.find(&svc, &acct))?;
            Ok(vec![(svc, value)])
        };
        Ok(Snapshot {
            oauth_items: items(ItemKind::OAuth)?,
            managed_items: items(ItemKind::ManagedKey)?,
            credentials_file: present_or_err(read_bytes(&paths.credentials_file))?,
            global_config: present_or_err(read_bytes(&paths.global_config))?,
        })
    }
```

`restore` is unchanged: it already walks whatever items the snapshot holds.

**`crates/tagteam-cc/src/provider.rs`**: change the import on line 21 to

```rust
use crate::naming::{ItemKind, keychain_account, keychain_service};
```

and replace `keychain_items`:

```rust
/// `kind`'s one Keychain item for `env` (Appendix A.2), paired with the account: the
/// credential- or managed-key half of §3's identity surface. Empty off macOS, where CC never
/// touches the Keychain (Appendix A.3).
fn keychain_items(env: &Env, kind: ItemKind, acct: &str, mac: bool) -> Vec<(String, String)> {
    if !mac {
        return vec![];
    }
    vec![(keychain_service(env, kind), acct.to_owned())]
}
```

**`crates/tagteam-provider/src/provider.rs`**: in the `ShadowingItem` doc comment (line 304),
"A Keychain item `remove_items` could not verify gone" becomes "A Keychain item `remove_item`
could not verify gone". Replace `DoomedEntry` with

```rust
/// A live entry holding secrets that a `LiveChange` overwrites or deletes. `Debug` is derived:
/// `Read<T>`'s own `Debug` redacts the bytes.
#[derive(Debug, Clone)]
pub struct DoomedEntry {
    /// Its current contents.
    pub bytes: Read<Vec<u8>>,
}
```

and in `tests::debug_output_never_contains_secret_bytes` the literal becomes

```rust
        let doomed = DoomedEntry {
            bytes: Read::Present(SENTINEL.as_bytes().to_vec()),
        };
```

**`crates/tagteam-fake/src/provider.rs`**: replace `FakeAgent::doomed` with

```rust
    fn doomed(
        &self,
        env: &Env,
        _locks: &LiveLocks<'_>,
        change: LiveChange<'_>,
    ) -> Vec<DoomedEntry> {
        match change {
            // The credential file is the one entry a write replaces.
            LiveChange::Write(_) => vec![DoomedEntry {
                bytes: read_file(&FakePaths::resolve(env).credential),
            }],
            // There is no other axis to clear.
            LiveChange::ClearOther(_) => vec![],
        }
    }
```

and in `crates/tagteam-fake/tests/provider.rs`, `a_write_keeps_the_machines_device_key_and_undoes_exactly`,
delete the line `assert!(!doomed[0].on_fallback);` (line 179).

In `crates/tagteam-cc/tests/live_store.rs`, rename `a_counting_fence_stops_the_deletes_in_remove_items`
(line 1009) to `a_counting_fence_stops_the_delete_in_remove_item`.

**`crates/tagteam-engine/src/switch.rs`**, in `Engine::transact`, replace

```rust
        // Step 7's rule: every entry the write surely destroys is saved first, unless a vault
        // of either account, or steps 2 and 4, already hold its generation. What only a
        // Keychain-refusal fallback destroys is saved by `before_fallback`, if it happens.
        for id in outgoing.iter().map(|o| &o.id).chain([&target.id]) {
            self.hold_vault(p, &mut held, id);
        }
        for entry in doomed.iter().filter(|d| !d.on_fallback) {
```

with

```rust
        // Step 7's rule: every entry the write destroys is saved first, unless a vault of
        // either account, or steps 2 and 4, already hold its generation. A Keychain-refusal
        // fallback reports what it deletes to `before_fallback` too, which finds it held.
        for id in outgoing.iter().map(|o| &o.id).chain([&target.id]) {
            self.hold_vault(p, &mut held, id);
        }
        for entry in &doomed {
```

**`crates/tagteam-engine/src/active.rs`**, in `Engine::publish` (line 527), replace

```rust
        for entry in doomed.iter().filter(|d| !d.on_fallback) {
```

with

```rust
        for entry in &doomed {
```

- [ ] **Step 4: Run them and see them pass**, then the crates' whole suites

Run: `cargo test -p tagteam-provider -p tagteam-cc -p tagteam-fake`
Expected: PASS.

Run: `cargo test -p tagteam-engine --features test-hooks`
Expected: PASS, including the four rewritten engine tests. Every other `on_fallback` user was a
filter that let everything through, so no other engine test moves.

Run: `cargo test --workspace --features tagteam/test-support`
Expected: PASS.

Run: `rg -n 'read_services|on_fallback|find_first|report_items|remove_items|with_fallback_items|FALLBACK_' crates`
Expected: no output.

- [ ] **Step 5: Format and lint**

```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: no output from `fmt --check`, and both clippy runs finish with no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-cc/src/naming.rs crates/tagteam-cc/src/lib.rs crates/tagteam-cc/src/live.rs \
  crates/tagteam-cc/src/provider.rs crates/tagteam-provider/src/provider.rs \
  crates/tagteam-fake/src/provider.rs crates/tagteam-engine/src/switch.rs \
  crates/tagteam-engine/src/active.rs crates/tagteam-cc/tests/provider.rs \
  crates/tagteam-cc/tests/live_store.rs crates/tagteam-fake/tests/provider.rs \
  crates/tagteam-engine/tests/common/mod.rs crates/tagteam-engine/tests/switch.rs \
  crates/tagteam-engine/tests/destroyed.rs crates/tagteam-engine/tests/recover.rs \
  crates/tagteam-engine/tests/invariant.rs
git commit -m "Read, write and clear only the one Keychain item a spelling names, as Claude Code 2.1.286 does"
```

---

### Task 6: Session records, `lstart` liveness, `ProcessProbe`, the lock probe

§12.6 decides whether a session record's writer still lives, and §12.5 decides whether a launch
reservation does. Both feed Task 9's session state, which makes an account session-owned. This
task builds the two primitives in `tagteam-provider` and wires neither into the engine:
- `liveness.rs`: the record parser and listing, the `ps lstart` parser, the `ProcessProbe` port
  (§4.2) with its system and fake implementations, and `record_is_live`.
- `flock.rs`: `probe_lock`, the non-creating, non-waiting test of a reservation's lock.

The direction of every doubt is fixed by §12.6: anything that cannot be determined counts as
live, and a malformed record counts as unreadable. A wrong "live" keeps an account session-owned
(safe, and `doctor` lists it). A wrong "dead" lets tagteam refresh a token a running session
holds (unsafe). So every `None` from the probe means live.

**Readings of the spec this task commits to:**
- **Which files are records.** Every `*.json` in the records directory, in name order; other
  names are ignored. A record removed between the listing and its read was removed by its
  session's graceful exit, so it is skipped rather than reported unreadable. A listing that
  fails part-way is unreadable as a whole, since it may hide a record.
- **What is malformed** (§12.6, the contract's list): not UTF-8, not JSON, not an object, nested
  deeper than 32 (the record object is level 1; `serde_json`'s own limit of 128 also makes deeper
  text "not JSON"), or a `pid` that is missing, not an integer, or out of range. **The range is
  1 through `i32::MAX`.** `kill(0, 0)` and a negative pid address process groups, so neither may
  ever be probed. A `procStart` string is kept as is. A JSON number is kept as its digits, the
  all-digit legacy form. Any other shape is absent. A `startedAt` that is not an integer, or a
  `kind` that is not a string, is absent, not malformed: §12.6 names only the shape, the pid, the
  depth and the encoding, and both have a safe absence.
- **`parse_lstart`.** Runs of whitespace separate the fields, so the double space `ps` pads a
  single-digit day with parses, and so does a trailing newline. The weekday must be one of the
  seven C-locale names but is not checked against the date: the spec's example as written today
  (`Wed Oct  1 12:34:56 2026`, which Task 17 corrects to `Thu`) names a Wednesday that was a
  Thursday, `ps` always prints the right one, and a cross-check would only reject such text.
  The tests pin both spellings to the same epoch. The month is a C-locale name. The
  day has one or two digits and must exist in that month, with leap years by the Gregorian rule
  (÷4, except ÷100 unless ÷400). `HH:MM:SS` has exactly two digits per field, in range. The year
  has exactly four digits and is at least 1970. Digits only: `+1` is not a day. The text is UTC
  (`TZ=UTC`). The epoch comes from Howard Hinnant's `days_from_civil`, with no date crate
  (Decision 7).
- **`record_is_live`**, in order:
  1. `exists`: `Some(false)` is dead; `None` is live.
  2. A `procStart` that parses as `lstart`: live if the OS start time is within ±1 s of it, or
     unknown. A mismatch is recycled only when `mentions` is `Some(false)`; a process that
     mentions the launch command, or whose `mentions` is unknown, is live (Decision 16, which
     amends §12.6: a Linux start time moves with the wall clock).
  3. All digits: live if `/proc/<pid>/stat` field 22 equals it, or is unknown. macOS has no
     `/proc`, so such a record is always live there.
  4. Otherwise (absent, empty or unparseable), cswap's rule: recycled only if `startedAt` and the
     OS start time are both known, the process started more than 120 s after `startedAt`, and
     `mentions` is `Some(false)`. The OS start time is in whole seconds, rounded down, so the
     comparison can never call a process late too early.
- **`SystemProcessProbe`.**
  - `exists` refuses a pid outside 1..=`i32::MAX` (`None`), then calls `kill(pid, 0)`.
  - macOS: `start_time_s` is `proc_pidinfo(PROC_PIDTBSDINFO)`'s start (`process::start_of`, µs
    ÷ 10⁶), and `start_ticks` is `None`. `mentions` reads `sysctl(KERN_PROCARGS2)`: the
    executable path and the `argc` arguments, never the environment block that follows them,
    where a variable such as `CLAUDE_CONFIG_DIR=…/.claude` would always mention `claude`.
    Arguments that cannot be read, such as another user's process, are `None`.
  - Linux: `start_time_s` is `/proc/stat`'s `btime` plus field 22 ÷ `sysconf(_SC_CLK_TCK)`, and
    `start_ticks` is field 22. `mentions` reads `/proc/<pid>/comm` and `/proc/<pid>/cmdline`: it
    is `Some(true)` if either readable one contains the needle, and `None` if one cannot be read
    and the other does not contain it.
  - `process::start_of` becomes `pub(crate)` on both platforms for this. `ProcessStamp` is
    unchanged: the journal holder keeps its exact-match rule (§12.6).
- **`probe_lock`** opens with `File::open` (read-only; never `create`), then tries
  `flock(LOCK_EX | LOCK_NB)`. Acquired → `LOCK_UN` → `Free`. `EWOULDBLOCK` → `Held`.
  `NotFound`, including a missing parent directory → `Missing`. Any other error → `Err`, which
  Task 7's `launch_reservations` turns into unreadable. `flock` works on a read-only descriptor
  on both platforms. The lock belongs to an open file description, so a holder in this same
  process is seen as `Held` too, which is what lets the tests hold one with `FlockGuard`.

**Review Focus 2** is pinned by `a_recycled_pid_left_by_sigkill_is_dead_when_it_is_not_a_claude_process`
(a recycled pid whose `lstart` mismatches and which is not a claude process is dead),
`an_lstart_mismatch_is_recycled_only_when_the_process_does_not_mention_the_launch_command`
(Decision 16's three cases) and
`without_a_usable_proc_start_cswaps_heuristic_decides` (an absent, empty or unparseable
`procStart` falls back to the 120 s rule, case by case).

**Files:**
- Create: `crates/tagteam-provider/src/liveness.rs`
- Modify: `crates/tagteam-provider/src/process.rs` (both `start_of` definitions, lines 29 and 50,
  become `pub(crate)`)
- Modify: `crates/tagteam-provider/src/flock.rs` (`LockProbe` and `probe_lock` after
  `impl FlockGuard`, line 66; three tests in `mod tests`)
- Modify: `crates/tagteam-provider/src/lib.rs` (`pub mod liveness;` after line 7; the `flock`
  re-export on line 26; the `liveness` re-exports after line 32)
- Test: `crates/tagteam-provider/src/liveness.rs` and `crates/tagteam-provider/src/flock.rs`
  (in-module, as `process.rs` and `flock.rs` already test)

**Interfaces:**
- Consumes: `crate::process::start_of(pid: u32) -> io::Result<Option<u64>>` (made `pub(crate)`
  here); `crate::read::{Read, ReadError}`; `FlockGuard::try_lock` and `crate::FORK_GUARD` in
  tests
- Produces (Interface Contract, `src/liveness.rs` and `src/flock.rs`, all re-exported at the
  crate root):
  - `SessionRecord { pid: u32, proc_start: Option<String>, started_at_ms: Option<i64>, kind: Option<String> }`
  - `parse_session_record(bytes: &[u8]) -> Result<SessionRecord, String>`
  - `RecordEntry { Record(SessionRecord), Unreadable { path: PathBuf, detail: String } }`
  - `read_session_records(dir: &Path) -> Read<Vec<RecordEntry>>`
  - `parse_lstart(s: &str) -> Option<i64>`
  - `trait ProcessProbe: Send + Sync { exists, start_time_s, start_ticks, mentions }`,
    `SystemProcessProbe`, `FakeProcessProbe::{new, set}`,
    `FakeProcess { exists, start_time_s, start_ticks, mentions_launch }`
  - `record_is_live(probe: &dyn ProcessProbe, r: &SessionRecord, launch_command: &str) -> bool`
  - `LockProbe { Missing, Free, Held }`, `probe_lock(path: &Path) -> io::Result<LockProbe>`

**Spec:**
- §12.6: a record's pid is live if `kill(pid, 0)` succeeds or returns `EPERM` and it still
  belongs to the writer, judged from `procStart` (`lstart` ±1 s, all digits, or cswap's
  heuristic). Records of every `kind` count. Anything undeterminable is live, and a malformed
  record is unreadable.
- §12.5 "Launch reservation": a reservation is live while its file is locked, and others test it
  with a non-blocking `flock` and never wait on it.
- §4.2: `ProcessProbe` is one of the engine's injected ports.
- Appendix A.7: the record's fields, and `procStart` as `ps -o lstart=` with `LC_ALL=C` and
  `TZ=UTC`, omitted when `ps` fails.
- §4.3: every read is tri-state; unreadable never becomes absent.

#### Cycle 1: session records and liveness

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam-provider/src/liveness.rs` holding only its test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record(v: Value) -> Result<SessionRecord, String> {
        parse_session_record(v.to_string().as_bytes())
    }

    #[test]
    fn a_record_keeps_what_liveness_needs_and_ignores_the_rest() {
        let r = record(json!({
            "pid": 4242, "sessionId": "s", "cwd": "/w", "startedAt": 1_790_858_096_000_i64,
            "procStart": "Thu Oct  1 12:34:56 2026", "version": "2.1.286", "kind": "bg",
            "entrypoint": "cli", "status": "idle", "updatedAt": 1
        }))
        .unwrap();
        assert_eq!(
            r,
            SessionRecord {
                pid: 4242,
                proc_start: Some("Thu Oct  1 12:34:56 2026".into()),
                started_at_ms: Some(1_790_858_096_000),
                kind: Some("bg".into()),
            }
        );
        assert_eq!(
            record(json!({"pid": 7})).unwrap(),
            SessionRecord {
                pid: 7,
                proc_start: None,
                started_at_ms: None,
                kind: None
            }
        );
    }

    #[test]
    fn a_numeric_proc_start_is_the_all_digit_legacy_form_and_any_other_shape_is_absent() {
        assert_eq!(
            record(json!({"pid": 7, "procStart": 123456}))
                .unwrap()
                .proc_start
                .as_deref(),
            Some("123456")
        );
        for odd in [json!(null), json!(true), json!({"t": 1}), json!([1])] {
            assert_eq!(
                record(json!({"pid": 7, "procStart": odd}))
                    .unwrap()
                    .proc_start,
                None
            );
        }
        assert_eq!(
            record(json!({"pid": 7, "startedAt": "soon", "kind": 3})).unwrap(),
            SessionRecord {
                pid: 7,
                proc_start: None,
                started_at_ms: None,
                kind: None
            }
        );
    }

    #[test]
    fn a_malformed_record_is_an_error_that_never_quotes_its_bytes() {
        let nested = |levels: usize| {
            let mut s = String::from("{\"pid\": 7, \"x\": ");
            s.push_str(&"[".repeat(levels));
            s.push_str(&"]".repeat(levels));
            s.push('}');
            s.into_bytes()
        };
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("not JSON", b"SENTINEL{".to_vec()),
            ("an array", br#"["SENTINEL"]"#.to_vec()),
            ("a string", br#""SENTINEL""#.to_vec()),
            ("a number", b"7".to_vec()),
            ("no pid", br#"{"kind": "SENTINEL"}"#.to_vec()),
            ("a string pid", br#"{"pid": "SENTINEL"}"#.to_vec()),
            ("a fractional pid", br#"{"pid": 1.5}"#.to_vec()),
            ("pid 0", br#"{"pid": 0}"#.to_vec()),
            ("a negative pid", br#"{"pid": -1}"#.to_vec()),
            ("a pid above i32::MAX", br#"{"pid": 2147483648}"#.to_vec()),
            (
                "a huge pid",
                br#"{"pid": 99999999999999999999999999}"#.to_vec(),
            ),
            ("33 levels", nested(32)),
            ("200 levels", nested(199)),
            (
                "invalid UTF-8",
                b"{\"pid\": 7, \"k\": \"\xff\xfeSENTINEL\"}".to_vec(),
            ),
        ];
        for (case, bytes) in cases {
            let detail = parse_session_record(&bytes).expect_err(case);
            assert!(!detail.contains("SENTINEL"), "{case}: {detail}");
        }
        assert_eq!(
            parse_session_record(&nested(31)).unwrap().pid,
            7,
            "32 levels is the limit, not past it"
        );
        assert_eq!(
            record(json!({"pid": i32::MAX})).unwrap().pid,
            i32::MAX as u32
        );
    }

    #[test]
    fn records_are_listed_in_name_order_and_a_malformed_one_is_unreadable() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("sessions");
        assert!(
            matches!(read_session_records(&dir), Read::Present(v) if v.is_empty()),
            "a missing directory holds no records"
        );
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("20.json"), br#"{"pid": 20}"#).unwrap();
        fs::write(dir.join("10.json"), br#"{"pid": 10}"#).unwrap();
        fs::write(dir.join("30.json"), b"{").unwrap();
        fs::write(dir.join("notes.txt"), b"not a record").unwrap();
        fs::create_dir(dir.join("40.json")).unwrap();
        let Read::Present(entries) = read_session_records(&dir) else {
            panic!("the directory lists");
        };
        let pid = |p| RecordEntry::Record(record(json!({"pid": p})).unwrap());
        assert_eq!(entries[..2], [pid(10), pid(20)]);
        assert!(
            matches!(&entries[2], RecordEntry::Unreadable { path, .. } if path == &dir.join("30.json"))
        );
        assert!(
            matches!(&entries[3], RecordEntry::Unreadable { path, .. } if path == &dir.join("40.json")),
            "a directory named like a record cannot be read"
        );
        assert_eq!(entries.len(), 4, "notes.txt is not a record");
    }

    #[test]
    fn a_records_directory_that_cannot_be_listed_is_unreadable() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("sessions");
        fs::write(&file, b"").unwrap();
        assert!(matches!(read_session_records(&file), Read::Unreadable(_)));
    }

    #[test]
    fn lstart_text_parses_to_epoch_seconds() {
        for (text, epoch) in [
            ("Thu Jan  1 00:00:00 1970", 0),
            ("Thu Oct  1 12:34:56 2026", 1_790_858_096),
            ("Thu Oct 10 01:02:03 2030", 1_917_824_523),
            ("Fri Dec 31 23:59:59 1999", 946_684_799),
            ("Thu Feb 29 23:59:59 2024", 1_709_251_199),
            ("Tue Feb 29 00:00:00 2000", 951_782_400),
            ("Mon Mar  1 00:00:00 2100", 4_107_542_400),
            ("Thu Oct  1 12:34:56 2026\n", 1_790_858_096),
            ("  Thu Oct  1 12:34:56 2026  ", 1_790_858_096),
            // The weekday is not checked against the date: 2026-10-01 is a Thursday.
            ("Wed Oct  1 12:34:56 2026", 1_790_858_096),
        ] {
            assert_eq!(parse_lstart(text), Some(epoch), "{text:?}");
        }
    }

    #[test]
    fn anything_but_lstart_text_is_refused() {
        for text in [
            "",
            "garbage",
            "1790858096",
            "Mon Feb 29 00:00:00 2100",
            "Wed Feb 29 00:00:00 2023",
            "Thu Apr 31 00:00:00 2026",
            "Thu Oct  0 12:34:56 2026",
            "Thu Oct 32 12:34:56 2026",
            "Thu Oct +1 12:34:56 2026",
            "Thu Oct 001 12:34:56 2026",
            "Thu Foo  1 12:34:56 2026",
            "Xyz Oct  1 12:34:56 2026",
            "thu oct  1 12:34:56 2026",
            "Thu Oct  1 24:00:00 2026",
            "Thu Oct  1 12:60:00 2026",
            "Thu Oct  1 12:34:60 2026",
            "Thu Oct  1 12:34 2026",
            "Thu Oct  1 1:02:03 2026",
            "Thu Oct  1 12:34:56 1969",
            "Thu Oct  1 12:34:56 -2026",
            "Thu Oct  1 12:34:56 26",
            "Thu Oct  1 12:34:56 2026 UTC",
        ] {
            assert_eq!(parse_lstart(text), None, "{text:?}");
        }
    }

    #[test]
    fn the_civil_arithmetic_agrees_with_every_day_from_1970_to_2400() {
        let mut expected = 0;
        for y in 1970..2400 {
            for m in 1..=12 {
                for d in 1..=days_in_month(y, m) {
                    assert_eq!(days_from_civil(y, m, d), expected, "{y}-{m}-{d}");
                    expected += 1;
                }
            }
        }
    }

    fn running(start_time_s: i64) -> FakeProcess {
        FakeProcess {
            exists: Some(true),
            start_time_s: Some(start_time_s),
            ..FakeProcess::default()
        }
    }

    fn rec(proc_start: Option<&str>, started_at_ms: Option<i64>) -> SessionRecord {
        SessionRecord {
            pid: 100,
            proc_start: proc_start.map(str::to_owned),
            started_at_ms,
            kind: Some("interactive".into()),
        }
    }

    const LSTART: &str = "Thu Oct  1 12:34:56 2026";
    const STARTED: i64 = 1_790_858_096;

    /// A running process that does not mention the launch command, started at `start_time_s`.
    fn stranger(start_time_s: i64) -> FakeProcess {
        FakeProcess {
            mentions_launch: Some(false),
            ..running(start_time_s)
        }
    }

    #[test]
    fn an_lstart_record_of_another_program_is_live_only_within_a_second_of_the_os_start_time() {
        for (actual, live) in [
            (STARTED, true),
            (STARTED - 1, true),
            (STARTED + 1, true),
            (STARTED - 2, false),
            (STARTED + 2, false),
            (STARTED + 3_600, false),
        ] {
            let probe = FakeProcessProbe::new();
            probe.set(100, stranger(actual));
            assert_eq!(
                record_is_live(&probe, &rec(Some(LSTART), None), "claude"),
                live,
                "OS start {actual}"
            );
        }
    }

    #[test]
    fn a_recycled_pid_left_by_sigkill_is_dead_when_it_is_not_a_claude_process() {
        // Review Focus 2: the record outlived its writer, and the OS gave the pid to a new
        // process an hour later. The lstart mismatches and the process is not a claude
        // process, so the record is dead (Decision 16).
        let probe = FakeProcessProbe::new();
        probe.set(100, stranger(STARTED + 3_600));
        assert!(!record_is_live(
            &probe,
            &rec(Some(LSTART), Some(STARTED * 1000)),
            "claude"
        ));
    }

    #[test]
    fn an_lstart_mismatch_is_recycled_only_when_the_process_does_not_mention_the_launch_command() {
        // Decision 16: a Linux start time moves with the wall clock (suspend, a clock step), so
        // a mismatch alone is no proof. A process that mentions `claude` is live, and so is one
        // whose arguments cannot be read (§12.6: undetermined counts as live).
        for (mentions, live) in [(Some(true), true), (Some(false), false), (None, true)] {
            let probe = FakeProcessProbe::new();
            probe.set(
                100,
                FakeProcess {
                    mentions_launch: mentions,
                    ..running(STARTED + 3_600)
                },
            );
            assert_eq!(
                record_is_live(&probe, &rec(Some(LSTART), Some(STARTED * 1000)), "claude"),
                live,
                "mentions {mentions:?}"
            );
        }
    }

    #[test]
    fn a_pid_that_is_gone_is_dead_and_one_that_cannot_be_probed_is_live() {
        let probe = FakeProcessProbe::new();
        assert!(
            !record_is_live(&probe, &rec(Some(LSTART), None), "claude"),
            "unknown pid"
        );
        probe.set(
            100,
            FakeProcess {
                exists: Some(false),
                ..running(STARTED)
            },
        );
        assert!(!record_is_live(&probe, &rec(Some(LSTART), None), "claude"));
        probe.set(
            100,
            FakeProcess {
                exists: None,
                ..running(STARTED + 3_600)
            },
        );
        assert!(record_is_live(&probe, &rec(Some(LSTART), None), "claude"));
        probe.set(
            100,
            FakeProcess {
                start_time_s: None,
                ..running(0)
            },
        );
        assert!(
            record_is_live(&probe, &rec(Some(LSTART), None), "claude"),
            "an unknown start time counts as live"
        );
    }

    #[test]
    fn an_all_digit_proc_start_must_equal_the_start_ticks() {
        for (ticks, live) in [(Some(5_000), true), (Some(5_001), false), (None, true)] {
            let probe = FakeProcessProbe::new();
            probe.set(
                100,
                FakeProcess {
                    exists: Some(true),
                    start_ticks: ticks,
                    ..FakeProcess::default()
                },
            );
            assert_eq!(
                record_is_live(&probe, &rec(Some("5000"), None), "claude"),
                live,
                "{ticks:?}"
            );
        }
    }

    #[test]
    fn without_a_usable_proc_start_cswaps_heuristic_decides() {
        // Review Focus 2: recycled only if the process started more than 120 s after
        // `startedAt` and nothing about it mentions the launch command.
        let at_ms = STARTED * 1000;
        let cases = [
            (STARTED + 121, Some(false), Some(at_ms), true, false),
            (STARTED + 121, Some(true), Some(at_ms), true, true),
            (STARTED + 121, None, Some(at_ms), true, true),
            (STARTED + 120, Some(false), Some(at_ms), true, true),
            (STARTED + 5, Some(false), Some(at_ms), true, true),
            (STARTED + 121, Some(false), None, true, true),
            (STARTED + 121, Some(false), Some(at_ms), false, true),
        ];
        for (start, mentions, started_at, known_start, live) in cases {
            for proc_start in [None, Some(""), Some("not a date")] {
                let probe = FakeProcessProbe::new();
                probe.set(
                    100,
                    FakeProcess {
                        exists: Some(true),
                        start_time_s: known_start.then_some(start),
                        start_ticks: None,
                        mentions_launch: mentions,
                    },
                );
                assert_eq!(
                    record_is_live(&probe, &rec(proc_start, started_at), "claude"),
                    live,
                    "start {start}, mentions {mentions:?}, startedAt {started_at:?}, \
                     known {known_start}, procStart {proc_start:?}"
                );
            }
        }
    }

    #[test]
    fn procargs_yield_the_executable_and_arguments_but_never_the_environment() {
        let mut buf = 2_i32.to_ne_bytes().to_vec();
        buf.extend_from_slice(b"/usr/local/bin/node\0\0\0\0");
        buf.extend_from_slice(b"node\0/opt/claude-code/cli.js\0");
        buf.extend_from_slice(b"CLAUDE_CONFIG_DIR=/p\0");
        assert_eq!(
            parse_procargs(&buf).unwrap(),
            vec![
                b"/usr/local/bin/node".to_vec(),
                b"node".to_vec(),
                b"/opt/claude-code/cli.js".to_vec()
            ]
        );
        let mut env_only = 0_i32.to_ne_bytes().to_vec();
        env_only.extend_from_slice(b"/bin/sh\0claude=1\0");
        assert_eq!(
            parse_procargs(&env_only).unwrap(),
            vec![b"/bin/sh".to_vec()]
        );
        assert_eq!(parse_procargs(b"\x01"), None, "too short for argc");
        assert_eq!(
            parse_procargs(&1_i32.to_ne_bytes()),
            None,
            "no executable path"
        );
    }

    /// The `lstart` text for an epoch second, for the real-process test below.
    fn lstart_of(epoch_s: i64) -> String {
        let (days, secs) = (epoch_s.div_euclid(86_400), epoch_s.rem_euclid(86_400));
        let (mut y, mut d) = (1970, days);
        while d >= if is_leap(y) { 366 } else { 365 } {
            d -= if is_leap(y) { 366 } else { 365 };
            y += 1;
        }
        let mut m = 1;
        while d >= i64::from(days_in_month(y, m)) {
            d -= i64::from(days_in_month(y, m));
            m += 1;
        }
        format!(
            "{} {} {:>2} {:02}:{:02}:{:02} {y}",
            WEEKDAYS[(days + 4).rem_euclid(7) as usize],
            MONTHS[m as usize - 1],
            d + 1,
            secs / 3_600,
            secs / 60 % 60,
            secs % 60
        )
    }

    #[test]
    fn this_process_is_live_under_its_own_start_time_and_recycled_under_another() {
        let me = std::process::id();
        let p = SystemProcessProbe;
        assert_eq!(p.exists(me), Some(true));
        let start = p.start_time_s(me).expect("this process's start time");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert!(
            start <= now + 1 && now - start < 86_400,
            "{start} vs now {now}"
        );
        assert_eq!(parse_lstart(&lstart_of(start)), Some(start));
        // The launch command is a word this process never mentions, so a mismatch is decided by
        // the start time alone (Decision 16), wherever the test binary was built.
        let stranger = "no-such-word-7f3a";
        let mine = SessionRecord {
            pid: me,
            proc_start: Some(lstart_of(start)),
            started_at_ms: Some(start * 1000),
            kind: None,
        };
        assert!(record_is_live(&p, &mine, stranger));
        let recycled = SessionRecord {
            proc_start: Some(lstart_of(start - 3_600)),
            ..mine.clone()
        };
        assert!(!record_is_live(&p, &recycled, stranger));
        assert!(
            record_is_live(&p, &recycled, "tagteam_provider"),
            "a mismatch on a process that mentions the launch command is live"
        );
    }

    #[test]
    fn this_process_mentions_its_own_binary_and_not_a_stranger() {
        let me = std::process::id();
        let p = SystemProcessProbe;
        assert_eq!(p.mentions(me, "tagteam_provider"), Some(true));
        assert_eq!(p.mentions(me, "no-such-word-7f3a"), Some(false));
        if cfg!(target_os = "linux") {
            assert!(p.start_ticks(me).is_some());
        } else {
            assert_eq!(p.start_ticks(me), None);
        }
    }

    #[test]
    fn a_pid_no_process_can_hold_is_gone_and_unprobeable_pids_are_undetermined() {
        let p = SystemProcessProbe;
        let dead = i32::MAX as u32;
        assert_eq!(p.exists(dead), Some(false));
        assert_eq!(p.start_time_s(dead), None);
        assert_eq!(p.start_ticks(dead), None);
        assert_eq!(p.mentions(dead, "claude"), None);
        assert_eq!(p.exists(0), None, "pid 0 is a process group");
        assert_eq!(p.exists(u32::MAX), None);
        let gone = SessionRecord {
            pid: dead,
            proc_start: None,
            started_at_ms: None,
            kind: None,
        };
        assert!(!record_is_live(&p, &gone, "claude"));
    }
}
```

and declare it in `crates/tagteam-provider/src/lib.rs`, after `pub mod keychain;`:

```rust
pub mod liveness;
```

The two real-process tests use this test process's own pid (`std::process::id()`) and `i32::MAX`,
which no process can hold (it is above every pid_max). Nothing forks.

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-provider --lib liveness`
Expected: compile errors only, about 70 of them:
- E0425 for `parse_session_record`, `read_session_records`, `parse_lstart`, `record_is_live`,
  `parse_procargs`, `days_from_civil`, `days_in_month`, `is_leap`, `WEEKDAYS`, `MONTHS` and
  `SystemProcessProbe`.
- E0412/E0422/E0433 for `SessionRecord`, `RecordEntry`, `FakeProcess` and `FakeProcessProbe`.
- The same codes for `Value`, `Read` and `fs`, which reach the tests through the module's own
  imports in Step 3.

- [ ] **Step 3: Implement**

In `crates/tagteam-provider/src/process.rs`, make both `start_of` definitions `pub(crate)`:

```rust
/// `/proc/<pid>/stat` field 22, counted after the last `)`.
#[cfg(target_os = "linux")]
pub(crate) fn start_of(pid: u32) -> io::Result<Option<u64>> {
```

```rust
/// `proc_pidinfo(PROC_PIDTBSDINFO)` start time, in microseconds.
#[cfg(target_os = "macos")]
pub(crate) fn start_of(pid: u32) -> io::Result<Option<u64>> {
```

In `crates/tagteam-provider/src/liveness.rs`, above the test module, add:

```rust
//! §12.6: session records and whether the process that wrote one still lives. Reservations are
//! judged by their lock alone (§12.5, `flock::probe_lock`); this is the record side.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use serde_json::Value;

use crate::read::{Read, ReadError};

/// One `<config_home>/sessions/<pid>.json` (Appendix A.7), reduced to what liveness needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRecord {
    pub pid: u32,
    /// `ps -o lstart=` text, or the all-digit legacy form; `None` when absent or not text.
    pub proc_start: Option<String>,
    /// `startedAt`, epoch ms.
    pub started_at_ms: Option<i64>,
    pub kind: Option<String>,
}

/// A record nested deeper than this is malformed (§12.6 "deep nesting").
const MAX_DEPTH: usize = 32;

fn depth(v: &Value) -> usize {
    match v {
        Value::Array(a) => 1 + a.iter().map(depth).max().unwrap_or(0),
        Value::Object(o) => 1 + o.values().map(depth).max().unwrap_or(0),
        _ => 0,
    }
}

/// `pid` as a process id `kill` may be given: 1 through `i32::MAX`. Zero and negative values
/// would address a process group, so they are out of range, never a pid to probe.
fn pid_of(v: Option<&Value>) -> Result<u32, String> {
    let Some(v) = v else {
        return Err("it has no pid".into());
    };
    let Value::Number(n) = v else {
        return Err("its pid is not an integer".into());
    };
    if let Some(p) = n.as_u64() {
        return u32::try_from(p)
            .ok()
            .filter(|p| (1..=i32::MAX as u32).contains(p))
            .ok_or_else(|| "its pid is out of range".to_owned());
    }
    let text = n.to_string();
    let integral = text
        .strip_prefix('-')
        .unwrap_or(&text)
        .bytes()
        .all(|b| b.is_ascii_digit());
    if integral {
        Err("its pid is out of range".into())
    } else {
        Err("its pid is not an integer".into())
    }
}

/// §12.6: non-object JSON, a missing or non-integer or out-of-range `pid`, depth > 32, or
/// invalid UTF-8 is `Err(detail)`; unknown fields are ignored. `detail` never quotes the bytes.
pub fn parse_session_record(bytes: &[u8]) -> Result<SessionRecord, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "it is not UTF-8".to_owned())?;
    let v: Value = serde_json::from_str(text).map_err(|_| "it is not JSON".to_owned())?;
    let Value::Object(o) = &v else {
        return Err("it is not a JSON object".into());
    };
    if depth(&v) > MAX_DEPTH {
        return Err(format!("it is nested deeper than {MAX_DEPTH} levels"));
    }
    let pid = pid_of(o.get("pid"))?;
    let proc_start = match o.get("procStart") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    };
    Ok(SessionRecord {
        pid,
        proc_start,
        started_at_ms: o.get("startedAt").and_then(Value::as_i64),
        kind: o.get("kind").and_then(Value::as_str).map(str::to_owned),
    })
}

#[derive(Debug, Clone, PartialEq)]
pub enum RecordEntry {
    Record(SessionRecord),
    /// Present but not a readable record (§12.6: it counts as unreadable). `detail` never
    /// quotes the file's bytes.
    Unreadable {
        path: PathBuf,
        detail: String,
    },
}

/// Every `*.json` in `dir`, in name order. A missing `dir` is `Present(vec![])`; a `dir` that
/// cannot be listed is `Unreadable`. A record that vanishes between the listing and its read
/// has been removed by its session's exit, so it is skipped.
pub fn read_session_records(dir: &Path) -> Read<Vec<RecordEntry>> {
    let unreadable =
        |e: io::Error| Read::Unreadable(ReadError::new(dir.display().to_string(), e.to_string()));
    let listing = match fs::read_dir(dir) {
        Ok(l) => l,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Read::Present(vec![]),
        Err(e) => return unreadable(e),
    };
    let mut paths = Vec::new();
    for entry in listing {
        match entry {
            Ok(e) => {
                let path = e.path();
                if path.extension().is_some_and(|x| x == "json") {
                    paths.push(path);
                }
            }
            // A listing that fails part-way may hide a record.
            Err(e) => return unreadable(e),
        }
    }
    paths.sort();
    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        match fs::read(&path) {
            Ok(bytes) => out.push(match parse_session_record(&bytes) {
                Ok(r) => RecordEntry::Record(r),
                Err(detail) => RecordEntry::Unreadable { path, detail },
            }),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => out.push(RecordEntry::Unreadable {
                path,
                detail: e.to_string(),
            }),
        }
    }
    Read::Present(out)
}

const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// `s` as a number of `min..=max` ASCII digits; no sign, no spaces.
fn digits(s: &str, min: usize, max: usize) -> Option<u32> {
    let ok = (min..=max).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit());
    ok.then(|| s.parse().ok()).flatten()
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        2 if is_leap(y) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days from 1970-01-01 to the proleptic Gregorian date `y-m-d` (H. Hinnant's
/// `days_from_civil`). `m` is 1–12 and `d` is valid for the month.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `ps -o lstart=` with `LC_ALL=C TZ=UTC` (`Thu Oct  1 12:34:56 2026`) to epoch seconds. Runs of
/// spaces separate the fields, as `ps` pads a single-digit day. The weekday name must be one of
/// the seven but is not checked against the date. `None` for anything else.
pub fn parse_lstart(s: &str) -> Option<i64> {
    let fields: Vec<&str> = s.split_whitespace().collect();
    let [weekday, month, day, time, year] = fields.as_slice() else {
        return None;
    };
    if !WEEKDAYS.contains(weekday) {
        return None;
    }
    let month = MONTHS.iter().position(|m| m == month)? as u32 + 1;
    let year = i64::from(digits(year, 4, 4)?);
    if year < 1970 {
        return None;
    }
    let day = digits(day, 1, 2)?;
    if day == 0 || day > days_in_month(year, month) {
        return None;
    }
    let hms: Vec<&str> = time.split(':').collect();
    let [h, m, sec] = hms.as_slice() else {
        return None;
    };
    let (h, m, sec) = (digits(h, 2, 2)?, digits(m, 2, 2)?, digits(sec, 2, 2)?);
    if h > 23 || m > 59 || sec > 59 {
        return None;
    }
    Some(
        days_from_civil(year, month, day) * 86_400
            + i64::from(h) * 3_600
            + i64::from(m) * 60
            + i64::from(sec),
    )
}

/// §4.2's process port: the OS facts §12.6 judges a session record by. Every answer is
/// `None` when it cannot be determined, which `record_is_live` counts as live.
pub trait ProcessProbe: Send + Sync {
    /// `kill(pid, 0)`: `Some(true)` on success or `EPERM`, `Some(false)` on `ESRCH`, `None` otherwise.
    fn exists(&self, pid: u32) -> Option<bool>;
    /// The process's start time in epoch seconds.
    fn start_time_s(&self, pid: u32) -> Option<i64>;
    /// Linux `/proc/<pid>/stat` field 22, for the all-digit legacy `procStart`.
    fn start_ticks(&self, pid: u32) -> Option<u64>;
    /// Whether the executable name or the arguments contain `needle`.
    fn mentions(&self, pid: u32, needle: &str) -> Option<bool>;
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

/// The real OS: `kill(2)`, then `proc_pidinfo` and `sysctl(KERN_PROCARGS2)` on macOS, or
/// `/proc` on Linux.
pub struct SystemProcessProbe;

impl ProcessProbe for SystemProcessProbe {
    fn exists(&self, pid: u32) -> Option<bool> {
        let pid = libc::pid_t::try_from(pid).ok().filter(|p| *p > 0)?;
        // SAFETY: signal 0 sends nothing; `kill` only checks that `pid` exists and may be
        // signalled. `pid` is positive, so it names one process, never a group.
        let rc = unsafe { libc::kill(pid, 0) };
        if rc == 0 {
            return Some(true);
        }
        match io::Error::last_os_error().raw_os_error() {
            Some(libc::EPERM) => Some(true),
            Some(libc::ESRCH) => Some(false),
            _ => None,
        }
    }

    #[cfg(target_os = "macos")]
    fn start_time_s(&self, pid: u32) -> Option<i64> {
        let micros = crate::process::start_of(pid).ok()??;
        i64::try_from(micros / 1_000_000).ok()
    }

    #[cfg(target_os = "linux")]
    fn start_time_s(&self, pid: u32) -> Option<i64> {
        let ticks = crate::process::start_of(pid).ok()??;
        let hz = clock_ticks_per_s()?;
        Some(boot_time_s()? + i64::try_from(ticks / hz).ok()?)
    }

    #[cfg(target_os = "macos")]
    fn start_ticks(&self, _pid: u32) -> Option<u64> {
        None
    }

    #[cfg(target_os = "linux")]
    fn start_ticks(&self, pid: u32) -> Option<u64> {
        crate::process::start_of(pid).ok()?
    }

    #[cfg(target_os = "macos")]
    fn mentions(&self, pid: u32, needle: &str) -> Option<bool> {
        let words = process_words(pid)?;
        Some(words.iter().any(|w| contains(w, needle.as_bytes())))
    }

    #[cfg(target_os = "linux")]
    fn mentions(&self, pid: u32, needle: &str) -> Option<bool> {
        let needle = needle.as_bytes();
        let comm = fs::read(format!("/proc/{pid}/comm")).ok();
        let cmdline = fs::read(format!("/proc/{pid}/cmdline")).ok();
        match (comm, cmdline) {
            (Some(c), Some(a)) => Some(contains(&c, needle) || contains(&a, needle)),
            (Some(one), None) | (None, Some(one)) if contains(&one, needle) => Some(true),
            _ => None,
        }
    }
}

/// `btime` in `/proc/stat`: the boot time, epoch seconds.
#[cfg(target_os = "linux")]
fn boot_time_s() -> Option<i64> {
    let stat = fs::read_to_string("/proc/stat").ok()?;
    stat.lines()
        .find_map(|l| l.strip_prefix("btime "))?
        .trim()
        .parse()
        .ok()
}

/// `sysconf(_SC_CLK_TCK)`: the unit of `/proc/<pid>/stat` field 22.
#[cfg(target_os = "linux")]
fn clock_ticks_per_s() -> Option<u64> {
    // SAFETY: `sysconf` only reads a configuration value.
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    u64::try_from(hz).ok().filter(|h| *h > 0)
}

/// The executable path and the arguments, from `sysctl(KERN_PROCARGS2)`. `None` when they
/// cannot be read: another user's process, a zombie, or a pid that is gone.
#[cfg(target_os = "macos")]
fn process_words(pid: u32) -> Option<Vec<Vec<u8>>> {
    let pid = libc::c_int::try_from(pid).ok()?;
    let mut argmax: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
    // SAFETY: `mib` names a two-level integer sysctl, `argmax` is a writable `c_int` and
    // `size` holds its size; nothing is written back to the kernel.
    let rc = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            2,
            (&raw mut argmax).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    let len = usize::try_from(argmax).ok().filter(|n| rc == 0 && *n > 0)?;
    let mut buf = vec![0u8; len];
    let mut size = buf.len();
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    // SAFETY: `buf` is writable for `size` bytes; `sysctl` writes at most that many and
    // stores the count it wrote in `size`.
    let rc = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buf.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return None;
    }
    parse_procargs(&buf[..size.min(len)])
}

/// Splits a `KERN_PROCARGS2` buffer: a native-endian `int` argc, the executable path, NUL
/// padding, then `argc` NUL-terminated arguments. The environment after them is never read: a
/// variable such as `CLAUDE_CONFIG_DIR` must not count as mentioning the launch command.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_procargs(buf: &[u8]) -> Option<Vec<Vec<u8>>> {
    let n = std::mem::size_of::<i32>();
    let argc = i32::from_ne_bytes(buf.get(..n)?.try_into().ok()?);
    let argc = usize::try_from(argc).ok()?;
    let rest = &buf[n..];
    let end = rest.iter().position(|b| *b == 0)?;
    let mut words = vec![rest[..end].to_vec()];
    let mut rest = &rest[end..];
    let start = rest.iter().position(|b| *b != 0).unwrap_or(rest.len());
    rest = &rest[start..];
    for _ in 0..argc {
        if rest.is_empty() {
            break;
        }
        let end = rest.iter().position(|b| *b == 0).unwrap_or(rest.len());
        words.push(rest[..end].to_vec());
        rest = rest.get(end + 1..).unwrap_or(&[]);
    }
    Some(words)
}

/// One process as `FakeProcessProbe` answers for it. Every field defaults to `None`, "cannot be
/// determined"; `exists: None` therefore makes a record live whatever else is set.
#[derive(Debug, Clone, Default)]
pub struct FakeProcess {
    pub exists: Option<bool>,
    pub start_time_s: Option<i64>,
    pub start_ticks: Option<u64>,
    /// `mentions`' answer, whatever the needle.
    pub mentions_launch: Option<bool>,
}

/// Tests only (no feature gate: it touches nothing). Unknown pids are `exists: Some(false)`.
#[derive(Default)]
pub struct FakeProcessProbe {
    procs: Mutex<BTreeMap<u32, FakeProcess>>,
}

impl FakeProcessProbe {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&self, pid: u32, p: FakeProcess) {
        self.procs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(pid, p);
    }

    fn get(&self, pid: u32) -> Option<FakeProcess> {
        self.procs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&pid)
            .cloned()
    }
}

impl ProcessProbe for FakeProcessProbe {
    fn exists(&self, pid: u32) -> Option<bool> {
        match self.get(pid) {
            Some(p) => p.exists,
            None => Some(false),
        }
    }

    fn start_time_s(&self, pid: u32) -> Option<i64> {
        self.get(pid)?.start_time_s
    }

    fn start_ticks(&self, pid: u32) -> Option<u64> {
        self.get(pid)?.start_ticks
    }

    fn mentions(&self, pid: u32, _needle: &str) -> Option<bool> {
        self.get(pid)?.mentions_launch
    }
}

/// How long after `startedAt` a process must have started, by cswap's rule, before it can be
/// judged a recycled pid (§12.6).
const RECYCLE_GRACE_MS: i64 = 120_000;

/// §12.6: live if the pid exists and still belongs to the record's writer. `launch_command`
/// is the provider's (`claude`), used by the heuristic for an absent `procStart` and, under
/// Decision 16, to judge an `lstart` mismatch (recycled only if the process does not mention it).
/// Anything that cannot be determined counts as live.
pub fn record_is_live(probe: &dyn ProcessProbe, r: &SessionRecord, launch_command: &str) -> bool {
    match probe.exists(r.pid) {
        Some(false) => return false,
        None => return true,
        Some(true) => {}
    }
    let proc_start = r.proc_start.as_deref().map(str::trim).unwrap_or("");
    if let Some(recorded) = parse_lstart(proc_start) {
        // The OS start time equal to the record's within ±1 s, or unknown, is live. A mismatch
        // alone is no proof (Decision 16): a Linux start time moves with the wall clock, so it
        // is recycled only when the process does not mention the launch command either. An
        // unknown answer counts as mentioning it.
        let matches = probe
            .start_time_s(r.pid)
            .is_none_or(|actual| (actual - recorded).abs() <= 1);
        return matches || probe.mentions(r.pid, launch_command).unwrap_or(true);
    }
    if !proc_start.is_empty() && proc_start.bytes().all(|b| b.is_ascii_digit()) {
        return match (proc_start.parse::<u64>().ok(), probe.start_ticks(r.pid)) {
            (Some(recorded), Some(actual)) => actual == recorded,
            _ => true,
        };
    }
    // Absent or unparseable: recycled only if the process started more than 120 s after
    // `startedAt` and neither its executable name nor its arguments mention the launch command.
    let started_late = match (r.started_at_ms, probe.start_time_s(r.pid)) {
        // Whole seconds round the start down, so this never calls a process late early.
        (Some(at_ms), Some(start_s)) => {
            start_s.saturating_mul(1000) > at_ms.saturating_add(RECYCLE_GRACE_MS)
        }
        _ => false,
    };
    !started_late || probe.mentions(r.pid, launch_command) != Some(false)
}
```

In `crates/tagteam-provider/src/lib.rs`, after `pub use keychain::{FakeKeychain, Keychain, KeychainError, LockState};`, add:

```rust
pub use liveness::{
    FakeProcess, FakeProcessProbe, ProcessProbe, RecordEntry, SessionRecord, SystemProcessProbe,
    parse_lstart, parse_session_record, read_session_records, record_is_live,
};
```

- [ ] **Step 4: Run them and see them pass**, then the crate's whole suite

Run: `cargo test -p tagteam-provider --lib liveness`
Expected: PASS, 18 tests.

Run: `cargo test -p tagteam-provider`
Expected: PASS.

On macOS, also compile the Linux branch: `cargo clippy -p tagteam-provider --all-targets --target x86_64-unknown-linux-gnu -- -D warnings`
Expected: no warnings. CI's Linux job runs the real-process tests there.

- [ ] **Step 5: Format and lint**

```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-provider/src/liveness.rs crates/tagteam-provider/src/process.rs crates/tagteam-provider/src/lib.rs
git commit -m "Judge a session record live by its writer's start time, through a process probe"
```

#### Cycle 2: the lock probe

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-provider/src/flock.rs`, `mod tests`, before
`the_mutation_guard_lives_in_the_data_dir`, add:

```rust
    #[test]
    fn a_probe_never_creates_a_missing_lock_file() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join(".tagteam-launch/42.lock");
        assert_eq!(probe_lock(&p).unwrap(), LockProbe::Missing);
        assert!(!p.parent().unwrap().exists(), "nor its directory");
        fs::create_dir(p.parent().unwrap()).unwrap();
        assert_eq!(probe_lock(&p).unwrap(), LockProbe::Missing);
        assert!(!p.exists());
    }

    #[test]
    fn a_probe_sees_a_held_lock_and_releases_a_free_one() {
        let _fork = crate::FORK_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("42.lock");
        let held = FlockGuard::try_lock(&p).unwrap().unwrap();
        assert_eq!(probe_lock(&p).unwrap(), LockProbe::Held);
        drop(held);
        fs::set_permissions(&p, fs::Permissions::from_mode(0o400)).unwrap();
        assert_eq!(
            probe_lock(&p).unwrap(),
            LockProbe::Free,
            "read-only is enough"
        );
        fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(
            FlockGuard::try_lock(&p).unwrap().is_some(),
            "the probe released what it took"
        );
    }

    #[test]
    fn a_probe_that_cannot_open_the_path_is_an_error() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("not-a-dir");
        fs::write(&file, b"").unwrap();
        assert!(probe_lock(&file.join("42.lock")).is_err());
    }
```

`a_probe_sees_a_held_lock_and_releases_a_free_one` drops a lock and then expects it free, so it
holds `FORK_GUARD`, as the existing re-lock tests do: a child forked in that window would briefly
hold a duplicate of the lock.

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-provider --lib flock`
Expected: compile errors: E0425 for `probe_lock`, E0433 for `LockProbe`.

- [ ] **Step 3: Implement**

In `crates/tagteam-provider/src/flock.rs`, after `impl FlockGuard { … }` (line 66), add:

```rust
/// What a non-blocking test of a lock file found (§12.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockProbe {
    /// No file at the path.
    Missing,
    /// The file exists and nothing holds its lock.
    Free,
    /// Another open file description holds its lock.
    Held,
}

/// Tests `path` with a non-blocking `flock`, opening it read-only and never creating it. A
/// reservation is only ever tested, never waited on (§12.5), so a lock this takes is released
/// before it returns.
pub fn probe_lock(path: &Path) -> io::Result<LockProbe> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(LockProbe::Missing),
        Err(e) => return Err(e),
    };
    // SAFETY: `file` owns a valid descriptor for the duration of the call.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        // SAFETY: as above; this releases the lock the probe just took.
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
        return Ok(LockProbe::Free);
    }
    let e = io::Error::last_os_error();
    if e.raw_os_error() == Some(libc::EWOULDBLOCK) {
        Ok(LockProbe::Held)
    } else {
        Err(e)
    }
}
```

In `crates/tagteam-provider/src/lib.rs`, line 26 becomes:

```rust
pub use flock::{FlockGuard, LockProbe, MutationGuard, probe_lock};
```

- [ ] **Step 4: Run them and see them pass**, then the crate's whole suite

Run: `cargo test -p tagteam-provider --lib flock`
Expected: PASS, five tests.

Run: `cargo test -p tagteam-provider`, then `cargo clippy -p tagteam-provider --all-targets --target x86_64-unknown-linux-gnu -- -D warnings`
Expected: PASS; no warnings.

- [ ] **Step 5: Format and lint**

```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-provider/src/flock.rs crates/tagteam-provider/src/lib.rs
git commit -m "Probe a launch reservation's lock without creating or waiting on it"
```

---

### Task 7: Profile files and the `Provider` session methods (CC, FakeAgent)

Everything below the engine that a session profile needs:
- `tagteam-provider` learns where a profile lives (§5), reads and writes tagteam's three files in
  it (the marker, the seed and the links record), lists its launch reservations with Task 6's
  probe, and matches share-list patterns.
- `Env` gains `vars`, the provider-owned variables a registry asks for (Decision 5).
- The `Provider` trait gains the twelve session methods of the Interface Contract, which both
  providers implement:
  - Claude Code from §12.2's tables and its existing live-store paths, run against a profile
    `Env`.
  - FakeAgent with deliberately different shapes: one home variable instead of two, records in
    `procs/`, no normalization of the spelling, and nothing outside the directory to delete.

Nothing calls the new methods yet. Task 8 detects the run shell with `session_dir` and the marker,
Task 9 reads reservations and records, and Task 13 consumes `share_policy`.

**Readings of the spec this task commits to:**
- **The two files the spec gives no shape.** The seed is
  `{"format": "tagteam-seed", "version": 1, "loginEpoch", "seedFp", "needsBootstrap"}`, and the
  links record is
  `{"format": "tagteam-links", "version": 1, "links": {<name>: <source>}, "notedUnknown": [<name>]}`.
  The marker has the spec's shape. All three are pretty-printed with a trailing newline, written
  atomically at 0600, with the profile directory created 0700 (§5). A seed written before
  `needsBootstrap` existed reads it as `false`.
- **Absent against unreadable** (§4.3, §12.8). No file, or a profile path that is not a directory
  (`ENOTDIR`), is `Absent`: no directory holds the marker there, so that is not a run shell. Any
  other read error, or bytes that are not valid UTF-8 JSON in the right envelope and fields, is
  `Unreadable`, naming the file with a detail that never quotes its bytes (`rescue.rs`'s pattern).
- **Marker validity.** `provider` and `accountId` are non-empty strings. `configDir` is an
  absolute path with no NUL, since it is exported as an environment variable. `outer` is an
  object; its contents are the provider's to check in `apply_outer_home`. Unknown fields are
  ignored.
- **A links record never names a path outside the profile.** Every name is one path component
  (not empty, `.` or `..`, with no `/` or NUL), and every source is absolute. A record naming
  `../x` would otherwise make Task 13's sync remove a path outside the profile, so it is
  unreadable instead. A source path that is not UTF-8 cannot be written as JSON: `write` refuses
  with `InvalidInput` and writes nothing.
- **`launch_reservations`.** Only `*.lock` files count, in name order. A listing error or a probe
  error makes the whole read unreadable, since it may hide a live session (§10.3). A reservation
  that vanished between the listing and its probe is reported `Missing`, which is not held.
- **`entry_matches`.** Each `*` matches any run of characters, possibly empty, so
  `policy-limits.json*` also matches `policy-limits.json`. The contract's doc comment and
  Decision 14 say so, and its own example `.*_auth_refresh-*` has two stars, as does the
  `CC_PRIVATE` entry of the same text.
- **`Env.vars`** is filled only by `capture_vars`; `for_test` and `from_process` leave it empty, and
  Task 8's CLI calls `capture_vars` in `Context::from_process`. A name that is no longer in the process environment drops an
  earlier value.
- **Claude Code's outer record** holds both variables: `null` when undefined, and their text when
  defined, `""` included (Appendix A.1). A value that is not UTF-8 is recorded as the same lossy
  text that Keychain naming already hashes. `apply_outer_home` refuses anything but a string or
  `null` under each key. It names the key, never the value, and changes nothing else in the
  `Env`.
- **The profile `Env`** sets `CLAUDE_CONFIG_DIR` to the recorded spelling and drops
  `CLAUDE_SECURESTORAGE_CONFIG_DIR`, as `run`'s scrub does (§12.5). So the profile's item is
  `"Claude Code-credentials-" + hex(sha256(NFC(spelling)))[..8]` whatever the outer environment
  holds, and its `.credentials.json` and `.claude.json` resolve inside the profile.
- **`delete_profile_credential`** deletes the OAuth item and the managed-key item for the
  spelling, each verified `Absent` with the existence probe (Appendix A.3). An absent item counts
  as deleted. An item that survives its delete, or a probe that cannot say, is `ShadowingItem`.
  There is no fence: a profile is never under CC's live locks, and Task 9's caller holds
  `MutationGuard` and the account lock. On Linux it does nothing: the profile's
  `.credentials.json` goes with the directory.
- **The twelve trait methods are all required**, with no default bodies. Every provider must say
  what its sessions are, and `capabilities().sessions` says whether the engine may use them.

**Files:**
- Create: `crates/tagteam-provider/src/profile.rs`
- Create: `crates/tagteam-cc/src/session.rs`
- Modify: `crates/tagteam-provider/src/lib.rs` (`pub mod profile;` after `pub mod process;`; the
  `profile` re-exports; `EntryKind`, `MustShare` and `SharePolicy` in the `provider` re-export,
  lines 37–42)
- Modify: `crates/tagteam-provider/src/env.rs` (imports, lines 1–2; `Env.vars`, after line 15;
  both constructors, lines 26–37 and 49–58; `capture_vars` and `var` after `for_test`)
- Modify: `crates/tagteam-provider/src/provider.rs` (import, line 4; the `ShadowingItem` doc
  comment, lines 304–306; `EntryKind`, `MustShare` and `SharePolicy` before `Capabilities`,
  line 136; the twelve methods at the end of `trait Provider`, after line 580)
- Modify: `crates/tagteam-cc/src/lib.rs` (`mod session;` after `pub mod provider;`)
- Modify: `crates/tagteam-cc/src/live.rs` (`LiveStore::delete_items`, before `snapshot`)
- Modify: `crates/tagteam-cc/src/provider.rs` (imports, lines 1, 9–14 and 23; the twelve methods
  at the end of `impl Provider for ClaudeCode`, after line 481)
- Modify: `crates/tagteam-fake/src/paths.rs` (`HOME_VAR`; `FakePaths::resolve`, lines 17–26)
- Modify: `crates/tagteam-fake/src/provider.rs` (imports, lines 14–19 and 23; `profile_env`;
  `capabilities`, lines 174–180; the twelve methods at the end of `impl Provider for FakeAgent`,
  after line 550)
- Test: `crates/tagteam-provider/src/profile.rs` and `crates/tagteam-provider/src/env.rs`
  (in-module), `crates/tagteam-cc/tests/session.rs` (new), `crates/tagteam-fake/tests/provider.rs`

**Interfaces:**
- Consumes:
  - Task 6: `LockProbe`, `probe_lock(path: &Path) -> io::Result<LockProbe>`
  - Task 5: `LiveStore::remove_item(&self, env: &Env, kind: ItemKind, fence: Fence<'_>) -> Result<(), ProviderError>`
  - Existing: `atomic::{ensure_private_dir, write_atomic_private}`, `Fingerprint::parse`,
    `CcPaths::resolve`, `config::live_identity`, `paths::nfc`, `LiveStore::read_credential`,
    `FakePaths::resolve`, `FakeAgent::{read_live_auth, live_identity}`, `tagteam_fake::login`
- Produces (Interface Contract, `tagteam-provider`, `tagteam-cc` and `tagteam-fake`):
  - `profile.rs`, re-exported: `MARKER_FILE`, `SEED_FILE`, `LINKS_FILE`, `LAUNCH_DIR`,
    `profile_path(env: &Env, id: &AccountId) -> PathBuf`,
    `canonical_profile_path(profile: &Path) -> io::Result<PathBuf>`,
    `ProfileMarker { provider, account_id, config_dir, outer }` with `read`/`write`,
    `Seed { login_epoch, seed_fp, needs_bootstrap }` with `read`/`write`,
    `LinksRecord { links, noted_unknown }` with `read`/`write`,
    `launch_reservations(profile: &Path) -> Read<Vec<(PathBuf, LockProbe)>>`,
    `entry_matches(pattern: &str, name: &str) -> bool`,
    `RunShell { Outside, Inside { profile, marker }, Unreadable { marker, detail } }`
  - `Env.vars: BTreeMap<String, OsString>`, `Env::capture_vars(&mut self, names: &[&str])`,
    `Env::var(&self, name: &str) -> Option<&OsStr>`
  - `EntryKind { Dir, File }`, `MustShare { name, kind }`,
    `SharePolicy { source, shared, must_share, private }`, re-exported
  - `Provider::{launch_command, session_dir_var, session_dir, outer_home, apply_outer_home, profile_spelling, share_policy, session_records_dir, read_profile_credential, profile_identity, delete_profile_credential, invoked_by}`
  - `tagteam_cc::session::{CC_SHARED, CC_MUST_SHARE, CC_PRIVATE, outer_home, apply_outer_home, profile_env}`
    (`pub(crate)`)
  - `LiveStore::delete_items(&self, env: &Env) -> Result<(), ProviderError>` (`pub(crate)`)
  - FakeAgent: `FakePaths::resolve` honours `FAKEAGENT_HOME`; `capabilities().sessions` is `true`

**Spec:**
- §5: the profile is `$XDG_DATA_HOME/tagteam/sessions/<id>/`, holding the marker, the seed and
  the links record; directories 0700, files 0600.
- §12.2:
  - "One spelling": the canonical path, NFC, recorded in the marker and used by every profile
    credential operation.
  - The marker's fields.
  - The shared, must-share and private tables.
  - Unknown entries, noted once in `.tagteam-links.json`.
- §12.5:
  - The seed's login epoch and seed fingerprint.
  - Every profile credential operation resolves paths with the session environment, without
    `CLAUDE_SECURESTORAGE_CONFIG_DIR`.
- §12.8: the marker makes a process a run shell; a present but invalid marker is unreadable.
- §4.5 "Parallel sessions": the provider supplies the launch command, the outer home, the share
  policy and the records.
- §8.1, §12.3 step 2: a profile's credential is read the way CC reads it, from the hashed item
  for the recorded spelling, then `<profile>/.credentials.json`.
- §10.3: `remove` deletes the profile's hashed item, named from its recorded spelling.
- §13.5: a process CC invoked has `CLAUDECODE` or `CLAUDE_CONFIG_DIR`.
- Appendix A.1: an empty `CLAUDE_CONFIG_DIR` is unset; a defined `CLAUDE_SECURESTORAGE_CONFIG_DIR`
  decides the secure-storage dir.
- Appendix A.2: the hashed name.
- Appendix A.3: the existence probe.

#### Cycle 1: the profile files

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam-provider/src/profile.rs` holding only its test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::PermissionsExt;

    use crate::flock::FlockGuard;

    fn marker() -> ProfileMarker {
        ProfileMarker {
            provider: ProviderId::new("claude-code"),
            account_id: AccountId::from_string("0192"),
            config_dir: "/data/tagteam/sessions/0192".into(),
            outer: json!({"CLAUDE_CONFIG_DIR": null, "CLAUDE_SECURESTORAGE_CONFIG_DIR": ""}),
        }
    }

    fn seed() -> Seed {
        Seed {
            login_epoch: 3,
            seed_fp: Fingerprint::of_secret(b"rt-1").as_str().to_owned(),
            needs_bootstrap: true,
        }
    }

    fn mode(p: &Path) -> u32 {
        fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    fn file_json(p: &Path) -> Value {
        serde_json::from_slice(&fs::read(p).unwrap()).unwrap()
    }

    /// Every `(case, bytes)` must read as `Unreadable` naming the file, never quoting it.
    fn assert_all_unreadable<T>(
        name: &str,
        cases: Vec<(&str, Vec<u8>)>,
        read: fn(&Path) -> Read<T>,
    ) {
        for (case, bytes) in cases {
            let d = tempfile::tempdir().unwrap();
            let path = d.path().join(name);
            fs::write(&path, &bytes).unwrap();
            match read(d.path()) {
                Read::Unreadable(e) => {
                    assert_eq!(e.what, path.display().to_string(), "{case}");
                    assert!(!e.detail.contains("SENTINEL"), "{case}: {}", e.detail);
                }
                other => panic!("{case}: {other:?}"),
            }
        }
    }

    /// `base` with `key` set to `v`, or removed when `v` is `None`, as bytes.
    fn with(base: &Value, key: &str, v: Option<Value>) -> Vec<u8> {
        let mut b = base.clone();
        match v {
            Some(v) => b[key] = v,
            None => {
                b.as_object_mut().unwrap().remove(key);
            }
        }
        b.to_string().into_bytes()
    }

    #[test]
    fn a_profile_lives_under_the_data_dir_by_account_id() {
        let env = Env::for_test(Path::new("/tmp/fixture"));
        assert_eq!(
            profile_path(&env, &AccountId::from_string("0192")),
            Path::new("/tmp/fixture/home/.local/share/tagteam/sessions/0192")
        );
    }

    #[test]
    fn the_canonical_path_resolves_links_and_needs_the_directory() {
        let d = tempfile::tempdir().unwrap();
        let real = d.path().join("real");
        fs::create_dir(&real).unwrap();
        let link = d.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_eq!(
            canonical_profile_path(&link).unwrap(),
            fs::canonicalize(&real).unwrap()
        );
        assert!(canonical_profile_path(&d.path().join("missing")).is_err());
    }

    #[test]
    fn a_marker_round_trips_privately_in_the_spec_s_shape() {
        let d = tempfile::tempdir().unwrap();
        let profile = profile_path(&Env::for_test(d.path()), &AccountId::from_string("0192"));
        assert!(
            matches!(ProfileMarker::read(&profile), Read::Absent),
            "no directory"
        );
        marker().write(&profile).unwrap();
        assert_eq!(mode(&profile), 0o700);
        assert_eq!(mode(&profile.join(MARKER_FILE)), 0o600);
        assert!(matches!(ProfileMarker::read(&profile), Read::Present(m) if m == marker()));
        let v = file_json(&profile.join(MARKER_FILE));
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "format",
                "version",
                "provider",
                "accountId",
                "configDir",
                "outer"
            ]
        );
        assert_eq!(
            v,
            json!({
                "format": "tagteam-profile", "version": 1, "provider": "claude-code",
                "accountId": "0192", "configDir": "/data/tagteam/sessions/0192",
                "outer": {"CLAUDE_CONFIG_DIR": null, "CLAUDE_SECURESTORAGE_CONFIG_DIR": ""}
            })
        );
    }

    #[test]
    fn a_marker_is_absent_without_its_file_and_ignores_unknown_fields() {
        let d = tempfile::tempdir().unwrap();
        assert!(matches!(ProfileMarker::read(d.path()), Read::Absent));
        let file = d.path().join("not-a-dir");
        fs::write(&file, b"").unwrap();
        assert!(
            matches!(ProfileMarker::read(&file), Read::Absent),
            "a path that is not a directory holds no marker"
        );
        let mut v = json!({
            "format": "tagteam-profile", "version": 1, "provider": "fake-agent",
            "accountId": "0193", "configDir": "/p", "outer": {}, "later": [1]
        });
        fs::write(d.path().join(MARKER_FILE), v.to_string()).unwrap();
        let Read::Present(m) = ProfileMarker::read(d.path()) else {
            panic!("a valid marker");
        };
        assert_eq!(
            (
                m.provider.as_str(),
                m.account_id.as_str(),
                m.config_dir.as_str()
            ),
            ("fake-agent", "0193", "/p")
        );
        v["outer"] = json!({"FAKEAGENT_HOME": "/h"});
        fs::write(d.path().join(MARKER_FILE), v.to_string()).unwrap();
        assert!(
            matches!(ProfileMarker::read(d.path()), Read::Present(m) if m.outer == json!({"FAKEAGENT_HOME": "/h"}))
        );
    }

    #[test]
    fn a_marker_that_is_not_valid_is_unreadable_and_never_quoted() {
        let base = json!({
            "format": "tagteam-profile", "version": 1, "provider": "claude-code",
            "accountId": "0192", "configDir": "/p", "outer": {}
        });
        let s = |v: &str| Some(json!(v));
        assert_all_unreadable(
            MARKER_FILE,
            vec![
                ("not JSON", b"SENTINEL".to_vec()),
                ("not an object", br#"["SENTINEL"]"#.to_vec()),
                ("another format", with(&base, "format", s("SENTINEL"))),
                ("version 2", with(&base, "version", Some(json!(2)))),
                ("a string version", with(&base, "version", s("1"))),
                ("no provider", with(&base, "provider", None)),
                ("an empty provider", with(&base, "provider", s(""))),
                ("no accountId", with(&base, "accountId", None)),
                (
                    "a numeric accountId",
                    with(&base, "accountId", Some(json!(192))),
                ),
                ("no configDir", with(&base, "configDir", None)),
                (
                    "a relative configDir",
                    with(&base, "configDir", s("SENTINEL/p")),
                ),
                ("no outer", with(&base, "outer", None)),
                ("a string outer", with(&base, "outer", s("SENTINEL"))),
                ("invalid UTF-8", b"{\"format\": \"\xff SENTINEL\"}".to_vec()),
            ],
            ProfileMarker::read,
        );
        let d = tempfile::tempdir().unwrap();
        fs::create_dir(d.path().join(MARKER_FILE)).unwrap();
        assert!(
            matches!(ProfileMarker::read(d.path()), Read::Unreadable(_)),
            "a directory where the marker belongs"
        );
    }

    #[test]
    fn a_seed_round_trips_and_an_older_one_needs_no_bootstrap() {
        let d = tempfile::tempdir().unwrap();
        assert!(matches!(Seed::read(d.path()), Read::Absent));
        seed().write(d.path()).unwrap();
        assert_eq!(mode(&d.path().join(SEED_FILE)), 0o600);
        assert!(matches!(Seed::read(d.path()), Read::Present(s) if s == seed()));
        let without = with(
            &file_json(&d.path().join(SEED_FILE)),
            "needsBootstrap",
            None,
        );
        fs::write(d.path().join(SEED_FILE), without).unwrap();
        assert!(matches!(Seed::read(d.path()), Read::Present(s) if !s.needs_bootstrap));
    }

    #[test]
    fn a_seed_that_is_not_valid_is_unreadable() {
        let base = json!({
            "format": "tagteam-seed", "version": 1, "loginEpoch": 3,
            "seedFp": Fingerprint::of_secret(b"rt-1").as_str(), "needsBootstrap": false
        });
        assert_all_unreadable(
            SEED_FILE,
            vec![
                ("not JSON", b"{SENTINEL".to_vec()),
                (
                    "a marker",
                    with(&base, "format", Some(json!("tagteam-profile"))),
                ),
                ("version 2", with(&base, "version", Some(json!(2)))),
                ("no loginEpoch", with(&base, "loginEpoch", None)),
                (
                    "a string loginEpoch",
                    with(&base, "loginEpoch", Some(json!("3"))),
                ),
                (
                    "a fractional loginEpoch",
                    with(&base, "loginEpoch", Some(json!(3.5))),
                ),
                ("no seedFp", with(&base, "seedFp", None)),
                (
                    "a seedFp that is no fingerprint",
                    with(&base, "seedFp", Some(json!("sha256:SENTINEL"))),
                ),
                (
                    "a string needsBootstrap",
                    with(&base, "needsBootstrap", Some(json!("SENTINEL"))),
                ),
            ],
            Seed::read,
        );
    }

    #[test]
    fn a_links_record_round_trips_and_starts_empty() {
        let d = tempfile::tempdir().unwrap();
        assert!(matches!(LinksRecord::read(d.path()), Read::Absent));
        LinksRecord::default().write(d.path()).unwrap();
        assert!(
            matches!(LinksRecord::read(d.path()), Read::Present(r) if r == LinksRecord::default())
        );
        let record = LinksRecord {
            links: BTreeMap::from([
                ("projects".to_owned(), PathBuf::from("/h/.claude/projects")),
                ("CLAUDE.md".to_owned(), PathBuf::from("/dotfiles/CLAUDE.md")),
            ]),
            noted_unknown: BTreeSet::from(["new-feature".to_owned()]),
        };
        record.write(d.path()).unwrap();
        assert_eq!(mode(&d.path().join(LINKS_FILE)), 0o600);
        assert!(matches!(LinksRecord::read(d.path()), Read::Present(r) if r == record));
        assert_eq!(
            file_json(&d.path().join(LINKS_FILE)),
            json!({
                "format": "tagteam-links", "version": 1,
                "links": {"CLAUDE.md": "/dotfiles/CLAUDE.md", "projects": "/h/.claude/projects"},
                "notedUnknown": ["new-feature"]
            })
        );
    }

    #[test]
    fn a_links_record_never_names_a_path_outside_the_profile() {
        let base = json!({
            "format": "tagteam-links", "version": 1, "links": {"projects": "/h/p"},
            "notedUnknown": ["x"]
        });
        assert_all_unreadable(
            LINKS_FILE,
            vec![
                ("not JSON", b"SENTINEL".to_vec()),
                ("version 2", with(&base, "version", Some(json!(2)))),
                (
                    "links not an object",
                    with(&base, "links", Some(json!(["SENTINEL"]))),
                ),
                (
                    "a dot-dot name",
                    with(&base, "links", Some(json!({"..": "/h/SENTINEL"}))),
                ),
                (
                    "a nested name",
                    with(&base, "links", Some(json!({"a/SENTINEL": "/h/p"}))),
                ),
                (
                    "an empty name",
                    with(&base, "links", Some(json!({"": "/h/p"}))),
                ),
                (
                    "a relative source",
                    with(&base, "links", Some(json!({"p": "SENTINEL"}))),
                ),
                (
                    "a numeric source",
                    with(&base, "links", Some(json!({"p": 7}))),
                ),
                ("no notedUnknown", with(&base, "notedUnknown", None)),
                (
                    "a non-string noted name",
                    with(&base, "notedUnknown", Some(json!([7]))),
                ),
                (
                    "a noted dot",
                    with(&base, "notedUnknown", Some(json!(["."]))),
                ),
            ],
            LinksRecord::read,
        );
    }

    #[test]
    fn a_links_record_whose_source_is_not_utf8_is_never_written() {
        let d = tempfile::tempdir().unwrap();
        let record = LinksRecord {
            links: BTreeMap::from([(
                "projects".to_owned(),
                PathBuf::from(OsStr::from_bytes(b"/h/\xff")),
            )]),
            noted_unknown: BTreeSet::new(),
        };
        let err = record.write(d.path()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(!d.path().join(LINKS_FILE).exists());
    }

    #[test]
    fn launch_reservations_are_every_lock_file_probed_in_name_order() {
        let _fork = crate::FORK_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let d = tempfile::tempdir().unwrap();
        assert!(matches!(launch_reservations(d.path()), Read::Present(v) if v.is_empty()));
        let dir = d.path().join(LAUNCH_DIR);
        fs::create_dir(&dir).unwrap();
        let _held = FlockGuard::try_lock(&dir.join("200.lock"))
            .unwrap()
            .unwrap();
        fs::write(dir.join("100.lock"), b"").unwrap();
        fs::write(dir.join("notes.txt"), b"").unwrap();
        let Read::Present(found) = launch_reservations(d.path()) else {
            panic!("the directory lists");
        };
        assert_eq!(
            found,
            vec![
                (dir.join("100.lock"), LockProbe::Free),
                (dir.join("200.lock"), LockProbe::Held)
            ]
        );
        assert!(!dir.join("300.lock").exists(), "probing creates nothing");
    }

    #[test]
    fn a_launch_directory_that_cannot_be_listed_is_unreadable() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join(LAUNCH_DIR), b"").unwrap();
        assert!(matches!(launch_reservations(d.path()), Read::Unreadable(_)));
    }

    #[test]
    fn share_patterns_match_exactly_or_across_each_star() {
        for (pattern, name, matches) in [
            ("sessions", "sessions", true),
            ("sessions", "sessions2", false),
            ("sessions", "session", false),
            ("*.lock", "daemon.lock", true),
            ("*.lock", ".lock", true),
            ("*.lock", "daemon.lock.owner", false),
            ("*.lock.owner", ".oauth_refresh.lock.owner", true),
            ("daemon*", "daemon", true),
            ("daemon*", "daemon.json", true),
            ("daemon*", "xdaemon", false),
            ("daemon.*", "daemon", false),
            ("daemon.*", "daemon.status.json", true),
            (".*_auth_refresh-*", ".oauth_auth_refresh-123", true),
            (".*_auth_refresh-*", "._auth_refresh-", true),
            (".*_auth_refresh-*", ".oauth_refresh.lock", false),
            (".claude-*-oauth.json", ".claude-staging-oauth.json", true),
            (".claude-*-oauth.json", ".claude.json", false),
            ("policy-limits.json*", "policy-limits.json", true),
            ("policy-limits.json*", "policy-limits.json.signature", true),
            (".tagteam-*", ".tagteam-profile.json", true),
            (".tagteam-*", ".tagteam", false),
            ("a*a", "a", false),
            ("a*a", "aa", true),
            ("a*b*a", "aba", true),
            ("a*b*a", "ab", false),
            ("*", "anything", true),
            ("", "", true),
            ("", "x", false),
        ] {
            assert_eq!(entry_matches(pattern, name), matches, "{pattern} vs {name}");
        }
    }
}
```

and declare it in `crates/tagteam-provider/src/lib.rs`, after `pub mod process;`:

```rust
pub mod profile;
```

`launch_reservations_are_every_lock_file_probed_in_name_order` holds `FORK_GUARD` while it holds a
reservation's lock, as `flock.rs`'s tests do.

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-provider --lib profile`
Expected: compile errors only, about 130 of them:
- For the names Step 3 defines: `profile_path`, `canonical_profile_path`, `launch_reservations`,
  `entry_matches`, `ProfileMarker`, `Seed`, `LinksRecord`, `MARKER_FILE`, `SEED_FILE`,
  `LINKS_FILE` and `LAUNCH_DIR`.
- For those its imports bring in: `json!`, `Value`, `Read`, `Env`, `LockProbe`, `AccountId`,
  `ProviderId`, `Fingerprint`, `BTreeMap`, `BTreeSet`, `Path`, `PathBuf`, `fs` and `io`.

- [ ] **Step 3: Implement**

In `crates/tagteam-provider/src/profile.rs`, above the test module, add:

```rust
//! §12.2's session profile on disk, provider-neutral: where a profile lives, tagteam's three
//! files in it (the marker, the seed and the links record), its launch reservations, and the
//! share-list matcher. What a provider shares, and how it spells and reads a profile, come
//! from the `Provider` session methods.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use tagteam_core::{AccountId, Fingerprint, ProviderId};

use crate::atomic::{ensure_private_dir, write_atomic_private};
use crate::env::Env;
use crate::flock::{LockProbe, probe_lock};
use crate::read::{Read, ReadError};

pub const MARKER_FILE: &str = ".tagteam-profile.json";
pub const SEED_FILE: &str = ".tagteam-seed.json";
pub const LINKS_FILE: &str = ".tagteam-links.json";
pub const LAUNCH_DIR: &str = ".tagteam-launch";

const MARKER_FORMAT: &str = "tagteam-profile";
const SEED_FORMAT: &str = "tagteam-seed";
const LINKS_FORMAT: &str = "tagteam-links";
const VERSION: i64 = 1;

/// `<data_dir>/sessions/<id>` (§5).
pub fn profile_path(env: &Env, id: &AccountId) -> PathBuf {
    env.data_dir().join("sessions").join(id.as_str())
}

/// `realpath` of an existing profile directory (§12.2 "One spelling", before the provider's
/// own normalization).
pub fn canonical_profile_path(profile: &Path) -> io::Result<PathBuf> {
    fs::canonicalize(profile)
}

/// One of tagteam's files in `profile`: `Absent` when there is no such file, or `profile` is not
/// a directory at all; `Unreadable` when it exists but cannot be read, or `parse` refuses it.
/// `parse`'s detail never quotes the file's bytes.
fn read_own_file<T>(
    profile: &Path,
    name: &str,
    parse: impl FnOnce(&Value) -> Result<T, String>,
) -> Read<T> {
    let path = profile.join(name);
    let unreadable =
        |detail: String| Read::Unreadable(ReadError::new(path.display().to_string(), detail));
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Read::Absent;
        }
        Err(e) => return unreadable(e.to_string()),
    };
    let Ok(v) = serde_json::from_slice::<Value>(&bytes) else {
        return unreadable("it is not JSON".into());
    };
    match parse(&v) {
        Ok(t) => Read::Present(t),
        Err(detail) => unreadable(detail),
    }
}

/// Writes one of tagteam's files in `profile`: atomic, 0600, creating `profile` 0700 if absent.
fn write_own_file(profile: &Path, name: &str, v: &Value) -> io::Result<()> {
    ensure_private_dir(profile)?;
    let mut bytes = serde_json::to_vec_pretty(v).expect("a Value always serializes");
    bytes.push(b'\n');
    write_atomic_private(&profile.join(name), &bytes, 0o600)
}

fn check_envelope(v: &Value, format: &str, what: &str) -> Result<(), String> {
    if v["format"].as_str() == Some(format) && v["version"].as_i64() == Some(VERSION) {
        Ok(())
    } else {
        Err(format!("it is not a version 1 tagteam {what}"))
    }
}

fn non_empty<'v>(v: &'v Value, key: &str, missing: &str) -> Result<&'v str, String> {
    v[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| missing.to_owned())
}

/// A name tagteam may record for a profile entry: one path component, never `.` or `..`.
/// Anything else in a links record could make link sync touch a path outside the profile.
fn is_entry_name(s: &str) -> bool {
    !s.is_empty() && s != "." && s != ".." && !s.contains('/') && !s.contains('\0')
}

/// What makes a directory a run shell's profile (§12.2, §12.8). It holds no secret.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileMarker {
    pub provider: ProviderId,
    pub account_id: AccountId,
    /// The exported spelling (§12.2).
    pub config_dir: String,
    /// The provider's record of the outer home (§4.5 `outer_home`).
    pub outer: Value,
}

impl ProfileMarker {
    fn parse(v: &Value) -> Result<Self, String> {
        check_envelope(v, MARKER_FORMAT, "profile marker")?;
        let provider = non_empty(v, "provider", "it names no provider")?;
        let account = non_empty(v, "accountId", "it names no account")?;
        let config_dir = v["configDir"]
            .as_str()
            .filter(|s| s.starts_with('/') && !s.contains('\0'))
            .ok_or_else(|| "its configDir is not an absolute path".to_owned())?;
        let outer = v
            .get("outer")
            .filter(|o| o.is_object())
            .ok_or_else(|| "it has no outer record".to_owned())?;
        Ok(Self {
            provider: ProviderId::new(provider),
            account_id: AccountId::from_string(account),
            config_dir: config_dir.to_owned(),
            outer: outer.clone(),
        })
    }

    /// `Absent`: no marker file. `Unreadable`: present but not a valid marker (§12.8).
    pub fn read(profile: &Path) -> Read<ProfileMarker> {
        read_own_file(profile, MARKER_FILE, Self::parse)
    }

    /// Atomic, 0600; creates `profile` 0700 if absent.
    pub fn write(&self, profile: &Path) -> io::Result<()> {
        let v = json!({
            "format": MARKER_FORMAT,
            "version": VERSION,
            "provider": self.provider.as_str(),
            "accountId": self.account_id.as_str(),
            "configDir": self.config_dir,
            "outer": self.outer,
        });
        write_own_file(profile, MARKER_FILE, &v)
    }
}

/// §12.5's profile provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seed {
    /// The login epoch the profile was bootstrapped under (§12.5).
    pub login_epoch: i64,
    /// The generation fingerprint the profile last agreed on with the vault.
    pub seed_fp: String,
    /// Set by M4b's per-launch check when validation reported `invalid` (§12.3).
    pub needs_bootstrap: bool,
}

impl Seed {
    fn parse(v: &Value) -> Result<Self, String> {
        check_envelope(v, SEED_FORMAT, "seed")?;
        let login_epoch = v["loginEpoch"]
            .as_i64()
            .ok_or_else(|| "it has no integer loginEpoch".to_owned())?;
        let seed_fp = v["seedFp"]
            .as_str()
            .filter(|s| Fingerprint::parse(s).is_some())
            .ok_or_else(|| "its seedFp is not a fingerprint".to_owned())?;
        let needs_bootstrap = match v.get("needsBootstrap") {
            None => false,
            Some(Value::Bool(b)) => *b,
            Some(_) => return Err("its needsBootstrap is not a boolean".into()),
        };
        Ok(Self {
            login_epoch,
            seed_fp: seed_fp.to_owned(),
            needs_bootstrap,
        })
    }

    pub fn read(profile: &Path) -> Read<Seed> {
        read_own_file(profile, SEED_FILE, Self::parse)
    }

    pub fn write(&self, profile: &Path) -> io::Result<()> {
        let v = json!({
            "format": SEED_FORMAT,
            "version": VERSION,
            "loginEpoch": self.login_epoch,
            "seedFp": self.seed_fp,
            "needsBootstrap": self.needs_bootstrap,
        });
        write_own_file(profile, SEED_FILE, &v)
    }
}

/// §12.2's record of what link sync did in a profile.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LinksRecord {
    /// Entry name → the fully resolved source the link points at, for every link tagteam made.
    pub links: BTreeMap<String, PathBuf>,
    /// Unknown entries already noted once (§12.2).
    pub noted_unknown: BTreeSet<String>,
}

impl LinksRecord {
    fn parse(v: &Value) -> Result<Self, String> {
        check_envelope(v, LINKS_FORMAT, "links record")?;
        let Some(links) = v["links"].as_object() else {
            return Err("its links are not an object".into());
        };
        let mut out = LinksRecord::default();
        for (name, source) in links {
            if !is_entry_name(name) {
                return Err("it records a link name that is not one path component".into());
            }
            let source = source
                .as_str()
                .filter(|s| s.starts_with('/') && !s.contains('\0'))
                .ok_or_else(|| {
                    "it records a link source that is not an absolute path".to_owned()
                })?;
            out.links.insert(name.clone(), PathBuf::from(source));
        }
        let Some(noted) = v["notedUnknown"].as_array() else {
            return Err("its notedUnknown is not a list".into());
        };
        for name in noted {
            match name.as_str() {
                Some(n) if is_entry_name(n) => {
                    out.noted_unknown.insert(n.to_owned());
                }
                _ => return Err("it notes an entry name that is not one path component".into()),
            }
        }
        Ok(out)
    }

    /// `Absent`: no record yet, so the caller starts from `LinksRecord::default()`.
    pub fn read(profile: &Path) -> Read<LinksRecord> {
        read_own_file(profile, LINKS_FILE, Self::parse)
    }

    /// Refuses, writing nothing, when a source path is not UTF-8: JSON cannot hold it.
    pub fn write(&self, profile: &Path) -> io::Result<()> {
        let mut links = Map::new();
        for (name, source) in &self.links {
            let source = source.to_str().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "a linked source path is not UTF-8",
                )
            })?;
            links.insert(name.clone(), Value::String(source.to_owned()));
        }
        let v = json!({
            "format": LINKS_FORMAT,
            "version": VERSION,
            "links": links,
            "notedUnknown": self.noted_unknown,
        });
        write_own_file(profile, LINKS_FILE, &v)
    }
}

/// `<profile>/.tagteam-launch/*.lock`, each probed with `probe_lock`. A missing directory is
/// `Present(vec![])`. A directory that cannot be listed, or a reservation that cannot be
/// probed, makes the whole read `Unreadable`: it may hide a live session (§10.3).
pub fn launch_reservations(profile: &Path) -> Read<Vec<(PathBuf, LockProbe)>> {
    let dir = profile.join(LAUNCH_DIR);
    let unreadable = |what: &Path, e: io::Error| {
        Read::Unreadable(ReadError::new(what.display().to_string(), e.to_string()))
    };
    let listing = match fs::read_dir(&dir) {
        Ok(l) => l,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Read::Present(vec![]),
        Err(e) => return unreadable(&dir, e),
    };
    let mut paths = Vec::new();
    for entry in listing {
        match entry {
            Ok(e) => {
                let path = e.path();
                if path.extension().is_some_and(|x| x == "lock") {
                    paths.push(path);
                }
            }
            Err(e) => return unreadable(&dir, e),
        }
    }
    paths.sort();
    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        match probe_lock(&path) {
            Ok(state) => out.push((path, state)),
            Err(e) => return unreadable(&path, e),
        }
    }
    Read::Present(out)
}

/// A share-list entry against a directory entry name: exact, or with each `*` matching any run
/// of characters, possibly empty (`*.lock`, `daemon*`, `.*_auth_refresh-*`).
pub fn entry_matches(pattern: &str, name: &str) -> bool {
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or("");
    let Some(mut rest) = name.strip_prefix(first) else {
        return false;
    };
    let parts: Vec<&str> = parts.collect();
    let Some((last, middle)) = parts.split_last() else {
        return rest.is_empty();
    };
    // Each middle piece at its earliest place leaves the most room for the rest.
    for part in middle {
        match rest.find(part) {
            Some(i) => rest = &rest[i + part.len()..],
            None => return false,
        }
    }
    rest.ends_with(last)
}

/// §12.8: where a process stands.
#[derive(Debug, Clone, PartialEq)]
pub enum RunShell {
    Outside,
    Inside {
        profile: PathBuf,
        marker: ProfileMarker,
    },
    Unreadable {
        marker: PathBuf,
        detail: String,
    },
}
```

In `crates/tagteam-provider/src/lib.rs`, after `pub use process::ProcessStamp;`, add:

```rust
pub use profile::{
    LAUNCH_DIR, LINKS_FILE, LinksRecord, MARKER_FILE, ProfileMarker, RunShell, SEED_FILE, Seed,
    canonical_profile_path, entry_matches, launch_reservations, profile_path,
};
```

- [ ] **Step 4: Run them and see them pass**, then the crate's whole suite

Run: `cargo test -p tagteam-provider --lib profile`
Expected: PASS, 13 tests.

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
git add crates/tagteam-provider/src/profile.rs crates/tagteam-provider/src/lib.rs
git commit -m "Read and write a profile's marker, seed and links record"
```

#### Cycle 2: provider variables and the session methods

The trait's new methods break both implementations until both have them, so this cycle is one
commit across the three crates.

- [ ] **Step 1: Write the failing tests**

**`crates/tagteam-provider/src/env.rs`**, in `mod tests`, before
`a_run_shell_is_detected_from_claude_config_dir`, add:

```rust
    #[test]
    fn provider_variables_are_captured_only_when_asked_for() {
        const NEVER: &str = "TAGTEAM_TEST_NEVER_SET_7F3A";
        let mut env = Env::for_test(Path::new("/tmp/fixture"));
        assert!(env.vars.is_empty());
        assert_eq!(env.var("HOME"), None, "nothing is captured until asked");
        env.capture_vars(&["HOME", NEVER]);
        assert_eq!(env.var("HOME"), std::env::var_os("HOME").as_deref());
        assert_eq!(env.var(NEVER), None);
        assert!(
            !env.vars.contains_key(NEVER),
            "an unset variable is not inserted"
        );
        env.vars.insert(NEVER.into(), "stale".into());
        env.capture_vars(&[NEVER]);
        assert_eq!(
            env.var(NEVER),
            None,
            "a variable gone from the process is dropped"
        );
        assert!(Env::from_process().vars.is_empty());
    }
```

It changes no process environment: `HOME` is always set under test (`Env::for_test` requires it),
and the other name is never set.

**Create `crates/tagteam-cc/tests/session.rs`:**

```rust
//! Claude Code's session facts (§4.5 "Parallel sessions", §12): the share policy, the outer
//! home, the profile spelling, and the profile credential's read and deletion under the
//! hashed Keychain name for a recorded spelling (Appendix A.2).

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use sha2::{Digest, Sha256};
use tagteam_cc::live::{LiveStore, Platform};
use tagteam_cc::{CcPaths, ClaudeCode, keychain_account};
use tagteam_core::AccountId;
use tagteam_provider::{
    EntryKind, Env, FakeKeychain, Keychain, MustShare, Provenance, Provider, ProviderError, Read,
    canonical_profile_path, entry_matches, profile_path,
};

struct Fx {
    _d: tempfile::TempDir,
    env: Env,
    kc: Arc<FakeKeychain>,
    cc: ClaudeCode,
}

fn fx_on(platform: Platform) -> Fx {
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    fs::create_dir_all(env.home.join(".claude")).unwrap();
    let kc = Arc::new(FakeKeychain::new());
    let cc = ClaudeCode::with_store(
        LiveStore::new(kc.clone(), platform).with_retry_delay(Duration::ZERO),
    );
    Fx { _d: d, env, kc, cc }
}

fn fx() -> Fx {
    fx_on(Platform::MacOs)
}

/// `"<prefix>-" + hex(sha256(spelling))[..8]` (Appendix A.2), computed here independently of
/// the crate's naming code. `spelling` is already NFC, as `profile_spelling` makes it.
fn hashed(prefix: &str, spelling: &str) -> String {
    format!(
        "{prefix}-{}",
        &hex::encode(Sha256::digest(spelling.as_bytes()).as_slice())[..8]
    )
}

/// A profile directory under the fixture's data dir, and the spelling it is exported as.
fn profile(f: &Fx, id: &str) -> (PathBuf, String) {
    let dir = profile_path(&f.env, &AccountId::from_string(id));
    fs::create_dir_all(&dir).unwrap();
    let spelling =
        f.cc.profile_spelling(&canonical_profile_path(&dir).unwrap());
    (dir, spelling)
}

fn with_vars(env: &Env, config: Option<&str>, secure: Option<&str>) -> Env {
    let mut e = env.clone();
    e.claude_config_dir = config.map(OsString::from);
    e.claude_securestorage_config_dir = secure.map(OsString::from);
    e
}

const ENTRY: &[u8] = br#"{"claudeAiOauth":{"refreshToken":"rt-profile"}}"#;

#[test]
fn claude_code_s_session_facts() {
    let f = fx();
    assert_eq!(f.cc.launch_command(), "claude");
    assert_eq!(f.cc.session_dir_var(), Some("CLAUDE_CONFIG_DIR"));
    assert_eq!(f.cc.session_dir(&f.env), None);
    assert_eq!(
        f.cc.session_dir(&with_vars(&f.env, Some(""), None)),
        None,
        "an empty value is unset (Appendix A.1)"
    );
    assert_eq!(
        f.cc.session_dir(&with_vars(&f.env, Some("/p/0192"), Some(""))),
        Some(PathBuf::from("/p/0192"))
    );
    assert_eq!(
        f.cc.session_records_dir(Path::new("/p/0192")),
        Path::new("/p/0192/sessions")
    );
}

#[test]
fn the_share_policy_is_section_12_2_s_tables_from_the_resolved_home() {
    let f = fx();
    let policy = f.cc.share_policy(&f.env);
    assert_eq!(policy.source, f.env.home.join(".claude"));
    let custom = with_vars(&f.env, Some("/custom/home"), None);
    assert_eq!(
        f.cc.share_policy(&custom).source,
        CcPaths::resolve(&custom).config_home
    );
    assert_eq!(
        policy.must_share,
        vec![
            MustShare {
                name: "projects",
                kind: EntryKind::Dir
            },
            MustShare {
                name: "history.jsonl",
                kind: EntryKind::File
            },
        ]
    );
    assert_eq!(
        policy.shared,
        [
            "CLAUDE.md",
            "settings.json",
            "keybindings.json",
            "agents",
            "commands",
            "skills",
            "plugins",
            "hooks",
            "output-styles",
            "themes",
            "rules",
            "workflows",
            "file-history",
            "paste-cache",
            "shell-snapshots",
            "session-env"
        ]
    );
    let private = |name: &str| policy.private.iter().any(|p| entry_matches(p, name));
    // Every row of §12.2's private table, spelled as CC 2.1.286 writes it.
    for name in [
        ".credentials.json",
        ".claude.json",
        ".claude-custom-oauth.json",
        ".claude-local-oauth.json",
        ".claude-staging-oauth.json",
        ".config.json",
        "sessions",
        "ide",
        "jobs",
        "daemon",
        "daemon.json",
        "daemon.lock",
        "daemon.log",
        "daemon.status.json",
        "daemon.scheduled.status.json",
        "daemon-auth-cooldown",
        "daemon-auth-status.json",
        "backups",
        "cache",
        "mcp-needs-auth-cache.json",
        "stats-cache.json",
        "policy-limits.json",
        "policy-limits.json.signature",
        "policy-limits.json.stamp",
        "remote-settings.json",
        "remote-settings-consent.json",
        "remote-settings-helper-consent",
        ".session_ingress_token",
        "hfi-auth.json",
        "state",
        "seed-admin",
        "bridge-spawn",
        "chrome",
        "debug",
        "feedback",
        "routines",
        "settings.local.json",
        ".last-cleanup",
        ".cc-writes",
        ".device-keys.json",
        ".oauth_refresh.lock",
        ".oauth_refresh.lock.owner",
        ".storage-write",
        ".tagteam-profile.json",
        ".tagteam-launch",
    ] {
        assert!(private(name), "{name} is private");
    }
    for name in policy
        .shared
        .iter()
        .chain(policy.must_share.iter().map(|m| &m.name))
    {
        assert!(!private(name), "{name} is both shared and private");
    }
}

#[test]
fn the_outer_home_round_trips_undefined_set_and_defined_but_empty() {
    let f = fx();
    // In a run shell, CLAUDE_CONFIG_DIR names the profile and the secure-storage dir is scrubbed.
    let inside = with_vars(&f.env, Some("/data/sessions/0192"), None);
    for (config, secure) in [
        (None, None),
        (Some("/custom/home"), None),
        (None, Some("")),
        (Some("/custom/home"), Some("")),
        (Some(""), Some("/secure")),
    ] {
        let outer_env = with_vars(&f.env, config, secure);
        let outer = f.cc.outer_home(&outer_env);
        assert_eq!(
            outer,
            json!({"CLAUDE_CONFIG_DIR": config, "CLAUDE_SECURESTORAGE_CONFIG_DIR": secure}),
            "{config:?}, {secure:?}"
        );
        let restored = f.cc.apply_outer_home(&inside, &outer).unwrap();
        assert_eq!(
            (
                restored.claude_config_dir,
                restored.claude_securestorage_config_dir
            ),
            (
                outer_env.claude_config_dir.clone(),
                outer_env.claude_securestorage_config_dir.clone()
            ),
            "{config:?}, {secure:?}"
        );
        assert_eq!(restored.home, inside.home, "nothing else moves");
    }
}

#[test]
fn an_outer_record_that_is_not_claude_code_s_is_refused_without_quoting_it() {
    let f = fx();
    for outer in [
        json!("SENTINEL"),
        json!({}),
        json!({"CLAUDE_CONFIG_DIR": null}),
        json!({"CLAUDE_CONFIG_DIR": "SENTINEL", "CLAUDE_SECURESTORAGE_CONFIG_DIR": 7}),
        json!({"FAKEAGENT_HOME": "SENTINEL"}),
    ] {
        match f.cc.apply_outer_home(&f.env, &outer) {
            Err(ProviderError::Invalid(msg)) => assert!(!msg.contains("SENTINEL"), "{msg}"),
            other => panic!("{outer}: {other:?}"),
        }
    }
}

#[test]
fn the_spelling_is_the_nfc_of_the_canonical_path() {
    let f = fx();
    assert_eq!(
        f.cc.profile_spelling(Path::new("/p/cafe\u{301}")),
        "/p/caf\u{e9}"
    );
    let (dir, spelling) = profile(&f, "0192");
    assert_eq!(
        Path::new(&spelling),
        fs::canonicalize(&dir).unwrap(),
        "absolute, resolved, no trailing slash"
    );
}

#[test]
fn the_profile_credential_is_read_from_the_hashed_item_of_its_spelling() {
    let f = fx();
    let (dir, spelling) = profile(&f, "0192");
    let acct = keychain_account(&f.env);
    let item = hashed("Claude Code-credentials", &spelling);
    assert!(
        matches!(
            f.cc.read_profile_credential(&f.env, &spelling),
            Read::Absent
        ),
        "a new profile has none"
    );
    f.kc.put(&item, &acct, ENTRY);
    let c =
        f.cc.read_profile_credential(&f.env, &spelling)
            .present()
            .unwrap();
    assert_eq!((c.bytes(), c.provenance()), (ENTRY, Provenance::Fresh));
    // §12.5: the profile env drops the outer secure-storage dir, so it plays no part.
    let outer = with_vars(&f.env, Some("/elsewhere"), Some(""));
    assert_eq!(
        f.cc.read_profile_credential(&outer, &spelling)
            .present()
            .unwrap()
            .bytes(),
        ENTRY
    );
    // Another spelling of the same directory names another item (Appendix A.2).
    assert!(matches!(
        f.cc.read_profile_credential(&f.env, &format!("{spelling}/")),
        Read::Absent
    ));
    // Then `<profile>/.credentials.json`: it covers an unreadable item only as a degraded read.
    f.kc.set_unreadable(&item, &acct, true);
    assert!(matches!(
        f.cc.read_profile_credential(&f.env, &spelling),
        Read::Unreadable(_)
    ));
    fs::write(dir.join(".credentials.json"), b"file").unwrap();
    let c =
        f.cc.read_profile_credential(&f.env, &spelling)
            .present()
            .unwrap();
    assert_eq!(
        (c.bytes(), c.provenance()),
        (&b"file"[..], Provenance::Degraded)
    );
    f.kc.set_unreadable(&item, &acct, false);
    f.kc.delete(&item, &acct).unwrap();
    let c =
        f.cc.read_profile_credential(&f.env, &spelling)
            .present()
            .unwrap();
    assert_eq!(
        (c.bytes(), c.provenance()),
        (&b"file"[..], Provenance::Fresh)
    );
}

#[test]
fn on_linux_the_profile_credential_is_its_file_alone() {
    let f = fx_on(Platform::Linux);
    let (dir, spelling) = profile(&f, "0192");
    f.kc.put(
        &hashed("Claude Code-credentials", &spelling),
        &keychain_account(&f.env),
        ENTRY,
    );
    assert!(matches!(
        f.cc.read_profile_credential(&f.env, &spelling),
        Read::Absent
    ));
    fs::write(dir.join(".credentials.json"), b"file").unwrap();
    assert_eq!(
        f.cc.read_profile_credential(&f.env, &spelling)
            .present()
            .unwrap()
            .bytes(),
        b"file"
    );
}

#[test]
fn the_profile_identity_is_the_profile_s_own_oauth_account() {
    let f = fx();
    let (dir, spelling) = profile(&f, "0192");
    fs::write(
        f.env.home.join(".claude.json"),
        json!({"oauthAccount": {"emailAddress": "default@x.co"}}).to_string(),
    )
    .unwrap();
    assert!(matches!(
        f.cc.profile_identity(&f.env, &spelling),
        Read::Absent
    ));
    fs::write(
        dir.join(".claude.json"),
        json!({"oauthAccount": {"emailAddress": "p@x.co", "organizationUuid": "org-1"}})
            .to_string(),
    )
    .unwrap();
    let id = f.cc.profile_identity(&f.env, &spelling).present().unwrap();
    assert_eq!(
        (id.email.as_deref(), id.org_uuid.as_str()),
        (Some("p@x.co"), "org-1")
    );
    fs::write(dir.join(".claude.json"), b"{\"oauthAccount\": {").unwrap();
    assert!(matches!(
        f.cc.profile_identity(&f.env, &spelling),
        Read::Unreadable(_)
    ));
}

#[test]
fn deleting_a_profile_credential_removes_and_verifies_both_items_of_its_spelling_only() {
    let f = fx();
    let (dir, spelling) = profile(&f, "0192");
    let acct = keychain_account(&f.env);
    let oauth = hashed("Claude Code-credentials", &spelling);
    let managed = hashed("Claude Code", &spelling);
    let older = hashed("Claude Code-credentials", "/old/data/tagteam/sessions/0192");
    f.kc.put(&oauth, &acct, ENTRY);
    f.kc.put(&managed, &acct, b"sk-ant-api03-profile");
    f.kc.put(&older, &acct, b"older spelling");
    f.kc.put("Claude Code-credentials", &acct, b"default home");
    fs::write(dir.join(".credentials.json"), b"file").unwrap();
    f.cc.delete_profile_credential(&f.env, &spelling).unwrap();
    assert_eq!(
        (f.kc.get(&oauth, &acct), f.kc.get(&managed, &acct)),
        (None, None)
    );
    assert_eq!(f.kc.get(&older, &acct).unwrap(), b"older spelling");
    assert_eq!(
        f.kc.get("Claude Code-credentials", &acct).unwrap(),
        b"default home"
    );
    assert!(
        dir.join(".credentials.json").exists(),
        "the file goes with the directory"
    );
    f.cc.delete_profile_credential(&f.env, &spelling)
        .expect("absent items are already deleted");
    // An item that will not go is reported, never assumed gone.
    f.kc.put(&oauth, &acct, ENTRY);
    f.kc.set_fail_delete(&oauth, true);
    match f.cc.delete_profile_credential(&f.env, &spelling) {
        Err(ProviderError::ShadowingItem(svc)) => assert_eq!(svc, oauth),
        other => panic!("{other:?}"),
    }
}

#[test]
fn deleting_a_profile_credential_on_linux_touches_nothing() {
    let f = fx_on(Platform::Linux);
    let (dir, spelling) = profile(&f, "0192");
    let acct = keychain_account(&f.env);
    let oauth = hashed("Claude Code-credentials", &spelling);
    f.kc.put(&oauth, &acct, ENTRY);
    fs::write(dir.join(".credentials.json"), b"file").unwrap();
    f.cc.delete_profile_credential(&f.env, &spelling).unwrap();
    assert_eq!(f.kc.get(&oauth, &acct).unwrap(), ENTRY);
    assert_eq!(fs::read(dir.join(".credentials.json")).unwrap(), b"file");
}

#[test]
fn claude_code_invoked_a_process_that_has_claudecode_1_or_a_config_dir() {
    let f = fx();
    let with_var = |name: &str, v: &str| {
        let mut e = f.env.clone();
        e.vars.insert(name.into(), v.into());
        e
    };
    assert!(!f.cc.invoked_by(&f.env));
    assert!(f.cc.invoked_by(&with_var("CLAUDECODE", "1")));
    assert!(!f.cc.invoked_by(&with_var("CLAUDECODE", "0")));
    assert!(!f.cc.invoked_by(&with_var("CLAUDECODE", "")));
    assert!(f.cc.invoked_by(&with_vars(&f.env, Some("/p"), None)));
    assert!(!f.cc.invoked_by(&with_vars(&f.env, Some(""), None)));
}
```

Its profiles live under the fixture's `data_dir()/sessions/`. Every Keychain item is in
`FakeKeychain`, and `hashed` computes Appendix A.2's name on its own, so a regression in
`keychain_service` cannot hide behind itself.

**`crates/tagteam-fake/tests/provider.rs`**: the `tagteam_provider` import (lines 15–18) becomes

```rust
use tagteam_provider::{
    Capabilities, Credential, EntryKind, Env, LiveChange, LockError, MustShare, MutationGuard,
    Pace, PollBudget, Provenance, Provider, ProviderError, Read, SecretStore, SharePolicy,
    StoredLogin, UsageResult, Window, Written,
};
```

In `kinds_capabilities_endpoints_and_surface`, the capability assertion (lines 111–118) becomes

```rust
    assert_eq!(
        f.fake.capabilities(),
        Capabilities {
            usage: true,
            refresh: true,
            sessions: true,
            ..Capabilities::default()
        }
    );
```

and at the end of the file, add:

```rust
/// `f.env` with `FAKEAGENT_HOME` set to `v`.
fn with_home(f: &Fx, v: &str) -> Env {
    let mut env = f.env.clone();
    env.vars.insert("FAKEAGENT_HOME".into(), v.into());
    env
}

#[test]
fn fakeagent_home_moves_its_state_and_an_empty_one_is_unset() {
    let f = fx();
    let home = f.env.home.join("elsewhere");
    let p = FakePaths::resolve(&with_home(&f, home.to_str().unwrap()));
    assert_eq!(
        (p.dir.clone(), p.credential, p.identity, p.lock),
        (
            home.clone(),
            home.join("credential.json"),
            home.join("identity.json"),
            home.join(".live.lock")
        )
    );
    assert_eq!(
        FakePaths::resolve(&with_home(&f, "")).dir,
        f.env.home.join(".fakeagent")
    );
    assert_eq!(
        FakePaths::resolve(&f.env).dir,
        f.env.home.join(".fakeagent")
    );
}

#[test]
fn its_session_facts_are_deliberately_unlike_claude_code_s() {
    let f = fx();
    assert_eq!(f.fake.launch_command(), "fakeagent");
    assert_eq!(f.fake.session_dir_var(), Some("FAKEAGENT_HOME"));
    assert_eq!(f.fake.session_dir(&f.env), None);
    assert_eq!(f.fake.session_dir(&with_home(&f, "")), None);
    assert_eq!(
        f.fake.session_dir(&with_home(&f, "/p/0193")),
        Some(std::path::PathBuf::from("/p/0193"))
    );
    assert_eq!(
        f.fake.session_records_dir(std::path::Path::new("/p/0193")),
        std::path::Path::new("/p/0193/procs")
    );
    assert_eq!(
        f.fake
            .profile_spelling(std::path::Path::new("/p/cafe\u{301}")),
        "/p/cafe\u{301}",
        "the path as it is: no NFC"
    );
    let mut cc_shaped = with_home(&f, "/p/0193");
    cc_shaped.vars.insert("CLAUDECODE".into(), "1".into());
    cc_shaped.claude_config_dir = Some("/p/0193".into());
    assert!(!f.fake.invoked_by(&cc_shaped));
    assert_eq!(
        f.fake.share_policy(&f.env),
        SharePolicy {
            source: f.env.home.join(".fakeagent"),
            shared: vec!["notes", "prefs.json"],
            must_share: vec![MustShare {
                name: "journal.log",
                kind: EntryKind::File
            }],
            private: vec![
                "credential.json",
                "identity.json",
                "procs",
                ".live.lock",
                ".tagteam-*"
            ],
        }
    );
}

#[test]
fn its_outer_home_round_trips_unset_set_and_empty() {
    let f = fx();
    let inside = with_home(&f, "/data/sessions/0193");
    for home in [None, Some("/h/.fakeagent-custom"), Some("")] {
        let outer_env = match home {
            Some(h) => with_home(&f, h),
            None => f.env.clone(),
        };
        let outer = f.fake.outer_home(&outer_env);
        assert_eq!(outer, json!({"FAKEAGENT_HOME": home}), "{home:?}");
        let restored = f.fake.apply_outer_home(&inside, &outer).unwrap();
        assert_eq!(
            restored.var("FAKEAGENT_HOME"),
            home.map(std::ffi::OsStr::new),
            "{home:?}"
        );
    }
    for outer in [
        json!({}),
        json!({"FAKEAGENT_HOME": 7}),
        json!({"CLAUDE_CONFIG_DIR": "SENTINEL"}),
    ] {
        match f.fake.apply_outer_home(&inside, &outer) {
            Err(ProviderError::Invalid(msg)) => assert!(!msg.contains("SENTINEL"), "{msg}"),
            other => panic!("{outer}: {other:?}"),
        }
    }
}

#[test]
fn its_profile_credential_and_identity_are_read_under_the_spelling() {
    let f = fx();
    let profile = f.env.data_dir().join("sessions/0193");
    fs::create_dir_all(&profile).unwrap();
    let spelling = profile.to_str().unwrap();
    login(&f.env, "alice", "ws", "tok-live", "renew-live");
    assert!(matches!(
        f.fake.read_profile_credential(&f.env, spelling),
        Read::Absent
    ));
    assert!(matches!(
        f.fake.profile_identity(&f.env, spelling),
        Read::Absent
    ));
    login(&with_home(&f, spelling), "bob", "ws2", "tok-p", "renew-p");
    let c = f
        .fake
        .read_profile_credential(&f.env, spelling)
        .present()
        .unwrap();
    assert_eq!(c.provenance(), Provenance::Fresh);
    assert_eq!(
        f.fake.fingerprint(c.bytes()),
        Some(tagteam_core::Fingerprint::of_secret(b"renew-p"))
    );
    let id = f.fake.profile_identity(&f.env, spelling).present().unwrap();
    assert_eq!(id.label, "bob@ws2");
    f.fake.delete_profile_credential(&f.env, spelling).unwrap();
    assert!(
        profile.join("credential.json").exists(),
        "FakeAgent keeps nothing outside the directory to delete"
    );
    fs::write(profile.join("identity.json"), b"{\"identity\": {").unwrap();
    assert!(matches!(
        f.fake.profile_identity(&f.env, spelling),
        Read::Unreadable(_)
    ));
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-provider --lib env`
Expected: compile errors: E0609 (no field `vars` on `Env`) and E0599 (no method `capture_vars`
or `var`).

Run: `cargo test -p tagteam-cc --test session`
Expected: compile errors:
- E0432: unresolved imports `tagteam_provider::EntryKind` and `MustShare`.
- E0599: no method `launch_command`, `session_dir_var`, `session_dir`, `session_records_dir`,
  `share_policy`, `outer_home`, `apply_outer_home`, `profile_spelling`,
  `read_profile_credential`, `profile_identity`, `delete_profile_credential` or `invoked_by` on
  `ClaudeCode`.
- E0609: no field `vars` on `Env`.

Run: `cargo test -p tagteam-fake --test provider`
Expected: compile errors of the same kinds for `FakeAgent`, plus E0609 for `Env.vars`.

- [ ] **Step 3: Implement**

**`crates/tagteam-provider/src/env.rs`.** The imports become

```rust
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
```

The struct becomes

```rust
/// Everything tagteam resolves paths from (§15.1). Tests build one with `for_test`.
#[derive(Debug, Clone)]
pub struct Env {
    pub home: PathBuf,
    pub user: Option<String>,
    pub xdg_config_home: Option<PathBuf>,
    pub xdg_data_home: Option<PathBuf>,
    pub xdg_state_home: Option<PathBuf>,
    /// The raw string, never canonicalized; `None` when unset.
    pub claude_config_dir: Option<OsString>,
    /// `None` when undefined; `Some("")` when defined but empty (Appendix A.1).
    pub claude_securestorage_config_dir: Option<OsString>,
    /// Provider-owned variables the registry asked for (§4.5 `session_dir_var`, `CLAUDECODE`),
    /// captured by `capture_vars`. Empty in `for_test`.
    pub vars: BTreeMap<String, OsString>,
    forbidden_root: Option<PathBuf>,
}
```

Both constructors gain `vars: BTreeMap::new(),` before `forbidden_root`, in `from_process`
(line 36) and in `for_test` (line 57). After `for_test`, add:

```rust
    /// Reads each named variable from the process environment into `vars` (absent: not
    /// inserted, and any earlier value dropped).
    pub fn capture_vars(&mut self, names: &[&str]) {
        for name in names {
            match std::env::var_os(name) {
                Some(v) => {
                    self.vars.insert((*name).to_owned(), v);
                }
                None => {
                    self.vars.remove(*name);
                }
            }
        }
    }

    pub fn var(&self, name: &str) -> Option<&OsStr> {
        self.vars.get(name).map(OsString::as_os_str)
    }
```

**`crates/tagteam-provider/src/provider.rs`.** Line 4 becomes `use std::path::{Path, PathBuf};`.
The `ShadowingItem` doc comment (as Task 5 left it) becomes:

```rust
    /// A Keychain item `remove_item` could not verify gone: after a file fallback, after a
    /// managed-key fallback, when a managed key is removed, or when a profile's credential is
    /// deleted. Claude Code may still read it (L397); the wording holds for a removal as much
    /// as for a write.
    #[error("the Keychain item {0} could not be verified gone, so Claude Code may still read it")]
    ShadowingItem(String),
```

Before `Capabilities` (line 136), add:

```rust
/// Whether a must-share entry is a directory or a file (§12.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Dir,
    File,
}

/// An entry every profile links, created empty in the source home when absent (§12.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MustShare {
    pub name: &'static str,
    pub kind: EntryKind,
}

/// §12.2: what a profile shares with the outer home.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharePolicy {
    /// The outer home's config dir (CC: the resolved default config home).
    pub source: PathBuf,
    /// Linked when present in `source`.
    pub shared: Vec<&'static str>,
    /// Created empty in `source` when absent, then linked; a real copy in a profile refuses.
    pub must_share: Vec<MustShare>,
    /// Never linked; patterns for `entry_matches`.
    pub private: Vec<&'static str>,
}
```

At the end of `trait Provider`, after `live_identity_source`, add:

```rust
    // §4.5 "Parallel sessions" (§12). Every profile operation takes the recorded spelling,
    // never one derived again (§12.2).
    /// The command a session runs (CC: `claude`).
    fn launch_command(&self) -> &'static str;
    /// The variable that names a profile (CC: `CLAUDE_CONFIG_DIR`); `None` without sessions.
    fn session_dir_var(&self) -> Option<&'static str>;
    /// The directory `env` points this provider at, if set and non-empty.
    fn session_dir(&self, env: &Env) -> Option<PathBuf>;
    /// §12.2: the record of the home `env` resolves to, stored as the marker's `outer`.
    fn outer_home(&self, env: &Env) -> Value;
    /// §12.8: `env` with this provider's home variables restored from `outer`. A record that
    /// is not this provider's shape is `Invalid`, naming no value.
    fn apply_outer_home(&self, env: &Env, outer: &Value) -> Result<Env, ProviderError>;
    /// §12.2: the exported spelling for a canonical profile path (CC: NFC of the path).
    fn profile_spelling(&self, canonical: &Path) -> String;
    /// §12.2: the source home and the share lists that link sync applies.
    fn share_policy(&self, env: &Env) -> SharePolicy;
    /// Where the profile's session records live (CC: `<profile>/sessions`).
    fn session_records_dir(&self, profile: &Path) -> PathBuf;
    /// §8.1, §12.5: the profile's credential, read as the agent reads it, for `spelling`.
    fn read_profile_credential(&self, env: &Env, spelling: &str) -> Read<Credential>;
    /// §12.5 "Identity drift": the profile's login identity (CC: its `.claude.json` `oauthAccount`).
    fn profile_identity(&self, env: &Env, spelling: &str) -> Read<Identity>;
    /// §10.3: deletes the agent-owned credential items for `spelling` and verifies them gone
    /// (CC macOS: the hashed Keychain item; otherwise nothing outside the directory).
    fn delete_profile_credential(&self, env: &Env, spelling: &str) -> Result<(), ProviderError>;
    /// §13.5: whether `env` is a process this agent started (CC: `CLAUDECODE` or `CLAUDE_CONFIG_DIR`).
    fn invoked_by(&self, env: &Env) -> bool;
```

**`crates/tagteam-provider/src/lib.rs`**: the `provider` re-export (lines 37–42) becomes

```rust
pub use provider::{
    BeforeFallback, Capabilities, CapturedLogin, CredLocks, DeadReason, DoomedEntry, EntryKind,
    Identity, IdentitySurface, KindTraits, LiveAuth, LiveChange, LiveLockSet, LiveLocks, MustShare,
    Provider, ProviderError, RefreshResult, SecretStore, SharePolicy, StoredLogin, TransientKind,
    Undo, UsageResult, Written,
};
```

**Create `crates/tagteam-cc/src/session.rs`:**

```rust
//! Claude Code's half of §12: what a profile shares (§12.2's tables), the record of the outer
//! home, and the environment every profile credential operation resolves paths with (§12.5).

use std::ffi::OsString;

use serde_json::{Value, json};
use tagteam_provider::{EntryKind, Env, ProviderError};

pub(crate) const CC_SHARED: &[&str] = &[
    "CLAUDE.md",
    "settings.json",
    "keybindings.json",
    "agents",
    "commands",
    "skills",
    "plugins",
    "hooks",
    "output-styles",
    "themes",
    "rules",
    "workflows",
    "file-history",
    "paste-cache",
    "shell-snapshots",
    "session-env",
];

pub(crate) const CC_MUST_SHARE: &[(&str, EntryKind)] = &[
    ("projects", EntryKind::Dir),
    ("history.jsonl", EntryKind::File),
];

pub(crate) const CC_PRIVATE: &[&str] = &[
    ".credentials.json",
    ".claude.json",
    ".claude-*-oauth.json",
    ".config.json",
    "sessions",
    "ide",
    "jobs",
    "daemon",
    "daemon.*",
    "daemon-auth-cooldown",
    "daemon-auth-status.json",
    "backups",
    "cache",
    "mcp-needs-auth-cache.json",
    "stats-cache.json",
    "policy-limits.json*",
    "remote-settings.json",
    "remote-settings-consent.json",
    "remote-settings-helper-consent",
    ".session_ingress_token",
    "hfi-auth.json",
    "state",
    "seed-admin",
    "bridge-spawn",
    "chrome",
    "debug",
    "feedback",
    "routines",
    "settings.local.json",
    ".last-cleanup",
    ".cc-writes",
    ".device-keys.json",
    "*.lock",
    "*.lock.owner",
    ".storage-write",
    ".*_auth_refresh-*",
    ".tagteam-*",
];

const CONFIG_DIR: &str = "CLAUDE_CONFIG_DIR";
const SECURE_STORAGE_DIR: &str = "CLAUDE_SECURESTORAGE_CONFIG_DIR";

/// One variable as the record keeps it: its text, or `null` when undefined. A value that is not
/// UTF-8 is kept as the same lossy text Keychain naming already hashes (Appendix A.2).
fn recorded(v: Option<&OsString>) -> Value {
    v.map_or(Value::Null, |s| {
        Value::String(s.to_string_lossy().into_owned())
    })
}

/// `outer` is `{"CLAUDE_CONFIG_DIR": <string>|null, "CLAUDE_SECURESTORAGE_CONFIG_DIR": <string>|null}`:
/// null means undefined; "" is kept (defined-but-empty, Appendix A.1).
pub(crate) fn outer_home(env: &Env) -> Value {
    json!({
        CONFIG_DIR: recorded(env.claude_config_dir.as_ref()),
        SECURE_STORAGE_DIR: recorded(env.claude_securestorage_config_dir.as_ref()),
    })
}

/// `env` with both variables as `outer` records them. Anything but a string or `null` under
/// each key is refused: the outer home would be a guess.
pub(crate) fn apply_outer_home(env: &Env, outer: &Value) -> Result<Env, ProviderError> {
    let restored = |key: &str| match outer.get(key) {
        Some(Value::String(s)) => Ok(Some(OsString::from(s))),
        Some(Value::Null) => Ok(None),
        _ => Err(ProviderError::Invalid(format!(
            "the profile marker's outer record has no valid {key}"
        ))),
    };
    let mut out = env.clone();
    out.claude_config_dir = restored(CONFIG_DIR)?;
    out.claude_securestorage_config_dir = restored(SECURE_STORAGE_DIR)?;
    Ok(out)
}

/// The profile `Env`: `claude_config_dir = Some(spelling)`, `claude_securestorage_config_dir = None`.
/// It is the session environment's view of the profile (§12.5 scrubs the secure-storage dir),
/// so the Keychain item, `.credentials.json` and `.claude.json` all resolve inside it.
pub(crate) fn profile_env(env: &Env, spelling: &str) -> Env {
    let mut out = env.clone();
    out.claude_config_dir = Some(OsString::from(spelling));
    out.claude_securestorage_config_dir = None;
    out
}
```

and declare it in `crates/tagteam-cc/src/lib.rs`, after `pub mod provider;`:

```rust
mod session;
```

**`crates/tagteam-cc/src/live.rs`**: before `snapshot`, add:

```rust
    /// Deletes both axes' Keychain items that `env` names, verifying each `Absent` with the
    /// existence probe (§10.3, §12.3 step 5). Nothing off macOS, where CC keeps no item. A
    /// profile's items are deleted under tagteam's own locks, not CC's live locks, so no fence
    /// applies.
    pub(crate) fn delete_items(&self, env: &Env) -> Result<(), ProviderError> {
        if !self.mac() {
            return Ok(());
        }
        let unfenced = || -> Result<(), ProviderError> { Ok(()) };
        self.remove_item(env, ItemKind::OAuth, &unfenced)?;
        self.remove_item(env, ItemKind::ManagedKey, &unfenced)
    }
```

**`crates/tagteam-cc/src/provider.rs`.** Line 1 becomes `use std::path::{Path, PathBuf};`. The
`tagteam_provider` import (lines 9–14) becomes

```rust
use tagteam_provider::{
    BeforeFallback, Capabilities, CredLocks, Credential, DoomedEntry, Env, FreshCredential,
    Identity, IdentitySurface, Keychain, KindTraits, LiveAuth, LiveChange, LiveLocks, MustShare,
    MutationGuard, Pace, PollBudget, Provider, ProviderError, Read, SharePolicy, StoredLogin, Undo,
    UsageResult, Window, Written,
};
```

and `use crate::paths::CcPaths;` (line 23) becomes

```rust
use crate::paths::{CcPaths, nfc};
use crate::session::{self, CC_MUST_SHARE, CC_PRIVATE, CC_SHARED};
```

At the end of `impl Provider for ClaudeCode`, after `live_identity_source`, add:

```rust
    fn launch_command(&self) -> &'static str {
        "claude"
    }

    fn session_dir_var(&self) -> Option<&'static str> {
        Some("CLAUDE_CONFIG_DIR")
    }

    /// Appendix A.1: an empty `CLAUDE_CONFIG_DIR` counts as unset.
    fn session_dir(&self, env: &Env) -> Option<PathBuf> {
        env.claude_config_dir
            .as_deref()
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    }

    fn outer_home(&self, env: &Env) -> Value {
        session::outer_home(env)
    }

    fn apply_outer_home(&self, env: &Env, outer: &Value) -> Result<Env, ProviderError> {
        session::apply_outer_home(env, outer)
    }

    /// §12.2 "One spelling": the canonical path, NFC-normalized.
    fn profile_spelling(&self, canonical: &Path) -> String {
        nfc(canonical.as_os_str())
    }

    fn share_policy(&self, env: &Env) -> SharePolicy {
        SharePolicy {
            source: CcPaths::resolve(env).config_home,
            shared: CC_SHARED.to_vec(),
            must_share: CC_MUST_SHARE
                .iter()
                .map(|&(name, kind)| MustShare { name, kind })
                .collect(),
            private: CC_PRIVATE.to_vec(),
        }
    }

    fn session_records_dir(&self, profile: &Path) -> PathBuf {
        profile.join("sessions")
    }

    /// The hashed Keychain item for `spelling`, then `<spelling>/.credentials.json`, exactly
    /// as the live read takes them (§12.3 step 2).
    fn read_profile_credential(&self, env: &Env, spelling: &str) -> Read<Credential> {
        let profile = session::profile_env(env, spelling);
        self.live
            .read_credential(&profile, &CcPaths::resolve(&profile))
    }

    fn profile_identity(&self, env: &Env, spelling: &str) -> Read<Identity> {
        config::live_identity(&CcPaths::resolve(&session::profile_env(env, spelling)))
    }

    /// Both axes' items for `spelling`; the profile's `.credentials.json` goes with its
    /// directory. Nothing on Linux.
    fn delete_profile_credential(&self, env: &Env, spelling: &str) -> Result<(), ProviderError> {
        self.live.delete_items(&session::profile_env(env, spelling))
    }

    fn invoked_by(&self, env: &Env) -> bool {
        env.var("CLAUDECODE").is_some_and(|v| v == "1") || self.session_dir(env).is_some()
    }
```

**`crates/tagteam-fake/src/paths.rs`** becomes:

```rust
use std::path::PathBuf;

use tagteam_provider::Env;

/// FakeAgent's home variable: a profile is a directory it names (§4.5 `session_dir_var`).
pub(crate) const HOME_VAR: &str = "FAKEAGENT_HOME";

/// Where FakeAgent keeps its state: `$FAKEAGENT_HOME` when set and non-empty, else
/// `<home>/.fakeagent/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakePaths {
    pub dir: PathBuf,
    /// `{"identity": {...}, ...}`. Only the `identity` key is FakeAgent's login.
    pub identity: PathBuf,
    /// The live credential, mode 0600.
    pub credential: PathBuf,
    /// FakeAgent's one live lock, a `mkdir` lock.
    pub lock: PathBuf,
}

impl FakePaths {
    pub fn resolve(env: &Env) -> Self {
        let dir = match env.var(HOME_VAR).filter(|v| !v.is_empty()) {
            Some(v) => PathBuf::from(v),
            None => env.home.join(".fakeagent"),
        };
        let dir = env.guard(dir);
        Self {
            identity: dir.join("identity.json"),
            credential: dir.join("credential.json"),
            lock: dir.join(".live.lock"),
            dir,
        }
    }
}
```

**`crates/tagteam-fake/src/provider.rs`.** The `tagteam_provider` import (lines 14–19) becomes

```rust
use tagteam_provider::{
    BeforeFallback, Capabilities, CredLocks, Credential, DoomedEntry, EntryKind, Env,
    FreshCredential, Identity, IdentitySurface, KindTraits, LiveAuth, LiveChange, LiveLockSet,
    LiveLocks, LockError, MkdirLock, MkdirLockSpec, MustShare, MutationGuard, Pace, PollBudget,
    Provider, ProviderError, Read, ReadError, SecretStore, SharePolicy, StoredLogin, Undo,
    UsageResult, Window, Written,
};
```

and `use crate::paths::FakePaths;` (line 23) becomes `use crate::paths::{FakePaths, HOME_VAR};`.
Before `showable_token`, add:

```rust
/// FakeAgent run for a profile: its home variable names `spelling`.
fn profile_env(env: &Env, spelling: &str) -> Env {
    let mut out = env.clone();
    out.vars.insert(HOME_VAR.into(), spelling.into());
    out
}
```

`capabilities` becomes

```rust
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            usage: true,
            refresh: true,
            sessions: true,
            ..Capabilities::default()
        }
    }
```

and at the end of `impl Provider for FakeAgent`, after `live_identity_source`, add:

```rust
    fn launch_command(&self) -> &'static str {
        "fakeagent"
    }

    fn session_dir_var(&self) -> Option<&'static str> {
        Some(HOME_VAR)
    }

    fn session_dir(&self, env: &Env) -> Option<PathBuf> {
        env.var(HOME_VAR)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    }

    /// `{"FAKEAGENT_HOME": <string>|null}`: one variable, unlike Claude Code's two.
    fn outer_home(&self, env: &Env) -> Value {
        let home = env
            .var(HOME_VAR)
            .map(|v| Value::String(v.to_string_lossy().into_owned()));
        json!({ HOME_VAR: home })
    }

    fn apply_outer_home(&self, env: &Env, outer: &Value) -> Result<Env, ProviderError> {
        let mut out = env.clone();
        match outer.get(HOME_VAR) {
            Some(Value::String(s)) => {
                out.vars.insert(HOME_VAR.into(), s.into());
            }
            Some(Value::Null) => {
                out.vars.remove(HOME_VAR);
            }
            _ => {
                return Err(ProviderError::Invalid(format!(
                    "the profile marker's outer record has no valid {HOME_VAR}"
                )));
            }
        }
        Ok(out)
    }

    /// The path as it is: FakeAgent does not normalize.
    fn profile_spelling(&self, canonical: &Path) -> String {
        canonical.to_string_lossy().into_owned()
    }

    fn share_policy(&self, env: &Env) -> SharePolicy {
        SharePolicy {
            source: FakePaths::resolve(env).dir,
            shared: vec!["notes", "prefs.json"],
            must_share: vec![MustShare {
                name: "journal.log",
                kind: EntryKind::File,
            }],
            private: vec![
                "credential.json",
                "identity.json",
                "procs",
                ".live.lock",
                ".tagteam-*",
            ],
        }
    }

    fn session_records_dir(&self, profile: &Path) -> PathBuf {
        profile.join("procs")
    }

    /// `<spelling>/credential.json`, a plain file like the live one.
    fn read_profile_credential(&self, env: &Env, spelling: &str) -> Read<Credential> {
        self.read_live_auth(&profile_env(env, spelling)).credential
    }

    /// The `identity` key of `<spelling>/identity.json`.
    fn profile_identity(&self, env: &Env, spelling: &str) -> Read<Identity> {
        self.live_identity(&profile_env(env, spelling))
    }

    /// FakeAgent keeps nothing outside the profile directory.
    fn delete_profile_credential(&self, _env: &Env, _spelling: &str) -> Result<(), ProviderError> {
        Ok(())
    }

    fn invoked_by(&self, _env: &Env) -> bool {
        false
    }
```

`tagteam-fake` stays `#![forbid(unsafe_code)]`: none of this needs any.

- [ ] **Step 4: Run them and see them pass**, then the crates' whole suites

Run: `cargo test -p tagteam-provider -p tagteam-cc -p tagteam-fake`
Expected: PASS, including `session.rs`'s 11 tests and the four new FakeAgent tests.

Run: `cargo test --workspace --features tagteam/test-support`
Expected: PASS. Nothing in the engine or the CLI calls the new methods yet, and `vars` is empty
everywhere outside these tests, so `FakePaths` resolves as before.

Run: `cargo clippy -p tagteam-provider -p tagteam-cc -p tagteam-fake --all-targets --target x86_64-unknown-linux-gnu -- -D warnings`
Expected: no warnings.

- [ ] **Step 5: Format and lint**

```
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-provider/src/env.rs crates/tagteam-provider/src/provider.rs \
  crates/tagteam-provider/src/lib.rs crates/tagteam-cc/src/session.rs crates/tagteam-cc/src/lib.rs \
  crates/tagteam-cc/src/live.rs crates/tagteam-cc/src/provider.rs crates/tagteam-cc/tests/session.rs \
  crates/tagteam-fake/src/paths.rs crates/tagteam-fake/src/provider.rs crates/tagteam-fake/tests/provider.rs
git commit -m "Give Claude Code and FakeAgent the session facts a profile needs"
```

---

### Task 8: Run-shell detection, the outer home, refusals (L312)

§12.8: "A process is in a run shell exactly when `CLAUDE_CONFIG_DIR` names a directory that
holds a profile marker (§12.2). Nothing else is consulted: not the path's location, and not
`XDG_DATA_HOME`, which a run shell may have changed." Today `Env::inside_run_shell` decides by
path prefix (`claude_config_dir` under `data_dir()/sessions`), which is M1's carried item L312.
A Bash tool that changes `XDG_DATA_HOME`, or a profile reached through a symlink, defeats it,
and nothing restores the default home: every engine read of `self.env` then takes the profile
for `~/.claude`. In particular the refresh gate sees the default home's live account as
inactive and may refresh the very token Claude Code in `~/.claude` is using (Review Focus 1).

This task detects the run shell once, from the marker, before the engine is built
(Decision 6). `detect_run_shell` returns the `RunShell` and the effective `Env`, which has the
marker provider's home variables restored from `outer` (`apply_outer_home`). `EngineConfig`
carries both, so the ~45 `self.env` sites read the default home with no edit. The engine's
refusal covers the three states, and the CLI refuses every command but `statusline` under an
unreadable marker, naming it.

**Readings of the spec this task commits to:**
- **The provider's own variable finds the marker.** `detect_run_shell` asks each registered
  provider with `capabilities().sessions` for `session_dir(env)`, in registration order, and
  reads a marker there. The first marker found decides. A provider whose directory holds no
  marker is passed over.
- **A marker that names another provider is unreadable.** If the marker's `provider` is not the
  provider whose variable led to it, the outer home it records is another provider's. This
  provider's outer home is then unknown, so the marker counts as unreadable (§12.8's rule for an
  outer home that cannot be known). So does an `outer` record the provider's `apply_outer_home`
  rejects.
- **The profile is kept as the variable spells it.** `RunShell::Inside.profile` is
  `session_dir(env)` as given. A symlinked profile stays the link's path, and `configDir` is
  never compared with it: detection is by the marker alone (B.57).
- **Only the provider's own variables change.** The effective `Env` is the shell's `Env` with
  the marker provider's home variables restored. `XDG_*`, `HOME` and `USER` stay as the shell
  has them, because the marker records only the provider's home (§12.2).
- **The engine refuses where it always did.** The eight account-changing entry points already
  call `refuse_inside_run_shell` (`switch`, `refresh_active`, `add_live`, `add_token`,
  `remove`, `set_alias`, `set_disabled`, `move_to`). Inside a run shell they return
  `InsideRunShell` as before. Under an unreadable marker they return `RunShellUnreadable`.
  Read-only engine calls do not refuse: the CLI refuses every command but `statusline` before
  it builds the engine, and the engine's reads in a run shell see the default home.
- **`statusline` under an unreadable marker prints nothing and exits 0** (§12.8, Review
  Focus 4). This task makes that true, before any Keychain or store access. Task 15 adds the
  `Inside` case (the marker's account) and the provider order.
- **The variables are captured at the process boundary.** `Context::from_process` captures
  every production provider's `session_dir_var()` plus `CLAUDECODE` into `Env.vars`
  (Decision 5), next to `Env::from_process()`. `app::run` never captures, so an in-process test
  carries exactly the `vars` its hand-built `Env` sets (§15.1), and the statusline's early branch
  sees them too (Decision 13's third step). `app::locate` then only detects: `app::run` calls it
  over the registry it builds first, and the statusline's engine over its walled registry.
  Detection reads only the marker file: no Keychain, no network.

**Files:**
- Create: `crates/tagteam-engine/src/session.rs` (`detect_run_shell`; Task 9 adds `SessionState`)
- Modify: `crates/tagteam-engine/src/lib.rs` (`pub mod session;`, lines 1–29)
- Modify: `crates/tagteam-engine/src/engine.rs` (`EngineConfig` 16–27, `Engine` 29–44, `Engine::new` 47–63, new `Engine::run_shell`, `refuse_inside_run_shell` 124–130, test module 283–393)
- Modify: `crates/tagteam-engine/src/error.rs` (`RunShellUnreadable`, its `kind()` arm and pin; lines 32–33, 111–144, 153–277)
- Modify: `crates/tagteam-engine/src/testutil.rs` (`T::new`'s `EngineConfig`, lines 48–57)
- Modify: `crates/tagteam-engine/src/lazy_http.rs` (`an_engine_over_a_lazy_client_does_not_build_it`'s `EngineConfig`, lines 124–138)
- Modify: `crates/tagteam-provider/src/env.rs` (remove `inside_run_shell`, lines 101–107, and its test, lines 155–161; line numbers before Task 7's `vars`)
- Modify: `crates/tagteam/src/app.rs` (`Context::from_process` 120–133; new `build_registry`, `session_vars` and `locate`; `build_engine` 145–182; `run` 295–350; `run_statusline` 364–414; one new unit test)
- Modify: `crates/tagteam/src/statusline.rs` (`engine` 195–218; one new unit test)
- Modify: `crates/tagteam-engine/tests/common/mod.rs` (`run_shell` at the four `EngineConfig` sites 303–312, 649–660, 1028–1039, 1205–1214; new profile and run-shell helpers)
- Modify: `crates/tagteam-engine/tests/oracle.rs` (`a_hanging_profile_endpoint_delays_a_switch_by_its_timeout_and_holds_no_lock`'s `EngineConfig`, lines 248–257)
- Modify: `crates/tagteam-engine/tests/add.rs` (`account_commands_refuse_inside_a_run_shell`, lines 688–704, rewritten with a real marker)
- Test: `crates/tagteam-engine/tests/run_shell.rs` (new)
- Test: `crates/tagteam/tests/run_shell_cli.rs` (new)

**Interfaces:**
- Consumes:
  - Task 7: `tagteam_provider::profile::{ProfileMarker, RunShell, MARKER_FILE, profile_path, canonical_profile_path}`, `ProfileMarker::{read, write}`
  - Task 7: `Provider::{session_dir_var, session_dir, outer_home, apply_outer_home, profile_spelling}`, `Capabilities.sessions`
  - Task 7: `Env.vars`, `Env::capture_vars(&mut self, names: &[&str])`, `Env::var(&self, name: &str) -> Option<&OsStr>`; FakeAgent's `FAKEAGENT_HOME` and `outer_home`
  - Existing: `ProviderRegistry::all`, `EngineError::InsideRunShell`, `Engine::refuse_inside_run_shell`'s eight callers
- Produces:
  - `tagteam_engine::session::detect_run_shell(env: &Env, registry: &ProviderRegistry) -> (RunShell, Env)`
  - `EngineConfig.run_shell: RunShell`, `Engine::run_shell(&self) -> &RunShell`
  - `refuse_inside_run_shell`: `Outside` → `Ok`, `Inside` → `InsideRunShell`, `Unreadable` → `RunShellUnreadable`
  - `EngineError::RunShellUnreadable { marker: PathBuf, detail: String }`, kind `run-shell-unreadable`
  - `Context::from_process` captures `session_vars(&build_registry(&ctx))` into `ctx.env.vars`; `app::run` never captures
  - `crate::app::session_vars(registry: &ProviderRegistry) -> Vec<&'static str>`: every registered provider's `session_dir_var()`, then `CLAUDECODE` (crate-private)
  - `crate::app::locate(env: Env, registry: &ProviderRegistry) -> (RunShell, Env)`: detection only, never a capture (crate-private; `statusline::engine` uses it too)
  - `Env::inside_run_shell` is gone
  - Fixture helpers in `crates/tagteam-engine/tests/common/mod.rs`, the canonical profile fixtures later tasks use:
    - `Fx::profile_dir(&self, id: &AccountId) -> PathBuf`
    - `Fx::write_marker(&self, dir: &Path, id: &AccountId, outer: &Env) -> ProfileMarker`
    - `Fx::make_profile(&self, id: &AccountId) -> PathBuf`
    - `Fx::shell_env(&self, profile: &Path) -> Env`
    - `Fx::engine_located(&self, env: Env) -> Engine`
    - private: `Fx::engine_in(&self, env: Env, run_shell: RunShell, vault: Vault, oracle: Arc<dyn Oracle>) -> Engine`, which `engine_over` now calls with `RunShell::Outside`

**Spec:**
- §12.8: detection by marker alone. The outer home: the live login, the gate's ownership
  check, active-token refresh and usage see the default home. Account-changing commands
  refuse. An unreadable marker refuses everything but `statusline`, which prints nothing.
- §9.2, first bullet: inside a run shell every command that changes accounts or the live login
  is refused.
- §7.3 step 2, last sentence: the live login is always the default home's, inside a run shell
  too.
- §12.2 "Profile marker": `outer` is the provider's record of the home the profile shares from.
  The marker is what makes a process a run shell.
- §15.1, first bullet: in a run shell the provider's home comes from the marker, which tests
  write into the fixture.
- §15.2 "Run shell": the gate never refreshes the default home's live account; `list` and
  `status` show the default login; account-changing commands refuse; an unreadable marker
  refuses everything but `statusline`; detection holds with `XDG_DATA_HOME` changed and for a
  marker outside `sessions/`.
- B.57: a run shell is recognized from its profile marker alone, and inside one tagteam
  resolves the provider's home from the marker.

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-engine/tests/common/mod.rs`, add to the imports:

```rust
use tagteam_engine::session::detect_run_shell;
use tagteam_provider::profile::{ProfileMarker, RunShell, canonical_profile_path, profile_path};
```

In `Fx::build`, add the last field of the `EngineConfig` literal (after `settings: Settings::default(),`):

```rust
            run_shell: RunShell::Outside,
```

Replace `Fx::engine_over` (lines 648–660) with these two methods:

```rust
    /// A second engine over this fixture's provider and clock, as another tagteam process.
    fn engine_over(&self, env: Env, vault: Vault, oracle: Arc<dyn Oracle>) -> Engine {
        self.engine_in(env, RunShell::Outside, vault, oracle)
    }

    /// `engine_over`, for a process that stands where `run_shell` says (§12.8).
    fn engine_in(
        &self,
        env: Env,
        run_shell: RunShell,
        vault: Vault,
        oracle: Arc<dyn Oracle>,
    ) -> Engine {
        Engine::new(EngineConfig {
            env,
            registry: ProviderRegistry::new().with(self.cc.clone()),
            vault,
            oracle,
            clock: self.clock.clone(),
            http: self.http.clone(),
            default_provider: ProviderId::new(CLAUDE_CODE),
            settings: Settings::default(),
            run_shell,
        })
    }
```

In `FakeFx::new` and in `Fx::engine_over_http`, add `run_shell: RunShell::Outside,` as the last
field of each `EngineConfig` literal.

Append to `crates/tagteam-engine/tests/common/mod.rs`:

```rust
/// Session profiles (§12.2) and the run shells they make (§12.8).
impl Fx {
    /// `<data_dir>/sessions/<id>`: where `run` puts `id`'s profile (§5).
    pub fn profile_dir(&self, id: &AccountId) -> PathBuf {
        profile_path(&self.env, id)
    }

    /// Writes `id`'s profile marker into `dir`, created 0700 if absent, as a first launch from
    /// `outer`'s home would (§12.2): `configDir` is `dir`'s canonical spelling, and `outer` is
    /// Claude Code's record of that home.
    pub fn write_marker(&self, dir: &Path, id: &AccountId, outer: &Env) -> ProfileMarker {
        fs::create_dir_all(dir).unwrap();
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
        let marker = ProfileMarker {
            provider: self.provider(),
            account_id: id.clone(),
            config_dir: self
                .cc
                .profile_spelling(&canonical_profile_path(dir).unwrap()),
            outer: self.cc.outer_home(outer),
        };
        marker.write(dir).unwrap();
        marker
    }

    /// `id`'s profile in its place under `sessions/`, launched from the fixture's own home: a
    /// marker, and the account's login as the profile's `oauthAccount` (§12.4). Nothing runs in
    /// it and it has no seed. Returns its directory.
    pub fn make_profile(&self, id: &AccountId) -> PathBuf {
        let dir = self.profile_dir(id);
        self.write_marker(&dir, id, &self.env);
        let row = self.engine.store().unwrap().account(id).unwrap().unwrap();
        fs::write(
            dir.join(".claude.json"),
            json!({"oauthAccount": row.identity_json}).to_string(),
        )
        .unwrap();
        dir
    }

    /// The environment every process in `profile`'s run shell inherits (§12.8):
    /// `CLAUDE_CONFIG_DIR` names the profile and `CLAUDE_SECURESTORAGE_CONFIG_DIR` is scrubbed
    /// (§12.5); everything else is the fixture's.
    pub fn shell_env(&self, profile: &Path) -> Env {
        let mut env = self.env.clone();
        env.claude_config_dir = Some(profile.as_os_str().to_owned());
        env.claude_securestorage_config_dir = None;
        env
    }

    /// An engine for a process whose environment is `env`, located as the CLI locates one:
    /// `detect_run_shell` decides the run shell and the environment the engine runs on (§12.8).
    pub fn engine_located(&self, env: Env) -> Engine {
        let registry = ProviderRegistry::new().with(self.cc.clone());
        let (run_shell, env) = detect_run_shell(&env, &registry);
        self.engine_in(env, run_shell, self.keychain_vault(), self.oracle.clone())
    }
}
```

In `crates/tagteam-engine/tests/oracle.rs`, add `use tagteam_provider::profile::RunShell;` to the
imports, and `run_shell: RunShell::Outside,` as the last field of the `EngineConfig` literal in
`a_hanging_profile_endpoint_delays_a_switch_by_its_timeout_and_holds_no_lock`.

In `crates/tagteam-engine/tests/add.rs`, add `use tagteam_core::AccountId;` to the imports and
replace `account_commands_refuse_inside_a_run_shell` (lines 688–704) with:

```rust
#[test]
fn account_commands_refuse_inside_a_run_shell() {
    // §12.8, B.57: the marker makes the run shell, wherever the profile lies.
    let fx = Fx::new();
    fx.login("me@work.co", "rt-1");
    let profile = fx.dir.path().join("elsewhere");
    fx.write_marker(&profile, &AccountId::from_string("0192"), &fx.env);
    let engine = fx.engine_located(fx.shell_env(&profile));
    assert!(matches!(
        engine.add_live(add_opts(&fx)),
        Err(EngineError::InsideRunShell)
    ));
    assert!(matches!(
        engine.add_token(token_opts(&fx, "sk-ant-api03-x")),
        Err(EngineError::InsideRunShell)
    ));
}
```

Create `crates/tagteam-engine/tests/run_shell.rs`:

```rust
//! §12.8 inside a run shell: detection by the profile marker alone, the outer home the engine
//! runs on, and the refusals (Review Focus 1 and 4).

mod common;

use std::fs;
use std::os::unix::fs::symlink;

use common::{API_KEY, FakeFx, Fx, token_requests};
use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::active::ActiveTrigger;
use tagteam_engine::refresh::{GateOutcome, OwnedBy};
use tagteam_engine::registry::ProviderRegistry;
use tagteam_engine::session::detect_run_shell;
use tagteam_engine::views::StatusView;
use tagteam_engine::{Engine, EngineError};
use tagteam_fake::FAKE_AGENT;
use tagteam_provider::Provider;
use tagteam_provider::profile::{MARKER_FILE, ProfileMarker, RunShell};

fn registry(fx: &Fx) -> ProviderRegistry {
    ProviderRegistry::new().with(fx.cc.clone())
}

/// A call's error kind, or `None` when it went through.
fn kind<T>(r: Result<T, EngineError>) -> Option<&'static str> {
    r.err().map(|e| e.kind())
}

/// Every engine call that changes accounts or the live login (§9.2, §12.8), with its error kind.
fn account_changes(
    fx: &Fx,
    engine: &Engine,
    a: &AccountId,
) -> Vec<(&'static str, Option<&'static str>)> {
    vec![
        ("switch", kind(engine.switch(fx.switch_request(a, false)))),
        ("switch --force", kind(engine.switch(fx.switch_request(a, true)))),
        ("add", kind(engine.add_live(fx.add_options()))),
        ("add-token", kind(engine.add_token(fx.add_token_options(API_KEY)))),
        ("remove", kind(engine.remove(a))),
        ("alias", kind(engine.set_alias(a, Some("work")))),
        ("disable", kind(engine.set_disabled(a, true))),
        ("move", kind(engine.move_to(a, 2))),
        (
            "active refresh",
            kind(engine.refresh_active(&fx.provider(), ActiveTrigger::Expired)),
        ),
    ]
}

#[test]
fn a_marker_outside_sessions_is_a_run_shell() {
    // §15.2 "Run shell": the path's location is not consulted.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let elsewhere = fx.dir.path().join("elsewhere/profile");
    let marker = fx.write_marker(&elsewhere, &a, &fx.env);
    let (shell, effective) = detect_run_shell(&fx.shell_env(&elsewhere), &registry(&fx));
    assert_eq!(
        shell,
        RunShell::Inside {
            profile: elsewhere,
            marker
        }
    );
    assert_eq!(effective.claude_config_dir, None, "the outer home set none");
}

#[test]
fn a_profile_path_without_a_marker_is_not_a_run_shell() {
    // The path rule that `Env::inside_run_shell` applied is gone (L312).
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let dir = fx.profile_dir(&a);
    fs::create_dir_all(&dir).unwrap();
    let env = fx.shell_env(&dir);
    let (shell, effective) = detect_run_shell(&env, &registry(&fx));
    assert_eq!(shell, RunShell::Outside);
    assert_eq!(effective.claude_config_dir, env.claude_config_dir, "unchanged");
}

#[test]
fn detection_holds_with_xdg_data_home_changed_inside_the_shell() {
    // Review Focus 1: a tool that runs with its own `XDG_DATA_HOME` is still in the run shell.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    let mut env = fx.shell_env(&profile);
    env.xdg_data_home = Some(fx.dir.path().join("tool-data"));
    let (shell, effective) = detect_run_shell(&env, &registry(&fx));
    assert!(matches!(shell, RunShell::Inside { .. }), "{shell:?}");
    assert_eq!(
        effective.xdg_data_home, env.xdg_data_home,
        "only the provider's own variables are restored"
    );
    let engine = fx.engine_located(env);
    assert!(matches!(
        engine.add_live(fx.add_options()),
        Err(EngineError::InsideRunShell)
    ));
}

#[test]
fn a_profile_reached_through_a_symlink_is_a_run_shell() {
    // Review Focus 1: the marker is read through the link, and the profile keeps the spelling
    // the variable gave it.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    let link = fx.dir.path().join("via-link");
    symlink(&profile, &link).unwrap();
    let (shell, _) = detect_run_shell(&fx.shell_env(&link), &registry(&fx));
    let RunShell::Inside { profile: found, marker } = shell else {
        panic!("{shell:?}")
    };
    assert_eq!(found, link);
    assert_eq!(marker.account_id, a);
}

#[test]
fn the_engine_runs_on_the_outer_home_the_marker_records() {
    // §12.8: `outer` comes back exactly, a defined-but-empty CLAUDE_SECURESTORAGE_CONFIG_DIR
    // included (Appendix A.1).
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let mut outer = fx.env.clone();
    outer.claude_config_dir = Some(fx.env.home.join("custom-claude").into_os_string());
    outer.claude_securestorage_config_dir = Some("".into());
    let profile = fx.profile_dir(&a);
    fx.write_marker(&profile, &a, &outer);
    let (_, effective) = detect_run_shell(&fx.shell_env(&profile), &registry(&fx));
    assert_eq!(effective.claude_config_dir, outer.claude_config_dir);
    assert_eq!(effective.claude_securestorage_config_dir, Some("".into()));
    assert_eq!(effective.home, fx.env.home);
    assert_eq!(effective.data_dir(), fx.env.data_dir());
}

#[test]
fn an_unreadable_marker_leaves_the_environment_alone_and_names_the_file() {
    for bad in [&b"{ torn"[..], br#"{"format": "tagteam-profile", "version": 2}"#, b"[1]"] {
        let fx = Fx::new();
        let dir = fx.dir.path().join("p");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(MARKER_FILE), bad).unwrap();
        let env = fx.shell_env(&dir);
        let (shell, effective) = detect_run_shell(&env, &registry(&fx));
        let RunShell::Unreadable { marker, .. } = shell else {
            panic!("{}: {shell:?}", String::from_utf8_lossy(bad))
        };
        assert_eq!(marker, dir.join(MARKER_FILE));
        assert_eq!(effective.claude_config_dir, env.claude_config_dir);
    }
}

#[test]
fn a_marker_for_another_provider_is_unreadable() {
    // Its `outer` is another provider's record, so Claude Code's outer home is unknown.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let dir = fx.dir.path().join("p");
    fs::create_dir_all(&dir).unwrap();
    ProfileMarker {
        provider: ProviderId::new(FAKE_AGENT),
        account_id: a,
        config_dir: dir.display().to_string(),
        outer: serde_json::json!({"FAKEAGENT_HOME": null}),
    }
    .write(&dir)
    .unwrap();
    let (shell, _) = detect_run_shell(&fx.shell_env(&dir), &registry(&fx));
    assert!(matches!(shell, RunShell::Unreadable { .. }), "{shell:?}");
}

#[test]
fn the_first_registered_provider_with_a_marker_decides() {
    // Decision 5: FakeAgent's own variable finds its profile; with Claude Code's variable set
    // too, Claude Code, registered first, decides.
    let ffx = FakeFx::new();
    let registry = ProviderRegistry::new()
        .with(ffx.fx.cc.clone())
        .with(ffx.fake.clone());
    let fake_profile = ffx.fx.dir.path().join("fake-profile");
    fs::create_dir_all(&fake_profile).unwrap();
    let fake_marker = ProfileMarker {
        provider: ProviderId::new(FAKE_AGENT),
        account_id: AccountId::from_string("fake-1"),
        config_dir: fake_profile.display().to_string(),
        outer: ffx.fake.outer_home(&ffx.fx.env),
    };
    fake_marker.write(&fake_profile).unwrap();
    let mut env = ffx.fx.env.clone();
    env.vars.insert(
        "FAKEAGENT_HOME".into(),
        fake_profile.clone().into_os_string(),
    );
    let (shell, effective) = detect_run_shell(&env, &registry);
    assert_eq!(
        shell,
        RunShell::Inside {
            profile: fake_profile,
            marker: fake_marker
        }
    );
    assert_eq!(effective.var("FAKEAGENT_HOME"), None, "the outer home set none");

    let cc_profile = ffx.fx.dir.path().join("cc-profile");
    let cc_marker =
        ffx.fx
            .write_marker(&cc_profile, &AccountId::from_string("cc-1"), &ffx.fx.env);
    env.claude_config_dir = Some(cc_profile.clone().into_os_string());
    let (shell, _) = detect_run_shell(&env, &registry);
    assert_eq!(
        shell,
        RunShell::Inside {
            profile: cc_profile,
            marker: cc_marker
        }
    );
}

#[test]
fn inside_a_run_shell_the_gate_never_refreshes_the_default_live_login() {
    // Review Focus 1. The profile's `.claude.json` names the session's own account, a, so an
    // engine that took the profile for the default home would see b, the default home's live
    // login, as inactive, and refresh the very token Claude Code in ~/.claude is using.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let profile = fx.make_profile(&a);
    fx.expire_access(&b);
    fx.script_refresh(Some("rt-b2"));
    let snapshot = fx.vault_bytes(&b).unwrap();
    let located = fx.engine_located(fx.shell_env(&profile));
    let outcome = located
        .refresh_stored(fx.cc.as_ref(), &b, &snapshot)
        .unwrap();
    assert!(matches!(outcome, GateOutcome::Owned(OwnedBy::Live)), "{outcome:?}");
    assert_eq!(token_requests(&fx), 0);
    assert_eq!(fx.vault_refresh_token(&b).as_deref(), Some("rt-b"));
    // The fixture has teeth: the same environment taken at face value refreshes it.
    let face_value = fx.engine_with_env(fx.shell_env(&profile));
    let outcome = face_value
        .refresh_stored(fx.cc.as_ref(), &b, &snapshot)
        .unwrap();
    assert!(matches!(outcome, GateOutcome::Refreshed(_)), "{outcome:?}");
}

#[test]
fn list_and_status_inside_a_run_shell_see_the_default_home() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let engine = fx.engine_located(fx.shell_env(&fx.make_profile(&a)));
    let lists = engine.accounts(None).unwrap();
    assert_eq!(lists[0].active_position, Some(2));
    let active: Vec<AccountId> = lists[0]
        .accounts
        .iter()
        .filter(|v| v.active)
        .map(|v| v.row.id.clone())
        .collect();
    assert_eq!(active, [b.clone()]);
    let StatusView::Managed { account, .. } = engine.status(&fx.provider()).unwrap() else {
        panic!("the default home's live login is managed")
    };
    assert_eq!(account.row.id, b);
}

#[test]
fn inside_a_run_shell_every_account_change_refuses_and_changes_nothing() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let engine = fx.engine_located(fx.shell_env(&fx.make_profile(&a)));
    assert!(matches!(engine.run_shell(), RunShell::Inside { .. }));
    for (command, got) in account_changes(&fx, &engine, &a) {
        assert_eq!(got, Some("inside-run-shell"), "{command}");
    }
    let row = fx.engine.store().unwrap().account(&a).unwrap().unwrap();
    assert_eq!((row.position, row.alias, row.disabled), (1, None, false));
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
}

#[test]
fn under_an_unreadable_marker_every_account_change_refuses_naming_it() {
    // Review Focus 4: the outer home is unknown, so nothing acts on a guess.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let profile = fx.make_profile(&a);
    let marker = profile.join(MARKER_FILE);
    fs::write(&marker, "{\"format\": \"tagteam-profile\"").unwrap();
    let engine = fx.engine_located(fx.shell_env(&profile));
    assert!(
        matches!(engine.run_shell(), RunShell::Unreadable { marker: m, .. } if *m == marker),
        "{:?}",
        engine.run_shell()
    );
    for (command, got) in account_changes(&fx, &engine, &a) {
        assert_eq!(got, Some("run-shell-unreadable"), "{command}");
    }
    let err = engine.switch(fx.switch_request(&a, false)).unwrap_err();
    assert!(
        err.to_string().contains(&marker.display().to_string()),
        "{err}"
    );
}
```

Create `crates/tagteam/tests/run_shell_cli.rs`:

```rust
//! §12.8 through the binary: a run shell's refusals, and an unreadable marker's (Review Focus
//! 4). Needs `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{cmd, two_fresh_accounts};
use serde_json::{Value, json};
use tagteam_core::{AccountId, CLAUDE_CODE, ProviderId};
use tagteam_provider::Env;
use tagteam_provider::profile::{MARKER_FILE, ProfileMarker, canonical_profile_path};

/// Every command but `statusline`, each with arguments it accepts.
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
];

/// `id`'s profile directory under the fixture's `sessions/`.
fn profile_dir(root: &Path, id: &str) -> PathBuf {
    Env::for_test(root).data_dir().join("sessions").join(id)
}

/// `id`'s profile with a valid marker, launched from the fixture's home, where neither Claude
/// Code variable is defined (§12.2). The fixture's paths are ASCII, so the canonical path is
/// its own NFC spelling.
fn profile(root: &Path, id: &str) -> PathBuf {
    let dir = profile_dir(root, id);
    fs::create_dir_all(&dir).unwrap();
    ProfileMarker {
        provider: ProviderId::new(CLAUDE_CODE),
        account_id: AccountId::from_string(id),
        config_dir: canonical_profile_path(&dir).unwrap().display().to_string(),
        outer: json!({"CLAUDE_CONFIG_DIR": null, "CLAUDE_SECURESTORAGE_CONFIG_DIR": null}),
    }
    .write(&dir)
    .unwrap();
    dir
}

fn json_of(out: &std::process::Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn an_unreadable_marker_refuses_every_command_but_statusline() {
    // §12.8: every refusal names the file, and the status bar shows nothing.
    let d = tempfile::tempdir().unwrap();
    let (a, _) = two_fresh_accounts(d.path());
    let dir = profile_dir(d.path(), &a);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(MARKER_FILE), "{\"format\": \"tagteam-profile\"").unwrap();
    let marker = dir.join(MARKER_FILE).display().to_string();
    for args in COMMANDS {
        let out = cmd(d.path())
            .env("CLAUDE_CONFIG_DIR", &dir)
            .args(*args)
            .output()
            .unwrap();
        let stderr = String::from_utf8(out.stderr).unwrap();
        assert_eq!(out.status.code(), Some(1), "{args:?}: {stderr}");
        assert!(stderr.contains(&marker), "{args:?}: {stderr}");
        assert!(out.stdout.is_empty(), "{args:?}");
        let out = cmd(d.path())
            .env("CLAUDE_CONFIG_DIR", &dir)
            .args(*args)
            .arg("--json")
            .output()
            .unwrap();
        let v = json_of(&out);
        assert_eq!(v["error"]["type"], "run-shell-unreadable", "{args:?}");
        assert!(
            v["error"]["message"].as_str().unwrap().contains(&marker),
            "{args:?}: {v}"
        );
    }
    cmd(d.path())
        .env("CLAUDE_CONFIG_DIR", &dir)
        .arg("statusline")
        .assert()
        .success()
        .stdout("")
        .stderr("");
}

#[test]
fn a_run_shell_sees_the_default_home_and_refuses_account_changes() {
    // §12.8: the profile has no `.claude.json`, so a CLI that took it for the default home
    // would find no live login at all.
    let d = tempfile::tempdir().unwrap();
    let (a, _) = two_fresh_accounts(d.path());
    let dir = profile(d.path(), &a);
    let out = cmd(d.path())
        .env("CLAUDE_CONFIG_DIR", &dir)
        .args(["list", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(json_of(&out)["activeAccountNumber"], 2, "b, the default login");
    let out = cmd(d.path())
        .env("CLAUDE_CONFIG_DIR", &dir)
        .args(["status", "--json"])
        .output()
        .unwrap();
    assert_eq!(json_of(&out)["active"]["email"], "b@x.co");
    for args in [&["switch", "1"][..], &["add"], &["remove", "1"], &["move", "1", "2"]] {
        let out = cmd(d.path())
            .env("CLAUDE_CONFIG_DIR", &dir)
            .args(args)
            .arg("--json")
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert_eq!(json_of(&out)["error"]["type"], "inside-run-shell", "{args:?}");
    }
}
```

In `crates/tagteam/src/statusline.rs`'s test module, add `use tagteam_provider::profile::{MARKER_FILE, RunShell};`
to its imports and append:

```rust
    #[test]
    fn an_unreadable_marker_is_found_without_the_keychain() {
        // §12.8: the status bar learns from the file alone that the marker cannot be read, and
        // then prints nothing (`app::run_statusline`).
        let dir = tempfile::tempdir().unwrap();
        let mut env = Env::for_test(dir.path());
        let profile = dir.path().join("profile");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(profile.join(MARKER_FILE), "not json").unwrap();
        env.claude_config_dir = Some(profile.clone().into_os_string());
        let ctx = Context {
            env,
            keychain: Arc::new(FakeKeychain::new()),
            platform: Platform::MacOs,
            api_base: None,
            stdout_terminal: false,
            no_color_env: false,
            force_color_env: false,
        };
        let (built, http, keychain) = engine(ctx, &ProviderId::new(CLAUDE_CODE));
        assert!(
            matches!(built.run_shell(), RunShell::Unreadable { marker, .. } if *marker == profile.join(MARKER_FILE)),
            "{:?}",
            built.run_shell()
        );
        assert!(!http.is_built());
        assert_eq!(keychain.calls(), 0);
    }
```

In `crates/tagteam/src/app.rs`'s test module, add `use tagteam_provider::FakeKeychain;` to its
imports and append:

```rust
    #[test]
    fn the_process_boundary_captures_every_session_variable_and_claudecode() {
        // Decision 5: `Context::from_process` captures these names, and only it does, so a
        // hand-built `Context` carries exactly the `vars` its test set (§15.1).
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
        assert_eq!(
            session_vars(&build_registry(&ctx)),
            ["CLAUDE_CONFIG_DIR", "CLAUDECODE"]
        );
        assert!(ctx.env.vars.is_empty(), "nothing but the process boundary captures");
    }
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-engine --test run_shell --test add`
Expected: compile error, `unresolved import tagteam_engine::session` (from `tests/common/mod.rs`),
and `struct EngineConfig has no field named run_shell`.

Run: `cargo test -p tagteam --lib`
Expected: compile errors, `no method named run_shell found for struct Engine` (the new
statusline unit test) and `cannot find function session_vars in this scope` (the new `app.rs`
unit test).

Run: `cargo test -p tagteam --features test-support --test run_shell_cli`
Expected: it compiles, because it uses only Task 7's `profile` module, and both tests fail:
- `an_unreadable_marker_refuses_every_command_but_statusline`: `list` exits 0, not 1. Nothing
  reads the marker yet.
- `a_run_shell_sees_the_default_home_and_refuses_account_changes`: `activeAccountNumber` is
  `null`, not 2. The CLI takes the profile, which has no `.claude.json`, for the default home.

- [ ] **Step 3: Implement**

**3a. The provider crate.** In `crates/tagteam-provider/src/env.rs`, delete `inside_run_shell`
(its doc comment and body, today lines 101–107) and its unit test
`a_run_shell_is_detected_from_claude_config_dir` (lines 155–161). Detection needs the marker
file, which `tagteam-provider` cannot interpret without a registry. Its coverage moves to
`tests/run_shell.rs`: `a_marker_outside_sessions_is_a_run_shell` and
`a_profile_path_without_a_marker_is_not_a_run_shell`.

**3b. The error.** In `crates/tagteam-engine/src/error.rs`, add after `InsideRunShell`:

```rust
    /// §12.8: a marker is present but is not a valid marker, so the outer home is unknown.
    /// Every command but `statusline` refuses, naming it.
    #[error("the run-shell marker {} cannot be read ({detail})", marker.display())]
    RunShellUnreadable { marker: PathBuf, detail: String },
```

In `kind()`, after the `InsideRunShell` arm:

```rust
            EngineError::RunShellUnreadable { .. } => "run-shell-unreadable",
```

In `kind_is_pinned_for_every_variant`, after the `InsideRunShell` case:

```rust
            (
                EngineError::RunShellUnreadable {
                    marker: PathBuf::from("x"),
                    detail: "d".into(),
                },
                "run-shell-unreadable",
            ),
```

**3c. Detection.** Create `crates/tagteam-engine/src/session.rs`:

```rust
//! §12.8: where a process stands, inside a run shell or not. It is found once, before the
//! engine is built (Decision 6), so every engine read of `env` already sees the default home.

use std::path::Path;

use tagteam_provider::profile::{MARKER_FILE, ProfileMarker, RunShell};
use tagteam_provider::{Env, Read};

use crate::registry::ProviderRegistry;

fn unreadable_marker(dir: &Path, env: &Env, detail: String) -> (RunShell, Env) {
    (
        RunShell::Unreadable {
            marker: dir.join(MARKER_FILE),
            detail,
        },
        env.clone(),
    )
}

/// §12.8: the first registered provider with sessions whose `session_dir(env)` holds a marker
/// decides. Returns the run shell and the effective `Env` (the marker provider's
/// `apply_outer_home`, or `env` unchanged when `Outside` or `Unreadable`). The provider whose
/// variable found the marker must be the provider the marker names; a mismatch is `Unreadable`.
///
/// Only the marker is consulted (B.57): not where the directory lies, and not `XDG_DATA_HOME`,
/// which a run shell may have changed. A marker that names another provider leaves this
/// provider's outer home unknown, and so does an `outer` record the provider cannot apply, so
/// both count as unreadable.
pub fn detect_run_shell(env: &Env, registry: &ProviderRegistry) -> (RunShell, Env) {
    for p in registry.all() {
        if !p.capabilities().sessions {
            continue;
        }
        let Some(dir) = p.session_dir(env) else {
            continue;
        };
        let marker = match ProfileMarker::read(&dir) {
            Read::Absent => continue,
            Read::Unreadable(e) => return unreadable_marker(&dir, env, e.detail),
            Read::Present(m) => m,
        };
        if marker.provider != p.id() {
            let detail = format!("it names the provider {}, not {}", marker.provider, p.id());
            return unreadable_marker(&dir, env, detail);
        }
        return match p.apply_outer_home(env, &marker.outer) {
            Ok(outer) => (RunShell::Inside { profile: dir, marker }, outer),
            Err(e) => unreadable_marker(&dir, env, e.to_string()),
        };
    }
    (RunShell::Outside, env.clone())
}
```

In `crates/tagteam-engine/src/lib.rs`, add `pub mod session;` between `mod rescue;` and
`pub mod settings;`.

**3d. The engine.** In `crates/tagteam-engine/src/engine.rs`, add `use tagteam_provider::profile::RunShell;`
to the imports, and add the last field of `EngineConfig`:

```rust
    /// §12.8, from `detect_run_shell`; `env` is already the effective (outer) environment.
    pub run_shell: RunShell,
```

Add to `Engine`, after `pub(crate) settings: Settings,`:

```rust
    /// Where this process stands (§12.8).
    pub(crate) run_shell: RunShell,
```

In `Engine::new`, after `settings: cfg.settings,`:

```rust
            run_shell: cfg.run_shell,
```

Add this method after `Engine::http`:

```rust
    /// Where this process stands (§12.8): outside a run shell, inside one (`env` is then the
    /// outer home its marker records), or under a marker that cannot be read.
    pub fn run_shell(&self) -> &RunShell {
        &self.run_shell
    }
```

Replace `refuse_inside_run_shell` (lines 124–130) with:

```rust
    /// §12.8: commands that change accounts or the live login refuse inside a run shell, and
    /// under a marker that cannot be read, which the refusal names.
    pub(crate) fn refuse_inside_run_shell(&self) -> Result<(), EngineError> {
        match &self.run_shell {
            RunShell::Outside => Ok(()),
            RunShell::Inside { .. } => Err(EngineError::InsideRunShell),
            RunShell::Unreadable { marker, detail } => Err(EngineError::RunShellUnreadable {
                marker: marker.clone(),
                detail: detail.clone(),
            }),
        }
    }
```

Its eight callers stay as they are: `switch.rs` `Engine::switch`, `active.rs`
`Engine::refresh_active`, and `lifecycle.rs` `add_live`, `add_token`, `remove`, `set_alias`,
`set_disabled`, `move_to`.

In the test module, add `use tagteam_provider::profile::ProfileMarker;` (`RunShell` and
`PathBuf` come in through `use super::*;`), add `run_shell: RunShell::Outside,` as the last
field of `test_config`'s literal, and append:

```rust
    #[test]
    fn refuse_inside_run_shell_follows_the_three_states() {
        let d = tempfile::tempdir().unwrap();
        let with = |run_shell: RunShell| {
            Engine::new(EngineConfig {
                run_shell,
                ..test_config(Env::for_test(d.path()))
            })
        };
        assert!(with(RunShell::Outside).refuse_inside_run_shell().is_ok());
        let inside = RunShell::Inside {
            profile: PathBuf::from("/p"),
            marker: ProfileMarker {
                provider: ProviderId::new("p"),
                account_id: AccountId::from_string("a"),
                config_dir: "/p".into(),
                outer: serde_json::json!({}),
            },
        };
        let engine = with(inside.clone());
        assert_eq!(engine.run_shell(), &inside);
        assert!(matches!(
            engine.refuse_inside_run_shell(),
            Err(EngineError::InsideRunShell)
        ));
        let err = with(RunShell::Unreadable {
            marker: PathBuf::from("/p/.tagteam-profile.json"),
            detail: "not JSON".into(),
        })
        .refuse_inside_run_shell()
        .unwrap_err();
        assert_eq!(err.kind(), "run-shell-unreadable");
        assert_eq!(
            err.to_string(),
            "the run-shell marker /p/.tagteam-profile.json cannot be read (not JSON)"
        );
    }
```

**3e. The in-crate fixtures.** In `crates/tagteam-engine/src/testutil.rs` add
`use tagteam_provider::profile::RunShell;` and `run_shell: RunShell::Outside,` as the last field
of `T::new`'s `EngineConfig`. In `crates/tagteam-engine/src/lazy_http.rs`'s test module add the
same import and the same field to `an_engine_over_a_lazy_client_does_not_build_it`'s literal.

**3f. The CLI.** In `crates/tagteam/src/app.rs`, add to the imports:

```rust
use tagteam_engine::session::detect_run_shell;
use tagteam_provider::profile::RunShell;
```

and, below `FORCE_COLOR`:

```rust
/// Set to `1` in every process Claude Code starts (§13.5, Appendix A.7).
const CLAUDECODE: &str = "CLAUDECODE";
```

Replace `Context::from_process` (lines 120–133) with:

```rust
impl Context {
    pub fn from_process() -> Self {
        let o = test_overrides(&|k| std::env::var_os(k));
        let mut ctx = Self {
            env: Env::from_process(),
            keychain: o.keychain.unwrap_or_else(|| Arc::new(SecurityCli::new())),
            platform: o.platform.unwrap_or_else(Platform::current),
            api_base: o.api_base,
            stdout_terminal: std::io::stdout().is_terminal(),
            no_color_env: env_flag(NO_COLOR),
            force_color_env: env_flag(FORCE_COLOR),
        };
        // Decision 5: the variables this build's providers asked for, captured here at the
        // process boundary and never in `run`, so a hand-built `Context` carries exactly the
        // `vars` its test set (§15.1). Building the registry touches no Keychain item.
        let names = session_vars(&build_registry(&ctx));
        ctx.env.capture_vars(&names);
        ctx
    }
}
```

Replace `build_engine` (lines 141–182) with these four functions:

```rust
/// The providers this build registers (§4.5), over the context's Keychain and platform, with
/// every endpoint under the test base when one is set.
fn build_registry(ctx: &Context) -> ProviderRegistry {
    let mut cc = ClaudeCode::new(ctx.keychain.clone(), ctx.platform);
    if let Some(base) = &ctx.api_base {
        cc = cc.with_endpoints(Endpoints::with_base(base));
    }
    ProviderRegistry::new().with(Arc::new(cc))
}

/// The variables `Context::from_process` captures into `Env.vars` (Decision 5): every variable
/// `registry`'s providers name a profile with (§4.5 `session_dir_var`), then `CLAUDECODE`
/// (§13.5).
pub(crate) fn session_vars(registry: &ProviderRegistry) -> Vec<&'static str> {
    let mut names: Vec<&'static str> = registry
        .all()
        .iter()
        .filter_map(|p| p.session_dir_var())
        .collect();
    names.push(CLAUDECODE);
    names
}

/// §12.8: where this process stands. The marker decides the run shell and the environment the
/// engine runs on (Decision 6). It only detects: the variables are already in `env.vars`,
/// captured at the process boundary (`Context::from_process`).
pub(crate) fn locate(env: Env, registry: &ProviderRegistry) -> (RunShell, Env) {
    detect_run_shell(&env, registry)
}

/// The engine for one command, and the settings warnings for the caller to print (§6.4). The
/// settings are those of `provider`, the one the command resolves (`--provider`, else the
/// default): its own tables come first. `env` is the effective environment `locate` returned
/// with `run_shell`. The HTTP adapter is built on its first request, never before (§13.5), and
/// the oracle sends through the same one.
fn build_engine(
    ctx: Context,
    registry: ProviderRegistry,
    run_shell: RunShell,
    env: Env,
    provider: &ProviderId,
) -> (Engine, Vec<String>) {
    let vault = match ctx.platform {
        Platform::MacOs => Vault::new(Box::new(KeychainVault::new(ctx.keychain))),
        Platform::Linux => Vault::new(Box::new(FileVault::new(env.data_dir().join("vault")))),
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
    let default_provider = ProviderId::new(CLAUDE_CODE);
    let (settings, warnings) = Settings::load(&env, provider);
    let engine = Engine::new(EngineConfig {
        env,
        registry,
        vault,
        // §7.6: asked at most once per credential within one command.
        oracle: Arc::new(CachingOracle::new(HttpOracle::new(
            http.clone(),
            clock.clone(),
        ))),
        clock,
        http,
        default_provider,
        settings,
        run_shell,
    });
    (engine, warnings)
}
```

Replace `run` (lines 295–350) with:

```rust
pub fn run(cli: Cli, ctx: Context, io: &mut Io<'_>) -> i32 {
    let color = !cli.no_color && !ctx.no_color_env;
    init_logging(cli.debug, color);
    let json = cli.json;
    if let Err(msg) = root_guard::refuse_root() {
        return fail(io, json, KIND_ROOT, &msg);
    }
    // §13.5: the status bar's fast path, before anything else is built.
    if let Some(Command::Statusline { print_config }) = &cli.command {
        return run_statusline(ctx, io, json, cli.no_color, cli.provider, *print_config);
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
        return fail(io, json, e.kind(), &e.to_string());
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
            return fail(app.io, json, e.kind(), &e.to_string());
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
        Ok(()) => 0,
        Err(Failure::Engine(e)) => fail(app.io, json, e.kind(), &e.to_string()),
        Err(Failure::Message(kind, m)) => fail(app.io, json, kind, &m),
        Err(Failure::Usage(m)) => {
            fail(app.io, json, KIND_USAGE, &m);
            EXIT_USAGE
        }
    }
}
```

Replace `run_statusline` (lines 361–414) with:

```rust
/// §13.5's fast path, taken before `build_engine`: no lock check, no settings warnings (a status
/// bar has nowhere to show them), and an engine walled off from the Keychain and the network
/// (`statusline::engine`). `main_with_args` has already drained stdin. Under a marker that
/// cannot be read it prints nothing (§12.8).
fn run_statusline(
    ctx: Context,
    io: &mut Io<'_>,
    json: bool,
    no_color: bool,
    provider: Option<String>,
    print_config: bool,
) -> i32 {
    if json {
        fail(io, true, KIND_USAGE, STATUSLINE_UNDER_JSON);
        return EXIT_USAGE;
    }
    let provider = provider.map_or_else(|| ProviderId::new(CLAUDE_CODE), ProviderId::new);
    let (no_color_env, force_color_env) = (ctx.no_color_env, ctx.force_color_env);
    let (engine, _http, _keychain) = statusline::engine(ctx, &provider);
    // The outer home is unknown, so the status bar shows nothing rather than a guess.
    if matches!(engine.run_shell(), RunShell::Unreadable { .. }) {
        return 0;
    }
    let result = statusline_supported(&engine, &provider).and_then(|()| {
        if print_config {
            let _ = writeln!(io.err, "{}", statusline::config_hint(engine.env()));
            return Ok(statusline::config_snippet());
        }
        let view = engine.statusline(&provider)?;
        let settings = engine.settings();
        // The line goes to Claude Code, which renders ANSI colour but is never a terminal, so
        // `auto` colours it: the same rule as `list`, with the terminal test taken as met.
        let colour = color_enabled(
            no_color,
            no_color_env,
            force_color_env,
            settings.color,
            true,
        );
        Ok(statusline::line(
            &view,
            &settings.statusline_format,
            engine.now_ms() / 1000,
            colour,
        ))
    });
    match result {
        Ok(text) => {
            let _ = write!(io.out, "{text}");
            0
        }
        Err(Failure::Engine(e)) => fail(io, false, e.kind(), &e.to_string()),
        Err(Failure::Message(kind, m)) => fail(io, false, kind, &m),
        Err(Failure::Usage(m)) => {
            fail(io, false, KIND_USAGE, &m);
            EXIT_USAGE
        }
    }
}
```

In `crates/tagteam/src/statusline.rs`, change `use crate::app::Context;` to
`use crate::app::{Context, locate};` and replace `engine` (lines 195–218) with:

```rust
/// The engine the fast path runs on, built with walls rather than trust (§13.5): a Keychain that
/// refuses every call, no profile oracle, and a lazy HTTP port whose adapter could send nothing
/// even if it were built. It is located as every command is (§12.8), from the marker file
/// alone. The settings' warnings are dropped, since a status bar has nowhere to show them. The
/// walls are returned so a test can prove nothing reached them.
pub(crate) fn engine(
    ctx: Context,
    provider: &ProviderId,
) -> (Engine, Arc<LazyHttp>, Arc<NoKeychain>) {
    let keychain = Arc::new(NoKeychain::default());
    let http = Arc::new(LazyHttp::new(|| Arc::new(NoHttp) as Arc<dyn Http>));
    let registry =
        ProviderRegistry::new().with(Arc::new(ClaudeCode::new(keychain.clone(), ctx.platform)));
    let (run_shell, env) = locate(ctx.env, &registry);
    let (settings, _warnings) = Settings::load(&env, provider);
    let engine = Engine::new(EngineConfig {
        registry,
        vault: Vault::new(Box::new(KeychainVault::new(keychain.clone()))),
        oracle: Arc::new(NoOracle),
        clock: Arc::new(SystemClock),
        http: http.clone(),
        default_provider: ProviderId::new(CLAUDE_CODE),
        settings,
        env,
        run_shell,
    });
    (engine, http, keychain)
}
```

Until Task 15, a statusline inside a valid run shell prints the default home's live login,
because the effective `Env` is the outer home's. Task 15 replaces that with the marker's account.

- [ ] **Step 4: Run them and see them pass**, then the whole suite

Run:
```
cargo test -p tagteam-engine --test run_shell --test add
cargo test -p tagteam-engine --lib engine
cargo test -p tagteam --features test-support --test run_shell_cli
cargo test -p tagteam --lib statusline
cargo test -p tagteam --lib app::tests
```
Expected: PASS.

Run: `cargo test --workspace --features tagteam/test-support`
Expected: PASS. No other existing test depended on the path rule: the only test that set
`CLAUDE_CONFIG_DIR` under `sessions/` was `add.rs`'s, rewritten above, and
`Fx::with_fallback_items` sets it to `~/.claude`, which holds no marker.

- [ ] **Step 5: Format and lint**

```
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no output from `fmt --check`; both clippy runs clean.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-provider/src/env.rs crates/tagteam-engine/src/session.rs \
  crates/tagteam-engine/src/lib.rs crates/tagteam-engine/src/engine.rs \
  crates/tagteam-engine/src/error.rs crates/tagteam-engine/src/testutil.rs \
  crates/tagteam-engine/src/lazy_http.rs crates/tagteam-engine/tests/common/mod.rs \
  crates/tagteam-engine/tests/oracle.rs crates/tagteam-engine/tests/add.rs \
  crates/tagteam-engine/tests/run_shell.rs crates/tagteam/src/app.rs \
  crates/tagteam/src/statusline.rs crates/tagteam/tests/run_shell_cli.rs
git commit -m "Find a run shell from its profile marker, and run inside it on the outer home the marker records"
```

---

### Task 9: Session state, `Owned(Session)`, the destructive guard, profile removal

§12.5: an account is session-owned while it has "a live launch reservation, or a session record
that is live **or unreadable**" (§10.3). A reservation is live while its file is locked; a record
is live by §12.6's pid and start-time rules. This task computes that state on every call
(Decision 8) and puts it to use in three places:
- **The gate's step 2** (§7.3) returns `Owned(Session)`. The `session_owned` stub, which always
  said no, is replaced, and the check becomes fallible.
- **The destructive guard** (§10.3): `remove`, and `add` over an occupied position, refuse
  while the affected account is session-owned. `move` does not.
- **Removal** (§10.3): `remove` deletes the session profile. The profile's hashed Keychain item
  goes first, named from the spelling the marker records, then the directory, whose links are
  removed as links. With an unreadable marker the item for the current canonical spelling is
  deleted instead, with a warning (Decision 12).

**Readings of the spec this task commits to:**
- **Nothing is cached** (Decision 8). One `symlink_metadata` of the profile path, one directory
  read and one non-blocking `flock` per reservation, and one parse per record. A provider
  without `sessions` has no profiles, and nothing on disk is touched for it.
- **Owned wins over unreadable.** Reservations are tested first, then records. A live one
  returns `Owned` at once. Otherwise any reservation directory, records directory or record
  that cannot be read makes the state `Unreadable`, which `owned()` counts as owned (§12.6:
  "Anything that can't be determined counts as live").
- **`session_state` has no error path.** Every I/O failure is an `Unreadable` state. It stays
  `Result` as the contract says.
- **Only the occupant is "affected" by `add --position`.** §10.3 lists `add` over an occupied
  position as destructive, because the occupant is removed. Replacing the same account's login
  (`add` or `add-token` over the existing account) is not on the list: it stale-marks a running
  profile, which §12.5 says is "never touched". The guard runs under the mutation lock and the
  occupant's account lock, before any write. That is the only time a reservation cannot appear
  (§12.5).
- **`remove_locked` deletes in §10.3's order** (the M5 amendment, `089b873`): the vault
  entries, then the account's rescue files, then the session profile, and last the row. The
  vault goes first, so no generation older than a rescue or a rotated profile can outlive it.
  An account without a vault credential is never switched to, refreshed or launched. The row
  goes last, so a `remove` that stops part-way leaves the account listed, and running it again
  finishes, since every delete treats an absent item as done.
- **The warning of Decision 12** is a `WARN` log line. `remove` has no warnings channel today
  (it returns `Result<AccountRow, EngineError>`, and the CLI shows `tracing` only at ERROR by
  default), so the line reaches the log alone. That is Decision 18's ruling: it stays a WARN
  log, and `doctor` (M5) reports orphaned items.

**Files:**
- Modify: `crates/tagteam-engine/src/engine.rs` (`EngineConfig.process`, `Engine.process`, `Engine::new`; test module's `test_config`)
- Modify: `crates/tagteam-engine/src/session.rs` (`SessionState`, `Engine::session_state`, `Engine::refuse_session_owned`)
- Modify: `crates/tagteam-engine/src/refresh.rs` (`OwnedBy::Session`'s doc, lines 34–35; `owner_of` 412–439; delete `session_owned` 441–445)
- Modify: `crates/tagteam-engine/src/error.rs` (`SessionOwned`, its `kind()` arm and pin)
- Modify: `crates/tagteam-engine/src/lifecycle.rs` (`remove_locked` 185–204, new `remove_profile`, `remove` 598–608; the guard in `add_live` after line 463 and `add_token` after line 560)
- Modify: `crates/tagteam-engine/src/testutil.rs`, `crates/tagteam-engine/src/lazy_http.rs` (the `process` field)
- Modify: `crates/tagteam/src/app.rs` (`build_engine`'s `EngineConfig`), `crates/tagteam/src/statusline.rs` (`engine`'s `EngineConfig`)
- Modify: `crates/tagteam-engine/tests/common/mod.rs` (`Fx.process`; the `process` field at every `EngineConfig` site; seed, credential, item, reservation and record helpers)
- Modify: `crates/tagteam-engine/tests/oracle.rs` (the `process` field)
- Test: `crates/tagteam-engine/tests/session.rs` (new)

**Interfaces:**
- Consumes:
  - Task 6: `tagteam_provider::liveness::{ProcessProbe, SystemProcessProbe, FakeProcessProbe, FakeProcess, RecordEntry, read_session_records, record_is_live, parse_lstart}`, `tagteam_provider::flock::LockProbe`
  - Task 7: `tagteam_provider::profile::{profile_path, canonical_profile_path, launch_reservations, ProfileMarker, Seed, LAUNCH_DIR, MARKER_FILE}`, `Provider::{launch_command, session_records_dir, profile_spelling, delete_profile_credential}`
  - Task 8: `tagteam_engine::session` (the module), `Fx::{profile_dir, make_profile, write_marker}`
- Produces:
  - `EngineConfig.process: Arc<dyn ProcessProbe>`; `Engine.process` (crate-private)
  - `SessionState { NoProfile, Quiescent { profile }, Owned { profile }, Unreadable { profile, detail } }`, `SessionState::{owned, profile}`
  - `Engine::session_state(&self, p: &dyn Provider, row: &AccountRow) -> Result<SessionState, EngineError>`
  - `Engine::refuse_session_owned(&self, p: &dyn Provider, row: &AccountRow) -> Result<(), EngineError>` (crate-private; Task 11's `plan` uses it)
  - `EngineError::SessionOwned { position: u32, label: String }`, kind `session-owned`
  - `GateOutcome::Owned(OwnedBy::Session)` from the gate's step 2
  - Fixture helpers: `Fx.process`, `LSTART`, `record_json`, `Fx::{write_seed, set_profile_credential, item_for_spelling, profile_item, hold_reservation, plant_record, live_record, dead_record}`

**Spec:**
- §12.5 "Launch reservation": a reservation is live while its file is locked; others test it
  with a non-blocking `flock`. It is created only under `MutationGuard` and the account lock.
- §12.6: records are read from `<profile>/sessions/*.json`; every `kind` counts; a malformed
  record is unreadable; anything undeterminable counts as live; unreadable records block
  destructive operations.
- §7.3 step 2: `Owned` when the account is session-owned (§12.5).
- §10.3 `remove`: the profile's hashed Keychain item first, from the marker's spelling, then the
  directory; links removed as links. "Guard": destructive commands refuse while an affected
  account is session-owned; `move` is not destructive.
- §12.2 "One spelling": every operation on the profile's credential uses the recorded spelling,
  including the hashed-item deletion in `remove`.
- §4.2: the `ProcessProbe` port.
- Decisions 7, 8 and 12.

#### Cycle A: the process port, session state, and the gate's step 2

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-engine/tests/common/mod.rs`, add to the imports:

```rust
use std::sync::atomic::{AtomicU32, Ordering};
use tagteam_provider::FlockGuard;
use tagteam_provider::liveness::{FakeProcess, FakeProcessProbe, parse_lstart};
use tagteam_provider::profile::{LAUNCH_DIR, Seed};
```

Add the field `pub process: Arc<FakeProcessProbe>,` to `Fx`, after `pub http: Arc<ScriptedHttp>,`
with the doc comment `/// Judges every session record any engine of this fixture reads (§12.6); it never looks at a real process.`
In `Fx::build`, create it beside the other ports:

```rust
        let process = Arc::new(FakeProcessProbe::new());
```

Then add `process: process.clone(),` to `Fx::build`'s `EngineConfig` literal, before
`run_shell: RunShell::Outside,`, and `process,` to the `Fx { .. }` it returns. Add
`process: self.process.clone(),` before `run_shell` in `Fx::engine_in` and
`Fx::engine_over_http`, and `process: fx.process.clone(),` before `run_shell` in
`FakeFx::new`. In `crates/tagteam-engine/tests/oracle.rs`, add `process: fx.process.clone(),`
before `run_shell: RunShell::Outside,`.

Append to `crates/tagteam-engine/tests/common/mod.rs`:

```rust
/// A session record's `procStart` as CC 2.1.286 writes it (§12.6), for `Fx::live_record`.
pub const LSTART: &str = "Thu Oct  1 12:34:56 2026";

/// A session record as CC 2.1.286 writes it (Appendix A.7), with `LSTART` as its `procStart`.
pub fn record_json(pid: u32, kind: &str) -> String {
    json!({
        "pid": pid,
        "procStart": LSTART,
        "startedAt": 1_790_000_000_000i64,
        "kind": kind,
        "sessionId": "s-1"
    })
    .to_string()
}

/// What runs in a profile, and what it holds (§12.5, §12.6).
impl Fx {
    /// Records `seed_fp` as the generation `profile` last agreed on with the vault, under
    /// `login_epoch` (§12.5), as a bootstrap would.
    pub fn write_seed(&self, profile: &Path, login_epoch: i64, seed_fp: &str) {
        Seed {
            login_epoch,
            seed_fp: seed_fp.to_owned(),
            needs_bootstrap: false,
        }
        .write(profile)
        .unwrap();
    }

    /// Leaves `bytes` as the profile's `.credentials.json` (0600), the file Claude Code reads
    /// when the profile's hashed Keychain item is absent (§12.2).
    pub fn set_profile_credential(&self, profile: &Path, bytes: &[u8]) {
        let path = profile.join(".credentials.json");
        fs::write(&path, bytes).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    /// The (service, account) of the hashed OAuth Keychain item Claude Code names from
    /// `spelling` (Appendix A.2), in the profile's environment: secure storage undefined.
    pub fn item_for_spelling(&self, spelling: &str) -> (String, String) {
        let mut env = self.env.clone();
        env.claude_config_dir = Some(spelling.into());
        env.claude_securestorage_config_dir = None;
        (keychain_service(&env, ItemKind::OAuth), keychain_account(&env))
    }

    /// `item_for_spelling` for the spelling `profile`'s marker records.
    pub fn profile_item(&self, profile: &Path) -> (String, String) {
        let Read::Present(marker) = ProfileMarker::read(profile) else {
            panic!("{} has no readable marker", profile.display())
        };
        self.item_for_spelling(&marker.config_dir)
    }

    /// A live launch reservation in `profile` (§12.5): a file under `.tagteam-launch/` this
    /// test holds an exclusive `flock` on until it drops the guard.
    pub fn hold_reservation(&self, profile: &Path) -> FlockGuard {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let name = format!(
            "{}-{}.lock",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        );
        FlockGuard::try_lock(&profile.join(LAUNCH_DIR).join(name))
            .unwrap()
            .expect("a fresh reservation file is free")
    }

    /// Writes `record` as `<name>.json` in `profile`'s session-records directory (CC:
    /// `<profile>/sessions`, §12.6). Returns its path.
    pub fn plant_record(&self, profile: &Path, name: &str, record: &[u8]) -> PathBuf {
        let dir = self.cc.session_records_dir(profile);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{name}.json"));
        fs::write(&path, record).unwrap();
        path
    }

    /// A `kind` record for `pid`, whose process the fixture's probe says still runs, started at
    /// exactly `LSTART`: live (§12.6).
    pub fn live_record(&self, profile: &Path, pid: u32, kind: &str) -> PathBuf {
        self.process.set(
            pid,
            FakeProcess {
                exists: Some(true),
                start_time_s: parse_lstart(LSTART),
                ..FakeProcess::default()
            },
        );
        self.plant_record(profile, &pid.to_string(), record_json(pid, kind).as_bytes())
    }

    /// A record for `pid`, which the fixture's probe has never heard of: dead (§12.6).
    pub fn dead_record(&self, profile: &Path, pid: u32) -> PathBuf {
        self.plant_record(
            profile,
            &pid.to_string(),
            record_json(pid, "interactive").as_bytes(),
        )
    }
}
```

Create `crates/tagteam-engine/tests/session.rs`:

```rust
//! §12.5 and §12.6: session ownership from launch reservations and session records, the gate's
//! step 2 on it, and §10.3's destructive guard and profile removal.

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;

use common::{Fx, LSTART, due, token_requests};
use tagteam_core::AccountId;
use tagteam_engine::refresh::{GateOutcome, OwnedBy};
use tagteam_engine::session::SessionState;
use tagteam_provider::Provider;
use tagteam_provider::liveness::{FakeProcess, parse_lstart};
use tagteam_provider::profile::LAUNCH_DIR;

fn state(fx: &Fx, id: &AccountId) -> SessionState {
    let row = fx.engine.store().unwrap().account(id).unwrap().unwrap();
    fx.engine.session_state(fx.cc.as_ref(), &row).unwrap()
}

/// The gate on `id`, with the vault's current bytes as the caller's snapshot.
fn gate(fx: &Fx, id: &AccountId) -> GateOutcome {
    let snapshot = fx.vault_bytes(id).unwrap();
    fx.engine
        .refresh_stored(fx.cc.as_ref(), id, &snapshot)
        .unwrap()
}

#[test]
fn an_account_without_a_profile_has_no_session() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let s = state(&fx, &a);
    assert_eq!(s, SessionState::NoProfile);
    assert!(!s.owned());
    assert_eq!(s.profile(), None);
}

#[test]
fn a_profile_with_nothing_running_is_quiescent() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let dir = fx.make_profile(&a);
    let s = state(&fx, &a);
    assert_eq!(s, SessionState::Quiescent { profile: dir.clone() });
    assert!(!s.owned());
    assert_eq!(s.profile(), Some(dir.as_path()));
}

#[test]
fn a_held_reservation_owns_the_account_and_a_released_one_does_not() {
    // §12.5: a reservation is live while its file is locked, and only then.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let dir = fx.make_profile(&a);
    let held = fx.hold_reservation(&dir);
    let s = state(&fx, &a);
    assert_eq!(s, SessionState::Owned { profile: dir.clone() });
    assert!(s.owned());
    drop(held);
    assert!(
        fs::read_dir(dir.join(LAUNCH_DIR)).unwrap().next().is_some(),
        "the file is still there"
    );
    assert_eq!(state(&fx, &a), SessionState::Quiescent { profile: dir });
}

#[test]
fn a_live_record_of_any_kind_owns_the_account() {
    // §12.6: `bg` and `daemon` records count, so CC's daemon keeps the account session-owned
    // after the last `run` (§15.2 "Liveness").
    for kind in ["interactive", "bg", "daemon"] {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let dir = fx.make_profile(&a);
        fx.live_record(&dir, 4242, kind);
        assert_eq!(state(&fx, &a), SessionState::Owned { profile: dir }, "{kind}");
    }
}

#[test]
fn a_dead_or_recycled_record_does_not_own_the_account() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let dir = fx.make_profile(&a);
    fx.dead_record(&dir, 4242);
    assert_eq!(state(&fx, &a), SessionState::Quiescent { profile: dir.clone() });
    // Review Focus 2's case: the pid runs again, started at another time, and it is not a
    // claude process (Decision 16).
    fx.process.set(
        4242,
        FakeProcess {
            exists: Some(true),
            start_time_s: parse_lstart(LSTART).map(|s| s + 3_600),
            mentions_launch: Some(false),
            ..FakeProcess::default()
        },
    );
    assert_eq!(state(&fx, &a), SessionState::Quiescent { profile: dir.clone() });
    // The same mismatch on a process that mentions `claude` may be the session itself, under a
    // start time the wall clock moved (Decision 16): it still owns the account.
    fx.process.set(
        4242,
        FakeProcess {
            exists: Some(true),
            start_time_s: parse_lstart(LSTART).map(|s| s + 3_600),
            mentions_launch: Some(true),
            ..FakeProcess::default()
        },
    );
    assert_eq!(state(&fx, &a), SessionState::Owned { profile: dir });
}

#[test]
fn an_unreadable_record_counts_as_owned() {
    // §12.6: a malformed record is unreadable, and unreadable counts as owned (§10.3).
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let dir = fx.make_profile(&a);
    fx.dead_record(&dir, 4242);
    fx.plant_record(&dir, "torn", b"{\"pid\":");
    let s = state(&fx, &a);
    let SessionState::Unreadable { profile, detail } = &s else {
        panic!("{s:?}")
    };
    assert_eq!(profile, &dir);
    assert!(detail.contains("torn.json"), "{detail}");
    assert!(s.owned());
}

#[test]
fn a_directory_that_cannot_be_listed_counts_as_owned() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let dir = fx.make_profile(&a);
    for blocked in [dir.join(LAUNCH_DIR), fx.cc.session_records_dir(&dir)] {
        fs::create_dir_all(&blocked).unwrap();
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o000)).unwrap();
        let s = state(&fx, &a);
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            matches!(s, SessionState::Unreadable { .. }),
            "{}: {s:?}",
            blocked.display()
        );
        assert!(s.owned());
    }
}

#[test]
fn the_gate_leaves_a_session_owned_token_alone() {
    // §7.3 step 2: a held reservation, a live record of any kind, or an unreadable record.
    let fx = Fx::new();
    let a = due(&fx);
    fx.script_refresh(Some("rt-a2"));
    let dir = fx.make_profile(&a);
    let held = fx.hold_reservation(&dir);
    assert!(matches!(gate(&fx, &a), GateOutcome::Owned(OwnedBy::Session)));
    drop(held);
    let record = fx.live_record(&dir, 4242, "daemon");
    assert!(matches!(gate(&fx, &a), GateOutcome::Owned(OwnedBy::Session)));
    fs::remove_file(record).unwrap();
    fx.plant_record(&dir, "torn", b"{\"pid\":");
    assert!(matches!(gate(&fx, &a), GateOutcome::Owned(OwnedBy::Session)));
    assert_eq!(token_requests(&fx), 0);
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
}

#[test]
fn a_session_that_ended_leaves_the_token_to_the_gate_again() {
    let fx = Fx::new();
    let a = due(&fx);
    fx.script_refresh(Some("rt-a2"));
    let dir = fx.make_profile(&a);
    drop(fx.hold_reservation(&dir));
    fx.dead_record(&dir, 4242);
    assert!(matches!(gate(&fx, &a), GateOutcome::Refreshed(_)));
    assert_eq!(token_requests(&fx), 1);
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-engine --test session`
Expected: compile error, `struct EngineConfig has no field named process` (from
`tests/common/mod.rs`), and `unresolved import tagteam_engine::session::SessionState`.

- [ ] **Step 3: Implement**

**The port in the engine.** In `crates/tagteam-engine/src/engine.rs`, add
`use tagteam_provider::liveness::ProcessProbe;` and, in `EngineConfig` before `run_shell`:

```rust
    /// §4.2's process port, for session records (§12.6).
    pub process: Arc<dyn ProcessProbe>,
```

In `Engine`, before `run_shell`:

```rust
    /// Judges session records (§12.6): `SystemProcessProbe` in production.
    pub(crate) process: Arc<dyn ProcessProbe>,
```

In `Engine::new`, before `run_shell: cfg.run_shell,`:

```rust
            process: cfg.process,
```

Add `process: Arc::new(tagteam_provider::liveness::FakeProcessProbe::new()),` before
`run_shell` in the test module's `test_config`, in `testutil.rs`'s `T::new`, and in
`lazy_http.rs`'s `an_engine_over_a_lazy_client_does_not_build_it`.

In `crates/tagteam/src/app.rs` and `crates/tagteam/src/statusline.rs`, add
`use tagteam_provider::liveness::SystemProcessProbe;` and `process: Arc::new(SystemProcessProbe),`
before `run_shell` in `build_engine`'s and `engine`'s `EngineConfig` literals. The statusline
never computes session state (Task 15 reads the marker), so the port is never called there.

**Session state.** In `crates/tagteam-engine/src/session.rs`, extend the module doc with
"and §12.5's session state: whether a session owns an account", and change the imports to:

```rust
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use tagteam_provider::flock::LockProbe;
use tagteam_provider::liveness::{RecordEntry, read_session_records, record_is_live};
use tagteam_provider::profile::{
    MARKER_FILE, ProfileMarker, RunShell, launch_reservations, profile_path,
};
use tagteam_provider::{Env, Provider, Read};

use crate::engine::Engine;
use crate::error::EngineError;
use crate::registry::ProviderRegistry;
use crate::store::AccountRow;
```

Append:

```rust
/// §12.5: whether a session owns an account, computed on each call and never cached
/// (Decision 8).
#[derive(Debug, Clone, PartialEq)]
pub enum SessionState {
    NoProfile,
    Quiescent { profile: PathBuf },
    Owned { profile: PathBuf },
    /// A reservation or a record could not be read: counts as owned (§10.3, §12.6).
    Unreadable { profile: PathBuf, detail: String },
}

impl SessionState {
    /// Session-owned (§12.5): a live reservation or record, or one that could not be read.
    pub fn owned(&self) -> bool {
        matches!(
            self,
            SessionState::Owned { .. } | SessionState::Unreadable { .. }
        )
    }

    /// The profile directory, when the account has one.
    pub fn profile(&self) -> Option<&Path> {
        match self {
            SessionState::NoProfile => None,
            SessionState::Quiescent { profile }
            | SessionState::Owned { profile }
            | SessionState::Unreadable { profile, .. } => Some(profile),
        }
    }
}

fn unreadable_state(profile: &Path, detail: String) -> SessionState {
    SessionState::Unreadable {
        profile: profile.to_path_buf(),
        detail,
    }
}

impl Engine {
    /// §12.5: reservations (any `Held`) and session records (`record_is_live`, any `Unreadable`).
    ///
    /// The profile is `profile_path(env, id)` (§5). A held reservation, then a live record,
    /// makes the account `Owned`; failing that, anything that could not be read makes it
    /// `Unreadable`. Every I/O failure is a state, never an error. A provider without
    /// `sessions` has no profiles, and nothing on disk is touched for it.
    pub fn session_state(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
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
        match launch_reservations(&profile) {
            Read::Present(found) => {
                if found.iter().any(|(_, probe)| *probe == LockProbe::Held) {
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

**The gate's step 2.** In `crates/tagteam-engine/src/refresh.rs`, change `OwnedBy::Session`'s
doc comment to:

```rust
    /// A `tagteam run` session owns it (§12.5): a live launch reservation, or a session record
    /// that is live or unreadable.
```

Replace `owner_of` (lines 412–439) with the function below, and delete `session_owned`
(lines 441–445):

```rust
    /// §7.3 step 2: whether the account's token is someone else's to refresh.
    fn owner_of(
        &self,
        p: &dyn Provider,
        store: &Store,
        row: &AccountRow,
    ) -> Result<Option<OwnedBy>, EngineError> {
        let live = match p.live_identity(&self.env) {
            Read::Present(i) => p.identity_key(&i).as_str() == row.identity_key,
            Read::Absent => false,
            // It may be this account's login, and the gate never refreshes what might be live.
            Read::Unreadable(_) => true,
        };
        if live {
            return Ok(Some(OwnedBy::Live));
        }
        let journaled = store
            .journals()?
            .iter()
            .any(|j| j.to_id == row.id || j.from_id.as_ref() == Some(&row.id));
        if journaled {
            return Ok(Some(OwnedBy::Journal));
        }
        // A reservation is created only under this account's lock (§12.5), which the gate holds,
        // so no session can start before the request is sent.
        if self.session_state(p, row)?.owned() {
            return Ok(Some(OwnedBy::Session));
        }
        Ok(None)
    }
```

`owner_of`'s one caller, `refresh_stored`, is unchanged.

- [ ] **Step 4: Run them and see them pass**, then the whole suite

Run: `cargo test -p tagteam-engine --test session`
Expected: PASS.

Run: `cargo test --workspace --features tagteam/test-support`
Expected: PASS. Every existing account has no profile, so `owner_of` gives the same answer as
before.

- [ ] **Step 5: Format and lint**

```
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-engine/src/engine.rs crates/tagteam-engine/src/session.rs \
  crates/tagteam-engine/src/refresh.rs crates/tagteam-engine/src/testutil.rs \
  crates/tagteam-engine/src/lazy_http.rs crates/tagteam/src/app.rs \
  crates/tagteam/src/statusline.rs crates/tagteam-engine/tests/common/mod.rs \
  crates/tagteam-engine/tests/oracle.rs crates/tagteam-engine/tests/session.rs
git commit -m "Judge session ownership from launch reservations and session records, so the gate leaves a session's token alone"
```

#### Cycle B: the destructive guard and profile removal

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-engine/tests/session.rs`, replace the imports with:

```rust
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};

use common::{API_KEY, Fx, LSTART, capture_logs, credential, due, token_requests};
use tagteam_cc::live::Platform;
use tagteam_core::AccountId;
use tagteam_engine::lifecycle::{AddOptions, AddTokenOptions};
use tagteam_engine::refresh::{GateOutcome, OwnedBy};
use tagteam_engine::session::SessionState;
use tagteam_provider::liveness::{FakeProcess, parse_lstart};
use tagteam_provider::profile::{LAUNCH_DIR, MARKER_FILE, ProfileMarker, canonical_profile_path};
use tagteam_provider::{Provider, Read};
```

and append:

```rust
#[test]
fn remove_refuses_while_a_session_owns_the_account() {
    // §10.3 Guard.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let dir = fx.make_profile(&a);
    let held = fx.hold_reservation(&dir);
    let err = fx.engine.remove(&a).unwrap_err();
    assert_eq!(err.kind(), "session-owned", "{err}");
    assert!(err.to_string().contains("tagteam run"), "{err}");
    assert!(fx.vault_bytes(&a).is_some());
    assert!(fx.engine.store().unwrap().account(&a).unwrap().is_some());
    assert!(dir.join(MARKER_FILE).exists());
    drop(held);
    fx.engine.remove(&a).unwrap();
}

#[test]
fn remove_refuses_while_a_session_record_is_unreadable() {
    // §12.6: unreadable records block destructive operations.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let dir = fx.make_profile(&a);
    fx.plant_record(&dir, "torn", b"[1,");
    assert_eq!(fx.engine.remove(&a).unwrap_err().kind(), "session-owned");
    assert!(fx.engine.store().unwrap().account(&a).unwrap().is_some());
}

#[test]
fn add_over_a_session_owned_occupant_refuses_before_writing_anything() {
    // §10.3 Guard: the occupant is the account `add --position` removes.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let _held = fx.hold_reservation(&fx.make_profile(&a));
    fx.login("c@x.co", "rt-c");
    let err = fx
        .engine
        .add_live(AddOptions {
            position: Some(1),
            yes: true,
            ..fx.add_options()
        })
        .unwrap_err();
    assert_eq!(err.kind(), "session-owned", "{err}");
    let err = fx
        .engine
        .add_token(AddTokenOptions {
            position: Some(1),
            yes: true,
            ..fx.add_token_options(API_KEY)
        })
        .unwrap_err();
    assert_eq!(err.kind(), "session-owned", "{err}");
    let rows = fx.engine.store().unwrap().accounts(&fx.provider()).unwrap();
    let held: Vec<(u32, &str)> = rows.iter().map(|r| (r.position, r.label.as_str())).collect();
    assert_eq!(held, [(1, "a@x.co"), (2, "b@x.co")]);
    assert!(fx.vault_bytes(&a).is_some());
}

#[test]
fn replacing_a_session_owned_accounts_login_is_not_destructive() {
    // §10.3 lists what is destructive; an explicit replacement only stale-marks a running
    // profile, which is never touched (§12.5).
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let dir = fx.make_profile(&a);
    let _held = fx.hold_reservation(&dir);
    fx.login("a@x.co", "rt-a2");
    fx.engine.add_live(fx.add_options()).unwrap();
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"));
    assert!(dir.join(MARKER_FILE).exists(), "the running profile is untouched");
}

#[test]
fn move_is_not_destructive() {
    // §10.3: positions are display order only, and a profile is keyed by the account's ID.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let _held = fx.hold_reservation(&fx.make_profile(&a));
    assert_eq!(fx.engine.move_to(&a, 2).unwrap().position, 2);
}

#[test]
fn remove_deletes_the_profile_its_item_first_and_its_links_as_links() {
    // §10.3: within the profile, its hashed item goes before its directory.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let dir = fx.make_profile(&a);
    let projects = fx.env.home.join(".claude/projects");
    symlink(&projects, dir.join("projects")).unwrap();
    let (svc, acct) = fx.profile_item(&dir);
    fx.kc.put(&svc, &acct, &credential("a@x.co", "rt-a2"));
    fx.engine.remove(&a).unwrap();
    assert!(fs::symlink_metadata(&dir).is_err(), "the profile is gone");
    assert_eq!(fx.kc.get(&svc, &acct), None, "its hashed item is gone");
    assert!(
        projects.join("-work-app/memory/MEMORY.md").exists(),
        "nothing a link points at is touched"
    );
    assert!(fx.engine.store().unwrap().account(&a).unwrap().is_none());
}

#[test]
fn remove_deletes_the_item_under_the_spelling_the_marker_records() {
    // §12.2 "One spelling": never a spelling derived again. Here the data directory moved
    // since the profile was exported, so the canonical spelling is another one.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let dir = fx.make_profile(&a);
    let Read::Present(mut marker) = ProfileMarker::read(&dir) else {
        panic!("the fixture wrote a marker")
    };
    let canonical = marker.config_dir.clone();
    marker.config_dir = "/old/data/tagteam/sessions/a".into();
    marker.write(&dir).unwrap();
    let (old_svc, acct) = fx.item_for_spelling(&marker.config_dir);
    let (canonical_svc, _) = fx.item_for_spelling(&canonical);
    fx.kc.put(&old_svc, &acct, b"recorded");
    fx.kc.put(&canonical_svc, &acct, b"derived");
    fx.engine.remove(&a).unwrap();
    assert_eq!(fx.kc.get(&old_svc, &acct), None);
    assert_eq!(fx.kc.get(&canonical_svc, &acct).as_deref(), Some(&b"derived"[..]));
}

#[test]
fn remove_with_an_unreadable_marker_deletes_the_current_item_and_warns() {
    // Decision 12: refusing would leave an account that cannot be removed.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let dir = fx.make_profile(&a);
    let canonical = fx
        .cc
        .profile_spelling(&canonical_profile_path(&dir).unwrap());
    let (svc, acct) = fx.item_for_spelling(&canonical);
    fx.kc.put(&svc, &acct, b"x");
    fs::write(dir.join(MARKER_FILE), "{ torn").unwrap();
    let (result, logs) = capture_logs(|| fx.engine.remove(&a));
    result.unwrap();
    assert_eq!(fx.kc.get(&svc, &acct), None);
    assert!(fs::symlink_metadata(&dir).is_err());
    assert!(
        logs.iter()
            .any(|l| l.contains("WARN") && l.contains("older spelling")),
        "{logs:?}"
    );
}

#[test]
fn remove_never_trusts_a_marker_that_names_another_account() {
    // A marker copied from a's profile into b's names a's spelling. Removing b must delete b's
    // item under b's own canonical spelling, and leave a's item, which a running session of a
    // may be using, alone.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.add("c@x.co", "rt-c"); // live: c, so a and b are both removable
    let dir_a = fx.make_profile(&a);
    let dir_b = fx.make_profile(&b);
    let Read::Present(marker_a) = ProfileMarker::read(&dir_a) else {
        panic!("the fixture wrote a marker")
    };
    fs::copy(dir_a.join(MARKER_FILE), dir_b.join(MARKER_FILE)).unwrap();
    let (a_svc, acct) = fx.item_for_spelling(&marker_a.config_dir);
    let b_spelling = fx
        .cc
        .profile_spelling(&canonical_profile_path(&dir_b).unwrap());
    let (b_svc, _) = fx.item_for_spelling(&b_spelling);
    fx.kc.put(&a_svc, &acct, b"a's");
    fx.kc.put(&b_svc, &acct, b"b's");

    let (result, logs) = capture_logs(|| fx.engine.remove(&b));
    result.unwrap();

    assert_eq!(fx.kc.get(&a_svc, &acct).as_deref(), Some(&b"a's"[..]));
    assert_eq!(fx.kc.get(&b_svc, &acct), None);
    assert!(fs::symlink_metadata(&dir_b).is_err());
    assert!(fs::symlink_metadata(&dir_a).is_ok());
    assert!(
        logs.iter()
            .any(|l| l.contains("WARN") && l.contains("names another account")),
        "{logs:?}"
    );
}

#[test]
fn a_remove_that_stops_at_the_profile_has_already_deleted_the_vault_and_keeps_the_row() {
    // §10.3's order: the vault (and any rescue) goes before the profile, so a stop at the
    // profile leaves no older generation behind a newer profile one; the row goes last, so the
    // account stays listed and running `remove` again finishes.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let dir = fx.make_profile(&a);
    let (svc, acct) = fx.profile_item(&dir);
    fx.kc.put(&svc, &acct, &credential("a@x.co", "rt-a2"));
    fx.kc.set_fail_delete(&svc, true);
    assert!(fx.engine.remove(&a).is_err());
    assert!(fx.vault_bytes(&a).is_none(), "the vault went first");
    assert!(dir.join(MARKER_FILE).exists(), "the directory goes only after the item");
    assert!(
        fx.engine.store().unwrap().account(&a).unwrap().is_some(),
        "the row goes last"
    );
    fx.kc.set_fail_delete(&svc, false);
    fx.engine.remove(&a).unwrap();
    assert_eq!(fx.kc.get(&svc, &acct), None);
    assert!(fs::symlink_metadata(&dir).is_err());
    assert!(fx.engine.store().unwrap().account(&a).unwrap().is_none());
}

#[test]
fn on_linux_remove_deletes_the_profile_directory_and_touches_no_keychain() {
    let fx = Fx::with_platform(Platform::Linux);
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let dir = fx.make_profile(&a);
    fx.set_profile_credential(&dir, &credential("a@x.co", "rt-a2"));
    fx.engine.remove(&a).unwrap();
    assert!(fs::symlink_metadata(&dir).is_err());
    assert!(fx.kc.items().is_empty(), "Linux has no Keychain");
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-engine --test session`
Expected: the new tests fail, each at its first assertion on the remove or add:
- `remove_refuses_while_a_session_owns_the_account` and
  `remove_refuses_while_a_session_record_is_unreadable`: `unwrap_err` on `Ok`, because the
  remove goes through.
- `add_over_a_session_owned_occupant_refuses_before_writing_anything`: `unwrap_err` on `Ok`.
- `remove_deletes_the_profile_its_item_first_and_its_links_as_links`: the profile still exists.
- `remove_deletes_the_item_under_the_spelling_the_marker_records`: the recorded item is still
  there.
- `remove_with_an_unreadable_marker_deletes_the_current_item_and_warns`: the item is still
  there.
- `a_remove_that_stops_at_the_profile_has_already_deleted_the_vault_and_keeps_the_row`: the
  first remove succeeds, since nothing deletes the profile yet.
- `on_linux_remove_deletes_the_profile_directory_and_touches_no_keychain`: the directory still
  exists.

`replacing_a_session_owned_accounts_login_is_not_destructive` and `move_is_not_destructive`
already pass, and must keep passing.

- [ ] **Step 3: Implement**

**The error.** In `crates/tagteam-engine/src/error.rs`, add after `ForeignLiveCredential`:

```rust
    /// §9.2, §10.3: the account is session-owned (§12.5). Activating it would give the default
    /// home and the session two copies of one single-use refresh token; destroying it would
    /// pull the login from under a running session.
    #[error("position {position} ({label}) is in use by a `tagteam run` session; exit that session first")]
    SessionOwned { position: u32, label: String },
```

In `kind()`, after the `ForeignLiveCredential` arm:

```rust
            EngineError::SessionOwned { .. } => "session-owned",
```

In `kind_is_pinned_for_every_variant`, after the `ForeignLiveCredential` case:

```rust
            (
                EngineError::SessionOwned {
                    position: 1,
                    label: "a".into(),
                },
                "session-owned",
            ),
```

**The guard.** Append to `impl Engine` in `crates/tagteam-engine/src/session.rs`:

```rust
    /// §10.3 Guard and §9.2's session-owned target: refuses while `row` is session-owned. The
    /// answer holds while the caller holds the mutation lock and `row`'s account lock: no
    /// reservation is created without both (§12.5).
    pub(crate) fn refuse_session_owned(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
    ) -> Result<(), EngineError> {
        let state = self.session_state(p, row)?;
        if let SessionState::Unreadable { detail, .. } = &state {
            tracing::warn!(
                position = row.position,
                account = %row.id,
                "a session reservation or record could not be read ({detail}); the account counts as session-owned"
            );
        }
        if state.owned() {
            return Err(EngineError::SessionOwned {
                position: row.position,
                label: row.label.clone(),
            });
        }
        Ok(())
    }
```

**`add` over an occupied position.** In `crates/tagteam-engine/src/lifecycle.rs`, in `add_live`,
replace:

```rust
        check_identity_conflict(
            store.account(&prep.id)?.as_ref(),
            claimed_uuid,
            &identity.label,
        )?;
        let live_locks = p.lock_live(&self.env, &guard)?;
```

with:

```rust
        check_identity_conflict(
            store.account(&prep.id)?.as_ref(),
            claimed_uuid,
            &identity.label,
        )?;
        // §10.3 Guard: replacing an occupant removes it. Its lock is held now, so no session can
        // start on it before the remove.
        if let Some(occupant) = &prep.occupant {
            self.refuse_session_owned(p.as_ref(), occupant)?;
        }
        let live_locks = p.lock_live(&self.env, &guard)?;
```

In `add_token`, replace:

```rust
        let current = store.account(&prep.id)?;
        check_identity_conflict(current.as_ref(), claimed_uuid, &identity.label)?;
```

with:

```rust
        let current = store.account(&prep.id)?;
        check_identity_conflict(current.as_ref(), claimed_uuid, &identity.label)?;
        // §10.3 Guard, as in `add_live`.
        if let Some(occupant) = &prep.occupant {
            self.refuse_session_owned(p.as_ref(), occupant)?;
        }
```

These are anchored edits rather than whole functions because Task 2 rewrites other parts of
both `add_live` and `add_token` (replacement evidence and the activation epoch). The anchors
are the post-lock identity re-checks, which Task 2 does not move.

**`remove`.** Add to `lifecycle.rs`'s imports:

```rust
use std::fs;
use std::io;

use tagteam_provider::profile::{ProfileMarker, canonical_profile_path, profile_path};
```

Replace `remove_locked` (lines 185–204) with:

```rust
    /// §10.3's order: the vault entries (strict), then the account's rescue files (§6.3: each
    /// holds a live refresh token, readable or not, and none is left behind), then the session
    /// profile (`remove_profile`), and last the row (which cascades). The vault goes first, so
    /// no generation older than a rescue or a rotated profile can outlive it: an account
    /// without a vault credential is never switched to, refreshed or launched. Anything that
    /// fails stops before the row goes, so the account stays listed and the remove can be run
    /// again; every delete treats an absent item as done. The caller holds the mutation lock and
    /// this account's lock, and has refused a session-owned account. The live login is never
    /// touched.
    pub(crate) fn remove_locked(
        &self,
        row: &AccountRow,
        lock: &AccountLock,
    ) -> Result<(), EngineError> {
        let p = self.provider(&row.provider)?;
        self.vault.delete(lock)?;
        for rescue in self.rescues_for(&row.id) {
            let (RescueFile::Entry(RescueEntry { path, .. }) | RescueFile::Unreadable { path, .. }) =
                rescue;
            self.delete_rescue(&path)?;
        }
        self.remove_profile(p.as_ref(), row)?;
        self.store()?.delete_account(&row.id)?;
        self.event(&row.provider, "remove", Some(&row.id), None)?;
        Ok(())
    }

    /// §10.3: deletes `row`'s session profile, if it has one. The agent's credential items for
    /// the spelling the marker records go first (§12.2: never a spelling derived again), then
    /// the directory, whose links are removed as links: nothing they point to is touched. A
    /// marker is trusted only when it names this account and its provider: a marker copied from
    /// another account's profile names that account's spelling, whose Keychain item must never
    /// be deleted here. With no trusted marker, the items for the profile's current canonical
    /// spelling are deleted instead, and a warning says an item under an older spelling may
    /// remain (Decision 12). A failure stops before the directory goes.
    fn remove_profile(&self, p: &dyn Provider, row: &AccountRow) -> Result<(), EngineError> {
        let profile = profile_path(&self.env, &row.id);
        let meta = match fs::symlink_metadata(&profile) {
            Ok(meta) => meta,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        let current_spelling = |why: String| -> Result<String, EngineError> {
            tracing::warn!(
                position = row.position,
                account = %row.id,
                "the session profile's marker could not be read ({why}); deleting its Keychain item under its current spelling, so an item under an older spelling may remain"
            );
            Ok(p.profile_spelling(&canonical_profile_path(&profile)?))
        };
        let spelling = match ProfileMarker::read(&profile) {
            Read::Present(marker)
                if marker.account_id == row.id && marker.provider == row.provider =>
            {
                marker.config_dir
            }
            Read::Present(_) => current_spelling("it names another account".into())?,
            Read::Absent => current_spelling("it has none".into())?,
            Read::Unreadable(e) => current_spelling(e.to_string())?,
        };
        p.delete_profile_credential(&self.env, &spelling)?;
        // `remove_dir_all` removes a symlink inside the profile as a link, never following it.
        if meta.is_dir() {
            fs::remove_dir_all(&profile)?;
        } else {
            fs::remove_file(&profile)?;
        }
        Ok(())
    }
```

`remove_locked`'s two callers are `remove` (below) and `commit_login`'s occupant removal, whose
caller has already applied the guard.

Replace `remove` (lines 598–608) with:

```rust
    /// §10.3. The live login is never touched.
    pub fn remove(&self, id: &AccountId) -> Result<AccountRow, EngineError> {
        self.refuse_inside_run_shell()?;
        let provider = self.managed_row(id)?.provider;
        self.settle_or_refuse(&provider)?;
        let _guard = self.guard_or_refuse(&provider)?;
        let row = self.managed_row(id)?;
        let lock = self.lock_account(id)?;
        // §10.3 Guard: under the mutation lock and the account lock, no session can start
        // before the remove is done (§12.5).
        let p = self.provider(&row.provider)?;
        self.refuse_session_owned(p.as_ref(), &row)?;
        self.remove_locked(&row, &lock)?;
        Ok(row)
    }
```

- [ ] **Step 4: Run them and see them pass**, then the whole suite

Run: `cargo test -p tagteam-engine --test session`
Expected: PASS.

Run: `cargo test --workspace --features tagteam/test-support`
Expected: PASS. Every existing `remove` test has no profile, so `remove_profile` returns at its
first `symlink_metadata`. The invariant tests (`tests/invariant.rs`, `tests/fake_agent.rs`
"remove") see no new write.

- [ ] **Step 5: Format and lint**

```
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-engine/src/error.rs crates/tagteam-engine/src/session.rs \
  crates/tagteam-engine/src/lifecycle.rs crates/tagteam-engine/tests/session.rs
git commit -m "Refuse to remove or replace a session-owned account, and delete an account's session profile with it"
```

---

### Task 10: Provenance and lazy capture in the gate

§12.5: "Every comparison between a profile's credential P and the vault's V is made against the
seed S, never by expiry, because a rotated token need not expire later than the one it
replaced." §7.3 step 3: "If the account's profile is quiescent, apply its provenance (§12.5). If
the profile rotated since its seed, adopt its generation into the vault first. If provenance
reports a conflict, return `Conflict` without making a request."

This task writes the table as a pure function in `tagteam-core` and applies it in the engine,
under the account lock. It hooks that into the refresh gate between rescue settlement and the
second vault read. A lazy capture (§12.5) runs in the gate whenever a usage collection or a
`switch`'s freshen needs a refresh. The gate then picks up the adopted generation with no
further change. Task 11 applies the same check in `switch`'s transaction.

**Readings of the spec this task commits to:**
- **What "applies" means.** Provenance applies only to a quiescent profile that has a seed. A
  profile without one was never bootstrapped, and M4a has no bootstrap. It also needs a marker
  for this account and provider, whose `configDir` is the spelling every credential read uses
  (§12.2).
- **Identity drift ignores the profile.** The profile's identity is compared with the
  account's: the email (the label, for a provider whose identities carry none), and the
  organization when both name one. If they differ, or the profile names no login, provenance
  does not apply (`NotApplicable`): nothing is captured and the gate proceeds as without a
  profile. The rule is `identity_drifted`, defined once here as `pub(crate)`; Task 12's
  session-owned usage fetch calls the same function.
- **Only a credential with a refresh token is a generation.** A profile credential with no
  refresh token, or no fingerprint, is never captured (§6.2). Provenance does not apply to it.
  This is also what a profile holds after Claude Code wiped it on `invalid_grant`.
- **Unreadable stops the decision** (Decision 9). The gate returns
  `Transient { kind: "profile-unreadable" }` and sends nothing when any of these cannot be read:
  - the seed;
  - the marker (or it is absent beside a seed, or names another account);
  - the profile's identity;
  - its credential, or the credential is degraded.

  An unreadable vault is the gate's `vault-unreadable`, as at its other two vault reads.
- **The seed must move with the capture.** After a capture, or a reseed for `InStep`, the seed
  is written before the gate goes on. A failed write is returned as an error, and nothing is
  sent. The reason: the gate's next step may refresh the vault. With the seed left behind, the
  next comparison would see both sides moved and report a false `Conflict`. The next pass finds
  P = V and reseeds, so nothing is lost.
- **The stale mark is the seed's epoch against the account's.** A profile is stale-marked when
  `seed.login_epoch != row.login_epoch` (§12.5). The row is the one the gate read after
  reconciling any pending replacement, so the epoch is current under the lock.

**Files:**
- Create: `crates/tagteam-core/src/provenance.rs`
- Modify: `crates/tagteam-core/src/lib.rs` (`pub mod provenance;`, re-export)
- Create: `crates/tagteam-engine/src/provenance.rs` (`ProfileCheck`, `identity_drifted`, `Engine::apply_provenance`; a unit-test module for the drift rule)
- Modify: `crates/tagteam-engine/src/lib.rs` (`pub mod provenance;`)
- Modify: `crates/tagteam-engine/src/refresh.rs` (`GateOutcome::Conflict`'s doc, lines 47–48; `refresh_stored` 299–397)
- Test: `crates/tagteam-engine/tests/provenance.rs` (new)

**Interfaces:**
- Consumes:
  - Task 7: `tagteam_provider::profile::{ProfileMarker, Seed, MARKER_FILE, SEED_FILE}`, `Provider::{read_profile_credential, profile_identity}`
  - Task 9: `Engine::session_state`, `SessionState::Quiescent`; `Fx::{make_profile, write_seed, set_profile_credential, profile_item, hold_reservation}`
  - Existing: `Engine::persist_generation(&self, p, row, lock, bytes)`, `refresh.rs`'s `transient`
- Produces:
  - `tagteam_core::provenance::{ProvenanceVerdict, provenance}`, re-exported as `tagteam_core::{ProvenanceVerdict, provenance}`
  - `tagteam_engine::provenance::ProfileCheck`
  - `Engine::apply_provenance(&self, p: &dyn Provider, row: &AccountRow, lock: &AccountLock) -> Result<ProfileCheck, EngineError>` (crate-private; Task 11's `transact` uses it)
  - `crate::provenance::identity_drifted(identity: &Identity, row: &AccountRow) -> bool` (crate-private; Task 12's `collect.rs` uses it)
  - `GateOutcome::Conflict` and `Transient { kind: "profile-unreadable" }` from the gate

**Spec:**
- §12.5 "Profile provenance": the seed file's two fields; the stale mark; the five-row table;
  never by expiry.
- §12.5 "Lazy capture": quiescent, rotated according to its provenance, and the same identity;
  under the account lock; at the gate.
- §12.5 "Identity drift": a profile whose email, or org when both are set, differs is ignored.
- §7.3 step 3, third bullet: adopt a rotated profile first; return `Conflict` without a request.
- §6.2 "What automatic captures may write": never degraded, never a credential without a
  refresh token over one with it, a profile capture needs quiescence and provenance, never a
  stale-marked profile.
- §15.2 "Provenance": every row of the table, including a rotated refresh token whose
  `expiresAt` does not increase.
- B.50, B.52; Decision 9.

#### Cycle A: the table

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam-core/src/provenance.rs` with only its test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const P: &str = "sha256:p";
    const V: &str = "sha256:v";
    const S: &str = "sha256:s";

    #[test]
    fn every_row_of_the_table() {
        use ProvenanceVerdict::*;
        // (P, V, S, stale-marked) and the verdict, for each row of §12.5's table, with both
        // stale marks wherever the row does not fix it.
        let rows = [
            ((V, V, V, false), InStep { reseed: false }),
            ((V, V, S, false), InStep { reseed: true }),
            ((V, V, V, true), InStep { reseed: false }),
            ((V, V, S, true), InStep { reseed: true }),
            ((P, V, V, false), Capture),
            ((P, V, V, true), ReplacementWins),
            ((P, V, P, false), VaultMovedOn),
            ((P, V, P, true), VaultMovedOn),
            ((P, V, S, true), ReplacementWins),
            ((P, V, S, false), Conflict),
        ];
        for ((p, v, s, stale), want) in rows {
            assert_eq!(
                provenance(p, v, s, stale),
                want,
                "P={p} V={v} S={s} stale={stale}"
            );
        }
    }

    #[test]
    fn only_the_equalities_decide() {
        // Every assignment of three names to P, V and S, both stale marks: the verdict depends
        // on which of them are equal, read in the spec's order, and on nothing else.
        use ProvenanceVerdict::*;
        let names = ["sha256:a", "sha256:b", "sha256:c"];
        for p in names {
            for v in names {
                for s in names {
                    for stale in [false, true] {
                        let want = if p == v {
                            InStep { reseed: s != v }
                        } else if p == s {
                            VaultMovedOn
                        } else if stale {
                            ReplacementWins
                        } else if v == s {
                            Capture
                        } else {
                            Conflict
                        };
                        assert_eq!(provenance(p, v, s, stale), want, "{p} {v} {s} {stale}");
                    }
                }
            }
        }
    }
}
```

In `crates/tagteam-core/src/lib.rs`, add `pub mod provenance;` after `pub mod poll;`.

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-core provenance`
Expected: compile error, `cannot find function provenance in this scope` and
`cannot find type ProvenanceVerdict`.

- [ ] **Step 3: Implement**

Put above the test module in `crates/tagteam-core/src/provenance.rs`:

```rust
//! §12.5 "Profile provenance": a profile's credential P against the vault's V, through the seed
//! S the two last agreed on, never by expiry (B.52). Pure: the engine reads the three
//! fingerprints and the stale mark, and acts on the verdict.

/// §12.5's table, as a pure function of three generation fingerprints and the stale mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProvenanceVerdict {
    /// P = V. `reseed`: the seed differs from V and moves to it.
    InStep { reseed: bool },
    /// P ≠ V, V = S, not stale-marked: the profile rotated; capture P, and the seed becomes P.
    Capture,
    /// P ≠ V, P = S: the vault moved on; P is older and may be consumed. Never captured.
    VaultMovedOn,
    /// Stale-marked and P ≠ V (with P ≠ S): the explicit replacement wins. Never captured.
    ReplacementWins,
    /// P ≠ V, P ≠ S, V ≠ S, not stale-marked: both moved in an unknown order.
    Conflict,
}

/// `p`, `v`, `seed` are generation fingerprints (`sha256:…`, §2). Rows are checked in the
/// spec's order: P = V first, then P = S, then V = S, then the stale mark. V = S with the stale
/// mark set is `ReplacementWins`: a replacement never loses to a capture.
pub fn provenance(p: &str, v: &str, seed: &str, stale_marked: bool) -> ProvenanceVerdict {
    if p == v {
        return ProvenanceVerdict::InStep { reseed: seed != v };
    }
    if p == seed {
        return ProvenanceVerdict::VaultMovedOn;
    }
    match (v == seed, stale_marked) {
        (_, true) => ProvenanceVerdict::ReplacementWins,
        (true, false) => ProvenanceVerdict::Capture,
        (false, false) => ProvenanceVerdict::Conflict,
    }
}
```

In `crates/tagteam-core/src/lib.rs`, add after the `pub use poll::…;` line:

```rust
pub use provenance::{ProvenanceVerdict, provenance};
```

- [ ] **Step 4: Run them and see them pass**, then the crate's whole suite

Run: `cargo test -p tagteam-core provenance`, then `cargo test -p tagteam-core`
Expected: PASS.

- [ ] **Step 5: Format and lint**

```
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-core/src/provenance.rs crates/tagteam-core/src/lib.rs
git commit -m "Decide a profile's provenance from its seed, the vault and the stale mark"
```

#### Cycle B: applying it in the gate

- [ ] **Step 1: Write the failing tests**

The drift rule is crate-private, so its table is a unit test in the module itself. Create
`crates/tagteam-engine/src/provenance.rs` holding only this test module for now (Step 3 writes
the module above it), and add `pub mod provenance;` to `crates/tagteam-engine/src/lib.rs`
between `pub mod oracle;` and `pub mod quarantine;`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tagteam_core::{AccountId, ProviderId};

    fn row(email: Option<&str>, label: &str, org: &str) -> AccountRow {
        AccountRow {
            id: AccountId::from_string("0192"),
            provider: ProviderId::new("claude-code"),
            position: 1,
            identity_key: String::new(),
            label: label.into(),
            email: email.map(str::to_owned),
            org_uuid: org.into(),
            org_name: None,
            account_uuid: None,
            kind: "oauth".into(),
            alias: None,
            disabled: false,
            identity_json: serde_json::json!({}),
            login_expires_at: None,
            login_epoch: 0,
            replacing_fp: None,
            quarantine_reason: None,
            quarantine_fp: None,
            quarantine_at: None,
            added_at: 1,
        }
    }

    fn identity(email: Option<&str>, label: &str, org: &str) -> Identity {
        Identity {
            label: label.into(),
            email: email.map(str::to_owned),
            org_uuid: org.into(),
            org_name: None,
            account_uuid: None,
            raw: serde_json::json!({}),
        }
    }

    #[test]
    fn drift_compares_the_email_and_the_organization_only_when_both_name_one() {
        let ours = row(Some("a@x.co"), "a@x.co", "org-1");
        assert!(!identity_drifted(
            &identity(Some("a@x.co"), "a@x.co", "org-1"),
            &ours
        ));
        assert!(
            !identity_drifted(&identity(Some("a@x.co"), "a@x.co", ""), &ours),
            "only one side names an organization"
        );
        assert!(
            identity_drifted(&identity(Some("b@x.co"), "b@x.co", "org-1"), &ours),
            "another email"
        );
        assert!(
            identity_drifted(&identity(Some("a@x.co"), "a@x.co", "org-2"), &ours),
            "another organization"
        );
        let handle = row(None, "alice@ws", "ws");
        assert!(
            !identity_drifted(&identity(None, "alice@ws", "ws"), &handle),
            "no email: the label is compared"
        );
        assert!(identity_drifted(&identity(None, "bob@ws", "ws"), &handle));
    }
}
```

Create `crates/tagteam-engine/tests/provenance.rs`:

```rust
//! §12.5 "Profile provenance" and "Lazy capture" through the refresh gate (§7.3 step 3), every
//! row of the table with its request count (§15.2 "Provenance").

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{Fx, credential, due, prev_refresh_token, token_requests, two_accounts};
use serde_json::{Value, json};
use tagteam_cc::live::Platform;
use tagteam_core::AccountId;
use tagteam_engine::refresh::GateOutcome;
use tagteam_provider::http::Method;
use tagteam_provider::profile::{MARKER_FILE, SEED_FILE, Seed};
use tagteam_provider::{Clock, Provider, Read};

/// `rt`'s credential for a@x.co, its access token expiring at `expires_at`.
fn cred_at(rt: &str, expires_at: i64) -> Vec<u8> {
    let mut v = Fx::credential_json("a@x.co", rt);
    v["claudeAiOauth"]["expiresAt"] = json!(expires_at);
    v.to_string().into_bytes()
}

fn expires_at(bytes: &[u8]) -> i64 {
    let v: Value = serde_json::from_slice(bytes).unwrap();
    v["claudeAiOauth"]["expiresAt"].as_i64().unwrap()
}

/// The generation fingerprint of refresh token `rt` (§2).
fn fp(fx: &Fx, rt: &str) -> String {
    fx.cc
        .fingerprint(&credential("a@x.co", rt))
        .unwrap()
        .as_str()
        .to_owned()
}

/// A quiescent, bootstrapped profile for `id`: a marker, the account's login in its
/// `.claude.json`, a seed of `seed_rt`'s generation under the account's current epoch, and
/// `profile` as the credential Claude Code reads there. Returns its directory.
fn quiescent(fx: &Fx, id: &AccountId, seed_rt: &str, profile: &[u8]) -> PathBuf {
    let dir = fx.make_profile(id);
    let epoch = fx.engine.store().unwrap().account(id).unwrap().unwrap().login_epoch;
    fx.write_seed(&dir, epoch, &fp(fx, seed_rt));
    fx.set_profile_credential(&dir, profile);
    dir
}

/// An explicit replacement that landed since `id`'s profile was bootstrapped: the account's
/// `login_epoch` moved on, which stale-marks the profile (§12.5).
fn bump_epoch(fx: &Fx, id: &AccountId) {
    rusqlite::Connection::open(fx.env.data_dir().join("tagteam.db"))
        .unwrap()
        .execute(
            "UPDATE accounts SET login_epoch = login_epoch + 1 WHERE id = ?1",
            [id.as_str()],
        )
        .unwrap();
}

fn seed_of(dir: &Path) -> Seed {
    match Seed::read(dir) {
        Read::Present(seed) => seed,
        other => panic!("{other:?}"),
    }
}

/// The gate on `id`, with the vault's current bytes as the caller's snapshot.
fn gate(fx: &Fx, id: &AccountId) -> GateOutcome {
    let snapshot = fx.vault_bytes(id).unwrap();
    fx.engine
        .refresh_stored(fx.cc.as_ref(), id, &snapshot)
        .unwrap()
}

/// The refresh token each token request carried, in order.
fn sent_refresh_tokens(fx: &Fx) -> Vec<String> {
    let token = Fx::endpoints().token;
    fx.http
        .requests()
        .iter()
        .filter(|r| r.method == Method::Post && r.url == token)
        .map(|r| {
            let body: Value = serde_json::from_slice(r.body.as_deref().unwrap()).unwrap();
            body["refresh_token"].as_str().unwrap().to_owned()
        })
        .collect()
}

fn profile_unreadable(outcome: &GateOutcome) -> bool {
    matches!(outcome, GateOutcome::Transient { kind, rescued: false } if kind == "profile-unreadable")
}

#[test]
fn a_profile_in_step_leaves_the_vault_to_the_gate() {
    // P = V = S: nothing to do; the gate refreshes the vault's generation as without a profile.
    let fx = Fx::new();
    let a = due(&fx);
    let dir = quiescent(&fx, &a, "rt-a", &fx.vault_bytes(&a).unwrap());
    fx.script_refresh(Some("rt-a2"));
    assert!(matches!(gate(&fx, &a), GateOutcome::Refreshed(_)));
    assert_eq!(sent_refresh_tokens(&fx), ["rt-a"]);
    assert_eq!(seed_of(&dir).seed_fp, fp(&fx, "rt-a"), "the seed stays where they agreed");
}

#[test]
fn an_in_step_profile_whose_seed_lags_is_reseeded_before_the_request() {
    // P = V, S older: the seed moves to V first, so the refresh that follows leaves the profile
    // "vault moved on", not in conflict.
    let fx = Fx::new();
    let a = due(&fx);
    let dir = quiescent(&fx, &a, "rt-old", &fx.vault_bytes(&a).unwrap());
    fx.script_refresh(Some("rt-a2"));
    assert!(matches!(gate(&fx, &a), GateOutcome::Refreshed(_)));
    assert_eq!(seed_of(&dir).seed_fp, fp(&fx, "rt-a"));
    assert_eq!(token_requests(&fx), 1);
}

#[test]
fn a_rotated_profile_is_captured_when_its_token_expires_no_later() {
    // §15.2 "Provenance": the rotated generation's access token expires exactly when the
    // vault's does, so expiry cannot tell them apart; the seed does (B.52). The gate then
    // refreshes the profile's generation, never the consumed rt-a.
    let fx = Fx::new();
    let a = due(&fx);
    let rotated = cred_at("rt-a2", expires_at(&fx.vault_bytes(&a).unwrap()));
    let dir = quiescent(&fx, &a, "rt-a", &rotated);
    fx.script_refresh(Some("rt-a3"));
    let outcome = gate(&fx, &a);
    assert!(matches!(outcome, GateOutcome::Refreshed(_)), "{outcome:?}");
    assert_eq!(sent_refresh_tokens(&fx), ["rt-a2"]);
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a3"));
    assert_eq!(prev_refresh_token(&fx, &a).as_deref(), Some("rt-a2"));
    assert_eq!(seed_of(&dir).seed_fp, fp(&fx, "rt-a2"));
    assert_eq!(
        fs::read(dir.join(".credentials.json")).unwrap(),
        rotated,
        "the profile is never written"
    );
}

#[test]
fn a_rotated_profile_is_captured_when_its_token_expires_earlier() {
    // The vault's token is good for an hour, the profile's for half that: an expiry rule would
    // keep the vault's. The captured token is still valid, so no request is made.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let rotated = cred_at("rt-a2", fx.clock.now_ms() + 30 * 60_000);
    let dir = quiescent(&fx, &a, "rt-a", &rotated);
    let outcome = gate(&fx, &a);
    let GateOutcome::AlreadyFresh(bytes) = outcome else {
        panic!("{outcome:?}")
    };
    assert_eq!(bytes, rotated);
    assert_eq!(fx.vault_bytes(&a).unwrap(), rotated);
    assert_eq!(prev_refresh_token(&fx, &a).as_deref(), Some("rt-a"));
    assert_eq!(seed_of(&dir).seed_fp, fp(&fx, "rt-a2"));
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn on_linux_a_rotated_profile_is_captured_from_its_file() {
    let fx = Fx::with_platform(Platform::Linux);
    let a = two_accounts(&fx);
    let rotated = credential("a@x.co", "rt-a2");
    let dir = quiescent(&fx, &a, "rt-a", &rotated);
    assert!(matches!(gate(&fx, &a), GateOutcome::AlreadyFresh(_)));
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"));
    assert_eq!(seed_of(&dir).seed_fp, fp(&fx, "rt-a2"));
}

#[test]
fn a_profile_the_vault_moved_past_is_left_alone() {
    // P = S: the vault moved on; P may be consumed and is never captured.
    let fx = Fx::new();
    let a = due(&fx);
    let dir = quiescent(&fx, &a, "rt-old", &credential("a@x.co", "rt-old"));
    fx.script_refresh(Some("rt-a2"));
    assert!(matches!(gate(&fx, &a), GateOutcome::Refreshed(_)));
    assert_eq!(sent_refresh_tokens(&fx), ["rt-a"]);
    assert_eq!(seed_of(&dir).seed_fp, fp(&fx, "rt-old"));
}

#[test]
fn a_stale_marked_profile_never_wins_over_a_replacement() {
    // Both stale-marked rows: the vault still at the seed (V = S), and both moved.
    for seed_rt in ["rt-a", "rt-old"] {
        let fx = Fx::new();
        let a = due(&fx);
        quiescent(&fx, &a, seed_rt, &credential("a@x.co", "rt-a2"));
        bump_epoch(&fx, &a);
        fx.script_refresh(Some("rt-a3"));
        assert!(matches!(gate(&fx, &a), GateOutcome::Refreshed(_)), "{seed_rt}");
        assert_eq!(sent_refresh_tokens(&fx), ["rt-a"], "{seed_rt}: never captured");
    }
}

#[test]
fn a_conflict_sends_nothing_and_changes_nothing() {
    // P ≠ V, P ≠ S, V ≠ S, not stale-marked (B.52).
    let fx = Fx::new();
    let a = due(&fx);
    let profile = credential("a@x.co", "rt-a2");
    let dir = quiescent(&fx, &a, "rt-old", &profile);
    fx.script_refresh(Some("rt-a3"));
    assert!(matches!(gate(&fx, &a), GateOutcome::Conflict));
    assert_eq!(token_requests(&fx), 0);
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
    assert_eq!(seed_of(&dir).seed_fp, fp(&fx, "rt-old"));
    assert_eq!(fs::read(dir.join(".credentials.json")).unwrap(), profile);
}

#[test]
fn a_profile_that_cannot_be_read_stops_the_gate() {
    // Decision 9: the seed, the marker, the identity, or the credential.
    let breaks: [(&str, fn(&Fx, &Path)); 5] = [
        ("seed", |_, dir| fs::write(dir.join(SEED_FILE), "{").unwrap()),
        ("marker", |_, dir| fs::write(dir.join(MARKER_FILE), "{").unwrap()),
        ("identity", |_, dir| {
            fs::remove_file(dir.join(".claude.json")).unwrap();
            fs::create_dir(dir.join(".claude.json")).unwrap();
        }),
        ("degraded credential", |fx, dir| {
            let (svc, acct) = fx.profile_item(dir);
            fx.kc.set_unreadable(&svc, &acct, true);
        }),
        ("unreadable credential", |fx, dir| {
            let (svc, acct) = fx.profile_item(dir);
            fx.kc.set_unreadable(&svc, &acct, true);
            fs::remove_file(dir.join(".credentials.json")).unwrap();
        }),
    ];
    for (what, break_it) in breaks {
        let fx = Fx::new();
        let a = due(&fx);
        let dir = quiescent(&fx, &a, "rt-a", &credential("a@x.co", "rt-a2"));
        break_it(&fx, &dir);
        fx.script_refresh(Some("rt-a3"));
        let outcome = gate(&fx, &a);
        assert!(profile_unreadable(&outcome), "{what}: {outcome:?}");
        assert_eq!(token_requests(&fx), 0, "{what}");
        assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"), "{what}");
    }
}

#[test]
fn a_profile_whose_login_drifted_is_ignored() {
    // §12.5 "Identity drift": would be a capture, but the profile is logged in as c.
    let fx = Fx::new();
    let a = due(&fx);
    let dir = quiescent(&fx, &a, "rt-a", &credential("a@x.co", "rt-a2"));
    fs::write(
        dir.join(".claude.json"),
        json!({"oauthAccount": Fx::oauth_account("c@x.co")}).to_string(),
    )
    .unwrap();
    fx.script_refresh(Some("rt-a3"));
    assert!(matches!(gate(&fx, &a), GateOutcome::Refreshed(_)));
    assert_eq!(sent_refresh_tokens(&fx), ["rt-a"]);
}

#[test]
fn a_profile_never_bootstrapped_is_ignored() {
    let fx = Fx::new();
    let a = due(&fx);
    let dir = fx.make_profile(&a);
    fx.set_profile_credential(&dir, &credential("a@x.co", "rt-a2"));
    fx.script_refresh(Some("rt-a3"));
    assert!(matches!(gate(&fx, &a), GateOutcome::Refreshed(_)));
    assert_eq!(sent_refresh_tokens(&fx), ["rt-a"]);
}

#[test]
fn a_profile_credential_without_a_refresh_token_is_never_captured() {
    // §6.2: never a credential without a refresh token over one with it.
    let fx = Fx::new();
    let a = due(&fx);
    let access_only =
        json!({"claudeAiOauth": {"accessToken": "at-x", "expiresAt": 1_790_003_600_000i64}});
    quiescent(&fx, &a, "rt-a", access_only.to_string().as_bytes());
    fx.script_refresh(Some("rt-a2"));
    assert!(matches!(gate(&fx, &a), GateOutcome::Refreshed(_)));
    assert_eq!(sent_refresh_tokens(&fx), ["rt-a"]);
}

#[test]
fn a_running_profile_is_never_captured() {
    // §6.2: a capture from a profile requires it to be quiescent; the gate stops at step 2.
    let fx = Fx::new();
    let a = due(&fx);
    let dir = quiescent(&fx, &a, "rt-a", &credential("a@x.co", "rt-a2"));
    let _held = fx.hold_reservation(&dir);
    assert!(matches!(
        gate(&fx, &a),
        GateOutcome::Owned(tagteam_engine::refresh::OwnedBy::Session)
    ));
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
    assert_eq!(seed_of(&dir).seed_fp, fp(&fx, "rt-a"));
}
```

`rusqlite` is one of `tagteam-engine`'s own dependencies, so the test crate can use it, as
`tests/common/mod.rs` already does.

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-engine --lib provenance`
Expected: compile error, `cannot find function identity_drifted in this scope` (and the types
the module's imports will bring in).

Run: `cargo test -p tagteam-engine --test provenance`
Expected: FAIL. The gate has no step-3 provenance yet, so these fail on their first assertion:
- `a_rotated_profile_is_captured_when_its_token_expires_no_later`: the request carries `rt-a`.
- `a_rotated_profile_is_captured_when_its_token_expires_earlier`: the gate returns `Refreshed`,
  and the scripted port has no reply queued, so it is `Transient` instead of `AlreadyFresh`.
- `on_linux_a_rotated_profile_is_captured_from_its_file`: not `AlreadyFresh`.
- `an_in_step_profile_whose_seed_lags_is_reseeded_before_the_request`: the seed is still
  `rt-old`'s.
- `a_conflict_sends_nothing_and_changes_nothing`: `Refreshed`, not `Conflict`.
- `a_profile_that_cannot_be_read_stops_the_gate`: `seed: Refreshed(..)`.

These already pass, and must keep passing:
- `a_profile_in_step_leaves_the_vault_to_the_gate`
- `a_profile_the_vault_moved_past_is_left_alone`
- `a_stale_marked_profile_never_wins_over_a_replacement`
- `a_profile_whose_login_drifted_is_ignored`
- `a_profile_never_bootstrapped_is_ignored`
- `a_profile_credential_without_a_refresh_token_is_never_captured`
- `a_running_profile_is_never_captured`

- [ ] **Step 3: Implement**

Write `crates/tagteam-engine/src/provenance.rs` above the test module Step 1 created:

```rust
//! §12.5 "Profile provenance" and "Lazy capture": a quiescent profile's credential against the
//! vault's, through the profile's seed, never by expiry (B.52). It runs under the account's
//! lock, in the refresh gate (§7.3 step 3) and in `switch`'s transaction (§9.2).

use tagteam_core::{ProvenanceVerdict, provenance};
use tagteam_provider::profile::{ProfileMarker, Seed};
use tagteam_provider::{Identity, Provenance, Provider, Read, ReadError};

use crate::account_lock::AccountLock;
use crate::engine::Engine;
use crate::error::EngineError;
use crate::session::SessionState;
use crate::store::AccountRow;

/// What applying a profile's provenance found, and did (§12.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileCheck {
    /// No profile, not quiescent, no seed (never bootstrapped), or its identity drifted.
    NotApplicable,
    InStep,
    Captured,
    VaultMovedOn,
    ReplacementWins,
    Conflict,
    /// The seed, the marker, the profile credential or the profile identity could not be read,
    /// or the marker is missing beside a seed, or names another account or provider (Decision 9).
    Unreadable(String),
}

/// §12.5 "Identity drift": the profile's login is another account's than `row`. The email is
/// compared, or the label for a provider whose identities carry none, and the organization
/// only when both sides name one. The one definition: the session-owned usage fetch
/// (`collect.rs`, Task 12) calls it too.
pub(crate) fn identity_drifted(identity: &Identity, row: &AccountRow) -> bool {
    let email = identity.email.as_deref().unwrap_or(&identity.label)
        != row.email.as_deref().unwrap_or(&row.label);
    let org = !identity.org_uuid.is_empty()
        && !row.org_uuid.is_empty()
        && identity.org_uuid != row.org_uuid;
    email || org
}

/// A profile file's read error, as `ProfileCheck::Unreadable` carries it.
fn unreadable(e: &ReadError) -> ProfileCheck {
    ProfileCheck::Unreadable(format!("{}: {}", e.what, e.detail))
}

impl Engine {
    /// §12.5 under `lock` (the account's): reads the seed, the marker's spelling and the
    /// profile credential; applies `tagteam_core::provenance`; captures through
    /// `persist_generation` and moves the seed on `Capture`; reseeds on `InStep { reseed: true }`.
    ///
    /// A capture needs a quiescent profile with a seed, the same identity, a fresh read of a
    /// credential with a refresh token (§6.2), and the table's `Capture` row, which a stale
    /// mark never reaches. Expiry is never consulted (B.52). A seed that cannot be moved after a
    /// capture or a reseed is an error: going on would make the next comparison a false
    /// `Conflict`. An unreadable vault is `EngineError::Unreadable`, which the caller reports as
    /// it reports its own vault reads. Unreadable also covers a marker that is absent beside a
    /// seed or names another account, and a profile identity that cannot be read.
    pub(crate) fn apply_provenance(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        lock: &AccountLock,
    ) -> Result<ProfileCheck, EngineError> {
        debug_assert_eq!(lock.id(), &row.id, "the caller holds this account's lock");
        let SessionState::Quiescent { profile } = self.session_state(p, row)? else {
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
        // §12.2: every read of the profile's credential uses the recorded spelling.
        let spelling = marker.config_dir.as_str();
        match p.profile_identity(&self.env, spelling) {
            Read::Present(login) if !identity_drifted(&login, row) => {}
            // Another account's login, or none to compare: the profile is ignored (§12.5).
            Read::Present(_) | Read::Absent => return Ok(ProfileCheck::NotApplicable),
            Read::Unreadable(e) => return Ok(unreadable(&e)),
        }
        let held = match p.read_profile_credential(&self.env, spelling) {
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

(`pub mod provenance;` is already in `crates/tagteam-engine/src/lib.rs`, from Step 1.)

In `crates/tagteam-engine/src/refresh.rs`, add `use crate::provenance::ProfileCheck;` to the
imports, and change `GateOutcome::Conflict`'s doc comment to:

```rust
    /// A quiescent session profile and the vault both moved since they last agreed (§12.5):
    /// nothing is captured, refreshed or overwritten.
```

Replace `refresh_stored` (lines 299–397) with:

```rust
impl Engine {
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
        // 1.
        let Some(lock) = AccountLock::try_acquire(&self.env, id)? else {
            return Ok(GateOutcome::Busy);
        };
        // §6.2 "Pending replacements first".
        self.reconcile_replacement(&lock)?;
        let store = self.store()?;
        let Some(row) = store.account(id)? else {
            return Ok(transient("vault-absent"));
        };
        // §7.4: a quarantine holds only while the vault still holds the generation it is bound
        // to. A vault that has moved on releases it (§11.2 step 1), which also heals
        // `persist_generation`'s window between the vault write and the store update.
        if let Some(reason) = &row.quarantine_reason {
            if !self.quarantine_released(p, &row) {
                return Ok(GateOutcome::Dead(
                    QuarantineReason::parse(reason).unwrap_or(QuarantineReason::InvalidGrant),
                ));
            }
            self.unquarantine(&row)?;
        }
        if !p.kind_traits(&row.kind).refreshable {
            return Ok(transient("not-refreshable"));
        }
        // 2.
        if let Some(by) = self.owner_of(p, &store, &row)? {
            return Ok(GateOutcome::Owned(by));
        }
        // 3. The vault, then any rescue that succeeds it, then a quiescent profile's
        // provenance, then the vault again.
        match self.vault.read(id) {
            Read::Present(b) if !b.is_empty() => {}
            Read::Present(_) | Read::Absent => return Ok(transient("vault-absent")),
            Read::Unreadable(_) => return Ok(transient("vault-unreadable")),
        }
        match self.settle_rescues(p, &row, &lock) {
            Ok(()) => {}
            Err(EngineError::RescuePending { .. }) => return Ok(transient("rescue-unreadable")),
            Err(e) => return Err(e),
        }
        // §12.5 "Lazy capture": a rotation is adopted into the vault here, so the read below
        // picks it up. A conflict, or a profile that cannot be read, sends nothing (Decision 9).
        match self.apply_provenance(p, &row, &lock) {
            Ok(ProfileCheck::Conflict) => return Ok(GateOutcome::Conflict),
            Ok(ProfileCheck::Unreadable(detail)) => {
                tracing::warn!(
                    position = row.position,
                    account = %row.id,
                    "the session profile could not be read ({detail}); nothing is sent"
                );
                return Ok(transient("profile-unreadable"));
            }
            Ok(_) => {}
            Err(EngineError::Unreadable(_)) => return Ok(transient("vault-unreadable")),
            Err(e) => return Err(e),
        }
        let current = match self.vault.read(id) {
            Read::Present(b) if !b.is_empty() => b,
            Read::Present(_) | Read::Absent => return Ok(transient("vault-absent")),
            Read::Unreadable(_) => return Ok(transient("vault-unreadable")),
        };
        // 4.
        let now = self.now_ms();
        if p.access_fingerprint(&current) != p.access_fingerprint(snapshot)
            && !expired(p, &current, now)
        {
            return Ok(GateOutcome::AlreadyFresh(current));
        }
        // 5. Only the account lock is held across the request.
        let sent_fp = fp_str(p, &current);
        let fresh = Credential::fresh(current)
            .into_fresh()
            .expect("a vault read is authoritative, never degraded");
        hooks::point(self, "gate-before-request")?;
        let result = p.refresh(self.http.as_ref(), &fresh, now, GATE_TIMEOUT);
        let outcome = match result {
            RefreshResult::Refreshed { successor, owner } => {
                // §7.4: a successor the response says belongs to another account is marked
                // first, so no path, `Drop` included, ever stores it as this account's.
                let foreign = owner.filter(|o| names_another_account(o, &row));
                // From here on the successor is never discarded (§7.3): `received` keeps it if
                // this unwinds, and `abandon` if an error returns early.
                let mut received = Received::new(self, p, &row, &sent_fp, successor, foreign);
                let persisted = hooks::point(self, "gate-after-response")
                    .and_then(|()| self.persist_successor(p, &row, &lock, &sent_fp, &mut received));
                match persisted {
                    Ok(outcome) => outcome,
                    Err(e) if received.armed => {
                        match self.abandon(&row, &sent_fp, &mut received, e) {
                            Abandoned::Kept(e) => return Err(e),
                            Abandoned::Lost => GateOutcome::Unpersisted,
                        }
                    }
                    Err(e) => return Err(e),
                }
            }
            other => {
                hooks::point(self, "gate-after-response")?;
                self.verdict(p, &row, &sent_fp, other)?
            }
        };
        drop(lock);
        Ok(outcome)
    }
```

(The `impl Engine {` line above opens the same block it does today; nothing after
`refresh_stored` changes.) The gate's callers need no change: `switch.rs` `freshen` still maps
`Conflict` to its `invalid-input` stub, which Task 11 replaces, and `collect.rs` `Collector::gate`
already maps `Conflict` and every `Transient` to `refresh-failed`.

- [ ] **Step 4: Run them and see them pass**, then the whole suite

Run: `cargo test -p tagteam-engine --lib provenance`, then
`cargo test -p tagteam-engine --test provenance`, then
`cargo test -p tagteam-engine --features test-hooks`
Expected: PASS, including `drift_compares_the_email_and_the_organization_only_when_both_name_one`.
No existing account has a profile, so every existing gate test takes
`NotApplicable`.

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
git add crates/tagteam-engine/src/provenance.rs crates/tagteam-engine/src/lib.rs \
  crates/tagteam-engine/src/refresh.rs crates/tagteam-engine/tests/provenance.rs
git commit -m "Apply a quiescent profile's provenance in the refresh gate: capture a rotation, refuse a conflict"
```

---

### Task 11: `switch`'s session rules and refusal kinds (K17)

§9.2: "**Session-owned target** (§12.5): refuse with reason `session-owned`, with or without
`--force`. Activating it would give the default profile and the running session two
independently locked copies of one single-use refresh token. … If the profile is quiescent and
rotated since its seed, its generation is adopted into the vault before activation (lazy
capture, §12.5). If its provenance reports a conflict, the switch refuses with reason
`profile-conflict`, because the vault's generation may be consumed." §9.3: "Every strategy,
rotation included, skips session-owned candidates." §9.4 step 1: under the locks, "re-check that
the target is not session-owned."

M2a left the gate's two session outcomes on `invalid-input` stubs in `freshen`, with a comment
naming M4 (its carried item K17). This task gives them their kinds and puts the session rules in
the four places a switch decides:
- **`plan`** refuses a session-owned direct target before its vault is read.
- **The rotation's candidate list** skips session-owned accounts.
- **`rederive`** checks again under every lock.
- **`transact`** applies the target's provenance next to its rescues, so a rotation the profile
  holds is the generation activated.

**Readings of the spec this task commits to:**
- **The candidate filter is an engine method.** `is_candidate` is pure, over the row alone, and
  session state needs the file system and the process port. `switch_candidate(p, row)` is
  `is_candidate(row) && !session_state(p, row).owned()`, and `candidate_order` builds its list
  through it. So the fewer-than-two count, the walk and `rotation_pick_stands` all see the same
  filtered list. A bare `switch` whose only alternative is session-owned is `only-one-account`.
- **A session that starts during a switch.** A reservation can only appear between planning and
  the locks. A direct target is refused (`SessionOwned`). A rotation's pick is skipped: the
  switch plans again and the walk passes over it.
  - **In `freshen`,** when the gate finds the pick session-owned, a rotation plans again
    (`Freshened::Replan`, like a Dead pick) and a direct target is refused.
  - **In `rederive`,** a rotation re-plans (`Rederived::Replan`) and a direct target is refused.
- **A conflict refuses every kind of switch,** forced or not, direct or rotation: §9.2's
  conflict rule sits under the "with or without `--force`" bullet, and B.52 says nothing is
  activated while both moved.
- **Provenance is applied under the target's lock in `transact`,** right after
  `settle_rescues`. Both branches read the target's vault after that point (the direct branch's
  `read_target`, and the outgoing branch's `read_target` after `settle_outgoing`), so a captured
  generation is the one composed and written. An unreadable profile refuses as
  `UnreadableAccount` (Decision 9), naming the account. The gate's own provenance check in
  `freshen` stays as it is: a due target is captured there first, and `transact` then finds it
  in step.

**M3a's usage strategies at re-sync.** M3a adds `SwitchTarget::Usage`, `UsageStrategy` (`best`,
`next-available`) and `switch_planned`. Their candidates are counted and walked from the store
with `is_candidate`, then ranked from readings (§9.3). At re-sync:
- **Candidate lists.** Every place M3a builds a candidate list from `is_candidate` calls
  `self.switch_candidate(p, &row)?` instead. `best` and `next-available` then skip a
  session-owned account before any ranking and before the live account's headroom is compared.
- **The under-lock check.** M3a's "the pick stands while it is still switchable" goes through
  `switch_candidate` too.
- **`rederive`.** Its session re-check is an exhaustive `match` on `req.target`, so the compiler
  points at it when `Usage` arrives. A usage pick re-plans like a rotation.
- **`freshen`.** Its `rotation` flag (today `matches!(req.target, SwitchTarget::Rotation)`)
  becomes "not a direct target". A usage pick that a session took meanwhile then re-plans, and so
  does a Dead one.
- **M3b's tick** uses the same `switch_candidate` to skip session-owned candidates (§11.2).

**Files:**
- Modify: `crates/tagteam-engine/src/error.rs` (`ProfileConflict`, its `kind()` arm and pin)
- Modify: `crates/tagteam-engine/src/switch.rs` (new `session_owned`, `profile_conflict`; `is_candidate`'s doc, lines 252–257; new `Engine::switch_candidate`; `candidate_order` 512–552; `rotation` 554–582; `rotation_pick_stands` 584–601; `plan` 614–695; `freshen` 734–842; `rederive` 981–1070; new `settle_profile`; one statement inserted in `transact` after line 1121)
- Test: `crates/tagteam-engine/tests/session.rs` (append)
- Test: `crates/tagteam-engine/tests/provenance.rs` (append)

**Interfaces:**
- Consumes:
  - Task 9: `Engine::session_state`, `SessionState::owned`, `Engine::refuse_session_owned`, `EngineError::SessionOwned`; fixture helpers `Fx::{make_profile, hold_reservation, plant_record}`
  - Task 10: `ProfileCheck`, `Engine::apply_provenance`; `tests/provenance.rs`'s `quiescent`, `fp`, `seed_of`, `cred_at`
  - Existing: `Fx::engine_with_vault_probe`, `Engine::on_point` (feature `test-hooks`), `common::journal`
- Produces:
  - `EngineError::ProfileConflict { position: u32, label: String }`, kind `profile-conflict`
  - `Engine::switch_candidate(&self, p: &dyn Provider, row: &AccountRow) -> Result<bool, EngineError>` (crate-private; M3a's strategies at re-sync and M3b's tick use it)
  - `freshen`, on the gate's `Owned(Session)`: a direct target → `SessionOwned`; a rotation re-plans (`Freshened::Replan`; §9.3: every strategy skips session-owned candidates). `Conflict` → `ProfileConflict`
  - `transact`: `Captured` activates the adopted generation; `Conflict` → `ProfileConflict`; `Unreadable` → `UnreadableAccount`

**Spec:**
- §9.2 "Session-owned target": refuse `session-owned`, with or without `--force`. Lazy capture
  before activation. A conflict refuses `profile-conflict`.
- §9.3, last paragraph: every strategy, rotation included, skips session-owned candidates.
- §9.4 step 1: re-check under the locks that the target is not session-owned. Reservations are
  written under `MutationGuard` and refreshes under the account lock, so neither changes until
  release.
- §7.2's table, last row: `Owned` by a session, or `Conflict`, is the matching §9.2 refusal.
- §6.2 "Pending rescues before activation": rescues settle first, under the target's lock.
- Decisions 9 and 10.

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-engine/tests/session.rs`, add to the imports:

```rust
use std::path::Path;
use std::sync::Mutex;

use tagteam_engine::switch::SwitchReason;
use tagteam_provider::FlockGuard;
```

and add `journal` to the `common::{…}` import. Append:

```rust
// §9.2–§9.4: switch's session rules.

/// A vault probe that starts a session for `id` (a held reservation in `profile`) the first
/// time `id`'s vault is read: after planning has checked the account, and before freshening
/// refreshes it.
fn start_session_on_first_read(
    id: &AccountId,
    profile: &Path,
) -> impl Fn(&str) + Send + Sync + 'static {
    let key = id.as_str().to_owned();
    let launch = profile.join(LAUNCH_DIR).join("4242.lock");
    let held = Mutex::new(None);
    move |read: &str| {
        let mut held = held.lock().unwrap();
        if read == key && held.is_none() {
            *held = FlockGuard::try_lock(&launch).unwrap();
        }
    }
}

#[test]
fn a_session_owned_target_is_refused_with_or_without_force() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let _held = fx.hold_reservation(&fx.make_profile(&a));
    for force in [false, true] {
        let err = fx.switch_to(&a, force).unwrap_err();
        assert_eq!(err.kind(), "session-owned", "force {force}: {err}");
        assert!(err.to_string().contains("tagteam run"), "{err}");
    }
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    assert!(journal(&fx).is_none());
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn a_target_with_an_unreadable_session_record_is_refused() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let dir = fx.make_profile(&a);
    fx.plant_record(&dir, "torn", b"{\"pid\":");
    assert_eq!(fx.switch_to(&a, false).unwrap_err().kind(), "session-owned");
}

#[test]
fn a_rotation_skips_a_session_owned_candidate() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.add("c@x.co", "rt-c"); // live: the walk starts after it, at a
    let _held = fx.hold_reservation(&fx.make_profile(&a));
    let out = fx.engine.switch(fx.rotation_request(false)).unwrap();
    assert_eq!(out.to.map(|t| t.id), Some(b));
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
}

#[test]
fn a_rotation_whose_only_alternative_is_session_owned_stays_put() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let _held = fx.hold_reservation(&fx.make_profile(&a));
    let out = fx.engine.switch(fx.rotation_request(false)).unwrap();
    assert_eq!(out.reason, SwitchReason::OnlyOneAccount);
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
}

#[test]
fn a_session_that_starts_before_the_gate_refuses_a_direct_switch_and_sends_nothing() {
    // §7.2's table: `Owned` by a session is §9.2's refusal.
    let fx = Fx::new();
    let a = due(&fx); // a inactive and due, b live
    fx.script_refresh(Some("rt-a2"));
    let dir = fx.make_profile(&a);
    let engine = fx.engine_with_vault_probe(start_session_on_first_read(&a, &dir));
    let err = engine.switch(fx.switch_request(&a, false)).unwrap_err();
    assert_eq!(err.kind(), "session-owned", "{err}");
    assert_eq!(token_requests(&fx), 0, "the gate left the session's token alone");
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
}

#[test]
fn a_session_that_starts_before_the_gate_makes_a_rotation_move_on() {
    // §9.3: the rotation plans again, and its walk passes over the account a session took.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.add("c@x.co", "rt-c"); // live
    fx.expire_access(&a);
    let dir = fx.make_profile(&a);
    let engine = fx.engine_with_vault_probe(start_session_on_first_read(&a, &dir));
    let out = engine.switch(fx.rotation_request(false)).unwrap();
    assert_eq!(out.to.map(|t| t.id), Some(b));
    assert_eq!(token_requests(&fx), 0);
}

#[cfg(feature = "test-hooks")]
mod under_the_locks {
    use std::sync::Arc;

    use super::*;

    /// Starts a session for the account whose profile is `profile` once the switch has planned,
    /// before it takes the mutation lock. The guard is kept in `slot`.
    fn on_planned(fx: &Fx, profile: &Path) -> Arc<Mutex<Option<FlockGuard>>> {
        let slot: Arc<Mutex<Option<FlockGuard>>> = Arc::default();
        let (held, launch) = (slot.clone(), profile.join(LAUNCH_DIR).join("4242.lock"));
        fx.engine.on_point(
            "planned",
            Box::new(move || {
                let mut held = held.lock().unwrap();
                if held.is_none() {
                    *held = FlockGuard::try_lock(&launch).unwrap();
                }
            }),
        );
        slot
    }

    #[test]
    fn a_session_that_starts_after_planning_refuses_a_direct_switch() {
        // §9.4 step 1.
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        fx.add("b@x.co", "rt-b");
        let held = on_planned(&fx, &fx.make_profile(&a));
        let err = fx.switch_to(&a, false).unwrap_err();
        assert_eq!(err.kind(), "session-owned", "{err}");
        assert!(held.lock().unwrap().is_some(), "the session started after planning");
        assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
        assert!(journal(&fx).is_none());
    }

    #[test]
    fn a_session_that_starts_after_planning_makes_a_rotation_plan_again() {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let b = fx.add("b@x.co", "rt-b");
        fx.add("c@x.co", "rt-c"); // live: the plan picks a
        let _held = on_planned(&fx, &fx.make_profile(&a));
        let out = fx.engine.switch(fx.rotation_request(false)).unwrap();
        assert_eq!(out.to.map(|t| t.id), Some(b));
        assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    }
}
```

In `crates/tagteam-engine/tests/provenance.rs`, add `journal` to the `common::{…}` import and
append:

```rust
// §9.2: lazy capture and the conflict refusal in `switch`.

#[test]
fn a_switch_activates_a_rotated_profiles_generation() {
    // §12.5 "Lazy capture" at the switch: a's token is not due, so no gate runs; the
    // transaction adopts the profile's rotation and activates it, never the consumed rt-a.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let dir = quiescent(&fx, &a, "rt-a", &credential("a@x.co", "rt-a2"));
    fx.switch_to(&a, false).unwrap();
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a2"));
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"));
    assert_eq!(seed_of(&dir).seed_fp, fp(&fx, "rt-a2"));
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn a_rotated_profile_whose_access_token_is_due_is_freshened_before_activation() {
    // §7.2 and §9.2: the vault's rt-a is not due, but the profile's rotation rt-a2 expires
    // within the freshen window. Lazy capture runs before the freshen decision, so the gate
    // refreshes rt-a2 and the switch activates its successor, never an access token about
    // to expire.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let soon = fx.clock.now_ms() + 60_000;
    quiescent(&fx, &a, "rt-a", &cred_at("rt-a2", soon));
    fx.script_refresh(Some("rt-a3"));
    fx.switch_to(&a, false).unwrap();
    assert_eq!(token_requests(&fx), 1);
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a3"));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a3"));
}

#[cfg(feature = "test-hooks")]
#[test]
fn a_replacement_between_planning_and_freshening_stale_marks_the_profile() {
    // The plan read a's row before an explicit replacement landed. The replacement installs
    // the seed's own generation again, so the vault equals the seed while the profile holds a
    // rotation: judged with the planning row's epoch, that would capture the profile over the
    // replacement. Freshen reads the row again under the account lock, sees the moved epoch,
    // and the replacement wins (§12.5).
    use std::sync::atomic::{AtomicBool, Ordering};
    use tagteam_engine::store::LoginMeta;
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let dir = quiescent(&fx, &a, "rt-a", &credential("a@x.co", "rt-a2"));
    let store = fx.engine.store().unwrap();
    let cc = fx.cc.clone();
    let id = a.clone();
    let seed_fp = fp(&fx, "rt-a");
    let once = AtomicBool::new(false);
    fx.engine.on_point(
        "freshen-before-lock",
        Box::new(move || {
            if once.swap(true, Ordering::SeqCst) {
                return;
            }
            let row = store.account(&id).unwrap().unwrap();
            let identity = cc.parse_identity(&row.identity_json).unwrap();
            let meta = LoginMeta {
                identity_key: &row.identity_key,
                identity: &identity,
                kind: &row.kind,
                login_expires_at: row.login_expires_at,
                from_live: false,
            };
            // The vault already holds rt-a: the replacement's write leaves its bytes as they
            // are, and only the epoch moves.
            store.begin_replacement(&id, &seed_fp, &meta, false).unwrap();
            store.finish_replacement(&id).unwrap();
        }),
    );

    fx.switch_to(&a, false).unwrap();

    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
    assert_eq!(seed_of(&dir).seed_fp, fp(&fx, "rt-a"), "never captured");
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn a_switch_to_a_profile_the_vault_moved_past_activates_the_vault() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    quiescent(&fx, &a, "rt-old", &credential("a@x.co", "rt-old"));
    fx.switch_to(&a, false).unwrap();
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
}

#[test]
fn a_switch_to_a_conflicting_profile_refuses_with_or_without_force() {
    for force in [false, true] {
        let fx = Fx::new();
        let a = two_accounts(&fx);
        quiescent(&fx, &a, "rt-old", &credential("a@x.co", "rt-a2"));
        let err = fx.switch_to(&a, force).unwrap_err();
        assert_eq!(err.kind(), "profile-conflict", "force {force}: {err}");
        assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
        assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
        assert!(journal(&fx).is_none());
    }
}

#[test]
fn a_due_target_with_a_conflicting_profile_refuses_before_any_request() {
    // §7.2's table: freshening meets the gate's `Conflict`.
    let fx = Fx::new();
    let a = due(&fx);
    quiescent(&fx, &a, "rt-old", &credential("a@x.co", "rt-a2"));
    fx.script_refresh(Some("rt-a3"));
    let err = fx.switch_to(&a, false).unwrap_err();
    assert_eq!(err.kind(), "profile-conflict", "{err}");
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn a_switch_to_an_account_whose_profile_cannot_be_read_refuses() {
    // Decision 9, due or not: the switch refuses before any request, at the lazy capture that
    // precedes the freshen decision.
    for due_now in [false, true] {
        let fx = Fx::new();
        let a = if due_now { due(&fx) } else { two_accounts(&fx) };
        let dir = quiescent(&fx, &a, "rt-a", &credential("a@x.co", "rt-a2"));
        fs::write(dir.join(SEED_FILE), "{").unwrap();
        let err = fx.switch_to(&a, false).unwrap_err();
        assert_eq!(err.kind(), "unreadable", "due {due_now}: {err}");
        assert!(err.to_string().contains("position 1"), "{err}");
        assert_eq!(token_requests(&fx), 0, "due {due_now}");
        assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    }
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-engine --features test-hooks --test session --test provenance`
Expected: FAIL, each on its first assertion:
- `a_session_owned_target_is_refused_with_or_without_force`,
  `a_target_with_an_unreadable_session_record_is_refused`,
  `a_session_that_starts_after_planning_refuses_a_direct_switch`,
  `a_switch_to_a_conflicting_profile_refuses_with_or_without_force` and
  `a_switch_to_an_account_whose_profile_cannot_be_read_refuses` (not due): `unwrap_err` on `Ok`.
  Nothing refuses yet; the switch goes through.
- `a_rotation_skips_a_session_owned_candidate` and
  `a_session_that_starts_after_planning_makes_a_rotation_plan_again`: the rotation lands on a.
- `a_rotation_whose_only_alternative_is_session_owned_stays_put`: it switches to a.
- `a_session_that_starts_before_the_gate_refuses_a_direct_switch_and_sends_nothing`: kind
  `invalid-input`, the stub's.
- `a_session_that_starts_before_the_gate_makes_a_rotation_move_on`: an `invalid-input` error.
- `a_due_target_with_a_conflicting_profile_refuses_before_any_request`: kind `invalid-input`.
- `a_switch_activates_a_rotated_profiles_generation`: the live refresh token is `rt-a`.
- `a_switch_to_an_account_whose_profile_cannot_be_read_refuses` (due): `unwrap_err` on `Ok`,
  after freshen warned `profile-unreadable`.
- `a_rotated_profile_whose_access_token_is_due_is_freshened_before_activation`:
  `token_requests` is 0, not 1. Nothing adopts the profile, so the vault's rt-a, which is not
  due, is activated.

`a_switch_to_a_profile_the_vault_moved_past_activates_the_vault` and
`a_replacement_between_planning_and_freshening_stale_marks_the_profile` already pass, and must
keep passing. The second is a guard for freshen's re-read under the account lock: an
implementation that settled the profile with the planning row's epoch would capture rt-a2 over
the replacement and fail it.

- [ ] **Step 3: Implement**

**The error.** In `crates/tagteam-engine/src/error.rs`, add after `SessionOwned`:

```rust
    /// §9.2, §12.5: the account's quiescent session profile and the vault both moved since they
    /// last agreed, so the vault's generation may be consumed. An explicit replacement resolves
    /// it.
    #[error("position {position} ({label})'s session profile and the vault both moved since they last agreed; log in again with `tagteam add` to resolve it")]
    ProfileConflict { position: u32, label: String },
```

In `kind()`, after the `SessionOwned` arm:

```rust
            EngineError::ProfileConflict { .. } => "profile-conflict",
```

In `kind_is_pinned_for_every_variant`, after the `SessionOwned` case:

```rust
            (
                EngineError::ProfileConflict {
                    position: 1,
                    label: "a".into(),
                },
                "profile-conflict",
            ),
```

**`switch.rs`.** Add `use crate::provenance::ProfileCheck;` to the `use crate::…` lines. Below
`works_until_expiry`, add:

```rust
/// §9.2's session-owned refusal, for a target a session took after planning.
fn session_owned(target: &AccountRow) -> EngineError {
    EngineError::SessionOwned {
        position: target.position,
        label: target.label.clone(),
    }
}

/// §9.2's refusal for a target whose profile and vault both moved (§12.5).
fn profile_conflict(target: &AccountRow) -> EngineError {
    EngineError::ProfileConflict {
        position: target.position,
        label: target.label.clone(),
    }
}
```

Change `is_candidate`'s doc comment (lines 252–254) to:

```rust
/// A rotation candidate by the store alone (§9.3 "Reading the vault lazily"): enabled, not
/// quarantined, and with an identity. Whether its vault holds a credential is read only when
/// the walk reaches it. `Engine::switch_candidate` adds the session rule, and every candidate
/// list goes through that instead.
```

Add to `impl Engine`, directly before `candidate_order`:

```rust
    /// §9.3: a candidate for every strategy. `is_candidate`, and not session-owned (§12.5): a
    /// live reservation, or a live or unreadable session record. The vault is not read.
    pub(crate) fn switch_candidate(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
    ) -> Result<bool, EngineError> {
        Ok(is_candidate(row) && !self.session_state(p, row)?.owned())
    }
```

Replace `candidate_order`, `rotation` and `rotation_pick_stands` (lines 512–601) with:

```rust
    /// §9.3: the rotation's candidates in walk order, from the store and each account's session
    /// state (`switch_candidate`). `None` when the live anchor is managed and fewer than two
    /// accounts qualify (§9.2). The walk starts after the live account when it is managed
    /// (`live_row`), even if the store's active account disagrees (§6.1: the live identity
    /// wins). With no live login, or an unmanaged one, it starts at the store's active account
    /// if that is a candidate, then goes on from the first position.
    fn candidate_order(
        &self,
        p: &dyn Provider,
        store: &Store,
        provider: &ProviderId,
        live_row: Option<&AccountRow>,
    ) -> Result<Option<Vec<AccountRow>>, EngineError> {
        let mut candidates = Vec::new();
        for row in store.accounts(provider)? {
            if self.switch_candidate(p, &row)? {
                candidates.push(row);
            }
        }
        if live_row.is_some() && candidates.len() < 2 {
            return Ok(None);
        }
        let positions: Vec<u32> = candidates.iter().map(|a| a.position).collect();
        let order: Vec<u32> = match live_row {
            Some(live) => rotation_order(&positions, Some(live.position)),
            None => {
                let rest = rotation_order(&positions, None);
                let active = store
                    .active(provider)?
                    .and_then(|id| candidates.iter().find(|a| a.id == id))
                    .map(|a| a.position);
                match active {
                    Some(first) => std::iter::once(first)
                        .chain(rest.into_iter().filter(|p| *p != first))
                        .collect(),
                    None => rest,
                }
            }
        };
        Ok(Some(
            order
                .into_iter()
                .filter_map(|pos| candidates.iter().find(|a| a.position == pos).cloned())
                .collect(),
        ))
    }

    /// §9.3 rotation, reading the vault lazily.
    ///
    /// - The candidates are counted from the store and their session state: with a managed
    ///   live anchor and fewer than two of them, it stays put (§9.2).
    /// - Each vault is read only when the walk reaches it, and the walk stops at the first one
    ///   that holds a credential. An unreadable one before that could have been the pick, so
    ///   it fails naming the account; no account after the pick is ever read.
    fn rotation(
        &self,
        p: &dyn Provider,
        store: &Store,
        provider: &ProviderId,
        live_row: Option<&AccountRow>,
    ) -> Result<Rotation, EngineError> {
        const ONLY_ONE: &str = "there is only one switchable account";
        let Some(order) = self.candidate_order(p, store, provider, live_row)? else {
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

    /// Under the locks: whether the plan's rotation pick still stands, decided from the store
    /// and session state alone (§9.3 "under the locks only the chosen account is read again").
    /// It does when the pick is still a candidate and every candidate the new walk order puts
    /// before it is one the planning walk already read and passed over.
    fn rotation_pick_stands(
        &self,
        p: &dyn Provider,
        store: &Store,
        plan: &Plan,
        anchor: Option<&AccountRow>,
    ) -> Result<bool, EngineError> {
        let Some(order) = self.candidate_order(p, store, &plan.target.provider, anchor)? else {
            return Ok(false);
        };
        Ok(order
            .iter()
            .position(|r| r.id == plan.target.id)
            .is_some_and(|at| order[..at].iter().all(|r| plan.walked.contains(&r.id))))
    }
```

Replace `plan` (lines 614–695) with:

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
        let done = |reason: SwitchReason, message: String| {
            Planned::Done(noop(
                strategy,
                reason,
                message,
                live_row.clone(),
                unmanaged_email.clone(),
            ))
        };
        if let (Some(email), false) = (&unmanaged_email, req.force) {
            return Ok(done(
                SwitchReason::UnmanagedAccount,
                unmanaged_message(email),
            ));
        }
        let mut walked = Vec::new();
        let target = match &req.target {
            SwitchTarget::Account(id) => {
                // A switch never crosses providers (§9.3).
                let target = store
                    .account(id)?
                    .filter(|a| a.provider == req.provider)
                    .ok_or_else(|| EngineError::NoSuchAccount(id.to_string()))?;
                // §9.2: a session-owned target is refused, with or without --force, before its
                // vault is read. `rederive` checks again under the locks.
                self.refuse_session_owned(p, &target)?;
                target
            }
            SwitchTarget::Rotation => {
                match self.rotation(p, store, &req.provider, live_row.as_ref())? {
                    Rotation::To(a, passed) => {
                        walked = passed;
                        a
                    }
                    Rotation::Stay(reason, message) => return Ok(done(reason, message.into())),
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
                return Ok(done(SwitchReason::AlreadyActive, already_active(&target)));
            }
        }
        Ok(Planned::Go(Plan {
            target,
            strategy,
            self_switch,
            hint,
            walked,
            warnings: vec![],
        }))
    }
```

Replace `freshen` (lines 734–842) with:

```rust
    /// One row of §7.2's manual-switch table, for the plan's target.
    fn freshen(
        &self,
        p: &dyn Provider,
        req: &SwitchRequest,
        plan: &Plan,
    ) -> Result<Freshened, EngineError> {
        let target = &plan.target;
        // A self-switch activates what is already live: only CC, or §7.5, refreshes that token.
        // A forced one re-activates the vault's generation (§9.2), so it freshens like any other.
        if (plan.self_switch && !req.force) || !p.kind_traits(&target.kind).refreshable {
            return Ok(Freshened::Go(vec![]));
        }
        // §9.2 lazy capture before the freshen decision: a quiescent profile that rotated is
        // adopted first, so `due` judges the generation this switch will activate, which may
        // expire sooner than the vault's older one. The target's account lock is taken alone
        // and released before the switch takes `MutationGuard` (§4.3: no lock is taken while
        // a later one is held), and `transact` settles the profile again under every lock.
        // The row is read again once the lock is held: a replacement that landed since
        // planning moved `login_epoch`, and the profile's stale mark is judged against it.
        hooks::point(self, "freshen-before-lock")?;
        let reread;
        let target = {
            let lock = self.lock_account(&target.id)?;
            let current = |store: &Store| -> Result<AccountRow, EngineError> {
                store
                    .account(&target.id)?
                    .ok_or_else(|| EngineError::NoSuchAccount(target.label.clone()))
            };
            let store = self.store()?;
            let row = current(&store)?;
            self.settle_rescues(p, &row, &lock)?;
            self.settle_profile(p, &row, &lock)?;
            reread = current(&store)?;
            &reread
        };
        let vault = self.read_target(target)?;
        let due = self.due(p, &vault);
        let rotation = matches!(req.target, SwitchTarget::Rotation);
        if target.quarantine_reason.is_some() {
            // Never refreshed (§7.4): usable only while its current access token lasts.
            return match (due, rotation) {
                (false, _) => Ok(Freshened::Go(vec![works_until_expiry(target)])),
                (true, true) => Ok(Freshened::Replan),
                (true, false) => Err(needs_relogin(target)),
            };
        }
        if !due {
            return Ok(Freshened::Go(vec![]));
        }
        let label = target.label.as_str();
        let pending = |detail: String| EngineError::RescuePending {
            position: target.position,
            label: target.label.clone(),
            detail,
        };
        Ok(match self.refresh_stored(p, &target.id, &vault)? {
            // Busy: another process is refreshing it now. The account lock this switch waits
            // for, and the pending-rescue settle under it (§6.2), pick up that refresh.
            GateOutcome::Refreshed(_) | GateOutcome::AlreadyFresh(_) | GateOutcome::Busy => {
                Freshened::Go(vec![])
            }
            GateOutcome::Dead(_) if rotation => Freshened::Replan,
            GateOutcome::Dead(_) => return Err(needs_relogin(target)),
            GateOutcome::Transient { rescued: true, .. } => {
                return Err(pending(
                    "the refresh succeeded, but the vault could not be written; the new token is in rescue/".into(),
                ));
            }
            GateOutcome::Transient { kind, .. } if kind == "rescue-unreadable" => {
                // The gate reports both an unreadable rescue file and a failed adoption of a
                // readable one under this kind; only unreadable files can be named. The error's
                // own text adds "retry once the vault can be written".
                let damaged: Vec<String> = self
                    .rescues_for(&target.id)
                    .into_iter()
                    .filter_map(|r| match r {
                        RescueFile::Unreadable { path, .. } => Some(path.display().to_string()),
                        RescueFile::Entry(_) => None,
                    })
                    .collect();
                let detail = if damaged.is_empty() {
                    "a pending rescue could not be adopted".to_owned()
                } else {
                    format!("{} cannot be read", damaged.join(", "))
                };
                return Err(pending(detail));
            }
            // Nothing was spent, or what was spent is lost either way; once the account is
            // live, the gate leaves its refresh to CC (§7.3 step 2). A profile that cannot be
            // read (`profile-unreadable`) lands here too, and the transaction refuses it.
            GateOutcome::Transient { kind, .. } => {
                Freshened::Go(vec![cannot_refresh(p.display_name(), label, &kind)])
            }
            GateOutcome::Systemic(detail) => {
                Freshened::Go(vec![cannot_refresh(p.display_name(), label, &detail)])
            }
            // The successor is lost and the vault's generation is spent (§7.3 step 6): no
            // retry helps, and activating would hand CC a used refresh token.
            GateOutcome::Unpersisted => return Err(needs_relogin(target)),
            // A journal row names it. The gate cannot tell a switch still in progress from an
            // interrupted one, so this does not refuse: with `--force` the switch supersedes
            // the row, and without it `guard_or_refuse` decides under the mutation lock (it
            // waits for a live holder, and recovers or refuses a dead one with the right
            // message).
            GateOutcome::Owned(OwnedBy::Journal) => Freshened::Go(vec![cannot_refresh(
                p.display_name(),
                label,
                "an unfinished switch names it",
            )]),
            // The gate found it live where planning did not (it became the live login, or the
            // live identity could not be read): never refreshed here. `rederive` plans the
            // self-switch again, and the warning is carried into its outcome.
            GateOutcome::Owned(OwnedBy::Live) => Freshened::Go(vec![cannot_refresh(
                p.display_name(),
                label,
                "it may be the live login",
            )]),
            // §7.2's last row, §9.2's refusals. A session that started since planning owns the
            // target: a rotation plans again, and its walk now skips it (§9.3); a direct target
            // is refused. A conflicting profile refuses either way (§12.5).
            GateOutcome::Owned(OwnedBy::Session) if rotation => Freshened::Replan,
            GateOutcome::Owned(OwnedBy::Session) => return Err(session_owned(target)),
            GateOutcome::Conflict => return Err(profile_conflict(target)),
        })
    }
```

Replace `rederive` (lines 981–1070) with:

```rust
    /// §9.4 step 1: the live account, the target and the self-switch decision, all re-read
    /// under the locks. `Replan` when any of them moved, or the account locks held are no
    /// longer the outgoing account's; planning again then reaches the right outcome.
    fn rederive(
        &self,
        p: &dyn Provider,
        store: &Store,
        req: &SwitchRequest,
        plan: &Plan,
        outgoing: Option<&AccountRow>,
    ) -> Result<Rederived, EngineError> {
        let (live_identity, again) = self.live_row(p, store, &req.provider)?;
        // A login that became unmanaged is §9.2's no-op; a target removed meanwhile is
        // replaced (rotation) or reported (direct).
        if live_identity.is_some() && again.is_none() && !req.force {
            return Ok(Rederived::Replan);
        }
        let Some(target) = store.account(&plan.target.id)? else {
            return Ok(Rederived::Replan);
        };
        let self_switch = again.as_ref().is_some_and(|r| r.id == target.id);
        // Review Focus 3: another process landed exactly this rotation's target while this one
        // waited for the mutation lock (a double-fired `switch`). That was this command's work;
        // rotating on from there would switch twice. A direct target needs no such rule: planning
        // again finds the self-switch no-op.
        if matches!(req.target, SwitchTarget::Rotation) && self_switch && !plan.self_switch {
            return Ok(Rederived::Done(noop(
                plan.strategy,
                SwitchReason::AlreadyActive,
                already_active(&target),
                again,
                None,
            )));
        }
        // §9.4 step 1: the target is not session-owned. Launch reservations are written under
        // `MutationGuard` and refreshes under the account lock, both held here, so the answer
        // stands until the locks are released. A direct target is refused (§9.2); a rotation
        // plans again, and its walk skips the account (§9.3).
        if self.session_state(p, &target)?.owned() {
            return match req.target {
                SwitchTarget::Account(_) => Err(session_owned(&target)),
                SwitchTarget::Rotation => Ok(Rederived::Replan),
            };
        }
        // The rotation decision, recomputed from the store alone (§9.2, §9.3), including the
        // fewer-than-two case; no vault but the target's is read here. Its anchor is `again`,
        // which is `outgoing` when anything proceeds.
        // Only the target's vault is read here (§9.3): if it emptied while this command waited,
        // a rotation plans again and rotates on, and a direct switch reports it.
        if !self.has_login(&target)? {
            return Ok(Rederived::Replan);
        }
        // §9.4 step 1 (amended): a refresh that finished while this switch waited for the
        // target's account lock may have quarantined it (§7.4 `successor_lost`). This is
        // decided from the row just read, never from what the plan saw: a plan made on an
        // earlier attempt may already hold the quarantine, yet nothing else applies §7.2's
        // quarantined-target rule under the locks. A rotation plans again, and the walk skips
        // the row. A direct target is refused when its access token is due, and otherwise
        // activated with the warning (unless freshening already gave it). An unforced
        // self-switch activates the live generation, which no refresh has spent, so the rule
        // does not apply to it (§7.2: only CC refreshes a live token); a forced one re-activates
        // the vault's generation (§9.2), so it does.
        let mut warnings = Vec::new();
        if target.quarantine_reason.is_some() {
            if matches!(req.target, SwitchTarget::Rotation) {
                return Ok(Rederived::Replan);
            }
            if (!self_switch || req.force) && p.kind_traits(&target.kind).refreshable {
                let vault = self.read_target(&target)?;
                if self.due(p, &vault) {
                    return Err(needs_relogin(&target));
                }
                let warning = works_until_expiry(&target);
                if !plan.warnings.contains(&warning) {
                    warnings.push(warning);
                }
            }
        }
        let same_pick = match req.target {
            SwitchTarget::Account(_) => true,
            SwitchTarget::Rotation => self.rotation_pick_stands(p, store, plan, again.as_ref())?,
        };
        // Account-lock acquisition may have finished a pending replacement (§12.5), changing
        // the outgoing account's kind or identity: compare the rows, not just their IDs.
        let unchanged = login_of(again.as_ref()) == login_of(outgoing)
            && target.kind == plan.target.kind
            && target.identity_key == plan.target.identity_key
            && self_switch == plan.self_switch
            && same_pick;
        Ok(if unchanged {
            Rederived::Go(Locked {
                live_identity,
                target,
                outgoing: again,
                warnings,
            })
        } else {
            Rederived::Replan
        })
    }
```

Add this method directly after `read_target`:

```rust
    /// §9.2 and §12.5 "Lazy capture", under the target's account lock and after its rescues
    /// are settled (§6.2). A quiescent profile that rotated since its seed is adopted into the
    /// vault, so the activation that follows reads its generation. A conflict refuses, since the
    /// vault's generation may be consumed. A profile that cannot be read refuses as an
    /// unreadable vault does, naming the account (Decision 9).
    fn settle_profile(
        &self,
        p: &dyn Provider,
        target: &AccountRow,
        lock: &AccountLock,
    ) -> Result<(), EngineError> {
        let unreadable = |source: ReadError| EngineError::UnreadableAccount {
            position: target.position,
            label: target.label.clone(),
            source,
        };
        match self.apply_provenance(p, target, lock) {
            Ok(ProfileCheck::Conflict) => Err(profile_conflict(target)),
            Ok(ProfileCheck::Unreadable(detail)) => {
                Err(unreadable(ReadError::new("its session profile", detail)))
            }
            Ok(_) => Ok(()),
            Err(EngineError::Unreadable(source)) => Err(unreadable(source)),
            Err(e) => Err(e),
        }
    }
```

In `transact`, replace:

```rust
        let target_lock = account_locks
            .iter()
            .find(|l| l.id() == &target.id)
            .expect("the target is locked");
        self.settle_rescues(p, &target, target_lock)?;
```

with:

```rust
        let target_lock = account_locks
            .iter()
            .find(|l| l.id() == &target.id)
            .expect("the target is locked");
        self.settle_rescues(p, &target, target_lock)?;
        // §9.2 lazy capture, after the rescues and before either branch reads the target's
        // vault: a rotation the profile holds is the generation composed and written below. A
        // conflict or an unreadable profile refuses before the journal row exists, so there is
        // nothing to roll back.
        self.settle_profile(p, &target, target_lock)?;
```

This is an anchored insertion, not the whole of `transact`, because Task 3 (the journal's
`to_epoch`, the stale-live-store displace) and Task 5 (the `on_fallback` filter's removal) both
rewrite other parts of `transact` before this task runs. The anchor is the rescue settlement,
which neither moves.

Callers changed in `switch.rs`:
- `plan` passes `p` to `rotation`.
- `rotation` and `rotation_pick_stands` pass `p` to `candidate_order`.
- `rederive` passes `p` to `rotation_pick_stands`.

No other file calls these private functions.

- [ ] **Step 4: Run them and see them pass**, then the whole suite

Run: `cargo test -p tagteam-engine --features test-hooks --test session --test provenance`
Expected: PASS.

Run: `cargo test -p tagteam-engine --features test-hooks`, then
`cargo test --workspace --features tagteam/test-support`
Expected: PASS. No existing account has a profile, so `switch_candidate` agrees with
`is_candidate` everywhere and `settle_profile` finds `NotApplicable`. The freshen stubs had no
tests (preflight B §1).

- [ ] **Step 5: Format and lint**

```
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

The `under_the_locks` module is compiled only with `test-hooks`, so the clippy run without
features sees none of its imports.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-engine/src/error.rs crates/tagteam-engine/src/switch.rs \
  crates/tagteam-engine/tests/session.rs crates/tagteam-engine/tests/provenance.rs
git commit -m "Refuse a session-owned or conflicting switch target, skip session-owned candidates, and activate a profile's rotation"
```

---

### Task 12: The session-owned usage fetch

§8.1, "Session-owned account": "The fetch is read-only and uses the profile's token. CC in the
session owns that token, so tagteam never refreshes it, never writes the profile, and takes no
lock to read it." Today a session-owned account takes the inactive path. It sends the vault's
token, which the profile may have rotated past since bootstrap. When that token is due or
refused, the path goes to the gate, which after Task 9 refuses with `Owned(Session)`, so `list`
shows `refresh-failed` for every account a session is using. This task adds a third role beside
active and inactive. `collect_usage` decides it before any thread starts, and
`Collection::session` reads the profile's own token and sends it as it is.

**Readings of the spec this task commits to:**
- **The role is decided once, before the threads, beside the live role.** `role_of` runs
  `Engine::session_state` (Task 9) for every row the live login does not name. A row whose
  state is `owned()` (a held reservation, a live record, or a reservation or record that cannot
  be read, §12.6) takes the session branch with the state's profile. Decision 8 holds: the state
  is computed per call and never cached.
- **The live login wins (Decision 18).** An account that the live login names and that a
  session also owns is collected as the active account. §12.8 says "the live login ... and usage collection all see
  the default home, exactly as outside the shell". The two copies are separate lineages: §12.1
  never starts a session on the live login, so the only way to get both is a `/login` in the
  default home while the session runs. The rejected alternative, session first, would leave the
  default home's expired token with nobody to hand it to §7.5.
- **The read order is marker, then identity, then credential (Decision 18).** §12.5: "a profile
  whose identity drifted is not used". So the drift check comes before the credential read, and
  a drifted profile's Keychain item is never touched.
  - A marker that is absent, unreadable, or names another account or provider gives no
    recorded spelling, and §12.2 forbids deriving one. The credential is then unreadable in
    Decision 9's sense, which gives `keychain-unavailable`.
  - An identity that differs is `profile-drifted`. So is one that is absent or unreadable: it
    cannot be confirmed as the account's, and a reading attributed to the wrong account could
    drive a wrong switch. `profile-drifted` falls through to `usageStatus: unavailable`, which
    is §8.1's outcome for drift (Decision 10).
  - A degraded credential is used, unreadable gives `keychain-unavailable`, and absent or empty
    gives `no-access-token`.
- **The drift rule is §12.5's, defined once.** The email is compared, or the label for an
  identity that has no email (FakeAgent's). The organization is compared only when both sides
  name one. It is `identity_drifted` in `provenance.rs` (Task 10, `pub(crate)`), which provenance
  and this branch share. This task imports it and defines no rule of its own.
- **`send` already gives expired and refused tokens for free.** It refuses an expired or
  remembered-refused token before `authorize`, so the slot stays in `self.slot`, and
  `record_usage_failure` gives it back (§8.3). The session branch checks a remembered refusal
  itself, first, so a setup token's refusal stays `http-401`, as on every other path.
- **What a refusal records.** A refused token records `token-expired` when its kind refreshes,
  because the agent in the session refreshes it on its next call. Otherwise it records
  `http-401`, which covers a setup token (§8.1's last bullet, Decision 11). This applies to a new
  401, to a remembered `rejected_fp`, and to a stamp another process writes before the send
  (`authorize`'s `Rejected` arm). One helper, `refusal_kind`, gives the kind for all three, and
  for the active role as well, so that behaviour is unchanged.
- **No lock, no write, no refresh, no retry.** The branch never calls the gate, `refresh_active`,
  `live_bytes` or any lock. The tests hold the account lock and the mutation lock throughout, so
  any of those would show as `refresh-failed`.
- **Ownership comes from the reservation or the records, not the marker.** Task 9's
  `session_state` looks at the profile directory, its launch reservations and its session
  records, and never reads the marker. A profile directory whose marker is missing, corrupt or
  another account's, with a held reservation, is therefore `Owned`, and the session branch is
  what reports its marker problem (`keychain-unavailable`). The tests own an account by a held
  reservation, or by a record that cannot be parsed; none judges a pid or an `lstart`, so
  Decision 16's liveness rule changes no expectation here.
- **The degraded credential keeps its provenance.** `send` is split into `send(bytes)` and
  `send_credential(&Credential)`. The vault, gate and live paths keep wrapping their bytes as
  fresh, which they are. The session path passes its `Credential` as read, so
  `Credential::fresh`'s contract ("only for bytes from a successful, authoritative read") still
  holds.
- **Candidate poll policy.** The record's plan uses `active_now()`, which falls back to
  `self.role == Role::Active`, so a session account gets §8.6's candidate policy.
- **M3a.** Its cancellation points are before reserving (in `collect_one`) and before sending.
  Re-sync puts the second one in `send_credential`, which the session branch shares, so the
  branch needs no point of its own.

**Files:**
- Modify: `crates/tagteam-engine/src/collect.rs`:
  - the module doc (:1-5) and imports (:7-25);
  - `collect_usage` (:78-145) and `collect_one` (:175-232);
  - the `Collection` struct (:235-262);
  - `send` (:591-614), split into `send` and `send_credential`;
  - `authorize` (:616-671) and `active_now` (:727-746);
  - new: `Role`, `Engine::role_of` and `Collection::{session, profile_credential, refusal_kind}`.
    The drift rule is not defined here: `collect.rs` imports `crate::provenance::identity_drifted`
    (Task 10). The unit-test module (:813-824) is unchanged.
- Modify: `crates/tagteam-engine/src/views.rs` (the test `usage_status_follows_the_table_row_by_row`, one row, after the one Task 4 added)
- Modify: `crates/tagteam-engine/tests/common/mod.rs`: two methods, `Fx::set_profile_identity` and
  `Fx::profile_credential`, appended after Task 9's session-helpers `impl Fx` block. No import
  changes: `Platform`, `fs`, `json`, `Path` and `Value` are already imported there. Every other
  profile fixture comes from Task 8 and Task 9.
- Create: `crates/tagteam-engine/tests/session_usage.rs`

**Interfaces:**
- Consumes:
  - **Task 7:**
    - `tagteam_provider::profile::{ProfileMarker, MARKER_FILE}`;
    - `Provider::read_profile_credential(&self, env: &Env, spelling: &str) -> Read<Credential>`;
    - `Provider::profile_identity(&self, env: &Env, spelling: &str) -> Read<Identity>`.
  - **Task 8:** `Fx::make_profile(&self, id: &AccountId) -> PathBuf` (the marker and the stored
    row's identity in `.claude.json`).
  - **Task 9:**
    - `Engine::session_state(&self, p: &dyn Provider, row: &AccountRow) -> Result<SessionState, EngineError>`;
    - `SessionState::{owned, profile}`;
    - every `Fx` engine built with an `EngineConfig.process`;
    - `Fx::{hold_reservation(&self, profile: &Path) -> FlockGuard, plant_record(&self, profile: &Path, name: &str, record: &[u8]) -> PathBuf, set_profile_credential(&self, profile: &Path, bytes: &[u8]), profile_item(&self, profile: &Path) -> (String, String)}`.
  - **Task 10:** `crate::provenance::identity_drifted(identity: &Identity, row: &AccountRow) -> bool`
    (`pub(crate)`).
  - **Task 6:** the reservation probe behind `launch_reservations`. `session_state` reaches it;
    this task calls it only through Task 9.
  - **Task 5:** one Keychain item per spelling (`keychain_service` of the profile `Env`).
  - **Existing:**
    - `Collection::{usable, is_rejected, reject, authorize}`, `windows`, `failed`;
    - `Store::{reserve_usage, authorize_send, set_rejected_fp, record_usage, record_usage_failure}`;
    - `FlockGuard::try_lock`, `AccountLock::try_acquire`, `MutationGuard::acquire`;
    - the fixture: `Fx::{collect, usage_state, script_usage, script_refresh, put_vault, expire_access, add_token_options}`
      and `usage_bearers`, `usage_requests`, `token_requests`, `access_fp`, `failed`,
      `refused`, `credential`, `two_accounts`.
- Produces:
  - **Private:** the session branch (`Role::Session`, `Collection::session`), which M3b's
    scheduled collection inherits with no change.
  - **Shared test helpers** in `tests/common/mod.rs`, both on `Fx`:
    - `pub fn set_profile_identity(&self, profile: &Path, email: &str)`: writes
      `<profile>/.claude.json` as `{"oauthAccount": Fx::oauth_account(email)}`, where Claude Code
      reads the profile's identity (§12.5);
    - `pub fn profile_credential(&self, profile: &Path) -> Option<Vec<u8>>`: what Claude Code's
      profile credential holds, which is the hashed Keychain item (`profile_item(profile)`) on
      macOS and `<profile>/.credentials.json` on Linux.

**Spec:**
- §8.1 "Session-owned account": read-only, the profile's token, no refresh, no profile write,
  no lock. A degraded read may be used, and an unreadable one is `keychain_unavailable`. An
  expired token is `token_expired` with no request, as a fetch that ends before sending. A 401
  stamps `rejected_fp` and is `token_expired`. The same bytes are not sent again. A drifted
  identity is `unavailable`.
- §8.1, last bullet: a token that cannot be refreshed records `http-401` on every path, new or
  remembered.
- §8.3: a fetch that ends before sending gives back its slot.
- §8.6: the candidate policy for any account that is not the default home's live login.
- §12.2 "One spelling": every profile credential operation uses the recorded spelling.
- §12.5 "Identity drift": compare the email, and the org when both are set.
- §12.6: an unreadable record counts as live, so the account is session-owned.
- §12.8: the live login is always the default home's.
- Decision 9: an unreadable profile credential gives `keychain_unavailable`.
- Decision 10: `profile-drifted` gives `usageStatus: unavailable`.

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-engine/tests/common/mod.rs`, append two methods to Task 9's session-helpers
`impl Fx` block (after `dead_record`). Every other profile fixture the tests use comes from
Task 8 (`make_profile`) and Task 9 (`hold_reservation`, `plant_record`, `set_profile_credential`,
`profile_item`); no import changes are needed.

```rust
    /// Writes `profile`'s `.claude.json` with `email`'s `oauthAccount`, where Claude Code reads
    /// the profile's identity (§12.5). `make_profile` already wrote the stored row's own login
    /// there; this replaces it.
    pub fn set_profile_identity(&self, profile: &Path, email: &str) {
        fs::write(
            profile.join(".claude.json"),
            json!({"oauthAccount": Self::oauth_account(email)}).to_string(),
        )
        .unwrap();
    }

    /// What Claude Code's profile credential holds, if anything: the profile's hashed Keychain
    /// item on macOS (`profile_item`), `<profile>/.credentials.json` on Linux.
    pub fn profile_credential(&self, profile: &Path) -> Option<Vec<u8>> {
        match self.platform {
            Platform::MacOs => {
                let (svc, acct) = self.profile_item(profile);
                self.kc.get(&svc, &acct)
            }
            Platform::Linux => fs::read(profile.join(".credentials.json")).ok(),
        }
    }
```

Create `crates/tagteam-engine/tests/session_usage.rs`:

```rust
//! §8.1 "Session-owned account": an account a `tagteam run` session owns is collected from its
//! profile, read as Claude Code in the session reads it, with no lock, and never refreshed,
//! written or retried. A session is a held launch reservation (§12.5) or an unreadable session
//! record (§12.6); no test reaches a real pid.
mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use common::{
    Fx, access_fp, credential, failed, refused, token_requests, two_accounts, usage_bearers,
    usage_fixture, usage_requests,
};
use serde_json::json;
use tagteam_cc::live::Platform;
use tagteam_core::AccountId;
use tagteam_engine::account_lock::AccountLock;
use tagteam_engine::collect::Collected;
use tagteam_engine::store::UsageStateRow;
use tagteam_provider::profile::{MARKER_FILE, ProfileMarker};
use tagteam_provider::{FlockGuard, Keychain, MutationGuard, Provider, Read};

/// The fixture clock's start, in the seconds the usage tables hold.
const NOW_S: i64 = 1_790_000_000;

/// An account a running session owns: its profile, the credential the profile holds, and the
/// session's reservation, held while this lives.
struct Session {
    id: AccountId,
    profile: PathBuf,
    bytes: Vec<u8>,
    _reservation: FlockGuard,
}

/// Puts `bytes` where Claude Code keeps `profile`'s credential: its hashed Keychain item on
/// macOS (`Fx::profile_item`, the item the profile's marker names), `<profile>/.credentials.json`
/// on Linux. Private to this file: no other test needs to write the item.
fn set_credential(fx: &Fx, profile: &Path, bytes: &[u8]) {
    match fx.platform {
        Platform::MacOs => {
            let (svc, acct) = fx.profile_item(profile);
            fx.kc.put(&svc, &acct, bytes);
        }
        Platform::Linux => fx.set_profile_credential(profile, bytes),
    }
}

/// Gives the stored account `id` a profile (`make_profile`: its marker, and its own login as the
/// profile's identity) holding `bytes`, and a running session: a held launch reservation.
fn owned(fx: &Fx, id: &AccountId, bytes: Vec<u8>) -> Session {
    let profile = fx.make_profile(id);
    set_credential(fx, &profile, &bytes);
    let reservation = fx.hold_reservation(&profile);
    Session {
        id: id.clone(),
        profile,
        bytes,
        _reservation: reservation,
    }
}

/// `a` inactive and session-owned, its profile holding `rt-p` while the vault still holds
/// `rt-a`; `b` is the live login.
fn session(fx: &Fx) -> Session {
    let a = two_accounts(fx);
    owned(fx, &a, credential("a@x.co", "rt-p"))
}

/// A setup-token account, inactive and session-owned (`b` is live). Its profile holds another
/// setup token than the vault's, so a request shows which one was sent.
fn setup_token_session(fx: &Fx) -> Session {
    fx.add("b@x.co", "rt-b");
    let s = fx
        .engine
        .add_token(fx.add_token_options("sk-ant-oat01-vault"))
        .unwrap()
        .account;
    fx.http.clear();
    let (_, bytes) = fx.cc.token_secret("sk-ant-oat01-profile");
    owned(fx, &s.id, bytes)
}

/// `rt`'s credential, with an access token that has expired.
fn expired_credential(rt: &str) -> Vec<u8> {
    let mut v = Fx::credential_json("a@x.co", rt);
    v["claudeAiOauth"]["expiresAt"] = json!(NOW_S * 1000 - 1);
    v.to_string().into_bytes()
}

fn state(fx: &Fx, id: &AccountId) -> UsageStateRow {
    fx.usage_state(id)
        .expect("the collection wrote a usage_state row")
}

/// Where Claude Code reads the profile's identity.
fn profile_config(s: &Session) -> PathBuf {
    s.profile.join(".claude.json")
}

type Breakage = fn(&Fx, &Session);

#[test]
fn a_session_owned_account_is_read_from_its_profile_and_planned_as_a_candidate() {
    for platform in [Platform::MacOs, Platform::Linux] {
        let fx = Fx::with_platform(platform);
        let s = session(&fx);
        let marker = fs::read(s.profile.join(MARKER_FILE)).unwrap();
        let identity = fs::read(profile_config(&s)).unwrap();
        let held = fx.profile_credential(&s.profile);
        fx.script_usage(200, usage_fixture());

        let report = fx.collect(&[&s.id]);

        assert_eq!(
            report.outcomes,
            [(s.id.clone(), Collected::Recorded)],
            "{platform:?}"
        );
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert_eq!(
            usage_bearers(&fx),
            ["at-rt-p"],
            "{platform:?}: the profile's token, not the vault's"
        );
        assert_eq!(token_requests(&fx), 0);
        assert_eq!(usage_requests(&fx), 1);
        // §8.6: not the default home's live login, so the candidate policy (300 s, grown to 450
        // by an idle first reading), never the active one (180 s, grown to 270).
        assert_eq!(state(&fx, &s.id).poll_interval_s, Some(450));
        // Read only: the profile and the vault are as they were.
        assert_eq!(fs::read(s.profile.join(MARKER_FILE)).unwrap(), marker);
        assert_eq!(fs::read(profile_config(&s)).unwrap(), identity);
        assert_eq!(fx.profile_credential(&s.profile), held);
        assert_eq!(fx.vault_refresh_token(&s.id).as_deref(), Some("rt-a"));
    }
}

#[test]
fn a_degraded_profile_read_is_still_sent() {
    // §8.1: an access token sent to the usage endpoint consumes nothing.
    let fx = Fx::new();
    let s = session(&fx);
    let (svc, acct) = fx.profile_item(&s.profile);
    fx.kc.set_unreadable(&svc, &acct, true);
    // The file Claude Code falls back to when the item cannot be read.
    fx.set_profile_credential(&s.profile, &credential("a@x.co", "rt-file"));
    fx.script_usage(200, usage_fixture());

    let report = fx.collect(&[&s.id]);

    assert_eq!(report.outcomes, [(s.id.clone(), Collected::Recorded)]);
    assert_eq!(usage_bearers(&fx), ["at-rt-file"]);
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn an_unreadable_profile_credential_is_keychain_unavailable_and_sends_nothing() {
    let fx = Fx::new();
    let s = session(&fx);
    let (svc, acct) = fx.profile_item(&s.profile);
    fx.kc.set_unreadable(&svc, &acct, true); // and no plaintext file covers it
    fx.script_usage(200, usage_fixture());

    let report = fx.collect(&[&s.id]);

    assert_eq!(
        report.outcomes,
        [(s.id.clone(), failed("keychain-unavailable"))]
    );
    assert!(fx.http.requests().is_empty());
    assert_eq!(usage_requests(&fx), 0, "the slot went back");
}

#[test]
fn a_profile_without_an_access_token_is_no_access_token_and_sends_nothing() {
    let cases: [(&str, Breakage); 3] = [
        ("no credential at all", |fx, s| {
            let (svc, acct) = fx.profile_item(&s.profile);
            fx.kc.delete(&svc, &acct).unwrap();
        }),
        ("an empty item", |fx, s| set_credential(fx, &s.profile, b"")),
        ("a refresh token alone", |fx, s| {
            set_credential(
                fx,
                &s.profile,
                json!({"claudeAiOauth": {"refreshToken": "rt-p"}})
                    .to_string()
                    .as_bytes(),
            )
        }),
    ];
    for (case, break_it) in cases {
        let fx = Fx::new();
        let s = session(&fx);
        break_it(&fx, &s);
        fx.script_usage(200, usage_fixture());

        let report = fx.collect(&[&s.id]);

        assert_eq!(
            report.outcomes,
            [(s.id.clone(), failed("no-access-token"))],
            "{case}"
        );
        assert!(fx.http.requests().is_empty(), "{case}");
        assert_eq!(usage_requests(&fx), 0, "{case}: the slot went back");
    }
}

#[test]
fn an_expired_profile_token_is_token_expired_without_a_request_and_gives_its_slot_back() {
    let fx = Fx::new();
    let s = session(&fx);
    set_credential(&fx, &s.profile, &expired_credential("rt-p"));
    fx.script_refresh(Some("rt-new"));
    fx.script_usage(200, usage_fixture());

    let report = fx.collect(&[&s.id]);

    assert_eq!(report.outcomes, [(s.id.clone(), failed("token-expired"))]);
    assert!(
        fx.http.requests().is_empty(),
        "no refresh and no usage request: the agent refreshes it on its next call"
    );
    assert_eq!(usage_requests(&fx), 0);
    assert_eq!(
        state(&fx, &s.id).last_error.as_deref(),
        Some("token-expired")
    );
}

#[test]
fn a_401_stamps_the_profile_token_which_is_not_sent_again_until_it_changes() {
    let fx = Fx::new();
    let s = session(&fx);
    fx.script_usage(401, refused());
    fx.script_refresh(Some("rt-new"));

    let report = fx.collect(&[&s.id]);

    assert_eq!(report.outcomes, [(s.id.clone(), failed("token-expired"))]);
    assert_eq!(usage_bearers(&fx), ["at-rt-p"], "no retry");
    assert_eq!(token_requests(&fx), 0, "no refresh");
    assert_eq!(usage_requests(&fx), 1);
    let fp = access_fp(&fx, &s.bytes);
    assert_eq!(
        state(&fx, &s.id).rejected_fp.as_deref(),
        Some(fp.as_str())
    );

    // Remembered: the same bytes are not sent, and the slot goes back.
    fx.http.clear();
    fx.clock.advance_ms(91_000);
    let report = fx.collect(&[&s.id]);
    assert_eq!(report.outcomes, [(s.id.clone(), failed("token-expired"))]);
    assert!(fx.http.requests().is_empty());
    assert_eq!(usage_requests(&fx), 1);

    // The agent in the session refreshed its token: the new bytes are sent.
    set_credential(&fx, &s.profile, &credential("a@x.co", "rt-p2"));
    fx.clock.advance_ms(91_000);
    fx.script_usage(200, usage_fixture());
    let report = fx.collect(&[&s.id]);
    assert_eq!(report.outcomes, [(s.id.clone(), Collected::Recorded)]);
    assert_eq!(usage_bearers(&fx), ["at-rt-p2"]);
    assert_eq!(state(&fx, &s.id).rejected_fp, None, "a success clears it");
}

#[test]
fn a_refused_setup_token_in_a_session_is_a_401_whether_first_or_remembered() {
    // §8.1's last bullet: a token that cannot be refreshed records `http-401` on every path.
    let fx = Fx::new();
    let s = setup_token_session(&fx);
    fx.script_usage(401, refused());

    let report = fx.collect(&[&s.id]);

    assert_eq!(report.outcomes, [(s.id.clone(), failed("http-401"))]);
    assert_eq!(
        usage_bearers(&fx),
        ["sk-ant-oat01-profile"],
        "the profile's token, once"
    );
    assert_eq!(token_requests(&fx), 0);
    let fp = access_fp(&fx, &s.bytes);
    assert_eq!(
        state(&fx, &s.id).rejected_fp.as_deref(),
        Some(fp.as_str())
    );

    fx.http.clear();
    fx.clock.advance_ms(91_000);
    let report = fx.collect(&[&s.id]);
    assert_eq!(report.outcomes, [(s.id.clone(), failed("http-401"))]);
    assert!(
        fx.http.requests().is_empty(),
        "the refused token is not sent again"
    );
    assert_eq!(
        usage_requests(&fx),
        1,
        "the remembered refusal gave its slot back"
    );
}

#[test]
fn a_profile_whose_identity_drifted_is_not_used() {
    let cases: [(&str, Breakage); 3] = [
        ("another login", |fx, s| {
            fx.set_profile_identity(&s.profile, "other@x.co")
        }),
        ("no oauthAccount", |fx, s| {
            fs::write(profile_config(s), "{}\n").unwrap()
        }),
        ("a torn .claude.json", |fx, s| {
            fs::write(profile_config(s), "{\"oauthAccount\": ").unwrap()
        }),
    ];
    for (case, break_it) in cases {
        let fx = Fx::new();
        let s = session(&fx);
        break_it(&fx, &s);
        // The credential of a profile that is not the account's is never read: an unreadable
        // one would otherwise say `keychain-unavailable`.
        let (svc, acct) = fx.profile_item(&s.profile);
        fx.kc.set_unreadable(&svc, &acct, true);
        fx.script_usage(200, usage_fixture());

        let report = fx.collect(&[&s.id]);

        assert_eq!(
            report.outcomes,
            [(s.id.clone(), failed("profile-drifted"))],
            "{case}"
        );
        assert!(fx.http.requests().is_empty(), "{case}");
        assert_eq!(usage_requests(&fx), 0, "{case}");
    }
}

#[test]
fn a_profile_whose_marker_cannot_name_its_credential_is_keychain_unavailable() {
    // §12.2: the credential is read under the recorded spelling, never one derived again. The
    // held reservation keeps the account session-owned whatever the marker says (Task 9's
    // `session_state` never reads it), so the branch itself reports the marker.
    let cases: [(&str, Breakage); 3] = [
        ("no marker", |_, s| {
            fs::remove_file(s.profile.join(MARKER_FILE)).unwrap()
        }),
        ("a corrupt marker", |_, s| {
            fs::write(s.profile.join(MARKER_FILE), "{").unwrap()
        }),
        ("another account's marker", |_, s| {
            let Read::Present(marker) = ProfileMarker::read(&s.profile) else {
                panic!("the marker reads");
            };
            ProfileMarker {
                account_id: AccountId::from_string("someone-else"),
                ..marker
            }
            .write(&s.profile)
            .unwrap();
        }),
    ];
    for (case, break_it) in cases {
        let fx = Fx::new();
        let s = session(&fx);
        break_it(&fx, &s);
        fx.script_usage(200, usage_fixture());

        let report = fx.collect(&[&s.id]);

        assert_eq!(
            report.outcomes,
            [(s.id.clone(), failed("keychain-unavailable"))],
            "{case}"
        );
        assert!(fx.http.requests().is_empty(), "{case}");
        assert_eq!(usage_requests(&fx), 0, "{case}");
    }
}

#[test]
fn an_unreadable_session_record_makes_the_account_session_owned_too() {
    // §12.6: a malformed record counts as unreadable, and an unreadable one as live. The record
    // never parses, so no pid is ever looked up.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = fx.make_profile(&a);
    set_credential(&fx, &profile, &credential("a@x.co", "rt-p"));
    fx.plant_record(&profile, "4242", b"[1, 2");
    fx.script_usage(200, usage_fixture());

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), Collected::Recorded)]);
    assert_eq!(usage_bearers(&fx), ["at-rt-p"]);
}

#[test]
fn the_gate_is_never_called_and_no_token_request_is_ever_made_for_a_session_owned_account() {
    // §8.1: the agent in the session owns the profile's token. The vault's token is due, so the
    // inactive path would go through the gate. The test holds the account lock and the mutation
    // lock throughout: the gate would return `Busy`, and a live read would stop, so any lock taken
    // would end as `refresh-failed` or `Dropped`, never as below.
    let fx = Fx::new();
    let s = session(&fx);
    fx.expire_access(&s.id);
    let _account = AccountLock::try_acquire(&fx.env, &s.id)
        .unwrap()
        .expect("the account lock is free");
    let _guard = MutationGuard::acquire(&fx.env, Duration::ZERO).unwrap();
    for _ in 0..3 {
        fx.script_refresh(Some("rt-new"));
    }

    // An expired profile token: nothing is sent at all.
    set_credential(&fx, &s.profile, &expired_credential("rt-p"));
    let report = fx.collect(&[&s.id]);
    assert_eq!(report.outcomes, [(s.id.clone(), failed("token-expired"))]);
    // A current one that the server refuses: one usage request, then the stamp, and no retry.
    fx.clock.advance_ms(91_000);
    set_credential(&fx, &s.profile, &credential("a@x.co", "rt-p2"));
    fx.script_usage(401, refused());
    let report = fx.collect(&[&s.id]);
    assert_eq!(report.outcomes, [(s.id.clone(), failed("token-expired"))]);
    // The same token, remembered: nothing is sent.
    fx.clock.advance_ms(91_000);
    let report = fx.collect(&[&s.id]);
    assert_eq!(report.outcomes, [(s.id.clone(), failed("token-expired"))]);

    assert_eq!(token_requests(&fx), 0, "no token request, ever");
    assert_eq!(
        usage_bearers(&fx),
        ["at-rt-p2"],
        "one usage request, with the profile's token"
    );
    assert_eq!(
        fx.vault_refresh_token(&s.id).as_deref(),
        Some("rt-a"),
        "the vault is untouched"
    );
}

#[test]
fn the_live_login_s_account_is_collected_as_the_active_one_even_when_a_session_holds_it() {
    // §12.8: the live login is always the default home's, and usage collection sees it as it
    // would outside a run shell. The session's copy is another lineage, never sent here.
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    let _s = owned(&fx, &b, credential("b@x.co", "rt-p"));
    fx.script_usage(200, usage_fixture());

    let report = fx.collect(&[&b]);

    assert_eq!(report.outcomes, [(b.clone(), Collected::Recorded)]);
    assert_eq!(usage_bearers(&fx), ["at-rt-b"], "the live token");
    assert_eq!(
        state(&fx, &b).poll_interval_s,
        Some(270),
        "the active policy"
    );
}

#[cfg(feature = "test-hooks")]
mod hooks {
    use super::*;

    /// Stamps `id`'s `rejected_fp` as another process's 401 would, outside this collection's
    /// lease (`Store::set_rejected_fp` is fenced by one).
    fn stamp(db: &Path, id: &AccountId, fp: &str) {
        rusqlite::Connection::open(db)
            .unwrap()
            .execute(
                "INSERT INTO usage_state (account_id, rejected_fp) VALUES (?1, ?2) \
                 ON CONFLICT(account_id) DO UPDATE SET rejected_fp = excluded.rejected_fp",
                rusqlite::params![id.as_str(), fp],
            )
            .unwrap();
    }

    #[test]
    fn a_stamp_written_by_another_process_before_the_send_is_refused_as_the_session_s_kind() {
        // C10 for the session branch: the store's authorization refuses a token stamped after
        // this collection read its state. The agent in the session refreshes an OAuth token
        // itself, so that is `token-expired`. A setup token never refreshes: `http-401`.
        for setup in [false, true] {
            let fx = Fx::new();
            let s = if setup {
                setup_token_session(&fx)
            } else {
                session(&fx)
            };
            let (db, id, fp) = (
                fx.env.data_dir().join("tagteam.db"),
                s.id.clone(),
                access_fp(&fx, &s.bytes),
            );
            fx.engine.on_point(
                "usage-before-send",
                Box::new(move || stamp(&db, &id, &fp)),
            );
            fx.script_usage(200, usage_fixture());

            let report = fx.collect(&[&s.id]);

            let want = if setup { "http-401" } else { "token-expired" };
            assert_eq!(report.outcomes, [(s.id.clone(), failed(want))], "{want}");
            assert!(fx.http.requests().is_empty(), "{want}: nothing was sent");
            assert_eq!(usage_requests(&fx), 0, "{want}: the slot went back");
        }
    }
}
```

In `crates/tagteam-engine/src/views.rs`, in `usage_status_follows_the_table_row_by_row`, add one
row to the table, after the `("live-replaced", Unavailable),` row that Task 4 added (which itself
follows `("refresh-failed", Unavailable),`), so both rows are present:

```rust
            ("profile-drifted", Unavailable),
```

`collect.rs`'s unit-test module (:813-824) is unchanged: the drift rule and its unit test are
Task 10's, in `provenance.rs`.

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-engine --test session_usage`
Expected: it compiles (the fixtures are Task 8's and Task 9's, plus the two methods above), and
FAILS. The session-owned account still takes the inactive path, so each test fails on an
assertion:
- `a_session_owned_account_is_read_from_its_profile_and_planned_as_a_candidate`: the bearer is
  `at-rt-a`, the vault's, not `at-rt-p`.
- `a_degraded_profile_read_is_still_sent`: `at-rt-a`, not `at-rt-file`.
- `an_unreadable_profile_credential_is_keychain_unavailable_and_sends_nothing` and
  `a_profile_without_an_access_token_is_no_access_token_and_sends_nothing`: `Recorded`, not
  the failure, because the vault's token was sent.
- `an_expired_profile_token_is_token_expired_without_a_request_and_gives_its_slot_back`:
  `Recorded`.
- `a_401_stamps_the_profile_token_which_is_not_sent_again_until_it_changes`: the vault's
  token's 401 goes to the gate, which Task 9 makes refuse with `Owned(Session)`, so the outcome
  is `refresh-failed`.
- `a_refused_setup_token_in_a_session_is_a_401_whether_first_or_remembered`: the bearer is
  `sk-ant-oat01-vault`.
- `a_profile_whose_identity_drifted_is_not_used` and
  `a_profile_whose_marker_cannot_name_its_credential_is_keychain_unavailable`: `Recorded`, not
  the failure, in every case, because the vault's token was sent.
- `an_unreadable_session_record_makes_the_account_session_owned_too`: the bearer is `at-rt-a`.
- `the_gate_is_never_called_and_no_token_request_is_ever_made_for_a_session_owned_account`: the
  first collection reaches the gate, which finds the account lock held and returns `Busy`, so
  the outcome is `refresh-failed`.

`the_live_login_s_account_is_collected_as_the_active_one_even_when_a_session_holds_it` already
passes, and must keep passing: it pins the active role winning.

Run: `cargo test -p tagteam-engine --features test-hooks --test session_usage hooks`
Expected: FAIL. The stamped fingerprint is the profile token's, the inactive path sends the
vault's token, and the outcome is `Recorded`.

Run: `cargo test -p tagteam-engine --lib views::tests`
Expected: PASS. `profile-drifted` already falls through to `Unavailable`. The row pins it, so a
later arm cannot capture it.

- [ ] **Step 3: Implement**

In `crates/tagteam-engine/src/collect.rs`, replace the module doc (:1-5) with:

```rust
//! The usage collector (§8.3): reserve, fetch and record, one thread per account. An inactive
//! account's token comes from the vault, through the refresh gate (§7.3) when it needs one; the
//! active account's comes from the live store and is never refreshed by a fetch (§8.1); a
//! session-owned account's comes from its profile, read without a lock and never refreshed,
//! written or retried, because the agent in the session owns it (§8.1, §12.5). Nothing is sent
//! without the store's authorization right before the request: the lease still held, the token
//! not refused, and a slot in the identity's hourly budget (§8.3, §8.6).
```

Replace the imports (:7-25) with:

```rust
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::thread;

use tagteam_core::backoff::failure_backoff_s;
use tagteam_core::poll::plan_after_fetch;
use tagteam_core::usage::{earliest_relevant_reset, max_relevant_pct};
use tagteam_core::{AccountId, PollBudget, PollInputs, PollPlan, ProviderId, Window};
use tagteam_provider::profile::ProfileMarker;
use tagteam_provider::provider::UsageResult;
use tagteam_provider::{Credential, LockError, Provenance, Provider, Read, TransientKind};

use crate::active::{ActiveOutcome, ActiveTrigger};
use crate::engine::Engine;
use crate::error::EngineError;
use crate::hooks;
use crate::provenance::identity_drifted;
use crate::refresh::{GateOutcome, expired};
use crate::store::{
    AccountRow, Ineligible, Reservation, Reserve, SendGrant, Slot, Store, StoreError, UsageStateRow,
};
```

Below `type Outcome = (Collected, Vec<String>);`, add:

```rust
/// Which token one account's collection reads (§8.1), decided before the threads start.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Role {
    /// The live login names the account: the live token, which only §7.5 refreshes.
    Active,
    /// The vault's token, refreshed through the gate (§7.3) when it needs it.
    Inactive,
    /// A `tagteam run` session owns the account (§12.5): its profile's token, read as the agent
    /// in the session reads it, and never refreshed, written or retried.
    Session { profile: PathBuf },
}
```

Replace `Engine::collect_usage` (:78-145) with:

```rust
    /// §8.3 on demand: every listed account on its own thread, and the call waits for them
    /// all. Each account's role is decided before any thread starts (§8.1, `role_of`): the
    /// account the live login names is the active one, any other that a `tagteam run` session
    /// owns takes the session branch (§12.5), and the rest are inactive. A usage failure is
    /// never an error here: it is recorded, and reported in the report's outcomes and warnings.
    /// So is an error that ends one account's collection, deciding its role included (the store
    /// or its profile failing under it): every thread is joined and kept, and that account's
    /// outcome is `Failed { kind: "error" }` with one warning naming it, so one account never
    /// costs the others' outcomes. `Err` only for an error before any thread starts (opening the
    /// store, reading the listed accounts, reading each provider's recorded active account). IDs
    /// that name no account are skipped. Never creates the store.
    pub fn collect_usage(&self, mode: CollectMode) -> Result<CollectReport, EngineError> {
        let CollectMode::OnDemand { accounts } = mode;
        hooks::point(self, "usage-collect-start")?;
        let Some(shared) = self.existing_store()? else {
            return Ok(CollectReport::default());
        };
        let store: &Store = &shared;
        let mut rows = Vec::new();
        for id in &accounts {
            if let Some(row) = store.account(id)? {
                rows.push(row);
            }
        }
        // Each provider's recorded active account is read before its live login: a switch
        // writes the live login before it commits the record, so a record read first can only
        // be older than the live role, never newer (`Collection::active_now`).
        let mut recorded: HashMap<ProviderId, Option<AccountId>> = HashMap::new();
        for row in &rows {
            if !recorded.contains_key(&row.provider) {
                recorded.insert(row.provider.clone(), store.active(&row.provider)?);
            }
        }
        hooks::point(self, "usage-roles-between-reads")?;
        let live = self.live_accounts(&rows);
        let roles: Vec<Result<Role, EngineError>> = rows
            .iter()
            .map(|row| self.role_of(row, live.contains(&row.id)))
            .collect();
        let results: Vec<Result<Outcome, EngineError>> = thread::scope(|s| {
            let running: Vec<_> = rows
                .iter()
                .zip(roles)
                .map(|(row, role)| {
                    let started_with = recorded[&row.provider].clone();
                    s.spawn(move || {
                        role.and_then(|role| self.collect_one(store, row, role, started_with))
                    })
                })
                .collect();
            running
                .into_iter()
                .map(|t| match t.join() {
                    Ok(result) => result,
                    Err(panic) => std::panic::resume_unwind(panic),
                })
                .collect()
        });
        let mut report = CollectReport::default();
        for (row, result) in rows.iter().zip(results) {
            let (collected, warnings) = result.unwrap_or_else(|e| {
                let warning = format!(
                    "usage for {} (position {}) was not collected: {e}",
                    row.label, row.position
                );
                (
                    Collected::Failed {
                        kind: "error".to_owned(),
                    },
                    vec![warning],
                )
            });
            report.outcomes.push((row.id.clone(), collected));
            report.warnings.extend(warnings);
        }
        Ok(report)
    }
```

After `live_accounts` (:147-173), add:

```rust
    /// §8.1's role for `row`. The account the live login names is the active one, inside a run
    /// shell too, since the live login is always the default home's (§12.8). Any other account
    /// that a session owns, or may own (a reservation or record that cannot be read, §12.6),
    /// takes the session branch with its profile; the rest are inactive. An account that
    /// `collect_one` reports as unsupported needs no session state. The state is computed for
    /// this call and never cached (Decision 8).
    fn role_of(&self, row: &AccountRow, live: bool) -> Result<Role, EngineError> {
        if live {
            return Ok(Role::Active);
        }
        let Some(p) = self.registry.get(&row.provider) else {
            return Ok(Role::Inactive);
        };
        if !p.capabilities().usage || p.kind_traits(&row.kind).managed_key_axis {
            return Ok(Role::Inactive);
        }
        let state = self.session_state(p.as_ref(), row)?;
        Ok(match state.profile() {
            Some(profile) if state.owned() => Role::Session {
                profile: profile.to_path_buf(),
            },
            _ => Role::Inactive,
        })
    }
```

Replace `collect_one` (:175-232) with:

```rust
    /// One account through §8.3's three phases, in the role `collect_usage` decided.
    fn collect_one(
        &self,
        store: &Store,
        row: &AccountRow,
        role: Role,
        recorded_active: Option<AccountId>,
    ) -> Result<Outcome, EngineError> {
        let Some(provider) = self.registry.get(&row.provider) else {
            return Ok((Collected::Unsupported, Vec::new()));
        };
        let p = provider.as_ref();
        if !p.capabilities().usage || p.kind_traits(&row.kind).managed_key_axis {
            return Ok((Collected::Unsupported, Vec::new()));
        }
        let budget = p.poll_budget();
        // Phase 1: eligibility, the lease and the slot, in one transaction.
        let reservation = match store.reserve_usage(row, self.now_ms(), true, &budget)? {
            Reserve::Reserved(r) => r,
            Reserve::Ineligible(why) => return Ok((Collected::Ineligible(why), Vec::new())),
            Reserve::OverBudget { next_free_at } => {
                return Ok((Collected::OverBudget { next_free_at }, Vec::new()));
            }
        };
        let slot = Slot {
            slot: reservation.slot,
            slot_at: reservation.slot_at,
        };
        let state = match hooks::point(self, "usage-reserved")
            .and_then(|()| Ok(store.usage_state(&row.id)?))
        {
            Ok(state) => state,
            Err(e) => {
                // Nothing was sent: give the slot back, best effort, before the error.
                let _ = store.release_slot(&reservation, &slot);
                return Err(e);
            }
        };
        let mut run = Collection {
            engine: self,
            store,
            p,
            row,
            role,
            budget,
            slot: Some(slot),
            rejected: state.as_ref().and_then(|s| s.rejected_fp.clone()),
            gated: false,
            recorded_active,
            state,
            reservation,
            warnings: Vec::new(),
        };
        // Phase 2, holding no lock but those the gate or §7.5 take for their own refresh; the
        // session branch takes none at all (§8.1).
        let fetched = match run.role.clone() {
            Role::Active => run.active(),
            Role::Inactive => run.inactive(),
            Role::Session { profile } => run.session(&profile),
        };
        // Phase 3.
        run.record(fetched)
    }
```

Replace the `Collection` struct (:235-262) with:

```rust
/// One account's fetch, from its reservation to its record.
struct Collection<'a> {
    engine: &'a Engine,
    store: &'a Store,
    p: &'a dyn Provider,
    row: &'a AccountRow,
    /// Which token this fetch reads (§8.1), as `collect_usage` decided it.
    role: Role,
    budget: PollBudget,
    reservation: Reservation,
    /// The slot reserved for the next request, while that request is unsent. `send` hands it
    /// to `authorize_send` and puts it back only if the request never left; one still here at
    /// the record is given back (§8.3).
    slot: Option<Slot>,
    /// The state read after reserving: the previous reading, the failure count, the plan.
    state: Option<UsageStateRow>,
    /// The access-token fingerprint the server refused (`rejected_fp`, §8.1), as read after
    /// reserving and stamped since. The store's copy is the one `authorize_send` checks.
    rejected: Option<String>,
    /// Whether this collection's own gate refresh (§7.3, `Refreshed`, not `AlreadyFresh`)
    /// has produced the token in use: a 401 on it is not refreshed again (§8.3: at most a
    /// gate refresh, a fetch and one retry).
    gated: bool,
    /// The store's record of the provider's active account, read before the live login
    /// (`collect_usage`).
    recorded_active: Option<AccountId>,
    warnings: Vec<String>,
}
```

In `impl Collection<'_>`, after `fn active` (:368-383), add:

```rust
    /// §8.1 for a session-owned account (§12.5). The agent in the session owns the profile's
    /// token, so this reads it without a lock (`profile_credential`), and never refreshes,
    /// writes or retries it. An expired token is `token-expired` with no request, and a refused
    /// one stays refused until its bytes change. Neither leaves the machine, and the slot goes
    /// back (§8.3). A 401 stamps `rejected_fp` and ends the fetch.
    fn session(&mut self, profile: &Path) -> Result<Vec<Window>, Stop> {
        let credential = self.profile_credential(profile)?;
        if self.is_rejected(credential.bytes()) {
            return Err(failed(self.refusal_kind()));
        }
        let first = self.send_credential(&credential)?;
        if !matches!(first, UsageResult::Unauthorized) {
            return windows(first);
        }
        self.reject(credential.bytes())?;
        Err(failed(self.refusal_kind()))
    }

    /// The profile's credential as the agent in the session reads it (§8.1, §12.2), under the
    /// spelling the marker records, never one derived again, and with no lock.
    /// - A marker that is missing, unreadable, or another account's names no spelling, so the
    ///   credential cannot be read: `keychain-unavailable` (Decision 9).
    /// - The identity comes first: the credential of a profile whose login is not the
    ///   account's is never read (§12.5 "Identity drift"). An identity that is absent or
    ///   cannot be read cannot be confirmed, so it counts as drifted: `profile-drifted`.
    /// - A degraded read is used, since a usage request consumes nothing. An unreadable one is
    ///   `keychain-unavailable`, and an absent or empty one `no-access-token`.
    fn profile_credential(&self, profile: &Path) -> Result<Credential, Stop> {
        let env = &self.engine.env;
        let spelling = match ProfileMarker::read(profile) {
            Read::Present(m) if m.provider == self.row.provider && m.account_id == self.row.id => {
                m.config_dir
            }
            Read::Present(_) | Read::Absent | Read::Unreadable(_) => {
                return Err(failed("keychain-unavailable"));
            }
        };
        match self.p.profile_identity(env, &spelling) {
            Read::Present(identity) if !identity_drifted(&identity, self.row) => {}
            Read::Present(_) | Read::Absent | Read::Unreadable(_) => {
                return Err(failed("profile-drifted"));
            }
        }
        match self.p.read_profile_credential(env, &spelling) {
            Read::Present(c) if !c.is_empty() => Ok(c),
            Read::Present(_) | Read::Absent => Err(failed("no-access-token")),
            Read::Unreadable(_) => Err(failed("keychain-unavailable")),
        }
    }

    /// The failure a refused token records wherever this collection may not refresh it (§8.1):
    /// `token-expired` when its kind refreshes and someone else refreshes it (§7.5 for the
    /// active account, the agent in the session for a session-owned one), otherwise `http-401`
    /// (Decision 11: a setup token never refreshes).
    fn refusal_kind(&self) -> &'static str {
        let refreshed_elsewhere = matches!(self.role, Role::Active | Role::Session { .. });
        if refreshed_elsewhere && self.p.kind_traits(&self.row.kind).refreshable {
            "token-expired"
        } else {
            "http-401"
        }
    }
```

Replace `send` (:591-614) with:

```rust
    /// One usage request with `bytes`, which a fresh read gave: the vault, the gate or the live
    /// store (`live_bytes` refuses a degraded one). See `send_credential`.
    fn send(&mut self, bytes: &[u8]) -> Result<UsageResult, Stop> {
        self.send_credential(&Credential::fresh(bytes.to_vec()))
    }

    /// One usage request with `credential`'s access token, its provenance kept: only a
    /// session's profile read may be degraded, and a usage request consumes nothing (§8.1). A
    /// token that has expired or was refused is never sent: the slot stays unsent. Right before
    /// the request, the store authorizes it and hands over the slot to send under
    /// (`authorize`). A request that never left (no access token, or a pre-send failure) puts
    /// its slot back.
    fn send_credential(&mut self, credential: &Credential) -> Result<UsageResult, Stop> {
        let bytes = credential.bytes();
        if !self.usable(bytes) {
            return Err(failed("token-expired"));
        }
        hooks::point(self.engine, "usage-before-send")?;
        let slot = self.authorize(bytes)?;
        let result = self.p.fetch_usage(self.engine.http(), credential);
        let unsent = matches!(
            result,
            UsageResult::NoAccessToken
                | UsageResult::Failed {
                    kind: TransientKind::PreSend,
                    ..
                }
        );
        self.slot = unsent.then_some(slot);
        Ok(result)
    }
```

Replace `authorize` (:616-671) with:

```rust
    /// §8.3, §8.6: the store's one fenced authorization, immediately before the request, with
    /// the fingerprint of the exact bytes about to be sent and the slot held (`None` for the
    /// retry). It re-checks the lease and the account's identity, the durable `rejected_fp`
    /// (another process may have been refused this token since this one read its state), and
    /// the slot's validity, replacing a stale slot (a suspend or a slow refresh) or reserving
    /// one for the retry.
    /// - `LeaseLost` (the lease was lost, the identity changed, or the account is
    ///   quarantined): nothing is sent or recorded, and the never-sent held slot goes back,
    ///   best effort, at the record.
    /// - `Rejected`: a stamp written by another process after this collection read its state
    ///   (the fence for `rejected_fp`): nothing is sent, and the slot goes back. Recorded as
    ///   `refusal_kind` says: `token-expired` for a refreshable active or session-owned account
    ///   (§7.5, or the agent in the session, refreshes it), otherwise `http-401`.
    /// - `OverBudget`: recorded as `over-budget`, backing off until a slot frees up; a stale
    ///   slot has already gone back.
    fn authorize(&mut self, bytes: &[u8]) -> Result<Slot, Stop> {
        let fp = self.access_fp(bytes);
        let held = self.slot.take();
        let grant = match self.store.authorize_send(
            &self.reservation,
            held.as_ref(),
            fp.as_deref(),
            self.now_ms(),
            &self.budget,
        ) {
            Ok(grant) => grant,
            Err(e) => {
                // The store's transaction rolled back, so the slot is still counted: keep it
                // for the record to give back.
                self.slot = held;
                return Err(e.into());
            }
        };
        match grant {
            SendGrant::Send(slot) => Ok(slot),
            SendGrant::LeaseLost => {
                self.slot = held;
                Err(Stop::LeaseLost)
            }
            SendGrant::Rejected => {
                self.slot = held;
                self.rejected = fp;
                Err(failed(self.refusal_kind()))
            }
            SendGrant::OverBudget { next_free_at } => Err(Stop::Failed(Failure {
                not_before: Some(next_free_at),
                ..Failure::new("over-budget")
            })),
        }
    }
```

Replace `active_now`'s body (:740-746), keeping its doc comment, with:

```rust
    fn active_now(&self) -> Result<bool, StoreError> {
        let recorded = self.store.active(&self.row.provider)?;
        Ok(match recorded {
            Some(id) if recorded != self.recorded_active => id == self.row.id,
            _ => self.role == Role::Active,
        })
    }
```

No other caller changes: `inactive`, `active`, `retry` and `refresh_live` still call
`send(bytes)`, and `send_credential` has only the two callers above. `identity_drifted` is
Task 10's: this task adds no definition and no unit test of it.

No existing test changes its expectation. Every existing collection test has no profile, so
each inactive account's session state is `NoProfile` and it takes the inactive path, as before.
The active role's `Rejected` arm gives the same kinds through `refusal_kind`.

- [ ] **Step 4: Run them and see them pass**, then the crate's whole suite

Run: `cargo test -p tagteam-engine --lib`
Expected: PASS, including the extended `usage_status_follows_the_table_row_by_row` (it now holds
Task 4's `live-replaced` row and this task's `profile-drifted` row) and Task 10's
`drift_compares_the_email_and_the_organization_only_when_both_name_one`.

Run: `cargo test -p tagteam-engine --test session_usage`
Expected: PASS, 12 tests.

Run: `cargo test -p tagteam-engine --features test-hooks`
Expected: PASS, the hooks test included, and `tests/collect.rs` and `tests/collect_active.rs`
unchanged.

Run: `cargo test --workspace --features tagteam/test-support`
Expected: PASS. The CLI's `list` perf test still holds: an account without a profile costs one
failed `stat` (Decision 8).

- [ ] **Step 5: Format and lint**

```
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no output from `fmt --check`, and both clippy runs finish with no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-engine/src/collect.rs crates/tagteam-engine/src/views.rs \
  crates/tagteam-engine/tests/common/mod.rs crates/tagteam-engine/tests/session_usage.rs
git commit -m "Collect a session-owned account's usage from its profile, read only and never refreshed or retried"
```

---

### Task 13: Link sync by allowlist, `run.share_extra`, create-only surface

§12.2: a profile shares the outer home's entries by allowlist, and every launch syncs the links.
This task builds the sync that M4b's launch (§12.5 step 3) calls. It has three parts, each with
its own red, green and commit cycle:
- **13a** reads `run.share_extra`.
- **13b** declares the create-only surface and holds the §15.3 comparison to it.
- **13c** is `Engine::sync_profile_links` itself, with its refusal, `ProfileSplit`.

Nothing in M4a calls the sync outside tests.

**Readings of the spec this task commits to:**
- **The source home comes from the marker.** The sync reads `<profile>/.tagteam-profile.json`
  and takes the source from `share_policy(apply_outer_home(env, marker.outer))`.
  - A quiescent launch updates `outer` before it syncs (§12.2 "Profile marker"), so the source
    is the launch's own home.
  - A joining launch leaves the marker alone, so a join links into the home the running session
    already shares.
  - "Outside the outer home that the marker records" then needs no test of its own. A recorded
    link whose target is not where the marker's home now resolves that name is relinked, or
    removed when the home lacks the entry.
  - A missing marker refuses with `invalid-input`, and an unreadable marker or links record
    with `unreadable`. tagteam never guesses which links are its own.
- **Ours means recorded and still as made.** A link is tagteam's only when `.tagteam-links.json`
  records its name and `read_link` still gives the recorded target.
  - Anything else at that name stays exactly as it is: a real file or directory, a link someone
    else made, or tagteam's link repointed since.
  - Such a name is dropped from the record, so a later sync can never remove it.
  - A foreign link that already resolves to the source is the share itself. It is left alone,
    with no warning, and not adopted into the record.
  - "Real history directories are never deleted" follows: removal only ever unlinks a symlink.
- **Refusals come before any write.** The must-share checks and the source listing run first,
  and read only. Two cases refuse:
  - A must-share entry the profile holds as a real copy, or as a link that resolves elsewhere:
    `ProfileSplit`, naming both paths, for a launch and for a join alike. Splitting memory or
    history silently is never an option.
  - A must-share source that is a link to nothing: `invalid-input`.

  A refused sync creates nothing in the source, links nothing and writes no record.
- **Creation is only for missing must-share entries, and it is empty.**
  - A directory is created 0700 and a file 0600. Each is created with `create`/`create_new`,
    only when absent (§3's create-only row). A race with the agent creating it is harmless.
  - A source home that does not exist yet is created 0700 first, with `ensure_private_dir`,
    which never changes an existing directory's mode. The §15.3 comparison tolerates that
    directory as an ancestor of a create-only entry, on its first appearance only, as it already
    does for tagteam's data dir.
  - A join creates them too: a dangling must-share link would otherwise make the agent start a
    private copy.
- **Every target is fully resolved.** Each target is `fs::canonicalize` of the source entry,
  because CC follows only one hop when it writes through a link (Appendix A.1). A source entry
  that does not resolve (absent, or a link to nothing) is not linked, and a recorded link to it
  is removed (unless joining).
- **The allowlist** is the must-share entries, then `shared`, then `run.share_extra`, in that
  order, with duplicates collapsed.
  - A name that matches a private pattern (`entry_matches`, one `*`) or starts with
    `.tagteam-` is never linked, whatever list names it.
  - A private name in `run.share_extra` warns on every sync (§12.2: "ignored with a warning").
- **Unknown entries come from the source home.** Review Focus 5 is about "a `~/.claude` entry
  tagteam does not know", so the scan is of the source home's entries.
  - An entry that is on no list and not private gets one warning, the first time, and is
    recorded in `noted_unknown`, which only grows.
  - The profile's own entries are the agent's private state, and are not scanned.
- **Settings validation.** `run.share_extra` takes a name or a list of names, read from the
  provider's table first. An item that is not a string, or is empty, `.` or `..`, or holds a
  `/`, is dropped with a warning naming it. A name with a dot inside (`hooks.json`, `.my-tool`)
  is valid. The Interface Contract's wording says the same: names that are empty, `.` or
  `..`, or contain `/`, are dropped.
- **The record** is written atomically (Task 7's `LinksRecord::write`), and only when it changed.
  It is written after a failed step too, so it always names exactly the links tagteam made.
- **Locks.** The caller holds `MutationGuard` and the account lock (§12.5 launch step 3). The
  sync takes no lock.

**Files:**
- **13a:**
  - Modify: `crates/tagteam-engine/src/settings.rs`:
    - `Settings` (:46-58) and its `Default` (:60-70);
    - new: `is_share_name` and `Reader::share_extra`, after `Reader::read` (:142-170);
    - `from_document` (:173-242).
  - Test: `crates/tagteam-engine/tests/settings.rs`:
    - `the_defaults_are_the_specs_table` (:28-39);
    - `every_key_is_read_from_a_full_file` (:61-90);
    - new tests.
- **13b:**
  - Modify: `crates/tagteam-provider/src/provider.rs` (`IdentitySurface`, :118-134)
  - Modify: `crates/tagteam-cc/src/provider.rs` (imports :16-25; `identity_surface`, :214-232)
  - Modify: `crates/tagteam-fake/src/provider.rs` (`identity_surface`, :182-191)
  - Modify: `crates/tagteam-engine/tests/common/mod.rs` (`assert_only_surface_changed_for`, :917-1011)
  - Test: `crates/tagteam-cc/tests/provider.rs` (`identity_surface_names_the_section_3_writes`, :612-634)
  - Test: `crates/tagteam-fake/tests/provider.rs` (`kinds_capabilities_endpoints_and_surface`, :107-147)
  - Test: `crates/tagteam-engine/tests/invariant.rs` (new test)
- **13c:**
  - Create: `crates/tagteam-engine/src/profiles.rs`
  - Modify: `crates/tagteam-engine/src/lib.rs` (`pub mod profiles;`)
  - Modify: `crates/tagteam-engine/src/error.rs`:
    - `ProfileSplit`, after `ForeignLiveCredential` (:99-104);
    - its `kind()` arm (:111-144);
    - the pin row (:153-277).
  - Modify: `crates/tagteam-engine/tests/common/mod.rs` (new `Fx::make_profile_for`, after Task 8's `impl Fx` of profile helpers: a provider-generic profile, which Task 8's Claude Code-only `make_profile` cannot give FakeAgent; Tasks 14–16 use it too)
  - Create: `crates/tagteam-engine/tests/profiles.rs`
  - Test: `crates/tagteam-engine/tests/invariant.rs` (new test)
  - Test: `crates/tagteam-engine/tests/fake_agent.rs` (new test)

**Interfaces:**
- Consumes:
  - **Task 7:**
    - `SharePolicy { source, shared, must_share, private }`, `MustShare { name, kind }`,
      `EntryKind::{Dir, File}`;
    - `Provider::{share_policy, apply_outer_home, outer_home}`;
    - `tagteam_provider::profile::{ProfileMarker, LinksRecord, entry_matches, MARKER_FILE, LINKS_FILE}`;
    - `tagteam_cc`'s crate-private `session::CC_MUST_SHARE`;
    - FakeAgent's share policy (shared `notes`, `prefs.json`; must-share `journal.log`).
  - **Task 8:** `Fx::{profile_dir, make_profile}`; Task 7's `canonical_profile_path` and
    `Provider::profile_spelling`, which `Fx::make_profile_for` uses.
  - **Existing:** `tagteam_provider::atomic::ensure_private_dir`, `Settings`, `Reader`,
    `IdentitySurface`, `Fx::{snapshot, assert_only_surface_changed_for, engine_with_settings}`,
    and `FakeFx`.
- Produces (as in the Interface Contract):
  - `pub struct SyncReport { pub created: Vec<String>, pub removed: Vec<String>, pub warnings: Vec<String> }`;
  - `Engine::sync_profile_links(&self, p: &dyn Provider, profile: &Path, joining: bool) -> Result<SyncReport, EngineError>`;
  - `Settings.share_extra: Vec<String>`, and the crate-private `settings::is_share_name`;
  - `IdentitySurface.create_only: Vec<PathBuf>`;
  - `EngineError::ProfileSplit { profile: PathBuf, shared: PathBuf }`, kind `profile-split`;
  - the test helper `Fx::make_profile_for(&self, p: &dyn Provider, id: &AccountId) -> PathBuf`
    (Tasks 14–16 use it).

**Spec:**
- §12.2 "Shared by allowlist": each allowlisted entry present in the source home is linked to its
  fully resolved path; everything else stays private.
- §12.2 "Unknown entries are private": a notice the first time, recorded in
  `.tagteam-links.json`.
- §12.2 "Must-share entries": created empty in the source home when absent, then linked. A real
  copy in the profile refuses, naming both paths.
- §12.2 "Shared files can split": a regular file where a shared link belongs warns, naming both
  paths. Neither copy is ever merged or replaced.
- §12.2 "Sync runs on every launch":
  - create missing links;
  - remove only recorded links whose source disappeared, left the allowlist, or left the outer
    home, and relink a moved source;
  - a join only creates;
  - never replace a real entry, never delete history, never link `.tagteam-*`.
- §3, create-only row: `~/.claude/projects/` and `~/.claude/history.jsonl` are created empty,
  only when absent.
- §6.4 `run.share_extra`: entry names added to the allowlist, the provider's table first. A
  known-private name is ignored with a warning.
- §15.3: every file outside the identity surface stays byte-identical.
- Appendix A.1: CC writes through one hop only, hence fully resolved targets.
- Decision 14: link sync is engine-generic, and the provider supplies the policy.
- Review Focus 5.

#### 13a: `run.share_extra`

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-engine/tests/settings.rs`, add to `the_defaults_are_the_specs_table`, after
the `color` assertion:

```rust
    assert!(d.share_extra.is_empty());
```

Replace `every_key_is_read_from_a_full_file` (:61-90). Its full `Settings` literal stops
compiling once the struct gains a field. It now also reads `run.share_extra`:

```rust
#[test]
fn every_key_is_read_from_a_full_file() {
    let (settings, warnings) = load(
        r#"
[autoswitch]
threshold = 75.5
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
            threshold: 75.5,
            models: models(&["Fable", "Opus"]),
            history_retention_days: 30,
            statusline_format: "{5h}%".to_owned(),
            color: ColorMode::Never,
            share_extra: vec!["hook-data".into()],
        }
    );
}
```

Append:

```rust
#[test]
fn run_share_extra_is_read_from_the_provider_s_table_first() {
    let text = "[run]\nshare_extra = [\"global\"]\n\
                [provider.claude-code.run]\nshare_extra = [\"hook-data\", \"tool-cache\"]\n";
    let (settings, warnings) = load(text);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(settings.share_extra, ["hook-data", "tool-cache"]);
    let (settings, _) = load_as(text, "fake-agent");
    assert_eq!(
        settings.share_extra,
        ["global"],
        "another provider's table does not apply"
    );
}

#[test]
fn run_share_extra_takes_a_name_or_a_list_and_collapses_repeats() {
    let (settings, warnings) = load("[run]\nshare_extra = \"hook-data\"\n");
    assert_eq!(settings.share_extra, ["hook-data"]);
    assert!(warnings.is_empty(), "{warnings:?}");

    let (settings, warnings) =
        load("[run]\nshare_extra = [\"hooks.json\", \".my-tool\", \"hooks.json\"]\n");
    assert_eq!(
        settings.share_extra,
        ["hooks.json", ".my-tool"],
        "a dot inside a name is fine"
    );
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn a_share_extra_item_that_is_not_an_entry_name_is_dropped_with_a_warning_naming_it() {
    let (settings, warnings) =
        load("[run]\nshare_extra = [\"hook-data\", \"\", \".\", \"..\", \"a/b\", 3]\n");
    assert_eq!(settings.share_extra, ["hook-data"]);
    assert_eq!(warnings.len(), 5, "{warnings:?}");
    assert!(
        warnings
            .iter()
            .all(|w| w.contains("`run.share_extra`") && w.contains("config.toml")),
        "{warnings:?}"
    );
    for name in ["\"\"", "\".\"", "\"..\"", "\"a/b\""] {
        assert!(
            warnings.iter().any(|w| w.contains(name)),
            "{name}: {warnings:?}"
        );
    }
}

#[test]
fn a_share_extra_that_is_neither_a_name_nor_a_list_warns_and_the_next_table_applies() {
    let (settings, warnings) = load(
        "[run]\nshare_extra = [\"global\"]\n[provider.claude-code.run]\nshare_extra = 3\n",
    );
    assert_eq!(settings.share_extra, ["global"]);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].contains("`provider.claude-code.run.share_extra`"),
        "{warnings:?}"
    );
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-engine --test settings`
Expected: compile error, `no field share_extra on type Settings` (and `struct Settings has no
field named share_extra` in the literal).

- [ ] **Step 3: Implement**

In `crates/tagteam-engine/src/settings.rs`, replace `Settings` and its `Default` (:46-70) with:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// `autoswitch.threshold`: 50–99.9. The provider's own table first.
    pub threshold: f64,
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
            threshold: DEFAULT_THRESHOLD,
            models: Vec::new(),
            history_retention_days: DEFAULT_HISTORY_RETENTION_DAYS,
            statusline_format: DEFAULT_STATUSLINE_FORMAT.to_owned(),
            color: ColorMode::Auto,
            share_extra: Vec::new(),
        }
    }
}

/// One entry of the source home, as `run.share_extra` names it: not empty, not `.` or `..`,
/// and without a `/`. A dot inside a name is fine.
pub(crate) fn is_share_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains('/')
}
```

In `impl<'a> Reader<'a>`, after `fn read` (:142-170), add:

```rust
    /// `run.share_extra` (§6.4): the first of `tables`, most specific first, that holds the
    /// key. Its value is a name or a list of names; any other value warns, and the next table
    /// is tried. Within a list, an item that is not a string, or not an entry name
    /// (`is_share_name`), warns, naming it, and is dropped; the rest stand, and a repeated name
    /// collapses into its first.
    fn share_extra(&mut self, tables: &[&[&str]]) -> Vec<String> {
        for path in tables {
            let Some(table) = self.table(path) else {
                continue;
            };
            let Some(item) = table.get("share_extra") else {
                continue;
            };
            let dotted = path
                .iter()
                .copied()
                .chain(["share_extra"])
                .collect::<Vec<_>>()
                .join(".");
            let items: Vec<Option<&str>> = match (item.as_str(), item.as_array()) {
                (Some(one), _) => vec![Some(one)],
                (None, Some(list)) => list.iter().map(|v| v.as_str()).collect(),
                (None, None) => {
                    self.warn(format!(
                        "{}: `{dotted}` must be an entry name or a list of entry names (ignored)",
                        self.path
                    ));
                    continue;
                }
            };
            let mut names: Vec<String> = Vec::new();
            for item in items {
                match item {
                    Some(name) if is_share_name(name) => {
                        if !names.iter().any(|n| n == name) {
                            names.push(name.to_owned());
                        }
                    }
                    Some(name) => self.warn(format!(
                        "{}: `{dotted}` entry {name:?} is not an entry name of the source home (ignored)",
                        self.path
                    )),
                    None => self.warn(format!(
                        "{}: `{dotted}` holds an item that is not a string (ignored)",
                        self.path
                    )),
                }
            }
            return names;
        }
        Vec::new()
    }
```

Replace `from_document` (:173-242) with:

```rust
fn from_document(doc: &DocumentMut, path: &str, provider: &ProviderId) -> (Settings, Vec<String>) {
    let defaults = Settings::default();
    let mut reader = Reader {
        doc,
        path,
        warnings: Vec::new(),
    };
    let global_autoswitch: &[&str] = &["autoswitch"];
    let global_statusline: &[&str] = &["statusline"];
    let global_run: &[&str] = &["run"];
    let provider_autoswitch = ["provider", provider.as_str(), "autoswitch"];
    let provider_statusline = ["provider", provider.as_str(), "statusline"];
    let provider_run = ["provider", provider.as_str(), "run"];

    let threshold = reader
        .read(
            &[&provider_autoswitch[..], global_autoswitch],
            "threshold",
            "must be a number from 50 to 99.9",
            parse_threshold,
        )
        .unwrap_or(defaults.threshold);
    let models = reader
        .read(
            &[&provider_autoswitch[..], global_autoswitch],
            "models",
            "must be a model name, a list of model names, or [\"all\"] alone",
            parse_models,
        )
        .unwrap_or(defaults.models);
    let history_retention_days = reader
        .read(
            &[&["usage"]],
            "history_retention_days",
            "must be a whole number of days from 1 to 3650",
            parse_retention,
        )
        .unwrap_or(defaults.history_retention_days);
    let format_expect = format!(
        "must be a non-empty string using only the placeholders {} and {{{STATUSLINE_MODEL_PREFIX}<name>}}, each closed",
        STATUSLINE_PLACEHOLDERS
            .iter()
            .map(|name| format!("{{{name}}}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let statusline_format = reader
        .read(
            &[&provider_statusline[..], global_statusline],
            "format",
            &format_expect,
            parse_format,
        )
        .unwrap_or(defaults.statusline_format);
    let color = reader
        .read(
            &[&["ui"]],
            "color",
            "must be \"auto\", \"always\" or \"never\"",
            parse_color,
        )
        .unwrap_or(defaults.color);
    let share_extra = reader.share_extra(&[&provider_run[..], global_run]);

    let settings = Settings {
        threshold,
        models,
        history_retention_days,
        statusline_format,
        color,
        share_extra,
    };
    (settings, reader.warnings)
}
```

Existing tests this changes:
- `tests/settings.rs::every_key_is_read_from_a_full_file`: rewritten in Step 1, with
  `share_extra: vec!["hook-data".into()]`.
- `the_defaults_are_the_specs_table`: one assertion added.

Every other `Settings` literal in the workspace uses `..Settings::default()`:
- `engine.rs`: the test `the_engine_keeps_the_settings_it_was_built_with`;
- `tests/views_usage.rs`: lines 548 and 610.

- [ ] **Step 4: Run them and see them pass**, then the crate's whole suite

Run: `cargo test -p tagteam-engine --test settings`  Expected: PASS
Run: `cargo test -p tagteam-engine`  Expected: PASS

- [ ] **Step 5: Format and lint**

```
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-engine/src/settings.rs crates/tagteam-engine/tests/settings.rs
git commit -m "Read run.share_extra, dropping a name that is not an entry of the source home"
```

#### 13b: The create-only surface and the §15.3 comparison

- [ ] **Step 7: Write the failing tests**

In `crates/tagteam-cc/tests/provider.rs`, in `identity_surface_names_the_section_3_writes`,
before the `assert_eq!(s.machine_shared_keys.len(), 5);` line, add:

```rust
    assert_eq!(
        s.create_only,
        vec![
            paths.config_home.join("projects"),
            paths.config_home.join("history.jsonl")
        ],
        "§3's create-only row: the must-share entries, in the config home"
    );
```

In `crates/tagteam-fake/tests/provider.rs`, in `kinds_capabilities_endpoints_and_surface`, after
`assert_eq!(s.machine_shared_keys, vec!["device"]);`, add:

```rust
    assert_eq!(s.create_only, vec![p.dir.join("journal.log")]);
```

In `crates/tagteam-engine/tests/invariant.rs`, append:

```rust
#[test]
fn the_comparison_allows_only_an_empty_create_only_entry_where_there_was_none() {
    let fx = Fx::new();
    let claude = fx.env.home.join(".claude");
    std::fs::remove_file(claude.join("history.jsonl")).unwrap();
    std::fs::remove_dir_all(claude.join("projects")).unwrap();

    // §3's create-only row: created empty where there was none.
    let before = fx.snapshot();
    std::fs::write(claude.join("history.jsonl"), b"").unwrap();
    std::fs::create_dir(claude.join("projects")).unwrap();
    let after = fx.snapshot();
    fx.assert_only_surface_changed(&before, &after, "created empty"); // must not panic

    // Once it exists, it is held to the byte-for-byte rule like anything else.
    let before = fx.snapshot();
    std::fs::write(claude.join("history.jsonl"), "{\"display\":\"x\"}\n").unwrap();
    let after = fx.snapshot();
    expect_violation(&fx, &before, &after, "existing", "history.jsonl");

    // Created with content: not what the row allows.
    std::fs::remove_file(claude.join("history.jsonl")).unwrap();
    let before = fx.snapshot();
    std::fs::write(claude.join("history.jsonl"), b"x").unwrap();
    let after = fx.snapshot();
    expect_violation(&fx, &before, &after, "content", "create-only");

    // Anything inside a created directory is outside the row.
    let before = fx.snapshot();
    std::fs::write(claude.join("projects/new.jsonl"), b"").unwrap();
    let after = fx.snapshot();
    expect_violation(&fx, &before, &after, "inside", "projects/new.jsonl");
}
```

- [ ] **Step 8: Run them and see them fail**

Run: `cargo test -p tagteam-cc --test provider identity_surface_names_the_section_3_writes`
Expected: compile error, `no field create_only on type IdentitySurface`.

Run: `cargo test -p tagteam-fake --test provider kinds_capabilities_endpoints_and_surface`
Expected: the same compile error.

Run: `cargo test -p tagteam-engine --test invariant the_comparison_allows_only`
Expected: FAIL, panicking at the first step with `created empty: …/.claude/history.jsonl changed`,
because the comparison does not know the create-only row yet.

- [ ] **Step 9: Implement**

In `crates/tagteam-provider/src/provider.rs`, replace `IdentitySurface` (:118-134) with:

```rust
/// The exact provider-owned state a switch may write (§3). Drives the pinned test (§15.3).
#[derive(Debug, Clone, Default)]
pub struct IdentitySurface {
    /// Files where only these top-level keys may change; every other byte stays identical.
    /// This type only names which top-level keys move — nested shape and append-only rules
    /// (§3: e.g. `customApiKeyResponses.approved` may only grow by appending) are enforced by
    /// the §15.3 invariant test, not by this type. Per-command scoping arrives with `run` in
    /// M4.
    pub json_keys: Vec<(PathBuf, Vec<String>)>,
    /// Credential files whose account-scoped keys may change; machine-shared keys may not.
    pub credential_files: Vec<PathBuf>,
    /// Keychain credential entries (service, account), compared like `credential_files`.
    pub credential_items: Vec<(String, String)>,
    /// Keychain items the provider may write wholesale (CC: the managed-key item).
    pub owned_items: Vec<(String, String)>,
    pub machine_shared_keys: Vec<&'static str>,
    /// Entries tagteam may create, empty, only when absent (§3's create-only row): the
    /// must-share entries a link sync creates in the source home (§12.2).
    pub create_only: Vec<PathBuf>,
}
```

In `crates/tagteam-cc/src/provider.rs`, `CC_MUST_SHARE` is already in scope through Task 7's
`use crate::session::{self, CC_MUST_SHARE, CC_PRIVATE, CC_SHARED};`, so no import changes.
Replace `identity_surface` (:214-232) with the version below. If Task 5 changed the
`keychain_items` calls, keep Task 5's lines; only `create_only` is new.

```rust
    fn identity_surface(&self, env: &Env) -> IdentitySurface {
        let paths = CcPaths::resolve(env);
        let acct = keychain_account(env);
        let mac = self.live.platform() == Platform::MacOs;
        IdentitySurface {
            json_keys: vec![(
                paths.global_config,
                vec![
                    "oauthAccount".into(),
                    "primaryApiKey".into(),
                    "customApiKeyResponses".into(),
                ],
            )],
            credential_files: vec![paths.credentials_file],
            credential_items: keychain_items(env, ItemKind::OAuth, &acct, mac),
            owned_items: keychain_items(env, ItemKind::ManagedKey, &acct, mac),
            machine_shared_keys: MACHINE_SHARED_KEYS.to_vec(),
            // §3's create-only row: the must-share entries, in the config home a link sync
            // shares from (§12.2).
            create_only: CC_MUST_SHARE
                .iter()
                .map(|(name, _)| paths.config_home.join(name))
                .collect(),
        }
    }
```

In `crates/tagteam-fake/src/provider.rs`, replace `identity_surface` (:182-191) with:

```rust
    fn identity_surface(&self, env: &Env) -> IdentitySurface {
        let p = FakePaths::resolve(env);
        IdentitySurface {
            json_keys: vec![(p.identity, vec!["identity".into()])],
            credential_files: vec![p.credential],
            credential_items: vec![],
            owned_items: vec![],
            machine_shared_keys: vec![DEVICE],
            // Its one must-share entry (`share_policy`), which a link sync creates empty.
            create_only: vec![p.dir.join("journal.log")],
        }
    }
```

In `crates/tagteam-engine/tests/common/mod.rs`, replace `assert_only_surface_changed_for`
(:914-1011, doc included) with:

```rust
    /// `assert_only_surface_changed` for any provider's declared surface. Since the walk covers
    /// all of HOME and every Keychain item, one provider's surface also proves that its
    /// commands left every other provider's state untouched (§15.3). A create-only entry (§3)
    /// may appear where there was none, empty, and a source home that did not exist may appear
    /// with it; once either exists, it is held to the byte-for-byte rule like anything else.
    pub fn assert_only_surface_changed_for(
        &self,
        surface: &IdentitySurface,
        before: &HomeSnapshot,
        after: &HomeSnapshot,
        step: &str,
    ) {
        let data_dir = self.env.data_dir();
        let json_keys: BTreeMap<PathBuf, Vec<String>> = surface
            .json_keys
            .iter()
            .map(|(p, keys)| (resolve(before, p), keys.clone()))
            .collect();
        let cred_files: BTreeSet<PathBuf> = surface
            .credential_files
            .iter()
            .map(|p| resolve(before, p))
            .collect();
        // Each resolved through a linked parent directory, as the walk records it.
        let create_only: BTreeSet<PathBuf> = surface
            .create_only
            .iter()
            .map(|p| match (p.parent(), p.file_name()) {
                (Some(dir), Some(name)) => resolve(before, dir).join(name),
                _ => p.clone(),
            })
            .collect();
        let paths: BTreeSet<&PathBuf> = before.files.keys().chain(after.files.keys()).collect();
        for path in paths {
            let (b, a) = (before.files.get(path), after.files.get(path));
            if b.is_none() && data_dir.starts_with(path) {
                // A bare ancestor of tagteam's own data dir, created lazily just now: tolerated
                // only on its first appearance. Once it exists in `before` too, it falls through
                // to the rules below like any other path, so a later change to it is still caught.
                continue;
            }
            if b.is_none() && create_only.contains(path) {
                // §3's create-only row: created where there was none, and only ever empty, a
                // directory or a file, never a link.
                let empty = match a.map(|e| &e.kind) {
                    Some(EntryKind::Dir) => true,
                    Some(EntryKind::File(bytes)) => bytes.is_empty(),
                    _ => false,
                };
                assert!(
                    empty,
                    "{step}: the create-only {} was created with content, or as a link",
                    path.display()
                );
                continue;
            }
            if b.is_none()
                && matches!(a.map(|e| &e.kind), Some(EntryKind::Dir))
                && create_only.iter().any(|c| c.starts_with(path))
            {
                // A source home that did not exist, created for its create-only entries.
                continue;
            }
            if let Some(keys) = json_keys.get(path) {
                let (bb, ab) = (
                    surface_file_bytes(b, step, path),
                    surface_file_bytes(a, step, path),
                );
                if let (Some(bm), Some(am)) = (b.map(|e| e.mode), a.map(|e| e.mode)) {
                    assert_eq!(bm, am, "{step}: {} changed mode", path.display());
                }
                if keys.iter().any(|k| k == "customApiKeyResponses") {
                    check_api_key_responses(bb, ab, step, path);
                }
                let strip = |doc: Option<&Vec<u8>>| {
                    doc.map(|d| {
                        keys.iter()
                            .fold(d.clone(), |acc, k| remove_top_level(&acc, k).unwrap())
                    })
                };
                assert_eq!(
                    strip(bb),
                    strip(ab),
                    "{step}: {} changed outside {keys:?}",
                    path.display()
                );
            } else if cred_files.contains(path) {
                let (bb, ab) = (
                    surface_file_bytes(b, step, path),
                    surface_file_bytes(a, step, path),
                );
                if let (Some(bm), Some(am)) = (b.map(|e| e.mode), a.map(|e| e.mode)) {
                    // A credential file is forced to 0600 on every write (never preserved,
                    // never chmod'ed to anything else), so a mode change is only ever
                    // allowed when it lands exactly there.
                    assert!(
                        bm == am || am == 0o600,
                        "{step}: {} changed mode from {bm:o} to {am:o}",
                        path.display()
                    );
                }
                assert_eq!(
                    shared_keys(bb, &surface.machine_shared_keys),
                    shared_keys(ab, &surface.machine_shared_keys),
                    "{step}: machine-shared keys changed in {}",
                    path.display()
                );
            } else {
                assert_eq!(b, a, "{step}: {} changed", path.display());
            }
        }
        let owned: BTreeSet<(String, String)> = surface.owned_items.iter().cloned().collect();
        let creds: BTreeSet<(String, String)> = surface.credential_items.iter().cloned().collect();
        let keys: BTreeSet<&(String, String)> =
            before.items.keys().chain(after.items.keys()).collect();
        for key in keys {
            let (b, a) = (before.items.get(key), after.items.get(key));
            if owned.contains(key) {
                continue;
            }
            if creds.contains(key) {
                assert_eq!(
                    shared_keys(b, &surface.machine_shared_keys),
                    shared_keys(a, &surface.machine_shared_keys),
                    "{step}: machine-shared keys changed in Keychain item {key:?}"
                );
            } else {
                assert_eq!(b, a, "{step}: Keychain item {key:?} changed");
            }
        }
    }
```

No existing test changes. The two `IdentitySurface` literals are the only construction sites,
and every existing invariant run keeps `projects/` and `history.jsonl`, so the new allowance
never applies to them.

- [ ] **Step 10: Run them and see them pass**, then the suites

Run: `cargo test -p tagteam-cc --test provider`  Expected: PASS
Run: `cargo test -p tagteam-fake --test provider`  Expected: PASS
Run: `cargo test -p tagteam-engine --test invariant --test fake_agent`  Expected: PASS
Run: `cargo test --workspace --features tagteam/test-support`  Expected: PASS

- [ ] **Step 11: Format and lint**

```
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 12: Commit**

```bash
git add crates/tagteam-provider/src/provider.rs crates/tagteam-cc/src/provider.rs \
  crates/tagteam-fake/src/provider.rs crates/tagteam-cc/tests/provider.rs \
  crates/tagteam-fake/tests/provider.rs crates/tagteam-engine/tests/common/mod.rs \
  crates/tagteam-engine/tests/invariant.rs
git commit -m "Declare the entries a link sync may create, and hold the local-state test to them"
```

#### 13c: `Engine::sync_profile_links`

- [ ] **Step 13: Write the failing tests**

In `crates/tagteam-engine/src/error.rs`, in `kind_is_pinned_for_every_variant`, add after the
`ForeignLiveCredential` row:

```rust
            (
                EngineError::ProfileSplit {
                    profile: PathBuf::from("p"),
                    shared: PathBuf::from("s"),
                },
                "profile-split",
            ),
```

In `crates/tagteam-engine/tests/common/mod.rs`, add to the `impl Fx` that holds Task 8's profile
helpers (after `make_profile`):

```rust
    /// `id`'s profile for any provider `p` with sessions, in its place under `sessions/`, as a
    /// first launch leaves it before its sync: `profile_dir(id)`, 0700, holding only a marker
    /// that names `p`, `id`, the exported spelling of the canonical path (§12.2 "One spelling")
    /// and `p`'s record of the fixture's home (§4.5 `outer_home`). No identity, no seed, nothing
    /// running. Returns its directory.
    pub fn make_profile_for(&self, p: &dyn Provider, id: &AccountId) -> PathBuf {
        let dir = self.profile_dir(id);
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        ProfileMarker {
            provider: p.id(),
            account_id: id.clone(),
            config_dir: p.profile_spelling(&canonical_profile_path(&dir).unwrap()),
            outer: p.outer_home(&self.env),
        }
        .write(&dir)
        .unwrap();
        dir
    }
```

Create `crates/tagteam-engine/tests/profiles.rs`:

```rust
//! §12.2's link sync: what a profile shares with the outer home, and what it never touches
//! (Review Focus 5). Profiles live under the fixture's data dir. The source home is the
//! fixture's `~/.claude`, unless a marker says otherwise.
mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use common::{FakeFx, Fx};
use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::Engine;
use tagteam_engine::profiles::SyncReport;
use tagteam_engine::settings::Settings;
use tagteam_fake::FAKE_AGENT;
use tagteam_provider::profile::{LINKS_FILE, LinksRecord, MARKER_FILE, ProfileMarker};
use tagteam_provider::{Provider, Read};

/// The fixture's `~/.claude` entries that Claude Code's allowlist names, in the policy's
/// order: the must-share ones first.
const FIXTURE_SHARED: [&str; 6] = [
    "projects",
    "history.jsonl",
    "CLAUDE.md",
    "settings.json",
    "skills",
    "plugins",
];

/// A profile as a first launch leaves it before its sync: the marker, and nothing else.
fn setup(fx: &Fx) -> PathBuf {
    fx.make_profile_for(fx.cc.as_ref(), &AccountId::from_string("0192-a"))
}

/// The source home Claude Code's policy names for the fixture.
fn source(fx: &Fx) -> PathBuf {
    fx.cc.share_policy(&fx.env).source
}

/// A launch's sync.
fn sync(fx: &Fx, profile: &Path) -> SyncReport {
    fx.engine
        .sync_profile_links(fx.cc.as_ref(), profile, false)
        .unwrap()
}

/// The sync of a launch that joins a running session.
fn join(fx: &Fx, profile: &Path) -> SyncReport {
    fx.engine
        .sync_profile_links(fx.cc.as_ref(), profile, true)
        .unwrap()
}

/// An engine over the fixture whose settings also share `names` (`run.share_extra`).
fn sharing(fx: &Fx, names: &[&str]) -> Engine {
    fx.engine_with_settings(Settings {
        share_extra: names.iter().map(|n| n.to_string()).collect(),
        ..Settings::default()
    })
}

/// `path`'s link target, when it is a link.
fn link(path: &Path) -> Option<PathBuf> {
    let meta = fs::symlink_metadata(path).ok()?;
    meta.file_type()
        .is_symlink()
        .then(|| fs::read_link(path).unwrap())
}

fn resolved(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap()
}

/// Nothing at all at `path`, not even a link.
fn absent(path: &Path) -> bool {
    fs::symlink_metadata(path).is_err()
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn record(profile: &Path) -> LinksRecord {
    match LinksRecord::read(profile) {
        Read::Present(r) => r,
        other => panic!("the links record should read: {other:?}"),
    }
}

/// The names in `dir`, sorted.
fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// What `path` is, to compare before and after: a link's target, a directory's names, or a
/// file's text.
fn describe(path: &Path) -> String {
    let meta = fs::symlink_metadata(path).unwrap();
    if meta.file_type().is_symlink() {
        format!("link {}", fs::read_link(path).unwrap().display())
    } else if meta.is_dir() {
        format!("dir {:?}", entries(path))
    } else {
        format!("file {:?}", fs::read_to_string(path).unwrap())
    }
}

/// Moves the source's `name` into `~/dotfiles` and links it back, as GNU stow does.
fn stow(fx: &Fx, name: &str) -> PathBuf {
    let dot = fx.env.home.join("dotfiles");
    fs::create_dir_all(&dot).unwrap();
    let moved = dot.join(name);
    fs::rename(source(fx).join(name), &moved).unwrap();
    symlink(&moved, source(fx).join(name)).unwrap();
    moved
}

/// Rewrites the profile's marker so that its outer home is `home`, as a launch with
/// `CLAUDE_CONFIG_DIR=<home>` records it.
fn point_marker_at(fx: &Fx, profile: &Path, home: &Path) {
    let mut env = fx.env.clone();
    env.claude_config_dir = Some(home.as_os_str().to_owned());
    let Read::Present(marker) = ProfileMarker::read(profile) else {
        panic!("the marker reads");
    };
    ProfileMarker {
        outer: fx.cc.outer_home(&env),
        ..marker
    }
    .write(profile)
    .unwrap();
}

#[test]
fn every_allowlisted_entry_of_the_source_home_is_linked_to_its_fully_resolved_path() {
    let fx = Fx::new();
    let profile = setup(&fx);
    let src = source(&fx);
    // A stow-style CLAUDE.md: the link must reach the file itself, in one hop (Appendix A.1).
    let moved = stow(&fx, "CLAUDE.md");

    let report = sync(&fx, &profile);

    assert_eq!(report.created, FIXTURE_SHARED);
    assert!(
        report.removed.is_empty() && report.warnings.is_empty(),
        "{report:?}"
    );
    for name in FIXTURE_SHARED {
        assert_eq!(
            link(&profile.join(name)),
            Some(resolved(&src.join(name))),
            "{name}"
        );
    }
    assert_eq!(
        link(&profile.join("CLAUDE.md")),
        Some(resolved(&moved)),
        "the fully resolved source, not the stow link"
    );
    for name in ["keybindings.json", "agents", "commands"] {
        assert!(
            absent(&profile.join(name)),
            "{name} is not in the source home, so nothing links it"
        );
    }
    let want: BTreeMap<String, PathBuf> = FIXTURE_SHARED
        .iter()
        .map(|n| (n.to_string(), resolved(&src.join(n))))
        .collect();
    assert_eq!(
        record(&profile).links,
        want,
        "the record names every link tagteam made"
    );
    assert_eq!(mode(&profile.join(LINKS_FILE)), 0o600);
    assert_eq!(
        fs::read_to_string(src.join("history.jsonl")).unwrap(),
        "{\"display\":\"hi\"}\n",
        "an existing must-share entry is never written"
    );
    assert_eq!(
        sync(&fx, &profile),
        SyncReport::default(),
        "a second launch changes nothing"
    );
}

#[test]
fn a_missing_must_share_entry_is_created_empty_in_the_source_home_and_linked() {
    let fx = Fx::new();
    let profile = setup(&fx);
    let src = source(&fx);
    fs::remove_dir_all(src.join("projects")).unwrap();
    fs::remove_file(src.join("history.jsonl")).unwrap();

    let report = sync(&fx, &profile);

    assert!(src.join("projects").is_dir());
    assert!(entries(&src.join("projects")).is_empty());
    assert_eq!(mode(&src.join("projects")), 0o700);
    assert_eq!(fs::read(src.join("history.jsonl")).unwrap(), b"");
    assert_eq!(mode(&src.join("history.jsonl")), 0o600);
    for name in ["projects", "history.jsonl"] {
        assert_eq!(
            link(&profile.join(name)),
            Some(resolved(&src.join(name))),
            "{name}"
        );
    }
    assert_eq!(report.created[..2], ["projects", "history.jsonl"]);
}

#[test]
fn a_source_home_that_does_not_exist_yet_is_created_with_its_must_share_entries() {
    let fx = Fx::new();
    let profile = setup(&fx);
    let fresh = fx.env.home.join("fresh-claude");
    point_marker_at(&fx, &profile, &fresh);

    let report = sync(&fx, &profile);

    assert_eq!(report.created, ["projects", "history.jsonl"]);
    assert_eq!(mode(&fresh), 0o700);
    assert!(fresh.join("projects").is_dir());
    assert_eq!(fs::read(fresh.join("history.jsonl")).unwrap(), b"");
}

#[test]
fn private_entries_and_tagteam_s_own_files_are_never_linked_even_through_share_extra() {
    let fx = Fx::new();
    let profile = setup(&fx);
    let src = source(&fx);
    let dirs = ["sessions", "cache", "daemon", ".oauth_refresh.lock"];
    let files = [
        ".credentials.json",
        "settings.local.json",
        "daemon.json",
        "policy-limits.json.signature",
        ".tagteam-x",
    ];
    for dir in dirs {
        fs::create_dir_all(src.join(dir)).unwrap();
    }
    for file in files {
        fs::write(src.join(file), "x").unwrap();
    }
    let engine = sharing(&fx, &["cache", "settings.local.json", ".tagteam-x"]);

    let report = engine
        .sync_profile_links(fx.cc.as_ref(), &profile, false)
        .unwrap();

    for name in dirs.iter().chain(files.iter()) {
        assert!(absent(&profile.join(name)), "{name} was linked");
        assert!(!report.created.iter().any(|n| n == name), "{name}");
    }
    assert_eq!(
        report.warnings.len(),
        3,
        "one for each private name in run.share_extra, and no unknown-entry notice: {:?}",
        report.warnings
    );
    for name in ["cache", "settings.local.json", ".tagteam-x"] {
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("run.share_extra") && w.contains(name)),
            "{name}: {:?}",
            report.warnings
        );
    }
}

#[test]
fn run_share_extra_shares_a_user_s_own_entry() {
    let fx = Fx::new();
    let profile = setup(&fx);
    let src = source(&fx);
    fs::create_dir_all(src.join("hook-data")).unwrap();

    let report = sharing(&fx, &["hook-data"])
        .sync_profile_links(fx.cc.as_ref(), &profile, false)
        .unwrap();

    assert!(report.created.iter().any(|n| n == "hook-data"), "{report:?}");
    assert_eq!(
        link(&profile.join("hook-data")),
        Some(resolved(&src.join("hook-data")))
    );
    assert!(
        report.warnings.is_empty(),
        "a shared entry is not unknown: {:?}",
        report.warnings
    );
}

#[test]
fn an_unknown_entry_of_the_source_home_stays_private_and_is_noted_once() {
    // Review Focus 5: a new CC feature directory tagteam does not know.
    let fx = Fx::new();
    let profile = setup(&fx);
    let src = source(&fx);
    fs::create_dir_all(src.join("newfeature")).unwrap();
    fs::create_dir_all(src.join("sessions")).unwrap(); // known-private: never noted

    let first = sync(&fx, &profile);

    assert_eq!(first.warnings.len(), 1, "{:?}", first.warnings);
    assert!(
        first.warnings[0].contains(&src.join("newfeature").display().to_string()),
        "{:?}",
        first.warnings
    );
    assert!(
        absent(&profile.join("newfeature")),
        "it stays private: no link"
    );
    assert!(absent(&profile.join("sessions")));
    assert_eq!(
        record(&profile).noted_unknown,
        BTreeSet::from(["newfeature".to_owned()])
    );

    let second = sync(&fx, &profile);
    assert!(
        second.warnings.is_empty(),
        "noted once: {:?}",
        second.warnings
    );
    assert!(absent(&profile.join("newfeature")));
}

#[test]
fn a_shared_file_the_profile_holds_as_a_regular_file_warns_naming_both_and_is_left_alone() {
    // Review Focus 5: something replaced the profile's settings.json link with a real file.
    let fx = Fx::new();
    let profile = setup(&fx);
    let src = source(&fx);
    sync(&fx, &profile);
    let copy = profile.join("settings.json");
    fs::remove_file(&copy).unwrap();
    fs::write(&copy, "{\"theme\":\"light\"}\n").unwrap();

    let report = sync(&fx, &profile);

    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    let w = &report.warnings[0];
    assert!(
        w.contains(&copy.display().to_string())
            && w.contains(&src.join("settings.json").display().to_string()),
        "{w}"
    );
    assert_eq!(
        fs::read_to_string(&copy).unwrap(),
        "{\"theme\":\"light\"}\n",
        "the profile's copy is untouched"
    );
    assert_eq!(
        fs::read_to_string(src.join("settings.json")).unwrap(),
        "{\"theme\":\"dark\"}\n",
        "and so is the shared one"
    );
    assert!(
        report.created.is_empty() && report.removed.is_empty(),
        "{report:?}"
    );
    assert!(
        !record(&profile).links.contains_key("settings.json"),
        "no longer tagteam's link"
    );
    assert!(link(&profile.join("CLAUDE.md")).is_some(), "the other links stay");
    assert_eq!(
        sync(&fx, &profile).warnings.len(),
        1,
        "a split file is reported at every launch"
    );
}

#[test]
fn a_real_directory_where_a_shared_link_belongs_warns_and_is_left_alone() {
    let fx = Fx::new();
    let profile = setup(&fx);
    let src = source(&fx);
    fs::create_dir_all(profile.join("skills/mine")).unwrap();
    fs::write(profile.join("skills/mine/SKILL.md"), "mine\n").unwrap();

    let report = sync(&fx, &profile);

    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    let w = &report.warnings[0];
    assert!(
        w.contains(&profile.join("skills").display().to_string())
            && w.contains(&src.join("skills").display().to_string()),
        "{w}"
    );
    assert_eq!(
        fs::read_to_string(profile.join("skills/mine/SKILL.md")).unwrap(),
        "mine\n"
    );
    assert!(link(&profile.join("skills")).is_none());
    assert!(!report.created.iter().any(|n| n == "skills"));
    assert!(
        src.join("skills/s/SKILL.md").is_file(),
        "the shared one is untouched"
    );
}

/// Plants `name` in a fresh profile with `plant`. The sync then refuses with
/// `profile-split`, naming both paths, for a launch and for a join, and writes nothing
/// anywhere.
fn assert_split(name: &str, plant: impl Fn(&Fx, &Path)) {
    let fx = Fx::new();
    let profile = setup(&fx);
    let src = source(&fx);
    plant(&fx, &profile.join(name));
    // The other must-share entry is missing, so a sync that went ahead would create it.
    let other = src.join(if name == "projects" {
        "history.jsonl"
    } else {
        "projects"
    });
    if other.is_dir() {
        fs::remove_dir_all(&other).unwrap();
    } else {
        fs::remove_file(&other).unwrap();
    }
    let (copy, shared) = (describe(&profile.join(name)), describe(&src.join(name)));

    for joining in [false, true] {
        let err = fx
            .engine
            .sync_profile_links(fx.cc.as_ref(), &profile, joining)
            .unwrap_err();
        assert_eq!(err.kind(), "profile-split", "{name}: {err}");
        let text = err.to_string();
        assert!(
            text.contains(&profile.join(name).display().to_string())
                && text.contains(&src.join(name).display().to_string()),
            "{text}"
        );
    }
    assert_eq!(
        describe(&profile.join(name)),
        copy,
        "the profile's copy is untouched"
    );
    assert_eq!(describe(&src.join(name)), shared, "and so is the shared one");
    assert!(absent(&other), "nothing was created in the source home");
    let mut want = vec![MARKER_FILE.to_owned(), name.to_owned()];
    want.sort();
    assert_eq!(entries(&profile), want, "no link was made and no record written");
}

#[test]
fn a_real_history_file_in_the_profile_refuses_with_profile_split_and_changes_nothing() {
    // Review Focus 5.
    assert_split("history.jsonl", |_, p| {
        fs::write(p, "private history\n").unwrap()
    });
}

#[test]
fn a_real_projects_directory_in_the_profile_refuses_with_profile_split_and_changes_nothing() {
    assert_split("projects", |_, p| {
        fs::create_dir_all(p.join("-x/memory")).unwrap();
        fs::write(p.join("-x/memory/MEMORY.md"), "private memory\n").unwrap();
    });
}

#[test]
fn a_must_share_link_that_resolves_elsewhere_refuses_as_a_split_too() {
    assert_split("history.jsonl", |fx, p| {
        let elsewhere = fx.env.home.join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        fs::write(elsewhere.join("history.jsonl"), "elsewhere\n").unwrap();
        symlink(elsewhere.join("history.jsonl"), p).unwrap();
    });
}

#[test]
fn a_link_tagteam_did_not_make_is_never_touched() {
    let fx = Fx::new();
    let profile = setup(&fx);
    let src = source(&fx);
    let elsewhere = fx.env.home.join("elsewhere");
    for dir in ["skills", "notes"] {
        fs::create_dir_all(elsewhere.join(dir)).unwrap();
    }
    symlink(elsewhere.join("skills"), profile.join("skills")).unwrap(); // where tagteam's goes
    symlink(elsewhere.join("notes"), profile.join("notes")).unwrap(); // on no list
    symlink(src.join("plugins"), profile.join("plugins")).unwrap(); // the share, unresolved

    let report = sync(&fx, &profile);

    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    let w = &report.warnings[0];
    assert!(
        w.contains(&profile.join("skills").display().to_string())
            && w.contains(&src.join("skills").display().to_string()),
        "{w}"
    );
    assert_eq!(link(&profile.join("skills")), Some(elsewhere.join("skills")));
    assert_eq!(link(&profile.join("notes")), Some(elsewhere.join("notes")));
    assert_eq!(
        link(&profile.join("plugins")),
        Some(src.join("plugins")),
        "a link that already reaches the share is left as it is"
    );
    let r = record(&profile);
    for name in ["skills", "notes", "plugins"] {
        assert!(!r.links.contains_key(name), "{name} is not tagteam's");
    }

    // tagteam's own link, repointed by someone since, is no longer tagteam's: even with its
    // source gone, it is never removed.
    fs::remove_file(profile.join("CLAUDE.md")).unwrap();
    fs::write(elsewhere.join("CLAUDE.md"), "mine\n").unwrap();
    symlink(elsewhere.join("CLAUDE.md"), profile.join("CLAUDE.md")).unwrap();
    fs::remove_file(src.join("CLAUDE.md")).unwrap();
    let again = sync(&fx, &profile);
    assert!(again.removed.is_empty(), "{again:?}");
    assert_eq!(
        link(&profile.join("CLAUDE.md")),
        Some(elsewhere.join("CLAUDE.md"))
    );
    assert!(!record(&profile).links.contains_key("CLAUDE.md"));
}

#[test]
fn a_link_whose_source_disappeared_is_removed() {
    let fx = Fx::new();
    let profile = setup(&fx);
    sync(&fx, &profile);
    fs::remove_dir_all(source(&fx).join("skills")).unwrap();

    let report = sync(&fx, &profile);

    assert_eq!(report.removed, ["skills"]);
    assert!(report.created.is_empty(), "{report:?}");
    assert!(absent(&profile.join("skills")));
    assert!(!record(&profile).links.contains_key("skills"));
}

#[test]
fn a_link_whose_entry_left_the_allowlist_is_removed_and_its_source_kept() {
    let fx = Fx::new();
    let profile = setup(&fx);
    let src = source(&fx);
    for dir in ["hook-data", "tool-cache"] {
        fs::create_dir_all(src.join(dir)).unwrap();
    }
    sharing(&fx, &["hook-data", "tool-cache"])
        .sync_profile_links(fx.cc.as_ref(), &profile, false)
        .unwrap();
    // One of the two links was replaced by a real directory meanwhile.
    fs::remove_file(profile.join("tool-cache")).unwrap();
    fs::create_dir(profile.join("tool-cache")).unwrap();

    let report = sync(&fx, &profile); // run.share_extra names neither any more

    assert_eq!(report.removed, ["hook-data"]);
    assert!(absent(&profile.join("hook-data")));
    assert!(src.join("hook-data").is_dir(), "the source is never touched");
    assert!(
        profile.join("tool-cache").is_dir() && link(&profile.join("tool-cache")).is_none(),
        "a real directory is never removed"
    );
    let r = record(&profile);
    assert!(!r.links.contains_key("hook-data") && !r.links.contains_key("tool-cache"));
}

#[test]
fn a_link_to_a_moved_source_is_made_again() {
    let fx = Fx::new();
    let profile = setup(&fx);
    sync(&fx, &profile);
    let moved = stow(&fx, "CLAUDE.md");

    let report = sync(&fx, &profile);

    assert_eq!(report.removed, ["CLAUDE.md"]);
    assert_eq!(report.created, ["CLAUDE.md"]);
    assert_eq!(link(&profile.join("CLAUDE.md")), Some(resolved(&moved)));
    assert_eq!(record(&profile).links["CLAUDE.md"], resolved(&moved));
}

#[test]
fn links_follow_the_outer_home_the_marker_records() {
    let fx = Fx::new();
    let profile = setup(&fx);
    sync(&fx, &profile);
    let other = fx.env.home.join("other-claude");
    fs::create_dir_all(&other).unwrap();
    fs::write(other.join("CLAUDE.md"), "other\n").unwrap();
    point_marker_at(&fx, &profile, &other);

    let report = sync(&fx, &profile);

    assert_eq!(report.removed, FIXTURE_SHARED);
    assert_eq!(report.created, ["projects", "history.jsonl", "CLAUDE.md"]);
    assert_eq!(
        link(&profile.join("CLAUDE.md")),
        Some(resolved(&other.join("CLAUDE.md")))
    );
    assert!(other.join("projects").is_dir(), "the new home's must-share entries");
    assert_eq!(fs::read(other.join("history.jsonl")).unwrap(), b"");
    for name in ["settings.json", "skills", "plugins"] {
        assert!(absent(&profile.join(name)), "{name}: the new home has none");
    }
    assert!(
        source(&fx)
            .join("projects/-work-app/memory/MEMORY.md")
            .is_file(),
        "the old home is untouched"
    );
}

#[test]
fn a_joining_sync_only_creates_missing_links() {
    let fx = Fx::new();
    let profile = setup(&fx);
    let src = source(&fx);
    sync(&fx, &profile);
    let (old_skills, old_claude) = (
        link(&profile.join("skills")),
        link(&profile.join("CLAUDE.md")),
    );
    fs::remove_dir_all(src.join("skills")).unwrap();
    stow(&fx, "CLAUDE.md");
    fs::create_dir_all(src.join("agents")).unwrap();

    let report = join(&fx, &profile);

    assert_eq!(report.created, ["agents"]);
    assert!(report.removed.is_empty(), "{report:?}");
    assert_eq!(
        link(&profile.join("skills")),
        old_skills,
        "a running session's links never change"
    );
    assert_eq!(link(&profile.join("CLAUDE.md")), old_claude);

    // The next launch that does not join tidies up.
    let report = sync(&fx, &profile);
    assert_eq!(report.removed, ["CLAUDE.md", "skills"]);
    assert_eq!(report.created, ["CLAUDE.md"]);
}

#[test]
fn a_join_refuses_a_must_share_link_that_no_longer_resolves_where_the_source_does() {
    // The outer home's projects/ moved behind a link while a session runs: the profile's link
    // still points at the old place, and a join cannot make it again under the session. The
    // session and the outer home would use different memory, so the join refuses before
    // writing anything; a quiescent launch makes the link again.
    let fx = Fx::new();
    let profile = setup(&fx);
    sync(&fx, &profile);
    let old = link(&profile.join("projects"));
    let moved = stow(&fx, "projects");
    let before = record(&profile);

    let err = fx
        .engine
        .sync_profile_links(fx.cc.as_ref(), &profile, true)
        .unwrap_err();

    assert_eq!(err.kind(), "profile-split", "{err}");
    assert_eq!(link(&profile.join("projects")), old, "nothing was written");
    assert_eq!(record(&profile), before);

    let report = sync(&fx, &profile);
    assert_eq!(report.removed, ["projects"]);
    assert_eq!(report.created, ["projects"]);
    assert_eq!(link(&profile.join("projects")), Some(resolved(&moved)));
}

#[test]
fn a_join_recreates_a_missing_must_share_source_its_link_points_at() {
    // history.jsonl vanished from the outer home while a session runs. Its link still names
    // the place step 2 creates it again, so the join recreates it empty and keeps the link.
    let fx = Fx::new();
    let profile = setup(&fx);
    sync(&fx, &profile);
    let src = source(&fx);
    let old = link(&profile.join("history.jsonl"));
    fs::remove_file(src.join("history.jsonl")).unwrap();

    let report = join(&fx, &profile);

    assert_eq!(fs::read(src.join("history.jsonl")).unwrap(), b"");
    assert_eq!(link(&profile.join("history.jsonl")), old);
    assert!(report.removed.is_empty(), "{report:?}");
}

#[test]
fn a_profile_whose_marker_or_record_cannot_be_read_is_not_synced() {
    // No marker: the outer home is unknown.
    let fx = Fx::new();
    let profile = setup(&fx);
    fs::remove_file(profile.join(MARKER_FILE)).unwrap();
    let err = fx
        .engine
        .sync_profile_links(fx.cc.as_ref(), &profile, false)
        .unwrap_err();
    assert_eq!(err.kind(), "invalid-input", "{err}");
    assert!(entries(&profile).is_empty(), "nothing was linked");

    // A corrupt marker or record is unreadable, never taken for absent (§4.3).
    for file in [MARKER_FILE, LINKS_FILE] {
        let fx = Fx::new();
        let profile = setup(&fx);
        fs::write(profile.join(file), "{").unwrap();
        let err = fx
            .engine
            .sync_profile_links(fx.cc.as_ref(), &profile, false)
            .unwrap_err();
        assert_eq!(err.kind(), "unreadable", "{file}: {err}");
        assert!(
            FIXTURE_SHARED.iter().all(|n| absent(&profile.join(n))),
            "{file}: nothing was linked"
        );
        assert_eq!(fs::read_to_string(profile.join(file)).unwrap(), "{");
    }

    // Another provider's profile.
    let fx = Fx::new();
    let profile = setup(&fx);
    let Read::Present(marker) = ProfileMarker::read(&profile) else {
        panic!("the marker reads");
    };
    ProfileMarker {
        provider: ProviderId::new(FAKE_AGENT),
        ..marker
    }
    .write(&profile)
    .unwrap();
    let err = fx
        .engine
        .sync_profile_links(fx.cc.as_ref(), &profile, false)
        .unwrap_err();
    assert_eq!(err.kind(), "invalid-input", "{err}");
}

#[test]
fn fake_agent_profiles_share_by_fake_agent_s_own_policy() {
    // §12.1: the sharing rules are engine-generic; FakeAgent's lists are deliberately unlike
    // Claude Code's (Decision 14).
    let ffx = FakeFx::new();
    let alice = ffx.fake_add("alice", "tok-a", "renew-a");
    let home = ffx.fake.share_policy(&ffx.fx.env).source;
    fs::create_dir_all(home.join("notes")).unwrap();
    fs::write(home.join("prefs.json"), "{}\n").unwrap();
    fs::create_dir_all(home.join("widgets")).unwrap();
    let profile = ffx.fx.make_profile_for(ffx.fake.as_ref(), &alice);

    let report = ffx
        .engine
        .sync_profile_links(ffx.fake.as_ref(), &profile, false)
        .unwrap();

    assert_eq!(report.created, ["journal.log", "notes", "prefs.json"]);
    assert_eq!(
        fs::read(home.join("journal.log")).unwrap(),
        b"",
        "its must-share entry, created empty"
    );
    for name in ["journal.log", "notes", "prefs.json"] {
        assert_eq!(
            link(&profile.join(name)),
            Some(resolved(&home.join(name))),
            "{name}"
        );
    }
    for name in ["credential.json", "identity.json", "widgets"] {
        assert!(absent(&profile.join(name)), "{name} stays private");
    }
    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    assert!(
        report.warnings[0].contains(&home.join("widgets").display().to_string()),
        "{:?}",
        report.warnings
    );
}
```

In `crates/tagteam-engine/tests/invariant.rs`, append:

```rust
#[test]
fn a_link_sync_writes_only_the_create_only_entries_outside_tagteam_s_data() {
    // §15.3 for §12.2's sync. The profile is tagteam's own, under the data dir the walk skips,
    // so outside it the sync may only create the create-only entries, empty, where there were
    // none. A launch, a second launch and a join, on both platforms, through a symlinked
    // settings.json.
    for platform in [Platform::MacOs, Platform::Linux] {
        let fx = Fx::with_platform(platform);
        seed_realistic_state(&fx);
        symlink_into_dotfiles(
            &fx.env.home,
            &fx.env.home.join(".claude/settings.json"),
            false,
        );
        let a = fx.add("a@x.co", "rt-a");
        fx.add("b@x.co", "rt-b");
        let claude = fx.env.home.join(".claude");
        std::fs::remove_dir_all(claude.join("projects")).unwrap();
        std::fs::remove_file(claude.join("history.jsonl")).unwrap();
        let profile = fx.make_profile_for(fx.cc.as_ref(), &a);
        let sync = |joining: bool| {
            fx.engine
                .sync_profile_links(fx.cc.as_ref(), &profile, joining)
                .unwrap();
        };

        check(&fx, "first sync", || sync(false));
        assert!(
            claude.join("projects").is_dir(),
            "the sync created the must-share directory"
        );
        assert_eq!(std::fs::read(claude.join("history.jsonl")).unwrap(), b"");
        check(&fx, "second sync", || sync(false));
        check(&fx, "joining sync", || sync(true));
        assert!(
            std::fs::symlink_metadata(claude.join("settings.json"))
                .unwrap()
                .file_type()
                .is_symlink(),
            "the settings.json link itself survives"
        );
    }
}
```

In `crates/tagteam-engine/tests/fake_agent.rs` (its `common` import, `use common::{FakeFx, Fx};`,
stays), append:

```rust
#[test]
fn a_fake_agent_link_sync_writes_only_its_create_only_entry() {
    // §15.3 for the sync, through FakeAgent's own policy: `journal.log` is created empty where
    // there was none. Nothing else of FakeAgent's home changes, and nothing of Claude Code's
    // beside it.
    let ffx = FakeFx::new();
    ffx.fx.add("cc@b.co", "rt-cc");
    let alice = ffx.fake_add("alice", "tok-a", "renew-a");
    let home = FakePaths::resolve(&ffx.fx.env).dir;
    fs::create_dir_all(home.join("notes")).unwrap();
    fs::write(home.join("notes/today.txt"), "note\n").unwrap();
    fs::write(home.join("prefs.json"), "{}\n").unwrap();
    let profile = ffx.fx.make_profile_for(ffx.fake.as_ref(), &alice);
    let surface = ffx.fake.identity_surface(&ffx.fx.env);

    check(&ffx, &surface, "fake sync", || {
        ffx.engine
            .sync_profile_links(ffx.fake.as_ref(), &profile, false)
            .unwrap();
    });

    assert_eq!(fs::read(home.join("journal.log")).unwrap(), b"");
    assert!(
        fs::symlink_metadata(profile.join("notes"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}
```

- [ ] **Step 14: Run them and see them fail**

Run: `cargo test -p tagteam-engine --lib error::tests`
Expected: compile error, `no variant named ProfileSplit found for enum EngineError`.

Run: `cargo test -p tagteam-engine --test profiles --test invariant --test fake_agent`
Expected: compile errors, `could not find profiles in tagteam_engine` and `no method named
sync_profile_links found for struct Engine`.

- [ ] **Step 15: Implement**

In `crates/tagteam-engine/src/error.rs`, add after `ForeignLiveCredential` (:99-104), beside the
variants Tasks 8, 9 and 11 add:

```rust
    /// §12.2: the profile holds a must-share entry as a real copy, or as a link that resolves
    /// elsewhere, where tagteam's link to the shared one belongs. Splitting memory or history
    /// silently is never an option, so the launch refuses until the user merges the two.
    #[error(
        "{} is a real copy where {} should be linked; merge the two by hand, then remove the copy",
        profile.display(),
        shared.display()
    )]
    ProfileSplit { profile: PathBuf, shared: PathBuf },
```

and to `kind()`, before the `Io` arm:

```rust
            EngineError::ProfileSplit { .. } => "profile-split",
```

In `crates/tagteam-engine/src/lib.rs`, add `pub mod profiles;` after `pub mod oracle;`.

Create `crates/tagteam-engine/src/profiles.rs`:

```rust
//! §12.2's link sync. A profile shares the outer home's entries by allowlist: the provider
//! names the source home, the shared and must-share entries and the known-private patterns
//! (`Provider::share_policy`, Decision 14), and `run.share_extra` adds names. Each link points
//! at the fully resolved source, because Claude Code follows at most one link when it writes
//! through one (Appendix A.1). tagteam removes only the links it made, as `.tagteam-links.json`
//! records them, and never replaces, merges or deletes a real file or directory.

use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, ErrorKind};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, symlink};
use std::path::{Path, PathBuf};

use tagteam_provider::atomic::ensure_private_dir;
use tagteam_provider::profile::{LinksRecord, MARKER_FILE, ProfileMarker, entry_matches};
use tagteam_provider::{EntryKind, Provider, Read, SharePolicy};

use crate::engine::Engine;
use crate::error::EngineError;
use crate::settings::is_share_name;

/// What one sync did, by entry name, and the lines it reports.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SyncReport {
    /// Links made in the profile.
    pub created: Vec<String>,
    /// tagteam's own links taken out of the profile. A link made again, to a source that
    /// moved, is in both lists.
    pub removed: Vec<String>,
    /// For stderr: a shared entry the profile holds as a real copy, a link tagteam did not
    /// make, a private name in `run.share_extra`, and an unknown entry of the source home,
    /// noted once (§12.2).
    pub warnings: Vec<String>,
}

/// tagteam's own files in a profile, never linked whatever a policy says (§12.2).
const OWN_PREFIX: &str = ".tagteam-";

/// One allowlisted entry, and the kind a must-share one is created as.
struct Wanted {
    name: String,
    must: Option<EntryKind>,
}

/// What the profile holds where a link may belong.
enum Held {
    Nothing,
    /// A symbolic link, and the target it was made with.
    Link(PathBuf),
    /// A real file or directory.
    Real,
}

impl Engine {
    /// §12.2's sync of the profile at `profile`, whose marker names `p`. The caller holds
    /// `MutationGuard` and the account lock (§12.5, launch step 3); the sync takes no lock of
    /// its own. The source home is the one the marker's `outer` records (§4.5 `outer_home`): a
    /// quiescent launch has just updated it, and a joining one leaves it as the running session
    /// found it.
    ///
    /// 1. Nothing is written until every must-share entry has been checked. One the profile
    ///    holds as a real copy, or as a link tagteam did not make that resolves elsewhere,
    ///    refuses with `ProfileSplit`, naming both paths; a must-share source that is a link to
    ///    nothing refuses too.
    /// 2. Unless `joining`, each link tagteam made (as recorded, and still as made) is removed
    ///    when its entry left the allowlist, its source disappeared, or its source now resolves
    ///    elsewhere (it moved, or the outer home changed). A source still there is linked again.
    /// 3. A must-share entry missing from the source home is created there, empty (§3's
    ///    create-only row). Every allowlisted entry the source holds and the profile lacks is
    ///    linked to its fully resolved path. A real file or directory where a link belongs, or
    ///    a link tagteam did not make that resolves elsewhere, is left alone with a warning
    ///    naming both paths.
    /// 4. An entry of the source home on no list is noted once (§12.2 "Unknown entries").
    ///
    /// The record is written, atomically, whenever it changed, even after a step failed, so it
    /// always names exactly the links tagteam made.
    pub fn sync_profile_links(
        &self,
        p: &dyn Provider,
        profile: &Path,
        joining: bool,
    ) -> Result<SyncReport, EngineError> {
        if !p.capabilities().sessions {
            return Err(EngineError::InvalidInput(format!(
                "{} has no `tagteam run` sessions",
                p.display_name()
            )));
        }
        let marker = match ProfileMarker::read(profile) {
            Read::Present(m) => m,
            Read::Absent => {
                return Err(EngineError::InvalidInput(format!(
                    "{} is missing, so the profile's outer home is unknown",
                    profile.join(MARKER_FILE).display()
                )));
            }
            Read::Unreadable(e) => return Err(EngineError::Unreadable(e)),
        };
        if marker.provider != p.id() {
            return Err(EngineError::InvalidInput(format!(
                "{} is a {} profile, not a {} one",
                profile.display(),
                marker.provider,
                p.id()
            )));
        }
        let policy = p.share_policy(&p.apply_outer_home(&self.env, &marker.outer)?);
        let before = match LinksRecord::read(profile) {
            Read::Present(r) => r,
            Read::Absent => LinksRecord::default(),
            Read::Unreadable(e) => return Err(EngineError::Unreadable(e)),
        };
        let mut report = SyncReport::default();
        let wanted = allowlist(&policy, &self.settings.share_extra, &mut report.warnings);
        let unknown = check(&policy, profile, &wanted, &before, joining)?;
        let mut record = before.clone();
        let applied = apply(&policy, profile, &wanted, joining, &mut record, &mut report);
        for name in unknown {
            if record.noted_unknown.insert(name.clone()) {
                report.warnings.push(format!(
                    "{} is on none of {}'s share lists, so it stays private to each profile",
                    policy.source.join(&name).display(),
                    p.display_name()
                ));
            }
        }
        let written = if record == before {
            Ok(())
        } else {
            record.write(profile)
        };
        applied?;
        written?;
        Ok(report)
    }
}

/// The must-share entries, then the shared ones, then `run.share_extra`, without repeats. A
/// private name, or one of tagteam's own, is never on it; one named in `run.share_extra`
/// warns.
fn allowlist(policy: &SharePolicy, extra: &[String], warnings: &mut Vec<String>) -> Vec<Wanted> {
    fn add(wanted: &mut Vec<Wanted>, name: &str, must: Option<EntryKind>) {
        if !wanted.iter().any(|w| w.name == name) {
            wanted.push(Wanted {
                name: name.to_owned(),
                must,
            });
        }
    }
    let mut wanted = Vec::new();
    for m in &policy.must_share {
        if !is_private(policy, m.name) {
            add(&mut wanted, m.name, Some(m.kind));
        }
    }
    for name in &policy.shared {
        if !is_private(policy, name) {
            add(&mut wanted, name, None);
        }
    }
    for name in extra {
        if !is_share_name(name) {
            // `Settings` drops these already; a hand-built one is held to the same rule.
            warnings.push(format!(
                "run.share_extra: {name:?} is not an entry name, so it is not shared"
            ));
        } else if is_private(policy, name) {
            warnings.push(format!(
                "run.share_extra: {name} is private to each profile, so it is not shared"
            ));
        } else {
            add(&mut wanted, name, None);
        }
    }
    wanted
}

/// Known-private (`entry_matches` against the policy's patterns), or tagteam's own.
fn is_private(policy: &SharePolicy, name: &str) -> bool {
    name.starts_with(OWN_PREFIX)
        || policy
            .private
            .iter()
            .any(|pattern| entry_matches(pattern, name))
}

/// Step 1, which writes nothing: every refusal, and the source home's entries on no list.
/// `joining`: a session runs, so a must-share link of tagteam's that no longer resolves where
/// the source does cannot be made again under it, and is a split (§12.2: memory and history
/// are never split).
fn check(
    policy: &SharePolicy,
    profile: &Path,
    wanted: &[Wanted],
    record: &LinksRecord,
    joining: bool,
) -> Result<Vec<String>, EngineError> {
    if resolved(&policy.source)?
        .is_some_and(|source| fs::canonicalize(profile).is_ok_and(|own| own == source))
    {
        return Err(EngineError::InvalidInput(format!(
            "the outer home of {} is the profile itself, so it has nothing to share",
            profile.display()
        )));
    }
    for w in wanted.iter().filter(|w| w.must.is_some()) {
        let (src, dst) = (policy.source.join(&w.name), profile.join(&w.name));
        let target = resolved(&src)?;
        if target.is_none() && exists(&src)? {
            return Err(EngineError::InvalidInput(format!(
                "{} is a link to nothing, so it cannot be shared; fix or remove it, then run again",
                src.display()
            )));
        }
        let split = match held(&dst)? {
            Held::Nothing => false,
            Held::Real => true,
            // tagteam's own link, which step 2 keeps or makes again; a join cannot make it
            // again, so one that no longer points where the source resolves is a split. A
            // missing source is created empty by step 2 at the source home's own path, so the
            // link stays good when it points exactly there.
            Held::Link(made) if record.links.get(&w.name) == Some(&made) => {
                joining
                    && match &target {
                        Some(t) => t != &made,
                        None => {
                            resolved(&policy.source)?
                                .map(|home| home.join(&w.name))
                                .as_ref()
                                != Some(&made)
                        }
                    }
            }
            // Anyone else's is the share only if it resolves where the source does.
            Held::Link(_) => target.is_none() || resolved(&dst)? != target,
        };
        if split {
            return Err(EngineError::ProfileSplit {
                profile: dst,
                shared: src,
            });
        }
    }
    let mut unknown = Vec::new();
    let listing = match fs::read_dir(&policy.source) {
        Ok(listing) => listing,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(unknown),
        Err(e) => return Err(e.into()),
    };
    for entry in listing {
        let name = entry?.file_name().to_string_lossy().into_owned();
        if !is_private(policy, &name) && !wanted.iter().any(|w| w.name == name) {
            unknown.push(name);
        }
    }
    unknown.sort();
    Ok(unknown)
}

/// Steps 2 and 3. Each change is recorded in `record` the moment it is made, so a failure part
/// way leaves a record naming exactly the links tagteam made.
fn apply(
    policy: &SharePolicy,
    profile: &Path,
    wanted: &[Wanted],
    joining: bool,
    record: &mut LinksRecord,
    report: &mut SyncReport,
) -> Result<(), EngineError> {
    if !joining {
        let left: Vec<String> = record
            .links
            .keys()
            .filter(|name| !wanted.iter().any(|w| w.name == **name))
            .cloned()
            .collect();
        for name in left {
            unlink_ours(profile, &name, record, report)?;
        }
    }
    for w in wanted {
        let (src, dst) = (policy.source.join(&w.name), profile.join(&w.name));
        if let Some(kind) = w.must {
            create_empty(&policy.source, &src, kind)?;
        }
        let target = resolved(&src)?;
        match held(&dst)? {
            Held::Nothing => {
                // A recorded link someone removed is no longer tagteam's.
                record.links.remove(&w.name);
                if let Some(target) = target {
                    link(w, &src, &dst, target, record, report)?;
                }
            }
            Held::Link(made) if record.links.get(&w.name) == Some(&made) => {
                if joining || target.as_ref() == Some(&made) {
                    continue;
                }
                fs::remove_file(&dst)?;
                record.links.remove(&w.name);
                report.removed.push(w.name.clone());
                if let Some(target) = target {
                    link(w, &src, &dst, target, record, report)?;
                }
            }
            Held::Link(made) => {
                record.links.remove(&w.name);
                if target.is_some() && resolved(&dst)? != target {
                    report.warnings.push(format!(
                        "{} links to {} rather than {}; tagteam did not make that link and leaves it as it is",
                        dst.display(),
                        made.display(),
                        src.display()
                    ));
                }
            }
            Held::Real => {
                record.links.remove(&w.name);
                if target.is_some() {
                    report.warnings.push(format!(
                        "{} is a real copy where {} should be linked; tagteam leaves both as they are",
                        dst.display(),
                        src.display()
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Removes `name`'s link when it is still the one tagteam made, and nothing else, and forgets
/// it either way.
fn unlink_ours(
    profile: &Path,
    name: &str,
    record: &mut LinksRecord,
    report: &mut SyncReport,
) -> Result<(), EngineError> {
    let dst = profile.join(name);
    let ours = match (record.links.get(name), held(&dst)?) {
        (Some(made), Held::Link(now)) => *made == now,
        _ => false,
    };
    if ours {
        fs::remove_file(&dst)?;
        report.removed.push(name.to_owned());
    }
    record.links.remove(name);
    Ok(())
}

/// Links `dst` to `target`, the fully resolved `src`, and records it. Something that appeared
/// at `dst` since it was checked is never replaced: for a must-share entry that is a split.
fn link(
    w: &Wanted,
    src: &Path,
    dst: &Path,
    target: PathBuf,
    record: &mut LinksRecord,
    report: &mut SyncReport,
) -> Result<(), EngineError> {
    match symlink(&target, dst) {
        Ok(()) => {
            record.links.insert(w.name.clone(), target);
            report.created.push(w.name.clone());
            Ok(())
        }
        Err(e) if e.kind() == ErrorKind::AlreadyExists && w.must.is_some() => {
            Err(EngineError::ProfileSplit {
                profile: dst.to_path_buf(),
                shared: src.to_path_buf(),
            })
        }
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {
            report.warnings.push(format!(
                "{} appeared while it was being linked; tagteam leaves it as it is",
                dst.display()
            ));
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

/// §3's create-only row: a must-share entry the source home lacks is created there, empty, as
/// the agent itself would create it, and only when there is none (step 1 has refused a link to
/// nothing). A source home that does not exist yet is created first, 0700.
fn create_empty(source: &Path, src: &Path, kind: EntryKind) -> io::Result<()> {
    if exists(src)? {
        return Ok(());
    }
    ensure_private_dir(source)?;
    let made = match kind {
        EntryKind::Dir => DirBuilder::new().mode(0o700).create(src),
        EntryKind::File => OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(src)
            .map(drop),
    };
    match made {
        // The agent created it meanwhile: it exists, which is all this wants.
        Err(e) if e.kind() == ErrorKind::AlreadyExists => Ok(()),
        other => other,
    }
}

fn held(path: &Path) -> io::Result<Held> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => Ok(Held::Link(fs::read_link(path)?)),
        Ok(_) => Ok(Held::Real),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(Held::Nothing),
        Err(e) => Err(e),
    }
}

/// The fully resolved path of `path`, or `None` when there is nothing to resolve (absent, or a
/// link to nothing).
fn resolved(path: &Path) -> io::Result<Option<PathBuf>> {
    match fs::canonicalize(path) {
        Ok(p) => Ok(Some(p)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Whether `path` is an entry of its own, a link to nothing included.
fn exists(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}
```

Existing tests this changes:
- `error.rs::kind_is_pinned_for_every_variant`: one row added (Step 13).
- Nothing else calls the sync.

- [ ] **Step 16: Run them and see them pass**, then the whole suite

Run: `cargo test -p tagteam-engine --lib error::tests`  Expected: PASS
Run: `cargo test -p tagteam-engine --test profiles`  Expected: PASS, 19 tests
Run: `cargo test -p tagteam-engine --test invariant --test fake_agent`  Expected: PASS
Run: `cargo test -p tagteam-engine --features test-hooks`  Expected: PASS
Run: `cargo test --workspace --features tagteam/test-support`  Expected: PASS

- [ ] **Step 17: Format and lint**

```
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 18: Commit**

```bash
git add crates/tagteam-engine/src/profiles.rs crates/tagteam-engine/src/lib.rs \
  crates/tagteam-engine/src/error.rs crates/tagteam-engine/tests/common/mod.rs \
  crates/tagteam-engine/tests/profiles.rs crates/tagteam-engine/tests/invariant.rs \
  crates/tagteam-engine/tests/fake_agent.rs
git commit -m "Sync a profile's links by allowlist, refusing a split must-share entry"
```

---

### Task 14: `list` and `status`: `inSession`, `▶`, the session's account

§13.1: "A session-owned account (§12.5) is marked `▶` after its position, and in a run shell the
session's own account `▶ this`." §13.2 adds `inSession?: true` to every row and, in a run shell,
`session: {number, position, id, email}` (or `null`) to every `status` shape. §12.8: "`status`
adds the session's account, and `list` marks it", while the live login stays the default
home's. §15.2's "Run shell" bullet pins both: `list` and `status` show the default login as
active and the session's account as in session.

The task has two cycles: the engine's views (cycle A) and the CLI's rendering (cycle B).

**Readings of the spec this task commits to:**
- **Session-owned rows are marked wherever they are listed,** inside a run shell or not. `this`
  is added only in a run shell, on the row whose id the marker names.
- **`in_session` is computed for `list`, `status` and the account commands' results, and never
  for `statusline` (Decision 17).** It comes from `Engine::session_state(..).owned()`:
  - `account_view_of` computes it, for the rows of `list` and `status` (and `history`'s account
    view, which no renderer reads for it);
  - `account_view` computes it for every account command's result (`add`, `remove`, `move`,
    `alias` and the rest), for the row's registered provider;
  - `account_view_with` takes it as an explicit parameter and never computes it, and
    `Engine::statusline` passes `false`. That keeps the status bar away from profile
    directories (§13.5): its view says `in_session: false` even for an account in a session.

  So `list`, `status`'s managed row and every account command's result say `inSession`. A
  provider without the `sessions` capability, or one this build does not register, has no
  profiles, so its rows are never in session. An `Err` from `session_state` counts as in session
  and is logged, by §12.6's rule that what cannot be determined counts as live. `session_state`
  answers `NoProfile` after one look at `<data_dir>/sessions/<id>`, which is what "only for rows
  that have a profile dir" costs (Decision 8).
- **Accounts are compared through the marker, never through paths.** `RunShell::Inside.profile`
  is the directory as the run shell's variable names it, which may be a symlink and is not
  canonicalized. `this` and `shell_account` therefore use `marker.account_id`.
- **The `▶` column is per table** (one per provider), present only while one of that table's rows
  is session-owned (Decision 11). Without one the table is byte-identical to today's, header
  included.
- **`status` names the session only in a run shell for the provider being shown:** the marker's.
  A `status --provider X` for another provider has no `session` key and no `This session:` line,
  since that session is not X's.
- **An unmanaged session** (the marker names an account the store does not hold) is
  `session: null` in JSON. The text line names the profile's own login, read with
  `Provider::profile_identity` for the marker's recorded spelling: its email, else its label.
  When the profile holds no readable login, the line says only `not managed by tagteam`. This
  read happens in the CLI for `status` alone, never in `Engine::shell_account`, which
  `statusline` also calls and which must not parse any `.claude.json` (§13.5).
- **`status` still collects the live account only** (§8.3); the session's account is named, not
  fetched.

**Files:**
- Modify: `crates/tagteam-engine/src/views.rs`:
  - `AccountView` (today 94–103) gains `in_session`;
  - new `ShellAccount`, after `StatusView` (today 120–126);
  - `account_view_of` (406–425), `account_view` (430–432) and `account_view_with` (434–465);
  - `Engine::statusline`'s one `account_view_with` call (629), which passes `in_session: false`
    (Decision 17; Task 15 rewrites the method and keeps it);
  - new `in_session` and `shell_account` in the same `impl Engine`;
  - the `tagteam_provider` import (9).
- Modify: `crates/tagteam/src/render.rs`:
  - `row_json` (50–94);
  - `Row`, `row`, `table`, `list_human` (360–491);
  - `status_json` (493–511) and `status_human` (544–565);
  - imports (1–10);
  - `testutil::view` (659–693);
  - the call sites in `mod tests` listed in cycle B step 3.
- Modify: `crates/tagteam/src/app.rs`: the `List` and `Status` arms of `App::dispatch` (542–574), new
  `App::{this_account, session_account, session_login}`, and imports (9–24).
- Modify: `crates/tagteam/tests/common/mod.rs` (`cc_profile`, `hold_launch`; imports 5–17).
- Create: `crates/tagteam-engine/tests/session_views.rs`.
- Modify (created by Task 8): `crates/tagteam/tests/run_shell_cli.rs`, appending one module.

**Interfaces:**
- Consumes:
  - Task 6: `FakeProcess { exists, start_time_s, start_ticks, mentions_launch }`,
    `FakeProcessProbe::set`, `parse_lstart`, all re-exported at the `tagteam_provider` root.
  - Task 7: `ProfileMarker { provider, account_id, config_dir, outer }` and `ProfileMarker::{read,
    write}`, `profile_path`, `canonical_profile_path`, `MARKER_FILE`, `LAUNCH_DIR`, `RunShell`,
    `Provider::{profile_spelling, outer_home, session_dir_var, session_records_dir,
    profile_identity}`, all re-exported at the `tagteam_provider` root.
  - Task 8: `Engine::run_shell(&self) -> &RunShell`, `EngineConfig.{process, run_shell}`,
    `EngineError::RunShellUnreadable { marker, detail }` (`run-shell-unreadable`),
    `tagteam_engine::session::detect_run_shell`; the binary detects a run shell from
    `CLAUDE_CONFIG_DIR` and applies the outer home; `list` and `status` run inside one.
    Engine fixtures (`crates/tagteam-engine/tests/common/mod.rs`):
    `Fx::make_profile(&self, id: &AccountId) -> PathBuf` (the profile in its place under
    `sessions/`: marker, and the stored account's login as `.claude.json`; the account must be in
    the store), `Fx::shell_env(&self, profile: &Path) -> Env` (`CLAUDE_CONFIG_DIR` names the
    profile as given, not canonicalized) and `Fx::engine_located(&self, env: Env) -> Engine`
    (Claude Code only, standing where `detect_run_shell` says).
  - Task 9: `Engine::session_state(&self, p: &dyn Provider, row: &AccountRow) ->
    Result<SessionState, EngineError>`, `SessionState::owned`. Engine fixtures: `Fx.process:
    Arc<FakeProcessProbe>` (every engine of the fixture judges records with it), `LSTART`,
    `Fx::hold_reservation(&self, profile: &Path) -> FlockGuard`, `Fx::live_record(&self, profile:
    &Path, pid: u32, kind: &str) -> PathBuf` and `Fx::dead_record(&self, profile: &Path, pid:
    u32) -> PathBuf`.
  - Task 13: `Fx::make_profile_for(&self, p: &dyn Provider, id: &AccountId) -> PathBuf` (the
    marker alone, for an account that is not in the store).
  - Existing: `tagteam_provider::FlockGuard::try_lock`, `tagteam_provider::atomic::ensure_private_dir`.
- Produces:
  - `AccountView.in_session: bool`.
  - `Engine::account_view_with(&self, row: AccountRow, active: bool, with_pace: bool, in_session:
    bool) -> AccountView` (crate-private): `in_session` is a parameter, never computed there.
    `Engine::account_view(&self, row: AccountRow, active: bool) -> AccountView` keeps its
    signature and now computes it.
  - `pub enum ShellAccount { NotInShell, Managed(AccountRow), Unmanaged }` and
    `Engine::shell_account(&self) -> Result<ShellAccount, EngineError>` (Task 15 uses both).
  - `render::row_json` emits `inSession: true` last, only when set.
  - `render::list_human(lists, this: Option<&AccountId>, display_names, now_s, color)`.
  - `render::status_json(s, session: &ShellAccount, provider, usage)`.
  - `render::status_human(s, session: &ShellAccount, session_login: Option<&str>, now_s, color)`.
  - Engine test helpers: none added to `tests/common/mod.rs`. `session_views.rs` has two
    file-private helpers, `listed(engine, id) -> AccountView` and `inside(fx, profile) ->
    Engine` (the engine of a tagteam command run inside `profile`'s Claude Code session), which
    Task 15 uses in the same file.
  - CLI test helpers: `cc_profile`, `hold_launch` (in `crates/tagteam/tests/common/mod.rs`).

**Spec:**
- §12.5: session-owned means a live launch reservation, or a session record that is live or
  unreadable.
- §12.6: a malformed record is unreadable; what cannot be determined counts as live.
- §12.8: in a run shell the live login is the default home's; `status` adds the session's
  account; `list` marks it.
- §13.1: `▶` after the position; `▶ this` for the run shell's own account.
- §13.2: the row's `inSession?: true` after `loginExpiresAt?`; `status`'s additive `session`
  in each shape, `null` when not managed.
- §15.2 "Run shell": `list` and `status` show the default login as active and the session's
  account as in session.

#### Cycle A: the engine's views

- [ ] **Step 1: Write the failing tests**

This step adds no helper to `crates/tagteam-engine/tests/common/mod.rs`: the tests use the
fixtures Tasks 8, 9 and 13 left there (`make_profile`, `make_profile_for`, `shell_env`,
`engine_located`, `hold_reservation`, `live_record`, `dead_record`, `process`, `LSTART`).

Create `crates/tagteam-engine/tests/session_views.rs`:

```rust
//! The views in and around sessions (§12.8, §13.1, §13.2): which accounts are in a session, and
//! the run shell's own account. Session records are judged by the fixture's `FakeProcessProbe`,
//! never by a real pid (§15.1).
mod common;

use std::fs;
use std::path::Path;

use common::{Fx, LSTART};
use tagteam_core::AccountId;
use tagteam_engine::Engine;
use tagteam_engine::views::{AccountView, ShellAccount, StatusView, StatuslineView};
use tagteam_provider::{FakeProcess, MARKER_FILE, Provider, parse_lstart};

/// `id`'s row as `engine`'s list shows it.
fn listed(engine: &Engine, id: &AccountId) -> AccountView {
    engine
        .accounts(None)
        .unwrap()
        .into_iter()
        .flat_map(|l| l.accounts)
        .find(|v| &v.row.id == id)
        .expect("the account is listed")
}

/// The engine of a tagteam command run inside `profile`'s Claude Code session: in its tools,
/// hooks or statusline.
fn inside(fx: &Fx, profile: &Path) -> Engine {
    fx.engine_located(fx.shell_env(profile))
}

#[test]
fn an_account_is_in_session_while_its_profile_holds_a_live_reservation() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    assert!(!listed(&fx.engine, &a).in_session, "no profile");
    let profile = fx.make_profile(&a);
    assert!(!listed(&fx.engine, &a).in_session, "a quiescent profile");

    let launch = fx.hold_reservation(&profile);
    let (va, vb) = (listed(&fx.engine, &a), listed(&fx.engine, &b));
    assert_eq!((va.in_session, va.active), (true, false));
    assert_eq!((vb.in_session, vb.active), (false, true));
    let row = fx.engine.store().unwrap().account(&a).unwrap().unwrap();
    assert!(
        fx.engine.account_view(row, false).in_session,
        "an account command's row says so too"
    );

    drop(launch);
    assert!(!listed(&fx.engine, &a).in_session, "the session ended");
}

#[test]
fn a_session_record_counts_while_its_process_runs_and_an_unreadable_one_always() {
    // §12.6: a recycled pid no longer belongs to the record's writer; a malformed record
    // blocks destructive commands, so the list shows it in session.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let profile = fx.make_profile(&a);
    let records = fx.cc.session_records_dir(&profile);
    fx.dead_record(&profile, 4242);
    assert!(!listed(&fx.engine, &a).in_session, "pid 4242 is not running");

    fx.live_record(&profile, 4242, "interactive");
    assert!(listed(&fx.engine, &a).in_session);

    // Decision 16: a start time that moved is no proof on its own. The pid was recycled only
    // when its new process does not mention the launch command either.
    fx.process.set(
        4242,
        FakeProcess {
            exists: Some(true),
            start_time_s: parse_lstart(LSTART).map(|s| s + 600),
            mentions_launch: Some(false),
            ..FakeProcess::default()
        },
    );
    assert!(!listed(&fx.engine, &a).in_session, "the pid was recycled");

    fs::write(records.join("7.json"), b"{\"pid\": ").unwrap();
    assert!(listed(&fx.engine, &a).in_session);
}

#[test]
fn in_a_run_shell_the_default_login_stays_live_and_the_marker_names_the_sessions_account() {
    // §12.8, §15.2 "Run shell".
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    assert!(matches!(
        fx.engine.shell_account().unwrap(),
        ShellAccount::NotInShell
    ));
    let profile = fx.make_profile(&a);
    let _launch = fx.hold_reservation(&profile);
    let engine = inside(&fx, &profile);

    match engine.shell_account().unwrap() {
        ShellAccount::Managed(row) => assert_eq!(row.id, a),
        other => panic!("{other:?}"),
    }
    let (va, vb) = (listed(&engine, &a), listed(&engine, &b));
    assert_eq!((va.in_session, va.active), (true, false));
    assert_eq!((vb.in_session, vb.active), (false, true));
    match engine.status(&fx.provider()).unwrap() {
        StatusView::Managed { account, total } => assert_eq!((account.row.id, total), (b, 2)),
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_statusline_view_never_computes_session_state() {
    // Decision 17: `list` marks a session-owned account, the status bar's view of the same
    // account does not, so the status bar never looks inside a profile directory (§13.5).
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    let profile = fx.make_profile(&b);
    let _launch = fx.hold_reservation(&profile);
    assert!(listed(&fx.engine, &b).in_session, "list says so");

    match fx.engine.statusline(&fx.provider()).unwrap() {
        StatuslineView::Managed { account } => {
            assert_eq!((account.row.id, account.active, account.in_session), (b, true, false))
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_marker_naming_an_account_the_store_lacks_is_unmanaged() {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile_for(fx.cc.as_ref(), &AccountId::from_string("0192-gone"));
    assert!(matches!(
        inside(&fx, &profile).shell_account().unwrap(),
        ShellAccount::Unmanaged
    ));
}

#[test]
fn a_run_shell_with_no_store_is_unmanaged_and_creates_none() {
    let fx = Fx::new();
    let profile = fx.make_profile_for(fx.cc.as_ref(), &AccountId::from_string("0192"));
    assert!(matches!(
        inside(&fx, &profile).shell_account().unwrap(),
        ShellAccount::Unmanaged
    ));
    assert!(
        !fx.env.data_dir().join("tagteam.db").exists(),
        "§5: a read creates nothing"
    );
}

#[test]
fn an_unreadable_marker_is_the_refusal_that_names_it() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    fs::write(profile.join(MARKER_FILE), b"{\"format\": \"tagteam-profile\", ").unwrap();

    let err = inside(&fx, &profile).shell_account().unwrap_err();

    assert_eq!(err.kind(), "run-shell-unreadable", "{err}");
    // The run shell names its profile as the variable gives it (`shell_env` does not
    // canonicalize), and the refusal names the marker in that directory.
    let marker = profile.join(MARKER_FILE);
    assert!(err.to_string().contains(&marker.display().to_string()), "{err}");
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-engine --test session_views`

Expected: a compile error. `AccountView` has no field `in_session`, `tagteam_engine::views::ShellAccount`
is unresolved, and `Engine` has no method `shell_account`. Nothing else is missing: the fixtures
the tests call (`make_profile`, `make_profile_for`, `shell_env`, `engine_located`,
`hold_reservation`, `live_record`, `dead_record`, `process`, `LSTART`) are Tasks 8, 9 and 13's.

- [ ] **Step 3: Implement**

In `crates/tagteam-engine/src/views.rs`, change the `tagteam_provider` import to:

```rust
use tagteam_provider::{Identity, KindTraits, Provider, Read, RunShell};
```

Replace `AccountView` with:

```rust
#[derive(Debug, Clone)]
pub struct AccountView {
    pub row: AccountRow,
    /// The live identity wins over the store's active account.
    pub active: bool,
    /// The row's credential kind, as its provider describes it (§4.5).
    pub kind: KindTraits,
    /// Its usage, from the store alone (§13.2).
    pub usage: UsageView,
    /// Session-owned (§12.5): a live launch reservation, or a session record that is live or
    /// cannot be read (§12.6). `list` marks it `▶`, and its row says `inSession` (§13.1,
    /// §13.2). Computed for `list`, `status` and the account commands' results; the
    /// statusline's view leaves it `false`, so the status bar never reads a profile directory
    /// (§13.5).
    pub in_session: bool,
}
```

Add after `StatusView`:

```rust
/// The run shell's own account (§12.8, §13.2's `session`), as its marker names it.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum ShellAccount {
    /// Not in a run shell.
    NotInShell,
    /// The account the marker names, as the store holds it.
    Managed(AccountRow),
    /// The marker names an account tagteam does not manage: the store lacks it, holds it under
    /// another provider, or does not exist.
    Unmanaged,
}
```

Replace `account_view_of` with:

```rust
    /// One account as the views show it, its usage read from `store` (with pace, or without:
    /// see `usage_view`).
    fn account_view_of(
        &self,
        p: &dyn Provider,
        store: Option<&Store>,
        row: AccountRow,
        active: bool,
        supported: bool,
        with_pace: bool,
    ) -> Result<AccountView, EngineError> {
        let kind = p.kind_traits(&row.kind);
        let usage = self.usage_view(store, &row, &kind, supported, &p.poll_budget(), with_pace)?;
        let in_session = self.in_session(p, &row);
        Ok(AccountView {
            kind,
            row,
            active,
            usage,
            in_session,
        })
    }
```

Replace `account_view` and `account_view_with` with the two below. `account_view` is the account
commands' results, so it computes `in_session`; `account_view_with` is also what `statusline`
calls, so it takes `in_session` as a parameter and never computes it (Decision 17):

```rust
    /// A row as the views show it, for a caller that already knows whether it is active. Its
    /// usage comes from the store; a store that cannot be read leaves it unread, logged, since
    /// the callers report a change that has already happened. Whether it is session-owned is
    /// computed, as for `list`.
    pub fn account_view(&self, row: AccountRow, active: bool) -> AccountView {
        // A provider this build does not register has no profile it could run.
        let in_session = self
            .registry
            .get(&row.provider)
            .is_some_and(|p| self.in_session(p.as_ref(), &row));
        self.account_view_with(row, active, true, in_session)
    }

    /// `account_view`, with or without pace on its windows (see `usage_view`), and with
    /// `in_session` as the caller knows it. It is never computed here: `statusline` passes
    /// `false`, which keeps the status bar away from profile directories (§13.5, Decision 17).
    fn account_view_with(
        &self,
        row: AccountRow,
        active: bool,
        with_pace: bool,
        in_session: bool,
    ) -> AccountView {
        let provider = self.registry.get(&row.provider);
        let kind = provider
            .as_ref()
            .map_or(UNREGISTERED, |p| p.kind_traits(&row.kind));
        let budget = provider
            .as_ref()
            .map_or(PollBudget::STANDARD, |p| p.poll_budget());
        let supported = provider.is_some_and(|p| p.capabilities().usage);
        let usage = self
            .existing_store()
            .and_then(|s| self.usage_view(s.as_deref(), &row, &kind, supported, &budget, with_pace))
            .unwrap_or_else(|e| {
                tracing::warn!(
                    position = row.position,
                    id = %row.id,
                    kind = e.kind(),
                    "could not read the account's usage"
                );
                UsageView::unread(
                    usage_status(supported, &kind, &row, None),
                    Some(e.kind().to_owned()),
                )
            });
        AccountView {
            row,
            active,
            kind,
            usage,
            in_session,
        }
    }
```

Add `in_session` and `shell_account` to the same `impl Engine`, after them:

```rust
    /// Whether `row` is session-owned, as `list`, `status` and the account commands mark it
    /// (§12.5, §13.1); `statusline` never asks (Decision 17). It is computed on each call
    /// (Decision 8): `session_state` answers `NoProfile` after one look at the
    /// profile directory, which most accounts lack. A state that cannot be determined counts as
    /// owned, as everywhere (§12.6), and is logged.
    fn in_session(&self, p: &dyn Provider, row: &AccountRow) -> bool {
        if !p.capabilities().sessions {
            return false;
        }
        match self.session_state(p, row) {
            Ok(state) => state.owned(),
            Err(e) => {
                tracing::warn!(
                    position = row.position,
                    id = %row.id,
                    kind = e.kind(),
                    "could not tell whether the account is in a session; marking it in session"
                );
                true
            }
        }
    }

    /// The run shell's own account (§12.8), as its marker names it: by id, from the store, with
    /// no live-identity read and no `.claude.json` parse (§13.5). `NotInShell` outside one. A
    /// marker naming an account the store does not hold, holds under another provider, or with
    /// no store at all, is `Unmanaged`. An unreadable marker is the refusal every command but
    /// `statusline` gives (§12.8).
    pub fn shell_account(&self) -> Result<ShellAccount, EngineError> {
        match self.run_shell() {
            RunShell::Outside => Ok(ShellAccount::NotInShell),
            RunShell::Unreadable { marker, detail } => Err(EngineError::RunShellUnreadable {
                marker: marker.clone(),
                detail: detail.clone(),
            }),
            RunShell::Inside { marker, .. } => {
                let Some(store) = self.existing_store()? else {
                    return Ok(ShellAccount::Unmanaged);
                };
                Ok(match store.account(&marker.account_id)? {
                    Some(row) if row.provider == marker.provider => ShellAccount::Managed(row),
                    Some(_) | None => ShellAccount::Unmanaged,
                })
            }
        }
    }
```

In `Engine::statusline`, the one call to `account_view_with` becomes the following, so that this
task compiles (Task 15 replaces the method and keeps the argument):

```rust
            Some(row) => StatuslineView::Managed {
                // The line shows no pace, and never asks whether the account is in a session:
                // the status bar stays away from profile directories (§13.5, Decision 17).
                account: self.account_view_with(row, true, false, false),
            },
```

In `crates/tagteam/src/render.rs`, `testutil::view` builds the one `AccountView` literal outside
the engine. Its closing lines become:

```rust
            active: false,
            kind,
            usage,
            in_session: false,
        }
    }
```

No other `AccountView` literal exists (`rg -n "AccountView \{" crates` finds the two engine sites
and this one).

- [ ] **Step 4: Run them and see them pass**, then the crate's whole suite

Run: `cargo test -p tagteam-engine --test session_views`
Expected: PASS, 7 tests.

Run: `cargo test -p tagteam-engine --features test-hooks` and `cargo test -p tagteam --lib`
Expected: PASS. A view of an account with no profile costs one `NoProfile` answer, so no
existing test moves.

- [ ] **Step 5: Format and lint**

```
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-engine/src/views.rs crates/tagteam-engine/tests/session_views.rs crates/tagteam/src/render.rs
git commit -m "Mark session-owned accounts in the views and name the run shell's own account"
```

#### Cycle B: rendering `▶`, `this`, `inSession` and `session`

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam/src/render.rs`'s `mod tests`, append:

```rust
    /// The rows of `the_list_is_a_table_of_windows_countdowns_pace_and_age`, none of them in a
    /// session, and the table they make (`PINNED`).
    fn pinned_rows() -> Vec<AccountView> {
        let mut live = view(
            1,
            "michael@example.com",
            OAUTH,
            read(120, 9.0, 77.0, true, vec![spend(0.0, 20.0), fable(0.0)]),
        );
        live.active = true;
        let spare = view(
            2,
            "spare@example.com",
            OAUTH,
            read(840, 31.0, 12.0, false, vec![]),
        );
        let mut work = view(
            3,
            "w@corp.com",
            OAUTH,
            unread(UsageStatus::ReloginRequired, None, None),
        );
        work.row.alias = Some("work".into());
        work.row.quarantine_reason = Some("invalid_grant".into());
        let key = view(
            4,
            "api-key-4@token.local",
            API_KEY,
            unread(UsageStatus::ApiKey, None, None),
        );
        vec![live, spare, work, key]
    }

    const PINNED: &str = concat!(
        "    #  ACCOUNT                5H           7D                   SPEND      FABLE        AGE\n",
        " *  1  michael@example.com      9%  2h40m   77%  3d09h  ▲ pace  €0 of €20    0%  3d09h  2m\n",
        "    2  spare@example.com       31%  2h40m   12%  3d09h          —             —         14m\n",
        "    3  work (w@corp.com)      relogin required\n",
        "    4  api-key-4@token.local  api key\n",
    );

    #[test]
    fn without_a_session_owned_row_the_table_is_byte_identical() {
        // Decision 11: the column exists only while some row is session-owned. A run shell
        // whose own account is not listed (one tagteam does not manage) leaves it as it was.
        let elsewhere = AccountId::from_string("0192-unmanaged");
        for this in [None, Some(&elsewhere)] {
            assert_eq!(
                list_human(&one(pinned_rows()), this, &names, NOW, false),
                PINNED
            );
        }
    }

    #[test]
    fn a_session_owned_row_is_marked_after_its_position_and_the_run_shells_own_is_noted() {
        // §13.1's `▶`, in a one-column field after the position, with the header moved to
        // match; the run shell's own account gets the trailing note `this` (Decision 11).
        let mut rows = pinned_rows();
        rows[1].in_session = true;
        let spare = rows[1].row.id.clone();
        assert_eq!(
            list_human(&one(rows.clone()), Some(&spare), &names, NOW, false),
            concat!(
                "    #    ACCOUNT                5H           7D                   SPEND      FABLE        AGE\n",
                " *  1    michael@example.com      9%  2h40m   77%  3d09h  ▲ pace  €0 of €20    0%  3d09h  2m\n",
                "    2 ▶  spare@example.com       31%  2h40m   12%  3d09h          —             —         14m  this\n",
                "    3    work (w@corp.com)      relogin required\n",
                "    4    api-key-4@token.local  api key\n",
            )
        );
        // Outside a run shell the same session shows, without the note.
        assert_eq!(
            list_human(&one(rows), None, &names, NOW, false),
            concat!(
                "    #    ACCOUNT                5H           7D                   SPEND      FABLE        AGE\n",
                " *  1    michael@example.com      9%  2h40m   77%  3d09h  ▲ pace  €0 of €20    0%  3d09h  2m\n",
                "    2 ▶  spare@example.com       31%  2h40m   12%  3d09h          —             —         14m\n",
                "    3    work (w@corp.com)      relogin required\n",
                "    4    api-key-4@token.local  api key\n",
            )
        );
    }

    #[test]
    fn row_json_marks_a_session_owned_row_last_and_only_when_it_is() {
        // §13.2: `alias?, disabled?: true, loginExpiresAt?, inSession?: true`.
        let mut v = view(1, "a@x.co", OAUTH, read(120, 9.0, 77.0, true, vec![]));
        v.row.alias = Some("w".into());
        v.row.disabled = true;
        v.row.login_expires_at = Some(1_797_000_000_000);
        let tail = [
            "usageFetchedAt",
            "usageAgeSeconds",
            "alias",
            "disabled",
            "loginExpiresAt",
        ];
        assert_eq!(keys(&row_json(&v, &count)), [&ALWAYS[..], &tail].concat());
        v.in_session = true;
        let row = row_json(&v, &count);
        assert_eq!(
            keys(&row),
            [&ALWAYS[..], &tail, &["inSession"]].concat()
        );
        assert_eq!(row["inSession"], json!(true));
        assert_eq!(
            account_json(&v, None, &count)["account"]["inSession"],
            json!(true),
            "an account command's row too"
        );
    }

    #[test]
    fn status_names_the_run_shells_account_in_every_shape_and_only_in_a_run_shell() {
        let mut side = view(3, "s@x.co", OAUTH, unread(UsageStatus::Ok, None, None));
        side.row.alias = Some("side".into());
        let session = ShellAccount::Managed(side.row.clone());
        let named = json!({"number": 3, "position": 3, "id": "id-3", "email": "s@x.co"});
        let mut live = view(1, "a@x.co", OAUTH, read(120, 9.0, 77.0, true, vec![]));
        live.active = true;
        let shapes = [
            StatusView::NoLogin,
            StatusView::Unmanaged {
                email: "u@x.co".into(),
            },
            StatusView::Managed {
                account: live,
                total: 3,
            },
        ];
        for shape in &shapes {
            let outside = status_json(shape, &ShellAccount::NotInShell, CLAUDE_CODE, &count);
            assert!(outside.get("session").is_none(), "{outside}");
            let inside = status_json(shape, &session, CLAUDE_CODE, &count);
            assert_eq!(keys(&inside).last(), Some(&"session"), "{inside}");
            assert_eq!(inside["session"], named);
            let unmanaged = status_json(shape, &ShellAccount::Unmanaged, CLAUDE_CODE, &count);
            assert_eq!(unmanaged.get("session"), Some(&Value::Null));
            // Everything else is as it is outside a run shell.
            let mut rest = inside.clone();
            rest.as_object_mut().unwrap().remove("session");
            assert_eq!((keys(&rest), &rest), (keys(&outside), &outside));
        }
        assert_eq!(
            status_json(
                &StatusView::NoLogin,
                &ShellAccount::NotInShell,
                CLAUDE_CODE,
                &count
            ),
            json!({"schemaVersion": 1, "provider": CLAUDE_CODE, "active": null})
        );
    }

    #[test]
    fn status_says_which_account_this_session_runs() {
        let mut side = view(3, "w@corp.com", OAUTH, unread(UsageStatus::Ok, None, None));
        side.row.alias = Some("work".into());
        let session = ShellAccount::Managed(side.row.clone());
        let failing = view(
            1,
            "a@x.co",
            OAUTH,
            unread(UsageStatus::Unavailable, Some("pre-send"), Some(30)),
        );
        let managed = StatusView::Managed {
            account: failing,
            total: 3,
        };
        assert_eq!(
            status_human(&managed, &session, None, NOW, false),
            "Live: a@x.co (position 1 of 3)\n  unavailable (pre-send, retry <1m)\nThis session: work (w@corp.com) (position 3)\n"
        );
        assert_eq!(
            status_human(&StatusView::NoLogin, &session, None, NOW, false),
            "No live login.\nThis session: work (w@corp.com) (position 3)\n"
        );
        let stranger = StatusView::Unmanaged {
            email: "u@x.co".into(),
        };
        assert_eq!(
            status_human(&stranger, &ShellAccount::Unmanaged, Some("c@x.co"), NOW, false),
            "Live: u@x.co (not managed by tagteam)\nThis session: c@x.co (not managed by tagteam)\n"
        );
        assert_eq!(
            status_human(&StatusView::NoLogin, &ShellAccount::Unmanaged, None, NOW, false),
            "No live login.\nThis session: not managed by tagteam\n"
        );
        assert_eq!(
            status_human(&managed, &ShellAccount::NotInShell, Some("ignored"), NOW, false),
            "Live: a@x.co (position 1 of 3)\n  unavailable (pre-send, retry <1m)\n",
            "outside a run shell, as before"
        );
    }
```

In `crates/tagteam/tests/common/mod.rs`, change the imports to:

```rust
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use assert_cmd::Command;
use serde_json::{Value, json};
use tagteam_cc::live::Platform;
use tagteam_cc::{ClaudeCode, ItemKind, keychain_account, keychain_service};
use tagteam_core::{AccountId, CLAUDE_CODE, PollBudget, PollPlan, ProviderId, Window, WindowKind};
use tagteam_engine::store::{Reserve, Store};
use tagteam_engine::vault::SERVICE;
use tagteam_provider::atomic::ensure_private_dir;
use tagteam_provider::splice::replace_top_level;
use tagteam_provider::{
    Env, FakeKeychain, FileKeychain, FlockGuard, Keychain, LAUNCH_DIR, ProfileMarker, Provider,
    canonical_profile_path, profile_path,
};
```

and append:

```rust
/// `id`'s session profile under `root`, as `tagteam run` leaves it for Claude Code (§12.2): the
/// directory and its marker, whose outer home is the default one. Returns the profile and its
/// exported spelling, the `CLAUDE_CONFIG_DIR` its run shell sees.
pub fn cc_profile(root: &Path, id: &str) -> (PathBuf, String) {
    let env = Env::for_test(root);
    let id = AccountId::from_string(id);
    let profile = profile_path(&env, &id);
    ensure_private_dir(&profile).unwrap();
    let cc = ClaudeCode::new(Arc::new(FakeKeychain::new()), Platform::MacOs);
    let spelling = cc.profile_spelling(&canonical_profile_path(&profile).unwrap());
    ProfileMarker {
        provider: ProviderId::new(CLAUDE_CODE),
        account_id: id,
        config_dir: spelling.clone(),
        outer: cc.outer_home(&env),
    }
    .write(&profile)
    .unwrap();
    (profile, spelling)
}

/// A live launch reservation of `profile` (§12.5) while the guard lives: a locked
/// `.tagteam-launch/4242.lock`, which the binary's non-blocking probe sees as held.
pub fn hold_launch(profile: &Path) -> FlockGuard {
    FlockGuard::try_lock(&profile.join(LAUNCH_DIR).join("4242.lock"))
        .unwrap()
        .expect("nothing else holds the reservation")
}
```

Append to `crates/tagteam/tests/run_shell_cli.rs` (Task 8 created it with
`#![cfg(feature = "test-support")]` and `mod common;` at its root; if it does not exist yet,
create it with exactly those two lines first):

```rust
/// Task 14: `list` and `status` in and around sessions (§12.8, §13.1, §13.2).
mod sessions_in_list_and_status {
    use std::fs;
    use std::path::Path;

    use serde_json::{Value, json};
    use tagteam_core::WindowKind;

    use crate::common::{
        cc_profile, cmd, hold_launch, now_epoch_s, record_reading, two_fresh_accounts,
        usage_window,
    };

    /// `a@x.co` at position 1 and `b@x.co` at position 2 (live), each with a reading taken just
    /// now: 5h at 9 % and 7d at 77 %, with no resets. Nothing is due for 180 s (§8.3), so
    /// neither `list` nor `status` sends a request.
    fn read_now(root: &Path) -> (String, String) {
        let (a, b) = two_fresh_accounts(root);
        let now = now_epoch_s();
        for id in [&a, &b] {
            record_reading(
                root,
                id,
                now,
                &[
                    usage_window("5h", "5h", WindowKind::Short, 9.0, None, None),
                    usage_window("7d", "7d", WindowKind::Long, 77.0, None, None),
                ],
            );
        }
        (a, b)
    }

    /// `tagteam <args>`, inside the run shell whose profile `shell` spells when given. It must
    /// succeed with nothing on stderr.
    fn run(root: &Path, shell: Option<&str>, args: &[&str]) -> String {
        let mut c = cmd(root);
        if let Some(dir) = shell {
            c.env("CLAUDE_CONFIG_DIR", dir);
        }
        let out = c.args(args).assert().success().get_output().clone();
        assert_eq!(String::from_utf8_lossy(&out.stderr), "", "{args:?}");
        String::from_utf8(out.stdout).unwrap()
    }

    fn json_of(root: &Path, shell: Option<&str>, args: &[&str]) -> Value {
        serde_json::from_str(&run(root, shell, args)).unwrap()
    }

    fn keys(v: &Value) -> Vec<&str> {
        v.as_object().unwrap().keys().map(String::as_str).collect()
    }

    /// The table with no account in a session: today's layout.
    const QUIET: &str = concat!(
        "    #  ACCOUNT  5H    7D    SPEND  AGE\n",
        "    1  a@x.co     9%   77%  —      <1m\n",
        " *  2  b@x.co     9%   77%  —      <1m\n",
    );

    #[test]
    fn list_in_a_run_shell_marks_the_sessions_account_and_keeps_the_default_login_live() {
        let d = tempfile::tempdir().unwrap();
        let (a, b) = read_now(d.path());
        let (profile, shell) = cc_profile(d.path(), &a);
        assert_eq!(
            run(d.path(), None, &["list"]),
            QUIET,
            "a quiescent profile changes nothing"
        );

        let _session = hold_launch(&profile);
        assert_eq!(
            run(d.path(), Some(&shell), &["list"]),
            concat!(
                "    #    ACCOUNT  5H    7D    SPEND  AGE\n",
                "    1 ▶  a@x.co     9%   77%  —      <1m  this\n",
                " *  2    b@x.co     9%   77%  —      <1m\n",
            )
        );
        let v = json_of(d.path(), Some(&shell), &["list", "--json"]);
        assert_eq!(v["activeAccountNumber"], 2, "the default home's login is live");
        let rows = v["accounts"].as_array().unwrap();
        let first = keys(&rows[0]);
        assert_eq!(
            (rows[0]["id"].as_str(), &first[first.len() - 2..]),
            (Some(a.as_str()), &["loginExpiresAt", "inSession"][..])
        );
        assert_eq!(rows[0]["inSession"], json!(true));
        assert_eq!(
            (rows[1]["id"].as_str(), rows[1].get("inSession")),
            (Some(b.as_str()), None)
        );
    }

    #[test]
    fn status_in_a_run_shell_names_the_sessions_account_in_text_and_json() {
        let d = tempfile::tempdir().unwrap();
        let (a, b) = read_now(d.path());
        let (profile, shell) = cc_profile(d.path(), &a);
        let _session = hold_launch(&profile);
        assert_eq!(
            run(d.path(), Some(&shell), &["status"]),
            "Live: b@x.co (position 2 of 2)\n  5h 9% · 7d 77% · <1m old\nThis session: a@x.co (position 1)\n"
        );
        let v = json_of(d.path(), Some(&shell), &["status", "--json"]);
        assert_eq!(
            keys(&v),
            ["schemaVersion", "provider", "active", "totalManagedAccounts", "session"]
        );
        assert_eq!(
            v["session"],
            json!({"number": 1, "position": 1, "id": a.as_str(), "email": "a@x.co"})
        );
        assert_eq!(
            (v["active"]["id"].as_str(), v["active"]["managed"].as_bool()),
            (Some(b.as_str()), Some(true))
        );
        assert!(v["active"].get("inSession").is_none(), "{v}");
    }

    #[test]
    fn a_run_shell_of_an_account_tagteam_does_not_manage_says_so() {
        let d = tempfile::tempdir().unwrap();
        read_now(d.path());
        let (profile, shell) = cc_profile(d.path(), "0192-not-managed");
        // The login Claude Code keeps in the profile: only `status`'s text reads it.
        let login = json!({"oauthAccount": {"emailAddress": "c@x.co", "organizationUuid": "", "accountUuid": "uuid-c"}});
        fs::write(profile.join(".claude.json"), login.to_string()).unwrap();
        let _session = hold_launch(&profile);
        assert_eq!(
            run(d.path(), Some(&shell), &["status"]),
            "Live: b@x.co (position 2 of 2)\n  5h 9% · 7d 77% · <1m old\nThis session: c@x.co (not managed by tagteam)\n"
        );
        let v = json_of(d.path(), Some(&shell), &["status", "--json"]);
        assert_eq!(v.get("session"), Some(&Value::Null));
        assert_eq!(
            run(d.path(), Some(&shell), &["list"]),
            QUIET,
            "no row is this session's"
        );
    }

    #[test]
    fn outside_a_run_shell_a_session_shows_without_this_and_status_names_none() {
        let d = tempfile::tempdir().unwrap();
        let (a, _b) = read_now(d.path());
        let (profile, _shell) = cc_profile(d.path(), &a);
        let _session = hold_launch(&profile);
        assert_eq!(
            run(d.path(), None, &["list"]),
            concat!(
                "    #    ACCOUNT  5H    7D    SPEND  AGE\n",
                "    1 ▶  a@x.co     9%   77%  —      <1m\n",
                " *  2    b@x.co     9%   77%  —      <1m\n",
            )
        );
        assert_eq!(
            run(d.path(), None, &["status"]),
            "Live: b@x.co (position 2 of 2)\n  5h 9% · 7d 77% · <1m old\n"
        );
        let v = json_of(d.path(), None, &["status", "--json"]);
        assert!(v.get("session").is_none(), "{v}");
    }
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam --lib render`
Expected: a compile error. `list_human` takes 4 arguments but 5 were supplied, `status_json`
takes 3 and `status_human` takes 3, and the three new tests pass more.

Run: `cargo test -p tagteam --features test-support --test run_shell_cli sessions_in_list_and_status`
Expected: the binary tests compile (the CLI helpers are complete) and four fail:
- `list_in_a_run_shell_…`: the table has no `▶` column and no `this`, so it is `QUIET` with
  `this` missing;
- `status_in_a_run_shell_…`: no `This session:` line;
- `a_run_shell_of_an_account_tagteam_does_not_manage_says_so`: no `session` key;
- `outside_a_run_shell_…`: no `▶`.

- [ ] **Step 3: Implement**

In `crates/tagteam/src/render.rs`, change the imports to:

```rust
use serde_json::{Value, json};
use tagteam_cc::usage::format_iso8601;
use tagteam_core::{AccountId, Pace, ProviderId, Window, WindowKind};
use tagteam_engine::store::AccountRow;
use tagteam_engine::switch::SwitchOutcome;
use tagteam_engine::views::{
    AccountView, NO_DATA, ProviderAccounts, ShellAccount, StatusView, UsageStatus, UsageView,
};
use tagteam_provider::SecretStore;
use unicode_width::UnicodeWidthStr;
```

Add below `SPEND_HEAD`:

```rust
/// §13.1's marker of a session-owned account, in a one-column field after the position
/// (Decision 11).
const IN_SESSION: &str = "▶";
/// The trailing note on the run shell's own row: §13.1's `▶ this`, kept with the notes so the
/// columns stay aligned.
const THIS: &str = "this";
```

In `row_json`, replace the closing lines:

```rust
    if let Some(e) = r.login_expires_at {
        o["loginExpiresAt"] = json!(e);
    }
    o
}
```

with:

```rust
    if let Some(e) = r.login_expires_at {
        o["loginExpiresAt"] = json!(e);
    }
    if v.in_session {
        o["inSession"] = json!(true);
    }
    o
}
```

Replace `Row`, `row`, `table` and `list_human` with:

```rust
/// One account's line before alignment: its cells (the window columns and AGE) when it has a
/// reading, and the notes that follow: its status in words, its kind, `disabled`, `this`.
struct Row {
    marker: char,
    position: u32,
    /// Session-owned (§12.5): `▶` in the session column, which the table has only then.
    session: bool,
    account: String,
    cells: Option<Vec<Cell>>,
    notes: Vec<String>,
}

/// `this`: the row is the run shell's own account (§13.1).
fn row(v: &AccountView, cols: &[Column], this: bool, now_s: i64, color: bool) -> Row {
    let u = &v.usage;
    let mut notes = Vec::new();
    let cells = match u.windows.as_deref() {
        Some(ws) if !ws.is_empty() => {
            let mut cells: Vec<Cell> = cols
                .iter()
                .map(|c| {
                    ws.iter()
                        .find(|(w, _)| c.key.as_deref() == Some(w.key.as_str()))
                        .map_or_else(|| c.missing(), |(w, p)| window_cell(w, p, now_s, color))
                })
                .collect();
            cells.push(Cell::text(
                u.age_s.map_or_else(|| MISSING.to_owned(), duration),
            ));
            if u.status != UsageStatus::Ok {
                notes.push(words(u, now_s));
            }
            Some(cells)
        }
        _ => {
            notes.push(words(u, now_s));
            None
        }
    };
    if let Some(kind) = v.kind.display {
        if !notes.iter().any(|n| n == kind) {
            notes.push(kind.to_owned());
        }
    }
    if v.row.disabled {
        notes.push("disabled".into());
    }
    if this {
        notes.push(THIS.into());
    }
    let account = match &v.row.org_name {
        Some(org) => format!("{} [{org}]", name(&v.row)),
        None => name(&v.row),
    };
    Row {
        marker: if v.active { '*' } else { ' ' },
        position: v.row.position,
        session: v.in_session,
        account,
        cells,
        notes,
    }
}

/// §13.1's table for one provider's accounts. `this` is the run shell's own account (§12.8).
fn table(accounts: &[AccountView], this: Option<&AccountId>, now_s: i64, color: bool) -> String {
    let cols = columns(accounts);
    let rows: Vec<Row> = accounts
        .iter()
        .map(|v| row(v, &cols, this == Some(&v.row.id), now_s, color))
        .collect();
    // Decision 11: the session column exists only while some row is session-owned, so every
    // other table keeps today's layout byte for byte.
    let sessions = rows.iter().any(|r| r.session);
    let account_w = rows
        .iter()
        .map(|r| width(&r.account))
        .fold(width("ACCOUNT"), usize::max);
    let heads: Vec<&str> = cols
        .iter()
        .map(|c| c.head.as_str())
        .chain((!cols.is_empty()).then_some("AGE"))
        .collect();
    let widths: Vec<usize> = heads
        .iter()
        .enumerate()
        .map(|(i, head)| {
            rows.iter()
                .filter_map(|r| r.cells.as_ref())
                .map(|cells| cells[i].width())
                .fold(width(head), usize::max)
        })
        .collect();
    let gap = if sessions { "    " } else { "  " };
    let mut header = format!("    #{gap}{}", pad("ACCOUNT", account_w));
    for (head, w) in heads.iter().zip(&widths) {
        header.push_str("  ");
        header.push_str(&pad(head, *w));
    }
    let mut out = format!("{}\n", header.trim_end());
    for r in &rows {
        let session = match (sessions, r.session) {
            (false, _) => String::new(),
            (true, true) => format!(" {IN_SESSION}"),
            (true, false) => "  ".to_owned(),
        };
        let mut line = format!(
            " {} {:>2}{session}  {}",
            r.marker,
            r.position,
            pad(&r.account, account_w)
        );
        for (c, w) in r.cells.iter().flatten().zip(&widths) {
            line.push_str("  ");
            line.push_str(&c.shown);
            line.push_str(&" ".repeat(w.saturating_sub(c.width())));
        }
        for n in &r.notes {
            line.push_str("  ");
            line.push_str(n);
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

/// §13.1's `list`: one table per provider that has accounts, headed by its name when there
/// is more than one. `this` is the run shell's own account (§12.8), noted `this`; `now_s`
/// measures the countdowns; `color` colours percentages.
pub fn list_human(
    lists: &[ProviderAccounts],
    this: Option<&AccountId>,
    display_names: &dyn Fn(&str) -> String,
    now_s: i64,
    color: bool,
) -> String {
    if lists.iter().all(|l| l.accounts.is_empty()) {
        return NO_ACCOUNTS.into();
    }
    let shown: Vec<&ProviderAccounts> = lists.iter().filter(|l| !l.accounts.is_empty()).collect();
    let mut s = String::new();
    for l in &shown {
        if shown.len() > 1 {
            s.push_str(&format!("{}\n", display_names(l.provider.as_str())));
        }
        s.push_str(&table(&l.accounts, this, now_s, color));
    }
    s
}
```

Replace `status_json` with:

```rust
/// §13.2. Every shape names `provider` (the one the command ran against) at the top level, and
/// again in `active` wherever there is one: a managed row carries it already. In a run shell
/// (§12.8) each shape also names the session's account, last: `session: {number, position,
/// id, email}`, or `null` for an account tagteam does not manage. Outside one there is no
/// `session` key at all.
pub fn status_json(
    s: &StatusView,
    session: &ShellAccount,
    provider: &str,
    usage: RenderUsage<'_>,
) -> Value {
    let mut v = match s {
        StatusView::NoLogin => {
            json!({"schemaVersion": 1, "provider": provider, "active": null})
        }
        StatusView::Unmanaged { email } => json!({
            "schemaVersion": 1,
            "provider": provider,
            "active": {"email": email, "provider": provider, "managed": false},
        }),
        StatusView::Managed { account, total } => {
            let mut row = row_json(account, usage);
            row["managed"] = json!(true);
            json!({"schemaVersion": 1, "provider": provider, "active": row, "totalManagedAccounts": total})
        }
    };
    match session {
        ShellAccount::NotInShell => {}
        ShellAccount::Managed(row) => {
            v["session"] = json!({
                "number": row.position,
                "position": row.position,
                "id": row.id.as_str(),
                "email": email(row),
            });
        }
        ShellAccount::Unmanaged => v["session"] = Value::Null,
    }
    v
}
```

Replace `status_human` with:

```rust
/// `status` (§13.1): the live login and its usage, and in a run shell (§12.8) the session's own
/// account on a last line. `session_login` names an unmanaged session's login as its profile
/// holds it; it is read only for `ShellAccount::Unmanaged`.
pub fn status_human(
    s: &StatusView,
    session: &ShellAccount,
    session_login: Option<&str>,
    now_s: i64,
    color: bool,
) -> String {
    let mut out = match s {
        StatusView::NoLogin => "No live login.\n".to_owned(),
        StatusView::Unmanaged { email } => format!("Live: {email} (not managed by tagteam)\n"),
        StatusView::Managed { account, total } => {
            let marker = if account.row.quarantine_reason.is_some() {
                ", relogin required"
            } else {
                ""
            };
            let mut out = format!(
                "Live: {} (position {} of {total}){marker}\n",
                name(&account.row),
                account.row.position
            );
            if let Some(line) = usage_line(&account.usage, now_s, color) {
                out.push_str(&format!("  {line}\n"));
            }
            out
        }
    };
    match session {
        ShellAccount::NotInShell => {}
        ShellAccount::Managed(row) => out.push_str(&format!(
            "This session: {} (position {})\n",
            name(row),
            row.position
        )),
        ShellAccount::Unmanaged => match session_login {
            Some(login) => {
                out.push_str(&format!("This session: {login} (not managed by tagteam)\n"))
            }
            None => out.push_str("This session: not managed by tagteam\n"),
        },
    }
    out
}
```

These existing tests in `render.rs`'s `mod tests` break only by arity. Their expected values do
not change. Edit the calls as follows:
- `list_human`: insert `None` as the second argument in
  `the_list_is_a_table_of_windows_countdowns_pace_and_age`,
  `a_row_says_in_words_what_keeps_its_reading_from_being_current`,
  `every_status_says_when_it_is_retried_while_that_is_ahead`,
  `the_spend_amount_is_coloured_by_its_severity_in_list_and_status` (the `table` closure),
  `wide_characters_take_two_columns_so_the_table_still_lines_up`,
  `percentages_are_coloured_by_severity_only_when_asked` (both calls) and
  `a_quarantined_account_is_marked_for_a_new_login`. For example,
  `list_human(&one(rows), &names, NOW, false)` becomes
  `list_human(&one(rows), None, &names, NOW, false)`.
- `status_human`: insert `&ShellAccount::NotInShell, None` after the first argument in
  `the_spend_amount_is_coloured_by_its_severity_in_list_and_status` (the `status` closure),
  `status_shows_the_reading_or_the_trouble_on_a_second_line` (all four calls) and
  `a_quarantined_account_is_marked_for_a_new_login`. For example,
  `status_human(&StatusView::NoLogin, NOW, false)` becomes
  `status_human(&StatusView::NoLogin, &ShellAccount::NotInShell, None, NOW, false)`.

In `crates/tagteam/src/app.rs`, change two imports:

```rust
use tagteam_engine::views::{AccountView, ShellAccount, StatusView};
use tagteam_provider::{Clock, Env, Keychain, LockState, RunShell, SystemClock};
```

(If Task 8 already imports `RunShell` into `app.rs`, keep one import.) In `App::dispatch`,
replace the `Command::List` and `Command::Status` arms with:

```rust
            Command::List => {
                // §8.3: every listed account is offered to the collector, which fetches only
                // those that are due; the views are then read with whatever it recorded.
                let listed = self.engine.accounts(self.provider_flag.as_ref())?;
                let ids: Vec<AccountId> = listed
                    .iter()
                    .flat_map(|l| l.accounts.iter().map(|v| v.row.id.clone()))
                    .collect();
                self.collect(ids);
                let lists = self.engine.accounts(self.provider_flag.as_ref())?;
                let this = self.this_account();
                let (now_s, color) = (self.now_s(), self.color());
                let engine = &self.engine;
                let names = |id: &str| {
                    engine
                        .provider(&ProviderId::new(id))
                        .map_or_else(|_| id.to_owned(), |p| p.display_name().to_owned())
                };
                let human = render::list_human(&lists, this.as_ref(), &names, now_s, color);
                let json = render::list_json(&lists, &self.provider(), &render_usage(engine));
                self.print(&human, json);
            }
            Command::Status => {
                let provider = self.provider();
                // §8.3: `status` collects the live account only.
                if let StatusView::Managed { account, .. } = self.engine.status(&provider)? {
                    self.collect(vec![account.row.id]);
                }
                let s = self.engine.status(&provider)?;
                let session = self.session_account(&provider)?;
                let login = match session {
                    ShellAccount::Unmanaged => self.session_login(),
                    ShellAccount::NotInShell | ShellAccount::Managed(_) => None,
                };
                let (now_s, color) = (self.now_s(), self.color());
                let human = render::status_human(&s, &session, login.as_deref(), now_s, color);
                let json = render::status_json(
                    &s,
                    &session,
                    provider.as_str(),
                    &render_usage(&self.engine),
                );
                self.print(&human, json);
            }
```

and add to `impl App<'_, '_>`, after `fn provider`:

```rust
    /// The run shell's own account id, as its marker names it (§13.1's `▶ this`).
    fn this_account(&self) -> Option<AccountId> {
        match self.engine.run_shell() {
            RunShell::Inside { marker, .. } => Some(marker.account_id.clone()),
            RunShell::Outside | RunShell::Unreadable { .. } => None,
        }
    }

    /// §12.8: the run shell's account, for a `status` of the provider whose run shell this is.
    /// Another provider's status has no session to name.
    fn session_account(&self, provider: &ProviderId) -> Result<ShellAccount, EngineError> {
        match self.engine.run_shell() {
            RunShell::Inside { marker, .. } if &marker.provider == provider => {
                self.engine.shell_account()
            }
            _ => Ok(ShellAccount::NotInShell),
        }
    }

    /// The login the run shell's profile holds, as its provider reads it for the marker's
    /// recorded spelling (§12.2): its email, else its label. Only `status`'s text asks, for an
    /// account tagteam does not manage; §13.2's JSON has `session: null`, and `statusline`
    /// never parses a profile (§13.5).
    fn session_login(&self) -> Option<String> {
        let RunShell::Inside { marker, .. } = self.engine.run_shell() else {
            return None;
        };
        let p = self.engine.provider(&marker.provider).ok()?;
        p.profile_identity(self.engine.env(), &marker.config_dir)
            .present()
            .map(|i| i.email.unwrap_or(i.label))
    }
```

`RunShell::Unreadable` never reaches `dispatch`, since Task 8's `app::run` refuses it first; the
arms above still name it, so the match stays exhaustive without a wildcard.

- [ ] **Step 4: Run them and see them pass**, then the crate's whole suite

Run: `cargo test -p tagteam --lib render`
Expected: PASS, including the five new tests and every pinned table unchanged.

Run: `cargo test -p tagteam --features test-support --test run_shell_cli sessions_in_list_and_status`
Expected: PASS, 4 tests.

Run: `cargo test -p tagteam --features test-support`
Expected: PASS. `tests/app.rs`'s `list --json` and `status --json` pins and `tests/usage_cli.rs`'s
`TABLE` are unchanged: outside a run shell, with no session, nothing is added.

- [ ] **Step 5: Format and lint**

```
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam/src/render.rs crates/tagteam/src/app.rs crates/tagteam/tests/common/mod.rs crates/tagteam/tests/run_shell_cli.rs
git commit -m "Show session-owned accounts in list and status, and the run shell's own account"
```

---

### Task 15: `statusline`: provider order and the run shell's account

§13.5: "the command resolves the provider from `--provider`, else from the environment it runs
in (a `CLAUDE_CONFIG_DIR` or a CC-invoked process means Claude Code), else `default_provider`",
and in a run shell "the session's account, named by the profile marker … the profile's
`.claude.json` is never parsed. A marker whose account is not managed prints nothing." §12.8:
under an unreadable marker, "`statusline` prints nothing". Decision 13 fixes the order:
`--provider`, the run shell's marker, a provider that says it started the process, then
`default_provider`.

Two cycles: the engine's `statusline` in a run shell (cycle A), and the CLI's fast path, its
provider order and the timing test (cycle B).

**Readings of the spec this task commits to:**
- **The marker's account applies to the marker's provider.** `Engine::statusline(p)` takes the
  marker's account only when `p` is the marker's provider. A `statusline --provider X` for
  another provider in that shell reads X's live login, in the outer home, as anywhere else.
- **An unreadable marker shows nothing for every provider,** and exits 0: the outer home is
  unknown (§12.8). `--print-config` is not a status line and is unaffected.
- **The session's account view is not `active`, and not `in_session`.** It is the session's
  login, not the default home's. The line never computes session state (Decision 17):
  `Engine::statusline` passes `in_session: false` to `account_view_with`, so the status bar
  reads no profile directory beyond the marker file, whatever runs in the profile.
- **`invoked_by` sees the process's own environment,** before the outer home is applied. In a run
  shell the marker decides first, so that only matters outside one.
- **The provider is resolved inside `statusline::engine`, through `statusline::resolve_provider`,**
  in Decision 13's order. The CLI contract (Task 8) is: `Context::from_process` captures
  `app::session_vars(&build_registry(&ctx))` (every registered provider's `session_dir_var()`
  plus `CLAUDECODE`) into `ctx.env.vars` at the process boundary, never in `app::run`;
  `app::locate(env, registry) -> (RunShell, Env)` only detects and never captures. The
  statusline branch of `app::run` runs before `locate`, and `statusline::engine` calls
  `detect_run_shell` itself, over its own walled registry. So the third step of Decision 13
  sees `CLAUDECODE` in the binary because `from_process` captured it, and in an in-process test
  only the `vars` that test puts in its hand-built `Env` (§15.1).
- **Detection runs over the walled registry,** whose Claude Code has the refusing `NoKeychain`.
  Detection reads the marker file only, and the line's view reads the store only: no Keychain
  item, no network, no session state (Decision 17).
- **The statusline engine's process probe is `SystemProcessProbe` only because `EngineConfig`
  requires the field.** The line never judges a session record, so the probe is never asked.
- **The timing test still holds a reservation,** as a real session does. Since the line does
  not compute session state, the reservation no longer puts that cost inside the 10 ms budget.
  It keeps the test honest in two ways: the run shell it times is one with a live session, and a
  later change that made `statusline` look at the profile's session state would be measured on
  its costliest path (a held `flock`, not a missing directory).

**Files:**
- Modify: `crates/tagteam-engine/src/views.rs` (`Engine::statusline`, today 611–633).
- Modify: `crates/tagteam-engine/tests/common/mod.rs` (`FakeFx::engine_located`).
- Modify: `crates/tagteam-engine/tests/session_views.rs` (four tests, imports).
- Modify: `crates/tagteam/src/statusline.rs`:
  - the imports (4–20, and Task 8's `use crate::app::{Context, locate};`);
  - `engine` (195–218), with a new `resolve_provider`;
  - the tests `the_settings_are_those_of_the_provider_the_command_resolves` (598–626),
    `the_engine_reaches_neither_the_keychain_nor_the_network` (628–653) and Task 8's
    `an_unreadable_marker_is_found_without_the_keychain`, plus new tests.
- Modify: `crates/tagteam/src/app.rs` (`run_statusline`, today 361–414).
- Modify: `crates/tagteam/Cargo.toml` (`[dev-dependencies]`, 25–29: `tagteam-fake`).
- Modify: `crates/tagteam/tests/run_shell_cli.rs` (one more module).
- Modify: `crates/tagteam/tests/perf.rs` (`nothing_due` 89–98, `timings` 100–127, the three
  existing tests, one new test, imports 9–20).

**Interfaces:**
- Consumes:
  - Task 14: `ShellAccount`, `Engine::shell_account`, `Engine::account_view_with(&self, row:
    AccountRow, active: bool, with_pace: bool, in_session: bool) -> AccountView` (the statusline
    passes `in_session: false`), and, in `session_views.rs`, the file-private helpers
    `listed(engine, id)` and `inside(fx, profile)`. CLI test helpers `cc_profile` and
    `hold_launch` (`crates/tagteam/tests/common/mod.rs`).
  - Task 8: `Engine::run_shell`, `EngineConfig.{process, run_shell}`, `detect_run_shell`, the
    binary's `run-shell-unreadable` refusal of every command but `statusline`, and the CLI
    contract: `Context::from_process` calls `ctx.env.capture_vars(&session_vars(&build_registry(&ctx)))`
    with `app::session_vars(registry: &ProviderRegistry) -> Vec<&'static str>`, and
    `app::locate(env: Env, registry: &ProviderRegistry) -> (RunShell, Env)` only detects.
    Engine fixtures: `Fx::{make_profile, shell_env}` and `Fx.engine`.
  - Task 9: `Fx.process: Arc<FakeProcessProbe>` (`FakeFx::new` passes it to its engine),
    `Fx::hold_reservation(&self, profile: &Path) -> FlockGuard`.
  - Task 13: `Fx::make_profile_for(&self, p: &dyn Provider, id: &AccountId) -> PathBuf`.
  - Task 7: `Provider::invoked_by`, `RunShell`, `ProfileMarker`, `MARKER_FILE`.
  - Task 6: `SystemProcessProbe`.
- Produces:
  - `Engine::statusline` in a run shell: the marker's account by id, with `in_session` left
    `false`. It answers `NoLogin` for an unmanaged account and under an unreadable marker.
  - `statusline::resolve_provider(flag: Option<&str>, shell: &RunShell, registry:
    &ProviderRegistry, env: &Env, default: &ProviderId) -> ProviderId` (Decision 13).
  - `statusline::engine(ctx: Context, flag: Option<&str>) -> (Engine, ProviderId,
    Arc<LazyHttp>, Arc<NoKeychain>)`: detects the run shell (`detect_run_shell`, over its walled
    registry) and resolves the provider with `resolve_provider`. It does not call `app::locate`.
  - Engine test helper `FakeFx::engine_located(&self, env: Env) -> Engine`.

**Spec:**
- §13.5: the provider comes from `--provider`, else the environment, else the default. In a run
  shell the line is the marker's account, from one small file, with no `.claude.json` parse. An
  unmanaged account prints nothing. There is no network and no Keychain access, and the `Http`
  adapter is never built.
- §12.8: an unreadable marker: `statusline` prints nothing, while every other command refuses,
  naming the file.
- §1.1: `statusline` within 10 ms p95.
- Decision 13: the order is `--provider`, then the marker, then `Provider::invoked_by`, then
  `default_provider`.
- Decision 17: the statusline's view never computes `in_session`.
- §15.1: an in-process test carries exactly the `vars` it sets; the binary's `vars` are captured
  once, by `Context::from_process` (Task 8).
- Review Focus 4: inside a run shell whose account was removed meanwhile, or whose marker is
  corrupt, `statusline` prints nothing, exits 0 and touches no Keychain item.

#### Cycle A: the engine's line in a run shell

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam-engine/tests/common/mod.rs`, add to `impl FakeFx` (its imports already hold
`detect_run_shell`, `ProviderRegistry`, `KeychainVault`, `Vault`, `Settings`, `ProviderId` and
`CLAUDE_CODE`):

```rust
    /// An engine for a process whose environment is `env`, over the registry `FakeFx::new` builds
    /// (Claude Code, then FakeAgent), located as the CLI locates one: `detect_run_shell` decides
    /// the run shell and the environment the engine runs on (§12.8). Everything else is as
    /// `FakeFx::new` builds its engine, including the fixture's process probe.
    pub fn engine_located(&self, env: Env) -> Engine {
        let registry = ProviderRegistry::new()
            .with(self.fx.cc.clone())
            .with(self.fake.clone());
        let (run_shell, env) = detect_run_shell(&env, &registry);
        Engine::new(EngineConfig {
            env,
            registry,
            vault: Vault::new(Box::new(KeychainVault::new(self.fx.kc.clone()))),
            oracle: self.fx.oracle.clone(),
            clock: self.fx.clock.clone(),
            default_provider: ProviderId::new(CLAUDE_CODE),
            http: self.fx.http.clone(),
            settings: Settings::default(),
            process: self.fx.process.clone(),
            run_shell,
        })
    }
```

In `crates/tagteam-engine/tests/session_views.rs`, change the imports to:

```rust
use std::fs;
use std::path::Path;

use common::{FakeFx, Fx, LSTART};
use serde_json::json;
use tagteam_core::AccountId;
use tagteam_engine::Engine;
use tagteam_engine::views::{AccountView, ShellAccount, StatusView, StatuslineView};
use tagteam_provider::{FakeProcess, MARKER_FILE, Provider, parse_lstart};
```

and append:

```rust
#[test]
fn statusline_in_a_run_shell_is_the_markers_account_and_reads_no_live_identity() {
    // §13.5: the marker names the account; neither the profile's `.claude.json` nor the
    // default home's is parsed, and the live-identity cache is neither read nor written.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    let profile = fx.make_profile(&a);
    // CC's own file in the profile, garbled: parsed, it would show nothing.
    fs::write(profile.join(".claude.json"), b"{\"oauthAccount\": ").unwrap();
    let _launch = fx.hold_reservation(&profile);

    match inside(&fx, &profile).statusline(&fx.provider()).unwrap() {
        StatuslineView::Managed { account } => assert_eq!(
            (account.row.id, account.active, account.in_session),
            (a, false, false),
            "the session's account, not the live login, and no session state computed for it \
             (Decision 17) even with a reservation held"
        ),
        other => panic!("{other:?}"),
    }
    let cache = || {
        fx.engine
            .store()
            .unwrap()
            .live_identity_cache(&fx.provider())
            .unwrap()
    };
    assert!(cache().is_none(), "nothing went through the live-identity cache");

    match fx.engine.statusline(&fx.provider()).unwrap() {
        StatuslineView::Managed { account } => assert_eq!(account.row.id, b),
        other => panic!("{other:?}"),
    }
    assert!(cache().is_some(), "outside, the live login goes through it");
}

#[test]
fn statusline_in_a_run_shell_of_an_account_tagteam_does_not_manage_shows_nothing() {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a"); // live: never shown in the session's place
    let profile = fx.make_profile_for(fx.cc.as_ref(), &AccountId::from_string("0192-gone"));
    assert!(matches!(
        inside(&fx, &profile).statusline(&fx.provider()).unwrap(),
        StatuslineView::NoLogin
    ));
}

#[test]
fn statusline_under_an_unreadable_marker_shows_nothing_and_parses_no_profile() {
    // §12.8: the outer home is unknown. The environment still names the profile, whose own
    // `.claude.json` names a: a parse of it would show a's line.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let profile = fx.make_profile(&a);
    let login = json!({"oauthAccount": Fx::oauth_account("a@x.co")});
    fs::write(profile.join(".claude.json"), login.to_string()).unwrap();
    fs::write(profile.join(MARKER_FILE), b"[").unwrap();
    assert!(matches!(
        inside(&fx, &profile).statusline(&fx.provider()).unwrap(),
        StatuslineView::NoLogin
    ));
}

#[test]
fn statusline_for_another_provider_in_a_run_shell_is_that_providers_live_login() {
    // The marker names Claude Code's session: FakeAgent's line is still its own live login,
    // read in the outer home.
    let ffx = FakeFx::new();
    let a = ffx.fx.add("a@x.co", "rt-a");
    ffx.fx.add("b@x.co", "rt-b");
    let alice = ffx.fake_add("alice", "tok-a", "renew-a");
    let profile = ffx.fx.make_profile(&a);
    let engine = ffx.engine_located(ffx.fx.shell_env(&profile));
    match engine.statusline(&ffx.fake_provider()).unwrap() {
        StatuslineView::Managed { account } => {
            assert_eq!((account.row.id, account.active), (alice, true))
        }
        other => panic!("{other:?}"),
    }
    match engine.statusline(&ffx.fx.provider()).unwrap() {
        StatuslineView::Managed { account } => assert_eq!(account.row.id, a),
        other => panic!("{other:?}"),
    }
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam-engine --test session_views statusline`

Expected: three FAIL and one passes.
- `statusline_in_a_run_shell_is_the_markers_account_…` fails: the effective environment is the
  outer home, so the live path answers `Managed` b, and its first `assert_eq!` sees b.
- `…_does_not_manage_shows_nothing` fails: `Managed` a (the live login), not `NoLogin`.
- `…_unreadable_marker_…` fails: `Managed` a, parsed from the profile's `.claude.json`.
- `…_another_provider_…` already passes (FakeAgent's live path) and must keep passing.

The first test's `in_session == false` assertion, with a reservation held, is Decision 17's pin:
it fails the day the view starts computing session state.

- [ ] **Step 3: Implement**

In `crates/tagteam-engine/src/views.rs`, replace `Engine::statusline` with:

```rust
    /// §13.5: the line's account and, when tagteam manages it, its usage. No network, no
    /// Keychain, and the store is never created.
    /// - In a run shell for `provider` (§12.8), the account the marker names, by id
    ///   (`shell_account`): no `.claude.json` is parsed, the profile's or the default home's,
    ///   and one tagteam does not manage shows nothing.
    /// - Under an unreadable marker, nothing: the outer home is unknown (§12.8).
    /// - Otherwise the live login, from `live_identity_cache` while
    ///   `Provider::live_identity_source`'s mtime and size are unchanged, re-parsed only when
    ///   they change. A missing, unreadable or garbled source is `NoLogin`, never an error.
    pub fn statusline(&self, provider: &ProviderId) -> Result<StatuslineView, EngineError> {
        let p = self.provider(provider)?;
        match self.run_shell() {
            RunShell::Unreadable { .. } => return Ok(StatuslineView::NoLogin),
            RunShell::Inside { marker, .. } if &marker.provider == provider => {
                return Ok(match self.shell_account()? {
                    // The session's login, not the default home's: not `active`. The line
                    // shows no pace, so none is computed, and it never asks whether the
                    // account is in a session (Decision 17): the status bar stays away from
                    // profile directories.
                    ShellAccount::Managed(row) => StatuslineView::Managed {
                        account: self.account_view_with(row, false, false, false),
                    },
                    ShellAccount::Unmanaged | ShellAccount::NotInShell => StatuslineView::NoLogin,
                });
            }
            RunShell::Inside { .. } | RunShell::Outside => {}
        }
        let store = self.existing_store()?;
        let Some(login) = self.live_login(p.as_ref(), store.as_deref())? else {
            return Ok(StatuslineView::NoLogin);
        };
        let row = match &store {
            Some(s) => s.find_by_identity_key(provider, &login.key)?,
            None => None,
        };
        Ok(match row {
            // The line shows no pace, so none is computed, and no session state (Decision 17).
            Some(row) => StatuslineView::Managed {
                account: self.account_view_with(row, true, false, false),
            },
            None => StatuslineView::Unmanaged { email: login.label },
        })
    }
```

- [ ] **Step 4: Run them and see them pass**, then the crate's whole suite

Run: `cargo test -p tagteam-engine --test session_views`
Expected: PASS, 11 tests (Task 14's 7 and these 4).

Run: `cargo test -p tagteam-engine --features test-hooks`
Expected: PASS. Outside a run shell the live path is unchanged, so `views_usage.rs`'s statusline
tests keep passing.

- [ ] **Step 5: Format and lint**

```
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-engine/src/views.rs crates/tagteam-engine/tests/common/mod.rs crates/tagteam-engine/tests/session_views.rs
git commit -m "Take a run shell's statusline account from its marker and show nothing under an unreadable one"
```

#### Cycle B: the fast path, its provider order, and the timing test

- [ ] **Step 1: Write the failing tests**

In `crates/tagteam/Cargo.toml`, add to `[dev-dependencies]` (a second provider tells Decision
13's steps apart; `tagteam-engine` already takes it the same way):

```toml
tagteam-fake.workspace = true
```

In `crates/tagteam/src/statusline.rs`'s `mod tests`, change the imports to:

```rust
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use clap::Parser;
    use tagteam_cc::live::Platform;
    use tagteam_cc::{ItemKind, keychain_account, keychain_service};
    use tagteam_core::{AccountId, PollBudget, PollPlan};
    use tagteam_engine::settings::DEFAULT_STATUSLINE_FORMAT;
    use tagteam_engine::store::{Reserve, Store};
    use tagteam_engine::views::{UsageStatus, UsageView};
    use tagteam_fake::{FAKE_AGENT, FakeAgent};
    use tagteam_provider::atomic::ensure_private_dir;
    use tagteam_provider::{
        FakeKeychain, MARKER_FILE, ProfileMarker, Provider, canonical_profile_path, profile_path,
    };

    use super::*;
    use crate::app::{Io, run};
    use crate::cli::Cli;
    use crate::prompt::Prompter;
    use crate::render::testutil::{NOW, OAUTH, fable, read, spend, unread, view};
```

Replace the bodies of the three tests that call `engine` (the third is Task 8's
`an_unreadable_marker_is_found_without_the_keychain`, which keeps its assertions and moves to the
new signature and the `context` helper below):

```rust
    #[test]
    fn an_unreadable_marker_is_found_without_the_keychain() {
        // §12.8: the status bar learns from the file alone that the marker cannot be read, and
        // then prints nothing (`app::run_statusline`).
        let dir = tempfile::tempdir().unwrap();
        let mut env = Env::for_test(dir.path());
        let profile = dir.path().join("profile");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(profile.join(MARKER_FILE), "not json").unwrap();
        env.claude_config_dir = Some(profile.clone().into_os_string());
        let (built, _, http, keychain) = engine(context(env), None);
        assert!(
            matches!(built.run_shell(), RunShell::Unreadable { marker, .. } if *marker == profile.join(MARKER_FILE)),
            "{:?}",
            built.run_shell()
        );
        assert!(!http.is_built());
        assert_eq!(keychain.calls(), 0);
    }

    #[test]
    fn the_settings_are_those_of_the_provider_the_command_resolves() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path());
        std::fs::create_dir_all(env.config_dir()).unwrap();
        std::fs::write(
            env.config_dir().join("config.toml"),
            "[statusline]\nformat = \"{5h}\"\n\n[provider.other.statusline]\nformat = \"{7d}\"\n",
        )
        .unwrap();
        let format_for = |provider: &str| {
            let (built, _, _, _) = engine(context(env.clone()), Some(provider));
            built.settings().statusline_format.clone()
        };
        assert_eq!(format_for(CLAUDE_CODE), "{5h}");
        assert_eq!(format_for("other"), "{7d}");
    }

    #[test]
    fn the_engine_reaches_neither_the_keychain_nor_the_network() {
        // §13.5: the walls count what reaches them, so none of this may.
        let (_dir, env) = managed_home();
        let (built, provider, http, keychain) = engine(context(env), None);
        assert_eq!(shown(&built, &provider), "a · 5h 9% · 7d 77%\n");
        assert!(!http.is_built(), "the HTTP adapter was built");
        assert_eq!(keychain.calls(), 0, "the Keychain was asked");
    }
```

and append:

```rust
    /// A context over `env`, with a Keychain the walled engine never uses. Nothing is captured
    /// from the test process (§15.1): `env.vars` holds exactly what a test put there, and that
    /// is all Decision 13's third step (`Provider::invoked_by`) sees. `Context::from_process`,
    /// which captures the registered providers' variables and `CLAUDECODE` (Task 8), is not
    /// involved.
    fn context(env: Env) -> Context {
        Context {
            env,
            keychain: Arc::new(FakeKeychain::new()),
            platform: Platform::MacOs,
            api_base: None,
            stdout_terminal: false,
            no_color_env: false,
            force_color_env: false,
        }
    }

    /// The line `built` prints for `provider`, without colour.
    fn shown(built: &Engine, provider: &ProviderId) -> String {
        let view = built.statusline(provider).unwrap();
        line(
            &view,
            &built.settings().statusline_format,
            built.now_ms() / 1000,
            false,
        )
    }

    /// The environment of a run shell for `id`'s profile: the profile and its marker as `run`
    /// writes them (§12.2), and `CLAUDE_CONFIG_DIR` naming it by its exported spelling.
    fn in_profile(env: &Env, id: &AccountId) -> Env {
        let profile = profile_path(env, id);
        ensure_private_dir(&profile).unwrap();
        let cc = ClaudeCode::new(Arc::new(FakeKeychain::new()), Platform::MacOs);
        let spelling = cc.profile_spelling(&canonical_profile_path(&profile).unwrap());
        ProfileMarker {
            provider: ProviderId::new(CLAUDE_CODE),
            account_id: id.clone(),
            config_dir: spelling.clone(),
            outer: cc.outer_home(env),
        }
        .write(&profile)
        .unwrap();
        let mut inside = env.clone();
        inside.claude_config_dir = Some(spelling.into());
        inside
    }

    /// `managed_home` with the live login moved to a stranger, so that only a marker can name
    /// `a@x.co`; and that account's id.
    fn session_home() -> (tempfile::TempDir, Env, AccountId) {
        let (dir, env) = managed_home();
        let stranger = json!({"oauthAccount": {"emailAddress": "s@x.co", "organizationUuid": "", "accountUuid": "uuid-s"}});
        std::fs::write(env.home.join(".claude.json"), stranger.to_string()).unwrap();
        let store = Store::open_existing(&env.data_dir().join("tagteam.db"))
            .unwrap()
            .unwrap();
        let id = store
            .accounts(&ProviderId::new(CLAUDE_CODE))
            .unwrap()
            .remove(0)
            .id;
        (dir, env, id)
    }

    #[test]
    fn in_a_run_shell_the_line_is_the_markers_account_and_reaches_nothing_else() {
        let (_dir, outside, a) = session_home();
        let (built, provider, _, _) = engine(context(outside.clone()), None);
        assert_eq!(shown(&built, &provider), "s@x.co\n", "outside: the live login");

        let (built, provider, http, keychain) = engine(context(in_profile(&outside, &a)), None);
        assert!(matches!(built.run_shell(), RunShell::Inside { .. }));
        assert_eq!(provider.as_str(), CLAUDE_CODE, "the marker's provider");
        assert_eq!(shown(&built, &provider), "a · 5h 9% · 7d 77%\n");
        assert!(!http.is_built(), "the HTTP adapter was built");
        assert_eq!(keychain.calls(), 0, "the Keychain was asked");
    }

    #[test]
    fn a_run_shell_whose_account_is_gone_or_whose_marker_is_corrupt_prints_nothing() {
        // Review Focus 4, at the unit level: never the live login's line, and no wall reached.
        let (_dir, outside, _a) = session_home();
        let gone = in_profile(&outside, &AccountId::from_string("0192-removed"));
        let (built, provider, http, keychain) = engine(context(gone), None);
        assert_eq!(shown(&built, &provider), "");
        assert_eq!((keychain.calls(), http.is_built()), (0, false));

        let corrupt = in_profile(&outside, &AccountId::from_string("0192-corrupt"));
        let marker = PathBuf::from(corrupt.claude_config_dir.clone().unwrap()).join(MARKER_FILE);
        std::fs::write(&marker, b"{\"format\": \"tagteam-profile\", ").unwrap();
        let (built, provider, http, keychain) = engine(context(corrupt.clone()), None);
        assert!(matches!(built.run_shell(), RunShell::Unreadable { .. }));
        assert_eq!(shown(&built, &provider), "");
        assert_eq!((keychain.calls(), http.is_built()), (0, false));

        // Through the command itself: exit 0, and nothing on either stream.
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run(
            Cli::try_parse_from(["tagteam", "statusline"]).unwrap(),
            context(corrupt),
            &mut Io {
                out: &mut out,
                err: &mut err,
                prompter: &mut NoPrompts,
            },
        );
        assert_eq!((code, out.as_slice(), err.as_slice()), (0, &b""[..], &b""[..]));
    }

    #[test]
    fn the_provider_is_the_flag_then_the_marker_then_the_invoking_agent_then_the_default() {
        // Decision 13 (§13.5). Claude Code alone has the capability today, so a second
        // provider, FakeAgent (whose `invoked_by` is always false), tells the steps apart.
        let dir = tempfile::tempdir().unwrap();
        let plain = Env::for_test(dir.path());
        let mut from_cc = plain.clone();
        from_cc.vars.insert("CLAUDECODE".into(), "1".into());
        let registry = ProviderRegistry::new()
            .with(Arc::new(FakeAgent::new()))
            .with(Arc::new(ClaudeCode::new(
                Arc::new(NoKeychain::default()),
                Platform::MacOs,
            )));
        let marker = |provider: &str| RunShell::Inside {
            profile: PathBuf::from("/profile"),
            marker: ProfileMarker {
                provider: ProviderId::new(provider),
                account_id: AccountId::from_string("0192"),
                config_dir: "/profile".into(),
                outer: json!({}),
            },
        };
        let unreadable = RunShell::Unreadable {
            marker: PathBuf::from("/profile").join(MARKER_FILE),
            detail: "torn".into(),
        };
        let (cc, fake) = (ProviderId::new(CLAUDE_CODE), ProviderId::new(FAKE_AGENT));
        let resolve = |flag: Option<&str>, shell: &RunShell, env: &Env, default: &ProviderId| {
            resolve_provider(flag, shell, &registry, env, default)
                .as_str()
                .to_owned()
        };
        assert_eq!(resolve(Some("other"), &marker(FAKE_AGENT), &from_cc, &fake), "other");
        assert_eq!(
            resolve(None, &marker(FAKE_AGENT), &from_cc, &cc),
            FAKE_AGENT,
            "the marker beats the invoking agent"
        );
        assert_eq!(
            resolve(None, &RunShell::Outside, &from_cc, &fake),
            CLAUDE_CODE,
            "the invoking agent beats the default"
        );
        assert_eq!(
            resolve(None, &unreadable, &from_cc, &fake),
            CLAUDE_CODE,
            "an unreadable marker names no provider"
        );
        assert_eq!(
            resolve(None, &RunShell::Outside, &plain, &fake),
            FAKE_AGENT,
            "nothing else: the default"
        );
    }
```

Append to `crates/tagteam/tests/run_shell_cli.rs`:

```rust
/// Task 15: `statusline` inside a run shell (§12.8, §13.5; Review Focus 4).
mod statusline_in_a_run_shell {
    use std::fs;
    use std::path::Path;

    use serde_json::{Value, json};
    use tagteam_core::{AccountId, WindowKind};
    use tagteam_engine::store::Store;
    use tagteam_provider::{Env, MARKER_FILE};

    use crate::common::{
        cc_profile, cmd, hold_launch, now_epoch_s, record_reading, two_fresh_accounts,
        usage_window,
    };

    /// `a@x.co` (position 1) read at 5h 31 % and 7d 12 %, and `b@x.co` (position 2, live) at
    /// 9 % and 77 %, both just now.
    fn read_now(root: &Path) -> (String, String) {
        let (a, b) = two_fresh_accounts(root);
        let now = now_epoch_s();
        for (id, five, seven) in [(&a, 31.0, 12.0), (&b, 9.0, 77.0)] {
            record_reading(
                root,
                id,
                now,
                &[
                    usage_window("5h", "5h", WindowKind::Short, five, None, None),
                    usage_window("7d", "7d", WindowKind::Long, seven, None, None),
                ],
            );
        }
        (a, b)
    }

    /// `tagteam statusline --no-color`, inside the run shell `shell` spells when given.
    fn statusline(root: &Path, shell: Option<&str>) -> assert_cmd::Command {
        let mut c = cmd(root);
        c.args(["statusline", "--no-color"]);
        if let Some(dir) = shell {
            c.env("CLAUDE_CONFIG_DIR", dir);
        }
        c
    }

    #[test]
    fn the_line_is_the_sessions_account_named_by_its_marker() {
        let d = tempfile::tempdir().unwrap();
        let (a, _b) = read_now(d.path());
        statusline(d.path(), None)
            .assert()
            .success()
            .stdout("b · 5h 9% · 7d 77%\n")
            .stderr("");
        let (profile, shell) = cc_profile(d.path(), &a);
        // CC's own file in the profile, naming the default login: a line from it would be b's.
        let login = json!({"oauthAccount": {"emailAddress": "b@x.co", "organizationUuid": ""}});
        fs::write(profile.join(".claude.json"), login.to_string()).unwrap();
        let _session = hold_launch(&profile);
        statusline(d.path(), Some(&shell))
            .assert()
            .success()
            .stdout("a · 5h 31% · 7d 12%\n")
            .stderr("");
    }

    #[test]
    fn a_run_shell_whose_account_was_removed_meanwhile_prints_nothing() {
        let d = tempfile::tempdir().unwrap();
        let (a, _b) = read_now(d.path());
        let (profile, shell) = cc_profile(d.path(), &a);
        let _session = hold_launch(&profile);
        // The row is gone and the profile stays, as a store reset leaves it.
        Store::open_existing(&Env::for_test(d.path()).data_dir().join("tagteam.db"))
            .unwrap()
            .unwrap()
            .delete_account(&AccountId::from_string(a.as_str()))
            .unwrap();
        statusline(d.path(), Some(&shell))
            .assert()
            .success()
            .stdout("")
            .stderr("");
    }

    #[test]
    fn a_corrupt_marker_prints_nothing_while_every_other_command_names_it_and_refuses() {
        let d = tempfile::tempdir().unwrap();
        let (a, _b) = read_now(d.path());
        let (profile, shell) = cc_profile(d.path(), &a);
        fs::write(
            profile.join(MARKER_FILE),
            b"{\"format\": \"tagteam-profile\", \"version\": ",
        )
        .unwrap();
        statusline(d.path(), Some(&shell))
            .assert()
            .success()
            .stdout("")
            .stderr("");
        cmd(d.path())
            .env("CLAUDE_CONFIG_DIR", &shell)
            .arg("status")
            .assert()
            .code(1)
            .stdout("")
            .stderr(predicates::str::contains(MARKER_FILE));
        let out = cmd(d.path())
            .env("CLAUDE_CONFIG_DIR", &shell)
            .args(["status", "--json"])
            .assert()
            .code(1)
            .get_output()
            .stdout
            .clone();
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["error"]["type"], "run-shell-unreadable");
    }
}
```

In `crates/tagteam/tests/perf.rs`, change the `common` import to:

```rust
use common::{
    bloat_claude_json, cc_profile, hold_launch, now_epoch_s, record_history, std_cmd,
    two_fresh_accounts, usage_window,
};
```

replace `nothing_due` and `timings` with:

```rust
/// Two accounts, `b` live, each with 48 hours of readings, the last taken just now, and a
/// `~/.claude.json` of a few hundred KB. Nothing is due for 180 s (§8.3's on-demand rule), far
/// longer than a timing run takes. Returns the two ids, `a` first.
fn nothing_due(root: &Path) -> (String, String) {
    let (a, b) = two_fresh_accounts(root);
    let now = now_epoch_s();
    let windows = windows_at(now);
    for id in [&a, &b] {
        record_history(root, id, now, READINGS, SPACING_S, &windows);
    }
    bloat_claude_json(root, CLAUDE_JSON_BYTES);
    (a, b)
}

/// The wall time of each of `RUNS` runs of `args`, after `WARM_UP` untimed ones, with every
/// endpoint pointed at `base` and `envs` set. `before(n)` runs untimed ahead of run `n`.
fn timings(
    root: &Path,
    base: &str,
    args: &[&str],
    envs: &[(&str, &str)],
    before: &dyn Fn(usize),
) -> Vec<Duration> {
    let run = |n: usize| {
        before(n);
        let started = Instant::now();
        let out = std_cmd(root)
            .env("TAGTEAM_TEST_API_BASE", base)
            .envs(envs.iter().copied())
            .args(args)
            .output()
            .unwrap();
        let took = started.elapsed();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        took
    };
    for n in 0..WARM_UP {
        run(n);
    }
    (0..RUNS).map(|n| run(WARM_UP + n)).collect()
}
```

The three existing timing tests pass `&[]` for `envs`:
- `statusline_p95_is_within_10_ms`: `timings(d.path(), &server.base_url(), &["statusline"], &[], &|_| {})`;
- `statusline_p95_on_a_cache_miss_is_within_10_ms`: `timings(d.path(), &server.base_url(), &["statusline"], &[], &touch)`;
- `list_p95_is_within_50_ms_when_nothing_is_due`: `timings(d.path(), &server.base_url(), &["list"], &[], &|_| {})`.

Append:

```rust
#[test]
#[ignore = "timing: run with --release on an idle machine"]
fn statusline_in_a_run_shell_p95_is_within_10_ms() {
    // §12.8, §13.5: in a run shell the line names the marker's account. The marker is one
    // small file and the 400 KB `~/.claude.json` is never parsed. The line computes no session
    // state (Decision 17), so the reservation held below is not part of the timed cost. It
    // makes the timed run a real session's, and would put a regression that made `statusline`
    // read the profile's session state (a directory listing and a non-blocking `flock`) inside
    // the budget.
    let _one_at_a_time = serial();
    let d = tempfile::tempdir().unwrap();
    let (a, _b) = nothing_due(d.path());
    let (profile, shell) = cc_profile(d.path(), &a);
    let _session = hold_launch(&profile);
    let server = MockServer::start();
    let envs = [("CLAUDE_CONFIG_DIR", shell.as_str())];
    // The timed path is the run-shell one: the line is the session's account, a, not b.
    let out = std_cmd(d.path())
        .env("TAGTEAM_TEST_API_BASE", server.base_url())
        .envs(envs)
        .args(["statusline", "--no-color"])
        .output()
        .unwrap();
    let line = String::from_utf8(out.stdout).unwrap();
    assert!(line.starts_with("a · "), "{line:?}");
    let runs = timings(d.path(), &server.base_url(), &["statusline"], &envs, &|_| {});
    let p = p95(&runs);
    eprintln!("statusline in a run shell p95 {p:?} over {RUNS} runs");
    assert_eq!(server.requests().len(), 0, "statusline sent a request");
    assert!(
        p <= Duration::from_millis(10),
        "statusline in a run shell p95 {p:?} over {RUNS} runs: {runs:?}"
    );
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p tagteam --lib statusline`
Expected: a compile error. `resolve_provider` is not found, `engine` takes `&ProviderId` where
an `Option<&str>` is given, and its result is a 3-tuple where a 4-tuple is bound.

Run: `cargo test -p tagteam --features test-support --test run_shell_cli statusline_in_a_run_shell`
Expected: it depends on Task 8's fast path. If Task 8 handed `statusline::engine` the process's
raw environment (no outer home applied) or `RunShell::Outside`, the profile's `.claude.json`
names b and `the_line_is_the_sessions_account_…` prints `b · 5h 9% · 7d 77%`. Then
`a_run_shell_whose_account_was_removed_…` prints b's line too. If Task 8 already detected the run
shell there, cycle A makes all three pass already, and they must keep passing.

Run: `cargo test --release -p tagteam --features test-support --test perf --no-run`
Expected: a compile error: `timings` takes 4 arguments and `nothing_due` returns `()`.

- [ ] **Step 3: Implement**

In `crates/tagteam/src/statusline.rs`, change the imports to:

```rust
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;
use tagteam_cc::{CcPaths, ClaudeCode};
use tagteam_core::{CLAUDE_CODE, ProviderId, Window, WindowKind};
use tagteam_engine::lazy_http::LazyHttp;
use tagteam_engine::oracle::NoOracle;
use tagteam_engine::registry::ProviderRegistry;
use tagteam_engine::session::detect_run_shell;
use tagteam_engine::settings::{STATUSLINE_MODEL_PREFIX, Settings, is_statusline_placeholder};
use tagteam_engine::vault::{KeychainVault, Vault};
use tagteam_engine::views::{AccountView, StatuslineView};
use tagteam_engine::{Engine, EngineConfig};
use tagteam_provider::{
    Capabilities, Env, Http, Keychain, KeychainError, LockState, NoHttp, Read, ReadError,
    RunShell, SystemClock, SystemProcessProbe,
};
```

and change Task 8's `use crate::app::{Context, locate};` back to `use crate::app::Context;`: the
engine below calls `detect_run_shell` itself, so `locate` is no longer used here (`app::run`
still calls it).

Replace `engine` (and whatever Task 8 left in it) with:

```rust
/// Decision 13 (§13.5): `--provider`; else the run shell's marker provider; else the first
/// registered provider that says it started this process (`Provider::invoked_by`, Claude Code's
/// `CLAUDECODE` or `CLAUDE_CONFIG_DIR`); else `default`. `env` is the process's own
/// environment, before any outer home is applied. An unreadable marker names no provider.
pub(crate) fn resolve_provider(
    flag: Option<&str>,
    shell: &RunShell,
    registry: &ProviderRegistry,
    env: &Env,
    default: &ProviderId,
) -> ProviderId {
    if let Some(p) = flag {
        return ProviderId::new(p);
    }
    if let RunShell::Inside { marker, .. } = shell {
        return marker.provider.clone();
    }
    registry
        .all()
        .iter()
        .find(|p| p.invoked_by(env))
        .map_or_else(|| default.clone(), |p| p.id())
}

/// The engine the fast path runs on, built with walls rather than trust (§13.5): a Keychain
/// that refuses every call, no profile oracle, and a lazy HTTP port whose adapter could send
/// nothing even if it were built. The run shell is detected over that walled registry (§12.8),
/// which reads the marker file alone, and the provider is resolved in Decision 13's order by
/// `resolve_provider`. This is not `app::locate`, which serves the other commands over the full
/// registry; the variables Decision 13's third step reads were captured into `ctx.env` by
/// `Context::from_process`. The settings are that provider's, and their warnings are dropped,
/// since a status bar has nowhere to show them. Returns the engine, the provider, and the walls,
/// so a test can prove nothing reached them.
pub(crate) fn engine(
    ctx: Context,
    flag: Option<&str>,
) -> (Engine, ProviderId, Arc<LazyHttp>, Arc<NoKeychain>) {
    let keychain = Arc::new(NoKeychain::default());
    let http = Arc::new(LazyHttp::new(|| Arc::new(NoHttp) as Arc<dyn Http>));
    let registry =
        ProviderRegistry::new().with(Arc::new(ClaudeCode::new(keychain.clone(), ctx.platform)));
    let (run_shell, env) = detect_run_shell(&ctx.env, &registry);
    let default = ProviderId::new(CLAUDE_CODE);
    let provider = resolve_provider(flag, &run_shell, &registry, &ctx.env, &default);
    let (settings, _warnings) = Settings::load(&env, &provider);
    let engine = Engine::new(EngineConfig {
        registry,
        vault: Vault::new(Box::new(KeychainVault::new(keychain.clone()))),
        oracle: Arc::new(NoOracle),
        clock: Arc::new(SystemClock),
        http: http.clone(),
        default_provider: default,
        settings,
        env,
        // `EngineConfig` requires a probe. The line never judges a session record (Decision 17),
        // so this one is never asked.
        process: Arc::new(SystemProcessProbe),
        run_shell,
    });
    (engine, provider, http, keychain)
}
```

In `crates/tagteam/src/app.rs`, replace `run_statusline` with:

```rust
/// §13.5's fast path, taken before `build_engine`: no lock check, no settings warnings (a status
/// bar has nowhere to show them), and an engine walled off from the Keychain and the network
/// (`statusline::engine`), which detects the run shell itself and resolves the provider in
/// Decision 13's order. An unreadable marker shows nothing and exits 0 (§12.8).
/// `main_with_args` has already drained stdin.
fn run_statusline(
    ctx: Context,
    io: &mut Io<'_>,
    json: bool,
    no_color: bool,
    provider: Option<String>,
    print_config: bool,
) -> i32 {
    if json {
        fail(io, true, KIND_USAGE, STATUSLINE_UNDER_JSON);
        return EXIT_USAGE;
    }
    let (no_color_env, force_color_env) = (ctx.no_color_env, ctx.force_color_env);
    let (engine, provider, _http, _keychain) = statusline::engine(ctx, provider.as_deref());
    let result = statusline_supported(&engine, &provider).and_then(|()| {
        if print_config {
            let _ = writeln!(io.err, "{}", statusline::config_hint(engine.env()));
            return Ok(statusline::config_snippet());
        }
        let view = engine.statusline(&provider)?;
        let settings = engine.settings();
        // The line goes to Claude Code, which renders ANSI colour but is never a terminal, so
        // `auto` colours it: the same rule as `list`, with the terminal test taken as met.
        let colour = color_enabled(
            no_color,
            no_color_env,
            force_color_env,
            settings.color,
            true,
        );
        Ok(statusline::line(
            &view,
            &settings.statusline_format,
            engine.now_ms() / 1000,
            colour,
        ))
    });
    match result {
        Ok(text) => {
            let _ = write!(io.out, "{text}");
            0
        }
        Err(Failure::Engine(e)) => fail(io, false, e.kind(), &e.to_string()),
        Err(Failure::Message(kind, m)) => fail(io, false, kind, &m),
        Err(Failure::Usage(m)) => {
            fail(io, false, KIND_USAGE, &m);
            EXIT_USAGE
        }
    }
}
```

In `app::run`, the statusline branch stays a short-circuit taken before `build_engine` and before
Task 8's `run-shell-unreadable` refusal: `return run_statusline(ctx, io, json, cli.no_color,
cli.provider, *print_config);`. If Task 8 resolved a run shell or a provider for this branch,
drop that: `statusline::engine` does both, over its walled registry (`detect_run_shell` and
`resolve_provider`; it does not call `app::locate`). `app::run` captures nothing for it either:
`Context::from_process` (Task 8) has already put every registered provider's `session_dir_var()`
and `CLAUDECODE` into `ctx.env.vars`, so Decision 13's third step sees `CLAUDECODE` in the binary.
In-process tests build their `Context` by hand and see only the `vars` they set (§15.1).

- [ ] **Step 4: Run them and see them pass**, then the crate's whole suite

Run: `cargo test -p tagteam --lib statusline`
Expected: PASS, including the three new tests.

Run: `cargo test -p tagteam --features test-support --test run_shell_cli`
Expected: PASS: Task 8's tests, Task 14's 4 and these 3.

Run: `cargo test -p tagteam --features test-support`
Expected: PASS. `tests/statusline.rs` runs outside a run shell with no `--provider`, where every
step of Decision 13 lands on `claude-code`, so its lines are unchanged.

Run: `cargo test --release -p tagteam --features test-support --test perf -- --ignored --nocapture --test-threads=1`
Expected: PASS, 4 tests, on an otherwise idle machine. The new test prints a p95 at or under
10 ms. One failure gets one re-run; a second is a real miss of the budget: stop and report the
measured p95.

- [ ] **Step 5: Format and lint**

```
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam/Cargo.toml Cargo.lock crates/tagteam/src/statusline.rs crates/tagteam/src/app.rs crates/tagteam/tests/run_shell_cli.rs crates/tagteam/tests/perf.rs
git commit -m "Resolve the statusline's provider from the flag, the marker, the invoking agent, then the default"
```

---

### Task 16: `FakeAgent` sessions across the engine; the negative surface test

§15.2: `FakeAgent` "supports sessions, with its own config-dir variable, share policy and
session records, so the generic `run` machinery is exercised against a second provider". It
also asserts that "a `run` writes nothing outside the provider's declared surface and its own
profile, for either provider". M4a has no `run`. So this task drives `run`'s M4a building blocks
for FakeAgent through the same engine as Claude Code:
- session state from its own records directory;
- `Owned(Session)` at the gate;
- lazy capture, and its conflict;
- the session-owned usage read;
- link sync by FakeAgent's own allowlist.

The carried-over negative surface test then pins what those writes may touch.

This task adds one test file and no production code, and no fixture helper:
- Every test exercises code from Tasks 7–13 (and, for the `inSession` flag, Task 14), here for
  a second provider. FakeAgent's create-only surface entry, `journal.log`, is already declared
  by Task 13 (13b), whose `assert_only_surface_changed_for` allows it where there was none.
- The fixtures are the canonical ones: `Fx::make_profile_for` (Task 13), `Fx::live_record` and
  `Fx::process` (Task 9), `LSTART` and `record_json` (Task 9). A FakeAgent session record is
  written by a helper private to `fake_sessions.rs`, because `Fx::plant_record` and
  `Fx::live_record` write Claude Code's records directory. FakeAgent's run-shell `Env`
  (`FAKEAGENT_HOME` naming the profile) is built inline in the one helper that needs it, so no
  `FakeFx::shell_env` is added.
- A test that fails here shows a provider-neutrality gap. Fix it in the owning task's code, in
  this task's commit, and say which in the commit body.

**Decision 16 changes one assertion.** An `lstart` mismatch on its own is not proof that a pid
was recycled: a record is dead on a mismatch only if `mentions(pid, launch_command)` is
`Some(false)`. The session-state test makes FakeAgent's pid "recycled" by moving its start time
an hour, so the probe entry also sets `mentions_launch: Some(false)`; with the default `None` the
record would still count as live.

**Files:**
- Create: `crates/tagteam-engine/tests/fake_sessions.rs`.

**Interfaces:**
- Consumes:
  - Task 7:
    - FakeAgent: `capabilities().sessions`, `FakePaths::resolve` honouring `FAKEAGENT_HOME`,
      `Provider::session_records_dir(&self, profile: &Path) -> PathBuf` (`<profile>/procs`),
      `Provider::share_policy(&self, env: &Env) -> SharePolicy` (shared `notes`, `prefs.json`;
      must-share `journal.log`), and `read_profile_credential` and `profile_identity`, which
      the gate's capture reads through;
    - `Seed::{read, write}` and `LinksRecord::read`, re-exported at the `tagteam_provider` root.
  - Task 9:
    - `Engine::session_state(&self, p: &dyn Provider, row: &AccountRow) -> Result<SessionState,
      EngineError>`, `SessionState::owned`, and the gate's `GateOutcome::Owned(OwnedBy::Session)`;
    - `Fx.process: Arc<FakeProcessProbe>`, which every engine the fixture builds uses
      (`FakeFx::new`'s too);
    - `pub const LSTART: &str`, `pub fn record_json(pid: u32, kind: &str) -> String`;
    - `Fx::live_record(&self, profile: &Path, pid: u32, kind: &str) -> PathBuf` (Claude Code's
      records directory).
  - Task 6: `tagteam_provider::liveness::{FakeProcess, parse_lstart}`; Decision 16's
    `mentions_launch`.
  - Task 8: `Fx::make_profile(&self, id: &AccountId) -> PathBuf` (Claude Code's profile, with
    its identity file).
  - Task 10: the gate's step-3 provenance (lazy capture, `GateOutcome::Conflict`).
  - Task 12: the session-owned usage branch of `collect_usage`.
  - Task 13:
    - `Fx::make_profile_for(&self, p: &dyn Provider, id: &AccountId) -> PathBuf`;
    - `Engine::sync_profile_links(&self, p: &dyn Provider, profile: &Path, joining: bool) ->
      Result<SyncReport, EngineError>`, `SyncReport`;
    - `IdentitySurface.create_only` (FakeAgent: `[<FakeAgent home>/journal.log]`), and
      `Fx::assert_only_surface_changed_for` honouring it;
    - `EngineError::ProfileSplit { profile: PathBuf, shared: PathBuf }`, kind `profile-split`.
  - Task 14: `AccountView.in_session: bool`.
  - Existing: `FakeFx::{new, fake_add, fake_provider}`, `Fx::{snapshot, vault_bytes, put_vault,
    add}`, `tagteam_fake::{login, credential_json, FakePaths}`,
    `Engine::{collect_usage, lock_account, refresh_stored, accounts}`.
- Produces: nothing other tasks use. The create-only entry and the comparison that honours it
  are Task 13's.

**Spec:**
- §15.2: provider neutrality. FakeAgent has sessions with its own variable, share policy and
  records, and a run writes nothing outside its surface and profile.
- §3: an entry tagteam may create, empty and only when absent (the create-only row); here a
  real sync of FakeAgent's profile creates `journal.log`.
- §7.3 steps 2–3: `Owned` while session-owned; a quiescent profile's provenance is applied
  before any request, and a conflict returns `Conflict`.
- §8.1: a session-owned fetch is read-only, with the profile's token, no lock and no refresh.
- §12.2: share by allowlist to the fully resolved source; an unknown entry stays private and is
  noted once; a split shared file warns, and a split must-share entry refuses.
- §12.5: lazy capture of a quiescent, rotated profile, and the provenance table.
- §12.6: record liveness and recycled pids (Decision 16); a malformed record counts as owned.

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam-engine/tests/fake_sessions.rs`:

```rust
//! §15.2 provider neutrality for sessions: FakeAgent's profiles go through the same engine as
//! Claude Code's, with its own config-dir variable (`FAKEAGENT_HOME`), session records
//! (`procs/`), credential file and share policy. Session records are judged by the fixture's
//! `FakeProcessProbe`, never by a real pid (§15.1).
mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use common::{FakeFx, LSTART, record_json};
use serde_json::{Value, json};
use tagteam_core::AccountId;
use tagteam_engine::collect::{CollectMode, Collected};
use tagteam_engine::refresh::{GateOutcome, OwnedBy};
use tagteam_engine::session::SessionState;
use tagteam_engine::vault::SERVICE;
use tagteam_fake::{FakePaths, credential_json};
use tagteam_provider::http::Method;
use tagteam_provider::liveness::{FakeProcess, parse_lstart};
use tagteam_provider::{Clock, IdentitySurface, LinksRecord, Provider, Seed};

/// The fixture clock's start, in seconds.
const NOW_S: i64 = 1_790_000_000;

/// FakeAgent's fingerprint of `secret`'s generation (§2): its renew token's.
fn fp(ffx: &FakeFx, secret: &[u8]) -> String {
    ffx.fake.fingerprint(secret).unwrap().as_str().to_owned()
}

/// A FakeAgent credential's access token and renew token.
fn tokens(secret: &[u8]) -> (String, String) {
    let v: Value = serde_json::from_slice(secret).unwrap();
    (
        v["fa"]["token"].as_str().unwrap().to_owned(),
        v["fa"]["renew"].as_str().unwrap().to_owned(),
    )
}

fn pair(token: &str, renew: &str) -> (String, String) {
    (token.to_owned(), renew.to_owned())
}

/// `alice` (stored, not live) and `bob`, FakeAgent's live login.
fn alice_and_bob(ffx: &FakeFx) -> (AccountId, AccountId) {
    (
        ffx.fake_add("alice", "tok-a", "renew-a"),
        ffx.fake_add("bob", "tok-b", "renew-b"),
    )
}

/// `id`'s FakeAgent profile as a quiescent `run` leaves it after bootstrapping it from the
/// vault: the marker, the seed (the vault's generation under the account's login epoch, §12.3
/// step 6), and the profile's own login, `handle` with `token`/`renew`, written where FakeAgent
/// writes a login when `FAKEAGENT_HOME` names the profile (its run shell's environment). With
/// the vault's own tokens it is in step; with others, it rotated since its seed (§12.5).
fn fake_profile(ffx: &FakeFx, id: &AccountId, handle: &str, token: &str, renew: &str) -> PathBuf {
    let profile = ffx.fx.make_profile_for(ffx.fake.as_ref(), id);
    let row = ffx.engine.store().unwrap().account(id).unwrap().unwrap();
    let vault = ffx.fx.vault_bytes(id).unwrap();
    Seed {
        login_epoch: row.login_epoch,
        seed_fp: fp(ffx, &vault),
        needs_bootstrap: false,
    }
    .write(&profile)
    .unwrap();
    let mut at = ffx.fx.env.clone();
    at.vars
        .insert("FAKEAGENT_HOME".into(), profile.as_os_str().to_owned());
    tagteam_fake::login(&at, handle, "ws", token, renew);
    profile
}

/// A FakeAgent session record for `pid` in FakeAgent's own records directory
/// (`<profile>/procs`), as a `run` writes it, with `LSTART` as its `procStart`. The pid's
/// process is whatever `ffx.fx.process` says; an unknown pid is dead.
/// (`Fx::plant_record` writes Claude Code's directory, so it does not fit.)
fn plant_fake_record(ffx: &FakeFx, profile: &Path, pid: u32) {
    let dir = ffx.fake.session_records_dir(profile);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join(format!("{pid}.json")),
        record_json(pid, "interactive"),
    )
    .unwrap();
}

/// A FakeAgent session running in `profile`: its record in FakeAgent's own records directory,
/// and its process alive, started exactly at the record's `procStart`.
fn start_session(ffx: &FakeFx, profile: &Path) {
    plant_fake_record(ffx, profile, 4242);
    ffx.fx.process.set(
        4242,
        FakeProcess {
            exists: Some(true),
            start_time_s: parse_lstart(LSTART),
            ..FakeProcess::default()
        },
    );
}

fn renew_requests(ffx: &FakeFx) -> usize {
    ffx.fx.http.count(Method::Post, &ffx.fake.renew_url())
}

/// The tokens FakeAgent's usage requests carried (`authorization: Fake <token>`).
fn fake_bearers(ffx: &FakeFx) -> Vec<String> {
    ffx.fx
        .http
        .requests()
        .iter()
        .filter(|r| r.url == ffx.fake.usage_url())
        .filter_map(|r| {
            r.headers
                .iter()
                .find(|(k, _)| k == "authorization")
                .and_then(|(_, v)| v.strip_prefix("Fake "))
                .map(str::to_owned)
        })
        .collect()
}

fn script_meters(ffx: &FakeFx) {
    ffx.fx.http.push_json(
        Method::Get,
        &ffx.fake.usage_url(),
        200,
        json!({"meters": [{"id": "daily", "used": 0.42, "renews": NOW_S + 3_600}]}),
    );
}

/// `id`'s stored access token moved inside §7.2's buffer: the inactive branch would renew it
/// before fetching.
fn make_due(ffx: &FakeFx, id: &AccountId) {
    let mut v: Value = serde_json::from_slice(&ffx.fx.vault_bytes(id).unwrap()).unwrap();
    v["fa"]["expires"] = json!(ffx.fx.clock.now_ms() + 60_000);
    ffx.fx.put_vault(id, v.to_string().as_bytes());
}

/// Every entry under `root`, never following a link: `dir`, a link's target, or a file's text.
fn tree(root: &Path) -> BTreeMap<PathBuf, String> {
    let mut out = BTreeMap::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let meta = fs::symlink_metadata(&path).unwrap();
            let shown = if meta.file_type().is_symlink() {
                format!("link {}", fs::read_link(&path).unwrap().display())
            } else if meta.is_dir() {
                dirs.push(path.clone());
                "dir".to_owned()
            } else {
                String::from_utf8_lossy(&fs::read(&path).unwrap()).into_owned()
            };
            out.insert(path, shown);
        }
    }
    out
}

#[test]
fn fake_agent_session_state_comes_from_its_own_records() {
    let ffx = FakeFx::new();
    let (alice, _bob) = alice_and_bob(&ffx);
    let row = ffx.engine.store().unwrap().account(&alice).unwrap().unwrap();
    let state = || ffx.engine.session_state(ffx.fake.as_ref(), &row).unwrap();
    assert_eq!(state(), SessionState::NoProfile);
    let profile = ffx.fx.make_profile_for(ffx.fake.as_ref(), &alice);
    assert!(matches!(state(), SessionState::Quiescent { .. }), "{:?}", state());

    // Where Claude Code keeps its records is nothing to FakeAgent: a live Claude Code record
    // for the same pid, in Claude Code's directory, owns nothing here.
    let procs = ffx.fake.session_records_dir(&profile);
    let cc_records = ffx.fx.cc.session_records_dir(&profile);
    assert_ne!(procs, cc_records, "the two agents keep their records apart");
    ffx.fx.live_record(&profile, 4242, "interactive");
    assert!(cc_records.join("4242.json").exists());
    assert!(matches!(state(), SessionState::Quiescent { .. }), "{:?}", state());

    plant_fake_record(&ffx, &profile, 4242);
    assert!(matches!(state(), SessionState::Owned { .. }), "{:?}", state());
    let listed = ffx
        .engine
        .accounts(Some(&ffx.fake_provider()))
        .unwrap()
        .remove(0)
        .accounts;
    assert!(listed.iter().any(|v| v.row.id == alice && v.in_session));

    // §12.6: its pid recycled (it runs again, started an hour later, and is not a FakeAgent
    // process: Decision 16), then a malformed record, which counts as owned.
    ffx.fx.process.set(
        4242,
        FakeProcess {
            exists: Some(true),
            start_time_s: parse_lstart(LSTART).map(|s| s + 3_600),
            mentions_launch: Some(false),
            ..FakeProcess::default()
        },
    );
    assert!(matches!(state(), SessionState::Quiescent { .. }), "{:?}", state());
    fs::write(procs.join("7.json"), b"[1, 2").unwrap();
    let unreadable = state();
    assert!(
        matches!(unreadable, SessionState::Unreadable { .. }) && unreadable.owned(),
        "{unreadable:?}"
    );
}

#[test]
fn a_fake_agent_account_in_a_session_is_owned_at_the_gate_and_never_renewed() {
    let ffx = FakeFx::new();
    let (alice, _bob) = alice_and_bob(&ffx);
    // The session rotated its token: a running profile is still never captured (§12.5).
    let profile = fake_profile(&ffx, &alice, "alice", "tok-a-2", "renew-a-2");
    start_session(&ffx, &profile);
    let vault = ffx.fx.vault_bytes(&alice).unwrap();
    let before = ffx.fx.snapshot();

    let out = ffx
        .engine
        .refresh_stored(ffx.fake.as_ref(), &alice, &vault)
        .unwrap();

    assert!(matches!(out, GateOutcome::Owned(OwnedBy::Session)), "{out:?}");
    assert_eq!(renew_requests(&ffx), 0);
    assert_eq!(ffx.fx.vault_bytes(&alice).unwrap(), vault);
    ffx.fx.assert_only_surface_changed_for(
        &IdentitySurface::default(),
        &before,
        &ffx.fx.snapshot(),
        "the gate on a FakeAgent account in a session",
    );
}

#[test]
fn a_quiescent_fake_agent_profile_that_rotated_is_captured_at_the_gate() {
    let ffx = FakeFx::new();
    let (alice, _bob) = alice_and_bob(&ffx);
    let profile = fake_profile(&ffx, &alice, "alice", "tok-a-2", "renew-a-2");
    let held = fs::read(profile.join("credential.json")).unwrap();
    let vault = ffx.fx.vault_bytes(&alice).unwrap();
    let before = ffx.fx.snapshot();

    let out = ffx
        .engine
        .refresh_stored(ffx.fake.as_ref(), &alice, &vault)
        .unwrap();

    // §7.3 step 3 adopted the profile's generation, so step 4 finds the vault already fresh.
    assert!(
        matches!(&out, GateOutcome::AlreadyFresh(bytes) if tokens(bytes) == pair("tok-a-2", "renew-a-2")),
        "{out:?}"
    );
    assert_eq!(
        tokens(&ffx.fx.vault_bytes(&alice).unwrap()),
        pair("tok-a-2", "renew-a-2")
    );
    let prev = ffx.fx.kc.get(SERVICE, &format!("{alice}.prev")).unwrap();
    assert_eq!(tokens(&prev), pair("tok-a", "renew-a"), "the vault keeps its own as .prev");
    assert_eq!(
        Seed::read(&profile).present().unwrap().seed_fp,
        fp(&ffx, &held),
        "the seed moves to the captured generation"
    );
    assert_eq!(
        fs::read(profile.join("credential.json")).unwrap(),
        held,
        "a capture reads the profile and never writes it"
    );
    assert_eq!(renew_requests(&ffx), 0);
    ffx.fx.assert_only_surface_changed_for(
        &IdentitySurface::default(),
        &before,
        &ffx.fx.snapshot(),
        "a FakeAgent capture",
    );
}

#[test]
fn a_fake_agent_profile_and_vault_that_both_moved_conflict_and_nothing_moves() {
    let ffx = FakeFx::new();
    let (alice, _bob) = alice_and_bob(&ffx);
    let profile = fake_profile(&ffx, &alice, "alice", "tok-a-2", "renew-a-2");
    // The seed is a third generation: since they last agreed, the vault and the profile moved.
    let row = ffx.engine.store().unwrap().account(&alice).unwrap().unwrap();
    let older = credential_json("tok-a-0", Some("renew-a-0"), None).to_string();
    let seed = Seed {
        login_epoch: row.login_epoch,
        seed_fp: fp(&ffx, older.as_bytes()),
        needs_bootstrap: false,
    };
    seed.write(&profile).unwrap();
    let vault = ffx.fx.vault_bytes(&alice).unwrap();

    let out = ffx
        .engine
        .refresh_stored(ffx.fake.as_ref(), &alice, &vault)
        .unwrap();

    assert!(matches!(out, GateOutcome::Conflict), "{out:?}");
    assert_eq!(ffx.fx.vault_bytes(&alice).unwrap(), vault);
    assert_eq!(Seed::read(&profile).present().unwrap(), seed);
    assert_eq!(renew_requests(&ffx), 0);
}

#[test]
fn a_session_owned_fake_agent_account_is_read_with_the_profiles_token_and_never_renewed() {
    // §8.1: read-only, with the profile's token, no lock and no refresh. The vault's own token
    // is due, so the inactive branch would have renewed it first.
    let ffx = FakeFx::new();
    let (alice, _bob) = alice_and_bob(&ffx);
    let profile = fake_profile(&ffx, &alice, "alice", "tok-a-2", "renew-a-2");
    start_session(&ffx, &profile);
    make_due(&ffx, &alice);
    script_meters(&ffx);
    let vault = ffx.fx.vault_bytes(&alice).unwrap();
    let held = fs::read(profile.join("credential.json")).unwrap();

    let lock = ffx.engine.lock_account(&alice).unwrap();
    let report = ffx
        .engine
        .collect_usage(CollectMode::OnDemand {
            accounts: vec![alice.clone()],
        })
        .unwrap();
    drop(lock);

    assert_eq!(report.outcomes, [(alice.clone(), Collected::Recorded)]);
    assert_eq!(fake_bearers(&ffx), ["tok-a-2"]);
    assert_eq!(renew_requests(&ffx), 0);
    assert_eq!(ffx.fx.vault_bytes(&alice).unwrap(), vault, "the vault is untouched");
    assert_eq!(fs::read(profile.join("credential.json")).unwrap(), held, "so is the profile");
    let windows = ffx
        .engine
        .store()
        .unwrap()
        .usage_state(&alice)
        .unwrap()
        .unwrap()
        .last_good
        .unwrap();
    assert_eq!(
        (windows[0].key.as_str(), windows[0].pct.round()),
        ("daily", 42.0)
    );
}

#[test]
fn a_fake_agent_profile_shares_by_fake_agents_own_allowlist() {
    let ffx = FakeFx::new();
    let (alice, _bob) = alice_and_bob(&ffx);
    let outer = FakePaths::resolve(&ffx.fx.env).dir;
    fs::create_dir_all(outer.join("notes")).unwrap();
    fs::write(outer.join("notes/today.md"), "note\n").unwrap();
    fs::write(outer.join("prefs.json"), "{\"theme\": \"x\"}\n").unwrap();
    fs::write(outer.join("mystery.db"), "?").unwrap();
    let profile = fake_profile(&ffx, &alice, "alice", "tok-a", "renew-a");
    let surface = ffx.fake.identity_surface(&ffx.fx.env);
    let before = ffx.fx.snapshot();

    let first = ffx
        .engine
        .sync_profile_links(ffx.fake.as_ref(), &profile, false)
        .unwrap();

    let shared = BTreeSet::from(["journal.log", "notes", "prefs.json"]);
    let created: BTreeSet<&str> = first.created.iter().map(String::as_str).collect();
    assert_eq!(created, shared, "{first:?}");
    for name in &shared {
        assert_eq!(
            fs::read_link(profile.join(name)).unwrap(),
            fs::canonicalize(outer.join(name)).unwrap(),
            "{name} links to the fully resolved source"
        );
    }
    assert_eq!(
        fs::read(outer.join("journal.log")).unwrap(),
        b"",
        "the must-share entry is created empty in the outer home"
    );
    for private in ["credential.json", "identity.json"] {
        let meta = fs::symlink_metadata(profile.join(private)).unwrap();
        assert!(!meta.file_type().is_symlink(), "{private} stays the profile's own");
    }
    assert!(
        fs::symlink_metadata(profile.join("mystery.db")).is_err(),
        "an unknown entry stays private"
    );
    let noted = first.warnings.iter().filter(|w| w.contains("mystery.db"));
    assert_eq!(noted.count(), 1, "{:?}", first.warnings);
    let links = LinksRecord::read(&profile).present().unwrap();
    let linked: BTreeSet<&str> = links.links.keys().map(String::as_str).collect();
    assert_eq!(linked, shared);
    assert_eq!(links.noted_unknown, BTreeSet::from(["mystery.db".to_owned()]));
    for cc_entry in ["projects", "history.jsonl"] {
        assert!(
            !outer.join(cc_entry).exists() && !profile.join(cc_entry).exists(),
            "Claude Code's must-share {cc_entry} is nothing to FakeAgent"
        );
    }
    // FakeAgent's surface allows `journal.log` created empty where there was none (§3), and
    // nothing else outside tagteam's own data.
    ffx.fx.assert_only_surface_changed_for(
        &surface,
        &before,
        &ffx.fx.snapshot(),
        "FakeAgent's sync",
    );

    // Again: nothing new, and the unknown entry is not noted twice.
    let second = ffx
        .engine
        .sync_profile_links(ffx.fake.as_ref(), &profile, false)
        .unwrap();
    assert!(second.created.is_empty() && second.removed.is_empty(), "{second:?}");
    assert!(
        !second.warnings.iter().any(|w| w.contains("mystery.db")),
        "{:?}",
        second.warnings
    );

    // A shared file replaced by a regular one in the profile: a warning, and both copies stay.
    fs::remove_file(profile.join("prefs.json")).unwrap();
    fs::write(profile.join("prefs.json"), "{\"theme\": \"mine\"}\n").unwrap();
    let third = ffx
        .engine
        .sync_profile_links(ffx.fake.as_ref(), &profile, false)
        .unwrap();
    assert!(
        third.warnings.iter().any(|w| w.contains("prefs.json")),
        "{:?}",
        third.warnings
    );
    assert_eq!(
        fs::read_to_string(profile.join("prefs.json")).unwrap(),
        "{\"theme\": \"mine\"}\n"
    );
    assert_eq!(
        fs::read_to_string(outer.join("prefs.json")).unwrap(),
        "{\"theme\": \"x\"}\n"
    );

    // The must-share entry split: the sync refuses, naming it, and both copies stay.
    fs::remove_file(profile.join("journal.log")).unwrap();
    fs::write(profile.join("journal.log"), "a private copy\n").unwrap();
    let err = ffx
        .engine
        .sync_profile_links(ffx.fake.as_ref(), &profile, false)
        .unwrap_err();
    assert_eq!(err.kind(), "profile-split", "{err}");
    assert!(err.to_string().contains("journal.log"), "{err}");
    assert_eq!(
        fs::read_to_string(profile.join("journal.log")).unwrap(),
        "a private copy\n"
    );
    assert_eq!(fs::read(outer.join("journal.log")).unwrap(), b"");
}

#[test]
fn fake_agent_sessions_write_nothing_outside_its_surface_and_its_profile() {
    // The negative surface test (§15.2, carried over). FakeAgent's profile is synced, then
    // lazily captured at the gate while quiescent; then a session starts and its usage is
    // read from the profile. Together these move only FakeAgent's declared surface (its
    // create-only must-share entry, §3) and its own profile. Claude Code's fixture home, its
    // Keychain items and its own profile stay byte-identical: the snapshot walks all of HOME
    // and every non-vault Keychain item, and the tree walks every profile.
    let ffx = FakeFx::new();
    let cc = ffx.fx.add("cc@b.co", "rt-cc");
    ffx.fx.add("cc2@b.co", "rt-cc2"); // Claude Code's live login
    let cc_profile = ffx.fx.make_profile(&cc);
    fs::write(cc_profile.join(".credentials.json"), b"{\"claudeAiOauth\": {}}").unwrap();
    let (alice, _bob) = alice_and_bob(&ffx);
    let outer = FakePaths::resolve(&ffx.fx.env).dir;
    fs::create_dir_all(outer.join("notes")).unwrap();
    fs::write(outer.join("notes/today.md"), "note\n").unwrap();
    fs::write(outer.join("prefs.json"), "{}\n").unwrap();
    let profile = fake_profile(&ffx, &alice, "alice", "tok-a-2", "renew-a-2");
    let surface = ffx.fake.identity_surface(&ffx.fx.env);
    let sessions = ffx.fx.env.data_dir().join("sessions");
    let (before, profiles_before) = (ffx.fx.snapshot(), tree(&sessions));

    ffx.engine
        .sync_profile_links(ffx.fake.as_ref(), &profile, false)
        .unwrap();
    let vault = ffx.fx.vault_bytes(&alice).unwrap();
    let gated = ffx
        .engine
        .refresh_stored(ffx.fake.as_ref(), &alice, &vault)
        .unwrap();
    assert!(matches!(gated, GateOutcome::AlreadyFresh(_)), "{gated:?}");
    start_session(&ffx, &profile);
    script_meters(&ffx);
    let report = ffx
        .engine
        .collect_usage(CollectMode::OnDemand {
            accounts: vec![alice.clone()],
        })
        .unwrap();
    assert_eq!(report.outcomes, [(alice.clone(), Collected::Recorded)]);

    let (after, profiles_after) = (ffx.fx.snapshot(), tree(&sessions));
    ffx.fx.assert_only_surface_changed_for(
        &surface,
        &before,
        &after,
        "a FakeAgent profile's sync, capture and session-owned usage",
    );
    let changed: Vec<&PathBuf> = profiles_before
        .keys()
        .chain(profiles_after.keys())
        .filter(|p| profiles_before.get(*p) != profiles_after.get(*p))
        .collect();
    assert!(!changed.is_empty(), "the sync and the capture wrote alice's profile");
    assert!(
        changed.iter().all(|p| p.starts_with(&profile)),
        "written outside alice's profile: {changed:?}"
    );
}
```

- [ ] **Step 2: Run them and see them pass**

Run: `cargo test -p tagteam-engine --test fake_sessions`
Expected: PASS, 7 tests, at once. There is nothing to implement here: every test pins behaviour
Tasks 7–14 already implement, for a second provider.
- The other six exercise the session state, the gate, capture, the session-owned read and the
  sync, which Tasks 9, 10, 12, 13 and 14 wrote provider-neutrally.
- The negative surface test
  (`fake_agent_sessions_write_nothing_outside_its_surface_and_its_profile`) passes at once too.
  It pins an invariant Tasks 9–13 already hold, for a second provider: Task 13 declared
  FakeAgent's `create_only` and taught `assert_only_surface_changed_for` to allow an empty
  create-only entry where there was none.
- Under Decision 16, `fake_agent_session_state_comes_from_its_own_records` sets
  `mentions_launch: Some(false)` on its recycled pid. Without it the record would still count as
  live.

A failure here is a provider-neutrality gap in the owning task's code (§15.2), not a test bug.
Step 3 says what to do with one.

- [ ] **Step 3: Implement**

No implementation. FakeAgent's create-only entry and the comparison's allowance for it are
Task 13's (13b). If Step 2 fails, find the Claude Code assumption in the owning task's engine
code, make it provider-neutral there, and name the task and the change in this commit's body.

- [ ] **Step 4: Run the crate's whole suite**

Run: `cargo test -p tagteam-engine --features test-hooks`
Expected: PASS. This task adds one test file and no production code, so nothing else changes.

- [ ] **Step 5: Format and lint**

```
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-engine/tests/fake_sessions.rs
git commit -m "Test FakeAgent sessions through the engine and pin that they write only its surface and its profile"
```

---

### Task 17: Spec probe facts; final verification

On 2026-10-01 three Appendix A facts were probed against CC 2.1.286 and a throwaway keychain
file. The spec still carries the earlier wording for each:
- **`.claude.json` through links.** A.1 marks the link-writing behaviour "not tested
  empirically". The probe confirmed one hop: CC kept the link to `.claude.json`, and replaced
  the middle link of a link-to-link with a regular file.
- **The existence probe.** A.3 says it never prompts on a locked keychain file. Confirmed: it
  answered at once, without a prompt.
- **`show-keychain-info`.** A.3 says it "returns rc 128" on a locked keychain file. In a GUI
  session it instead opened a SecurityAgent unlock dialog and blocked until the dialog was
  answered.

The lock check already treats a timeout as unknown and proceeds, so the code is right. Only the
spec text changes, plus one unit-test comment that cites the old rc.

Decision 16 also amends §12.6, and this task writes that into the spec: an `lstart` mismatch
counts as recycled only when the process does not mention the launch command either, because
the start time a Linux host computes moves with its wall clock. Task 6 implements the rule.
§12.6's `lstart` example also names the right weekday: 2026-10-01 is a Thursday.

Then the whole branch is verified, and the plan's status stays as the design-record rules want
it while the merge request is open.

**Files:**
- Modify: `docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md`: §12.6's `ps lstart`
  bullet (today 2092–2096), Appendix A.1's "File writes" bullet (2721–2726), and Appendix A.3's
  existence-probe bullet (2765–2767) and lock-check bullet (2776–2779).
- Modify: `crates/tagteam-provider/src/security.rs` (the comment at 877 in
  `the_lock_check_maps_show_keychain_info`).
- Check: `docs/superpowers/plans/2026-10-01-tagteam-m4a-sessions-foundation.md` (its
  `**Status:**` line).

**Interfaces:**
- Consumes: everything Tasks 1–16 produced.
- Produces: nothing new.

**Spec:**
- §12.6, `ps lstart` text: the start time must equal `procStart` within ±1 s; under Decision 16
  a mismatch is recycled only when the process does not mention the launch command.
- Appendix A.1, "File writes": CC writes `.claude.json` through one symlink hop.
- Appendix A.3: the existence probe never prompts; the lock check's rc table, where a timeout is
  unknown and the command proceeds.
- Global design-record rules: `In progress` while work or review is active, with the MR
  reference; `Implemented` with the MR in the final pre-merge commit.

- [ ] **Step 1: Record the probe facts**

In the spec, Appendix A.1, replace:

```
  renaming a temporary file over the path itself, which replaces a symlink with a regular file.
  Project and local settings refuse symlinks altogether (*2.1.286*, not tested empirically).
```

with:

```
  renaming a temporary file over the path itself, which replaces a symlink with a regular file.
  Verified on 2026-10-01 against CC 2.1.286: it wrote `.claude.json` through one symlink hop,
  keeping the link, and replaced the middle link of a link-to-link with a regular file. Project
  and local settings refuse symlinks altogether (*2.1.286*, not tested empirically).
```

The 2026-10-01 probe did not exercise project or local settings, so "not tested empirically"
stays on that sentence alone.

In Appendix A.3, replace:

```
- **Existence probe:** the same command without `-w` (attributes only; never prompts).
  - On a locked keychain file or a locked SSH session: rc 0 for present, rc 44 for absent; never prompts.
  - `show-keychain-info` on a locked keychain file returns rc 128. The argv/new-item versus `-i`/`-U` write difference does not change the item's ACL (both use the same binary).
```

with:

```
- **Existence probe:** the same command without `-w` (attributes only; never prompts).
  - On a locked keychain file or a locked SSH session: rc 0 for present, rc 44 for absent, at
    once and without a prompt (the keychain-file case verified on 2026-10-01 against a
    throwaway keychain).
  - `show-keychain-info` on a locked keychain file in a GUI session opens a SecurityAgent
    unlock dialog and blocks until it is answered (verified on 2026-10-01 against a throwaway
    keychain). The argv/new-item versus `-i`/`-U` write difference does not change the item's
    ACL (both use the same binary).
```

and replace:

```
  - A command that will read or write a Keychain item first runs `show-keychain-info` on the
    default keychain: rc 0 is unlocked, rc 36 locked. Any other rc (such as 128 for a locked
    keychain file) or a timeout is unknown, and the command proceeds; its tri-state reads
    refuse safely.
```

with:

```
  - A command that will read or write a Keychain item first runs `show-keychain-info` on the
    default keychain: rc 0 is unlocked, rc 36 locked. Any other rc, or a timeout, is unknown,
    and the command proceeds; its tri-state reads refuse safely. A locked keychain file in a
    GUI session is such a case: `show-keychain-info` waits on its unlock dialog (above), so
    unless the dialog is answered within the driver's 5 s timeout, the check ends as unknown.
```

Two consequences of the A.3 probe were not probed further, and are left for M5's `doctor` work.
A command that runs the lock check on a Mac whose login keychain is locked shows a SecurityAgent
dialog for up to the 5 s timeout, and whether killing `security` at the timeout dismisses the
dialog is unknown. A `real_keychain` test could pin the existence probe on a locked keychain
file (lock the throwaway keychain, then `exists`); it is left out because it would pass at once,
with no code to drive.

In `crates/tagteam-provider/src/security.rs`, `the_lock_check_maps_show_keychain_info`, the
third assertion's comment no longer names a keychain state the spec now says does not answer:

```rust
        assert_eq!(k.lock_state(), LockState::Unknown); // any other rc: unknown
```

Run: `cargo test -p tagteam-provider --lib security`
Expected: PASS (a comment changed).

Run: `rg -n "rc 128" docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md crates`
Expected: no output.

Run: `rg -n "not tested empirically" docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md`
Expected: one line, the project-and-local-settings sentence of A.1.

- [ ] **Step 2: Commit the probe facts**

```bash
git add docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md crates/tagteam-provider/src/security.rs
git commit -m "Record the 2026-10-01 probes of Claude Code's link writes and a locked keychain file"
```

- [ ] **Step 3: Amend §12.6 for Decision 16**

In the spec, §12.6, replace the `ps lstart` bullet:

```
- **`ps lstart` text**, which CC 2.1.286 writes on macOS and Linux alike (`LC_ALL=C`, `TZ=UTC`,
  for example `Wed Oct  1 12:34:56 2026`): the process's start time from the OS
  (`proc_pidinfo(PROC_PIDTBSDINFO)` on macOS; the boot time plus `/proc/<pid>/stat` field 22,
  counted after the last `)`, on Linux) must equal it to the second, within ±1 s. Any other
  start time means the pid was recycled.
```

with:

```
- **`ps lstart` text**, which CC 2.1.286 writes on macOS and Linux alike (`LC_ALL=C`, `TZ=UTC`,
  for example `Thu Oct  1 12:34:56 2026`): the process's start time from the OS
  (`proc_pidinfo(PROC_PIDTBSDINFO)` on macOS; the boot time plus `/proc/<pid>/stat` field 22,
  counted after the last `)`, on Linux) must equal it to the second, within ±1 s. A mismatch
  counts as recycled only when the process does not mention the launch command either; a
  mismatch on a process that mentions it counts as live, since the start time a Linux host
  computes moves with its wall clock.
```

Run: `rg -n "Wed Oct|Any other start time" docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md`
Expected: no output.

- [ ] **Step 4: Commit the §12.6 amendment**

```bash
git add docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md
git commit -m "Count an lstart mismatch as a recycled pid only when the process is not the launch command"
```

- [ ] **Step 5: Format, lint and test everything**

Run:
```bash
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --features tagteam/test-support --no-fail-fast 2>&1 | tee "$TMPDIR/m4a-test.log" | grep -E '^test result:|FAILED|panicked'
awk '/^test result:/ {p += $4; f += $6; i += $8} END {printf "%d/%d passed, %d ignored\n", p, p + f, i}' "$TMPDIR/m4a-test.log"
cargo test --workspace --features tagteam/test-support --no-fail-fast -- --ignored 2>&1 | tee "$TMPDIR/m4a-ignored.log" | grep -E '^test result:|FAILED|panicked'
awk '/^test result:/ {p += $4; f += $6} END {printf "%d/%d passed\n", p, p + f}' "$TMPDIR/m4a-ignored.log"
cargo test -p tagteam --lib
```

Expected:
- `fmt --check` prints nothing, and both clippy runs finish with no warning.
- Every `test result:` line says `ok`, no `FAILED` or `panicked` line appears, and the first
  `awk` prints `N/N passed, M ignored` with the two numbers before the slash equal. Report that
  line as the total.
- The `--ignored` run passes every ignored test (M2a's 9 s CC-lock test, `gate_race`'s 15 s
  stopped-holder test, and any M4a added). Its `awk` prints `K/K passed`.
- `cargo test -p tagteam --lib` runs without features, so it exercises the release branch of
  the test-override checks.

On a Mac, run the real Keychain tests:

```bash
cargo test -p tagteam-provider --features real_keychain --test real_keychain
```

Expected: PASS, with no GUI prompt; only an unlocked throwaway keychain's lock state is checked.
They drive `/usr/bin/security` against a throwaway keychain file, which needs the Security
daemons. An agent runs them sandboxed first, and unsandboxed only when that run fails with
sandbox evidence: `Operation not permitted`, or a `securityd` or `SecKeychain` error. A person
runs them in a plain terminal.

Run the timing tests, one at a time, on an otherwise idle machine:

```bash
cargo test --release -p tagteam --features test-support --test perf -- --ignored --nocapture --test-threads=1
```

Expected: PASS, 4 tests. `statusline` p95 ≤ 10 ms, on a cache hit, on a cache miss and in a run
shell (Task 15); `list` p95 ≤ 50 ms with nothing due. That covers Decision 8's session state per
account. One failure gets one re-run; a second is a real miss of the budget: stop and report the
measured p95.

Run: `cargo check -p tagteam-core -p tagteam-provider -p tagteam-cc -p tagteam-fake --target x86_64-unknown-linux-gnu`
Expected: `Finished`. This compiles Task 6's Linux-only liveness code (`/proc/<pid>/stat`, the
boot time) on a Mac. If the target is missing, `rustup target add x86_64-unknown-linux-gnu` first,
which needs the network. The engine and the binary are left out, as in M2b: bundled SQLite's
build script needs a Linux C cross-compiler.

- [ ] **Step 6: Build the release binary and check it carries no test hooks**

```bash
cargo build --release -p tagteam
grep -a -c TAGTEAM_TEST_ target/release/tagteam ; test $? -eq 1
```

Expected: the build succeeds, and `grep` prints `0` and exits 1, so the final `test` exits 0.
The keychain-directory, platform and API-base overrides exist only under `test-support`.

- [ ] **Step 7: The plan's status**

Run: `grep -n '^\*\*Status:\*\*' docs/superpowers/plans/2026-10-01-tagteam-m4a-sessions-foundation.md`

Expected: `3:**Status:** In progress`. The Execution notes set it when execution started, and it
stays so while the work and its reviews are active.
- If it still says `Approved`, set it to `**Status:** In progress` and commit that alone:
  `git commit -m "Mark the M4a plan in progress"`.
- Once the Draft MR is open, record it on the same line, `**Status:** In progress — <MR URL>`, in
  one commit: `git commit -m "Record the M4a merge request on the plan"`.
- The final pre-merge commit, after the pre-merge review, sets `**Status:** Implemented — <MR
  URL>`.

The spec stays `In progress` until M5. Opening the MR and the pre-merge review follow the
global workflow and are not steps of this plan.
