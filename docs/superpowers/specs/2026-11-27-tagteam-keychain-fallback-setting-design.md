# tagteam — `keychain.file_fallback`: refuse the plain-text Keychain fallback

**Status:** Approved
**Date:** 2026-11-27
**Size:** about 500 lines of non-test code, in one increment (§7). It rests on the files in
Scope: one new settings key (the `autoswitch.ceiling` change added one in about 120 lines), two
changed arguments on the `Provider` trait with two implementations, the engine's probe and its
two call sites, the rollback's error, one daemon backoff input and one `doctor` check.
**Scope:** The macOS credential write of the Claude Code provider
(`crates/tagteam-cc/src/live.rs`, `provider.rs`, `doctor.rs`), the `Provider` trait
(`crates/tagteam-provider/src/provider.rs`) and the fake (`crates/tagteam-fake`), one new setting
(`crates/tagteam-engine/src/settings.rs`), the engine's two live writers
(`switch/transact.rs`, `switch/rollback.rs`, `active.rs`), its error kinds (`error.rs`), the
daemon's keychain backoff (`crates/tagteam-daemon/src/backoff.rs`, `provider_job.rs`),
`README.md`, `SECURITY.md` and `CHANGELOG.md`. Linux, reads, session profiles and the default
behaviour do not change.
**Builds on:** the core spec,
[`2026-09-26-tagteam-core-cli-design.md`](2026-09-26-tagteam-core-cli-design.md), cited as
"core §n", and the daemon spec,
[`2026-10-14-tagteam-daemon-design.md`](2026-10-14-tagteam-daemon-design.md), cited as "D n". This
spec amends core §6.4, §13.6 and Appendix A.3, and D4.1. The PR edits those passages and marks
each one amended.

Line numbers are as of `b223d07c` and will drift.

---

## 1. Purpose

### 1.1 The problem

On macOS, when Claude Code's Keychain item cannot be written, tagteam stores the secret in plain
text instead, as Claude Code itself does (core Appendix A.3):

- **A login** (`write_entry`, `live.rs:748`): the old item is saved, the credential is written to
  `<secure-storage dir>/.credentials.json`, the Keychain item is deleted and the rest of the
  operation writes to the file.
- **An API key** (`write_managed_key`, `live.rs:978`): the key is written to `primaryApiKey` in
  Claude Code's global config (`~/.claude.json`) and the managed-key item is deleted.

On Michael's second Mac, Herdr panes started from an SSH login run in launchd's Background
session. There the login keychain refuses a write with "User interaction is not allowed", so a
`tagteam switch`, an auto-switch or the daemon moves the login out of the Keychain into a
plain-text file. He wants that refused on his machines.

### 1.2 The change

A setting `keychain.file_fallback`, a boolean, default `true`, which is today's behaviour. With
`false`, a macOS write that the Keychain refuses fails with an error that names the cause and the
two ways out, and leaves everything as it was.

### 1.3 Who is affected

- **Linux:** nobody. The file is Linux's normal store, and the setting is never read there.
- **macOS with the default:** nobody. A headless Mac used only over SSH keeps Claude Code's login
  in the file, and tagteam keeps working there.
- **macOS with `false`:** a write from a session that cannot write the Keychain is refused. The
  setting governs tagteam's writes only: Claude Code's own fallback is Claude Code's.

## 2. Goals, non-goals, anti-goals

**Goals**

1. With `false`, tagteam never puts a secret into `.credentials.json` or `primaryApiKey` on macOS
   because the Keychain refused a write.
2. A refusal leaves no partial state (§3.4).
3. The error says what happened and what to do (§3.5).
4. `auto` and the daemon report a refusal and back off (§3.6).

**Non-goals**

- Removing a plain-text login that is already there, or moving it into the Keychain.
- Session profiles (§3.8).
- Reads: a Keychain read the file covers stays `Degraded` as today.
- Detecting the Background session itself. The refused write is the only signal used.

**Anti-goals**

- Any change of behaviour with the default.
- A refusal that deletes, moves or rewrites a credential.

## 3. Design

### 3.1 The setting

| | |
|---|---|
| Name | `keychain.file_fallback` |
| Kind | `KeyKind::Bool` |
| Default | `true` |
| Per provider | no (`per_provider: false`) |

It joins `KEYS` after `notify.events`, with a `KeyId::KeychainFileFallback` and a
`Settings::keychain_file_fallback` field. `tagteam config`, `config get`, `config set` and
`config unset` take it through the registry with no code of their own. The engine reads it from the
file at each write and each probe, not from its settings snapshot: only the daemon reloads that
snapshot, and a running `tagteam auto` or TUI must follow an edit at its next write too.

### 3.2 Handing the choice to the provider

Settings live in the engine; a provider sees only `Env`. `Provider::write_credential`'s
`before_fallback: BeforeFallback<'_>` argument becomes a policy:

```rust
pub enum Fallback<'a> {
    /// Appendix A.3's fallback, which first hands `before` the item it will delete.
    Allow(BeforeFallback<'a>),
    /// A write the Keychain refuses fails with `ProviderError::FallbackRefused`.
    Refuse,
}
```

The engine builds it at its two call sites, `switch/transact.rs:306` and `active.rs:672`:
`Refuse` when `keychain_file_fallback` is `false`, else `Allow` with today's callback. The enum
keeps "refuse, yet with a save callback" unrepresentable. `tagteam-fake` never falls back and
ignores it. A provider's Linux path never looks at it.

### 3.3 The refused write

In `write_entry` and `write_managed_key`, when the Keychain write fails and the policy is
`Refuse`, the write returns `ProviderError::FallbackRefused` at once: before `report_item`, the
file or config write, `remove_item` and the file-mode pin. The existing WARN line with the `rc`
stays, with wording that no longer says "falling back" under `Refuse`.

`write_entry` reaches the fallback a second way, when `file_mode_pinned` is already set. A pin
is only ever set by a fallback of the same operation, which `Refuse` never takes, so the pinned
branch is unreachable under `Refuse`; the code returns `FallbackRefused` there too rather than
assume it.

`guarded` (`provider.rs:220`) runs the snapshot undo on the error, as for any failed write:

- **A login:** a failed `add-generic-password` changed nothing. `settle_item` reads the old
  bytes back, so the undo has nothing to restore.
- **An API key:** `approved` is spliced into the global config before the Keychain write
  (`live.rs:933`), and the undo puts the file back as snapshotted. The file's bytes after a
  refusal equal its bytes before.
- **A write that landed but failed its read-back** (`security.rs:543`): the undo restores the
  item, as it does today for any such failure.

### 3.4 No partial state: the write probe

The provider-level refusal alone leaves things behind, because the engine acts before the write:

- A switch settles the outgoing login first (`settle_pre`, `settle_branch`, `hold_doomed` in
  `switch/transact.rs`): it may capture a generation into the vault or save one to `displaced/`.
- The live-token refresh (`run_active_refresh`, `active.rs:201`) sends the token request,
  persists the new generation, and only then publishes it. `publish` reports any failed write as
  "not published" (`active.rs:634`). A refusal there leaves Claude Code's item on a generation
  whose refresh token is spent, and every later pass is refused the same way.

So under `Refuse`, on macOS, the engine asks first. It needs an item that always exists: the
account's own vault item (`KeychainVault`, `vault.rs:70`), which is in the same keychain as
Claude Code's item and written by the same `/usr/bin/security`.

**The probe:** under the account's lock, read the account's vault item and write the same bytes
back. A failed write is `EngineError::KeychainFallbackRefused`. A vault item that is unreadable
or absent ends the operation with the error it has today. With a file vault (Linux) there is no
probe.

**The assumption:** a session that cannot write one of these items cannot write the other. That
is what a Background session's "User interaction is not allowed" means, and it is not verified
on the Mac that motivated this. Where it fails, §3.3 is the backstop.

The engine probes only when the setting is `false`:

- **Switch:** the target account's item, at the end of preflight, before anything is captured,
  saved or journaled (before `settle_pre`).
- **Refresh:** the live account's item, before the token request and before the self-heal
  publish. No request is sent after a refused probe.
- **A stored account's refresh:** the account's item, at the refresh gate (`refresh.rs:569`),
  before its token request. A switch freshens a target close to expiry there, and usage
  collection refreshes inactive accounts there. Without the probe, a session that cannot write
  the Keychain spends the token, fails to store its successor in the vault and saves it to
  `rescue/` in plain text. While refusals last, those accounts' usage figures go stale.
  (Added 2026-11-27 after the plan's review, in Michael's absence: inside goal 2, and one call
  site to take out again.)

**What remains.** In a session where the vault item takes a write and Claude Code's item does
not, the write itself is refused as in §3.3: no file, no deletion, no pin. A `displaced/` copy
or a persisted refresh made just before it stays, as after any failed write today, and §3.6
reports it.

### 3.5 The error

Two sources, one kind:

- `EngineError::KeychainFallbackRefused`, from the probe.
- `ProviderError::FallbackRefused { item: String }`, from the write, where `item` is the
  Keychain service name, never bytes.

Both carry this text, the provider's naming its item:

> the Keychain refused a write and `keychain.file_fallback` is off, so the credential was not
> stored; run tagteam from the desktop login session, or allow the fallback with
> `tagteam config set keychain.file_fallback true`

`EngineError::kind()` gives both `keychain-fallback-refused`. A switch whose write is refused
and whose rollback put everything back returns the refusal itself, not `rolled-back`
(`Rollback::fail`, `rollback.rs:50`); a rollback that failed stays `rollback-failed`. The exit
code is the one a `provider` error has today; scripts tell it apart by the kind in `--json`.

### 3.6 `auto` and the daemon

- **A tick** that decides a switch and is refused ends as `TickOutcome::Error` with an `error`
  event, `transient: false`, like any failed switch. The `switch-failed` event keeps its 30-minute
  quiet period for the same target and message (`auto.rs:1187`). `auto --once` exits with its
  error code.
- **The foreground loop** keeps its normal cadence after a failed tick, as today (D "A failed tick
  keeps the normal cadence"). With the probe, a repeated attempt costs one refused Keychain write
  and changes nothing.
- **The refresh** returns a refused probe as its error. Collection reports it as it reports any
  failed refresh; the live account's usage is read with the token Claude Code holds, and goes
  stale when that expires, until a run can write. A refused publish (§3.4 "What remains") stays
  "not published", with the kind in its WARN line.
- **The daemon** adds the refusal to its keychain backoff (D4.1). A refusal reaches the job only
  as an event's text or a warning, so the engine counts refusals, from the probe and from the
  write, and the job compares the count before and after a run, as it does the keychain's read
  counts (`provider_job.rs:452`). `KeychainBackoff::after_run` takes whether the run was
  refused. Such a run counts as a locked run whatever its reads were: 60 s, doubling to 15 min.
  A run without a refusal is judged by its reads as today. The backoff shows in `daemon status`
  through the existing `backoff` field.
- **The cost:** the backoff delays the provider's whole job, so its usage figures are up to
  15 minutes old while refusals last. That is accepted: in such a session the daemon can
  neither switch nor refresh, and the first run that is not refused ends it.

### 3.7 The mirror after a successful write

Unchanged. When the Keychain write succeeds and `.credentials.json` already exists, tagteam
rewrites it so Claude Code reloads (`live.rs:768`). That file already held a login, put there by
Claude Code or an earlier fallback; skipping the rewrite would leave a stale token on disk, and
deleting the file would change Claude Code's own store. `doctor` reports it instead (§3.9).

### 3.8 Session profiles

Out of scope. `tagteam run` hands a profile its login in `sessions/<id>/.credentials.json`
(`write_credential_file`, `live.rs:800`) because tagteam never creates a profile's Keychain item:
Claude Code creates it on its first write and removes the file's secret. There is no Keychain
write to refuse and no other way to hand the login over, so refusing would disable `tagteam run`.
`README.md` and `SECURITY.md` say that the setting does not cover it.

### 3.9 `doctor`

On macOS with the setting `false`, one new check, `cc.keychain-fallback`. With the default there
is no such check, so `doctor`'s output does not change.

- `ok` when neither plain-text entry holds a secret.
- `warn` while `.credentials.json` holds a login or `primaryApiKey` holds a key, naming the
  file and saying that the setting stops new fallbacks and removes nothing. It offers no fix
  command: which program put the secret there, and what moves it back, is Claude Code's.

The check reads whether the entries hold a secret, never the secret. `Provider::doctor_checks`
takes no settings today (`provider.rs:911`), so it gains an argument that carries the choice.

## 4. Approaches

1. **A policy on `write_credential`, plus a probe of the vault item (chosen).** The choice is
   visible at the one interface every live write crosses, so no write can fall back under
   `false`. The probe refuses before anything is saved or a token is spent, on an item that
   always exists.
2. **The setting in `Env`.** No trait change, but `Env` is captured once per process, so a running
   daemon would not follow an edit, and a process-wide flag hides which writes obey it.
3. **Refuse in the engine only, by probing.** No change to the write itself, but a session that
   passes the probe could still fall back, and goal 1 would not hold.
4. **A probe of Claude Code's own item, in the provider.** It tests the very item, but that item
   is absent before most API-key activations and wherever the login lives in the file, which is
   where a refresh would then spend its token unprotected.

Michael picked, on 2026-11-27: the name and shape of the setting, API keys covered, the mirror
unchanged with a `doctor` warning, session profiles out of scope.

## 5. Tests

All with the doubles (`FakeKeychain` and the provider tests' temp homes); nothing touches the
login keychain. Each test names the regression it catches.

- `Refuse` with a failing `upsert`, for a login and for an API key: `FallbackRefused`; the item,
  `.credentials.json`, the global config (byte for byte) and the pin are as before;
  `before_fallback` was never called.
- `Allow` with a failing `upsert`: today's fallback, unchanged (the existing tests, re-pointed).
- Linux with the setting `false`: the file write, unchanged, and no probe.
- The probe: a failing vault write refuses; a passing probe leaves the vault item's bytes
  unchanged; with the default no probe write is made.
- A switch with `false`, an unheld live login and a keychain that refuses writes: refused with
  kind `keychain-fallback-refused`; `displaced/`, `rescue/`, the vault and the journal hold
  nothing new.
- A switch whose probe passes and whose live write is refused: the kind is
  `keychain-fallback-refused`, not `rolled-back`.
- A refresh with `false` and a keychain that refuses writes, with and without a Claude Code
  item: no token request is sent.
- An auto tick: one `error` event and one `switch-failed`; a second tick inside 30 minutes adds no
  second `switch-failed`.
- The daemon job: a refused run starts the backoff, a second doubles it, a clean run ends it.
- Settings: the key parses, defaults to `true`, refuses a provider table, and round-trips through
  `config set` and `unset`.
- `doctor`: no check with the default; `ok` and `warn` with `false`.

## 6. Left to the plan

- Where the engine's refusal count lives and how the two sources feed it.
- The shape of the argument `doctor_checks` gains.
- The wording of the amended core and daemon passages.

## 7. Increments

One increment, `keychain-file-fallback`, about 500 lines: everything in this spec. The default
branch works once it is merged because the default value keeps every code path as it is today.
It is not split: the setting without the probe would ship a refusal that can spend a refresh
token, which is the state this spec exists to prevent.

## 8. Follow-up outside this repository

Once a release carries the setting, Michael's dotfiles set `keychain.file_fallback = false` in
`dot_config/private_tagteam/private_config.toml`. Not part of this change.
