# tagteam M1 — Foundation and Manual Switching Implementation Plan

**Status:** In progress

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A working `tagteam` binary that stores Claude Code logins and switches between them
safely — `add`, `add-token`, `list`, `status`, `switch`, `remove`, `alias`, `disable`,
`enable`, `move` — with the local-state invariant, the switch journal and crash recovery in
place.

**Architecture:** A Cargo workspace of five crates (spec §4.1): pure policy in
`tagteam-core`, shared I/O primitives and the `Provider` trait in `tagteam-provider`, Claude
Code specifics in `tagteam-cc`, the store/vault/switch engine in `tagteam-engine`, and a thin
clap CLI in `tagteam`. M1 makes no network calls: the identity oracle is a port with a
`NoOracle` implementation, and nothing refreshes a token (CC remains the only consumer of
refresh tokens until M2 adds the refresh gate).

**Tech Stack:** Rust (edition 2024), rusqlite (bundled SQLite), serde_json
(`preserve_order`, `arbitrary_precision`), clap 4, sha2, libc, thiserror; tests with tempfile
and assert_cmd. CLI output is pinned by exact expected text and JSON written inline in the
tests (the spec's "snapshot tests"), so each test is self-contained.

**Spec:** `docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md`. Read §2, §3, §4,
§5, §6, §9, §10, §15 and Appendix A before starting any task. Section numbers below refer to
that spec.

## Execution notes

- When execution starts, set this plan's and the spec's `**Status:**` to `In progress` in one
  commit, per the design-record lifecycle in the global instructions.
- Task 1 is a human step and a decision gate; nothing else starts until it passes.
- Feature flags used by tests: `tagteam-provider/file-keychain`, `tagteam-engine/test-hooks`,
  `tagteam/test-support` (enables both), and `tagteam-provider/real_keychain` (macOS only,
  touches a temporary keychain file).

## Milestones

This plan is **M1** of five. Each milestone ends in working, tested software, and each later
plan is written when its predecessor has landed:

| Milestone | Scope |
|---|---|
| **M1 (this plan)** | R1 spike, workspace, primitives, store, vault, CC interop, switch + journal + recovery, account commands, CLI |
| M2 | Refresh gate, quarantine, active-token refresh, oracle over HTTP, usage collector and budget, `list` with usage, `history`, `statusline`, and the `FakeAgent` test provider (§15.2) as the trait grows |
| M3 | Auto-switch (`decide()`, simulations, `auto`) and the `best` / `next-available` strategies |
| M4 | `tagteam run`: profiles, sharing, reservations, provenance, merge-back, `map` / `unmap` / `shell-init` |
| M5 | Export/import, `doctor`, `config`, `displaced`, `purge`, `completions`, logging to file, the `cargo xtask compat` suite, CI and release |

**Deliberately absent from M1**, and why that is safe:
- No refresh, so no freshen-before-activation (§7.2): CC is the only process that ever sends a
  refresh token, and the switch's outgoing capture brings CC's rotations back into the vault.
- No oracle (§7.6): every non-matching outgoing credential classifies as `Unresolved`, which
  writes it to the vault with `.prev` keeping the old generation (§9.4 step 4).
- No profiles, so nothing is session-owned: the session-ownership checks arrive with M4, where
  profiles first exist.
- No usage data, so `list --json` rows report `usageStatus: "unavailable"` with
  `usageError: "no-data"` (an account never fetched), and the post-switch poll replan (§9.4
  "After unlocking") arrives with M2.

## Global Constraints

Every task's requirements include these. Values are copied from the spec.

- Platforms: macOS and Linux only. Nothing is built or tested for Windows (§1.2).
- Rust edition 2024, MSRV ≥ 1.85; `rust-toolchain.toml` pins the toolchain (§16).
- Keychain access only through the absolute path `/usr/bin/security`; never Security.framework
  (§4.4, Appendix A.3).
- JSON that is round-tripped uses `serde_json` with `preserve_order` and
  `arbitrary_precision`; `~/.claude.json` is never re-serialized, only spliced (§4.4, §9.5).
- SQLite via `rusqlite` with `bundled`; WAL, `busy_timeout = 5000`, `foreign_keys = ON`,
  `synchronous = NORMAL`; migrations embedded, forward-only, tracked by `PRAGMA user_version`
  (§6.1).
- Every file containing secrets is created with mode 0600 at creation time (`O_EXCL`, then
  write, then rename) and never chmod'ed afterwards (§5).
- Directories are created lazily with mode 0700: a command that changes nothing creates
  nothing (§5).
- `XDG_*` variables are honoured only when set to absolute paths (§5).
- Log lines identify accounts by position and ID, never by email (§4.4). Secrets never reach
  `Debug`, logs or error messages.
- `--json` stdout is exactly one JSON object; warnings and notices go to stderr (§13.2, B.36).
- Exit codes: `0` OK · `1` error · `2` usage error · `130` interrupted (§13.1).
- The Claude Code identity surface (§3) is the complete list of writes to CC-owned state.
- Lock order: tagteam mutation lock → account locks (ascending account ID) → CC credential
  locks (refresh → legacy) → CC config lock. `MutationGuard` is never taken while holding an
  account lock (§4.3).
- Tests never touch the real HOME or the login keychain (§15.1).
- Commits: small, imperative mood, no license headers, no agent attribution of any kind.

## Review Focus

Inputs and conditions the spec implies but no feature test would naturally hit. Each has a
pinning test in the task named.

1. **CC is mid-refresh (holds its credential lock) when the user switches.** Expected: the
   switch waits up to 9 s, then fails with a message naming the lock, and nothing changed.
   → Task 20.
2. **A prompt in a non-interactive context** (a script, a pipe, `--json`). Expected: never
   hangs; fails with a message naming `--yes`, or returns the JSON no-op. → Task 23.
3. **Two tagteam commands at once** (a double-fired alias, two terminals). Expected: one waits
   for the mutation lock; there is never a double switch or a torn store. → Task 21.
4. **A fresh machine** with no `~/.claude` and no `~/.claude.json`. Expected: `list` and
   `status` show nothing and create no files or directories; `add` fails with "no live
   login". → Task 23.
5. **A symlinked `~/.claude.json`** (dotfile managers). Expected: writes land in the link's
   target, the link stays a link, and the mode is preserved. → Task 14.

---

## File Structure

```
Cargo.toml                      workspace
rust-toolchain.toml
LICENSE, NOTICE, .gitignore
scripts/spikes/r1-keychain-ssh.sh
crates/tagteam-core/src/
  lib.rs
  ids.rs            ProviderId, AccountId, IdentityKey
  fingerprint.rs    Fingerprint (§2 "Generation")
  validate.rs       email regex, alias rules, account-ref parsing
  classify.rs       outgoing-credential classification (§9.4 step 4)
  rotation.rs       rotation target (§9.3)
crates/tagteam-provider/src/
  lib.rs
  read.rs           Read<T>, ReadError
  credential.rs     Credential, Provenance, FreshCredential
  env.rs            Env, harness guard
  clock.rs          Clock, SystemClock, FakeClock
  atomic.rs         symlink-following atomic writer
  splice.rs         span-preserving JSON splice
  keychain.rs       Keychain trait, FakeKeychain, FileKeychain (feature)
  security.rs       /usr/bin/security driver, Runner
  mkdir_lock.rs     proper-lockfile protocol with compromise detection
  flock.rs          FlockGuard, MutationGuard
  process.rs        ProcessStamp, liveness
  provider.rs       Provider trait and its types
crates/tagteam-cc/src/
  lib.rs
  paths.rs          Appendix A.1
  naming.rs         Appendix A.2
  shape.rs          Appendix A.4: kinds, fingerprints, compose
  config.rs         ~/.claude.json reads and splices
  live.rs           Appendix A.3: active credential and managed-key axes
  locks.rs          §9.1 CC lock set
  provider.rs       impl Provider for ClaudeCode
crates/tagteam-engine/src/
  lib.rs
  error.rs          EngineError
  store/mod.rs      Store
  store/schema.sql  migration 1 (§6.1, complete)
  vault.rs          VaultBackend, KeychainVault, FileVault, Vault
  account_lock.rs   AccountLock
  oracle.rs         Oracle port, NoOracle, FixedOracle (tests)
  registry.rs       ProviderRegistry
  engine.rs         Engine construction, guards, recovery hook
  lifecycle.rs      add, add-token, remove, alias, disable/enable, move
  refs.rs           account reference resolution (§10.4)
  views.rs          accounts / status views
  displace.rs       displaced/ storage (§6.3)
  switch.rs         the switch transaction (§9)
  recover.rs        interrupted-switch recovery (§9.6)
  hooks.rs          crash points for kill tests (feature "test-hooks")
crates/tagteam-engine/tests/common/mod.rs   shared fixture: home, fake Keychain, engine, invariant snapshot
crates/tagteam/src/
  main.rs, lib.rs   entry point
  cli.rs            clap surface
  app.rs            command dispatch
  render.rs         human and JSON output
  prompt.rs         TTY prompts
  root_guard.rs     root refusal (§5)
```

Tests live beside each crate in `tests/` (integration) or `#[cfg(test)] mod tests` (unit).

---
### Task 1: R1 spike — can `claude` read a tagteam-written Keychain item over SSH?

**Human step.** This task needs a real Mac, a GUI login session and SSH. The implementer
writes the script and records the results; Michael runs it. **Decision gate:** if `claude`
cannot read the item silently from SSH, stop and bring the results to Michael before any
further task — the Keychain approach in §4.4 and Appendix A.3 would need revisiting.

**Files:**
- Create: `scripts/spikes/r1-keychain-ssh.sh`
- Modify: `docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md` (§17 R1 row,
  Appendix A.3) with the findings

- [ ] **Step 1: Write the spike script**

```bash
#!/usr/bin/env bash
# R1 spike (spec §17): can `claude`, run from an SSH session, silently read a Keychain item
# that /usr/bin/security wrote? Uses a throwaway profile and a fake, never-expiring token, so
# no real login is touched and nothing is ever refreshed.
set -euo pipefail

PROFILE=/tmp/tagteam-r1-spike
SECURITY=/usr/bin/security
CRED='{"claudeAiOauth":{"accessToken":"sk-ant-oat01-r1-spike-not-a-real-token","refreshToken":"sk-ant-ort01-r1-spike-not-a-real-token","expiresAt":4102444800000,"scopes":["user:inference","user:profile"],"subscriptionType":"pro"}}'

acct() {
  local u="${USER:-}"
  if [[ "$u" =~ ^[a-zA-Z0-9._-]+$ ]]; then printf '%s' "$u"; else printf 'claude-code-user'; fi
}
svc() {
  printf 'Claude Code-credentials-%s' "$(printf '%s' "$PROFILE" | shasum -a 256 | cut -c1-8)"
}

keychain_state() {
  set +e; "$SECURITY" show-keychain-info 2>&1; echo "show-keychain-info rc=$?"; set -e
}
write() {
  mkdir -p "$PROFILE"; chmod 700 "$PROFILE"
  printf '%s\n' '{"oauthAccount":{"emailAddress":"r1-spike@example.com","organizationUuid":"","accountUuid":"00000000-0000-4000-8000-000000000001"},"hasCompletedOnboarding":true}' \
    > "$PROFILE/.claude.json"
  set +e
  "$SECURITY" add-generic-password -U -a "$(acct)" -s "$(svc)" \
    -X "$(printf '%s' "$CRED" | xxd -p | tr -d '\n')"
  echo "write: rc=$? service=$(svc) account=$(acct)"
  set -e
}
read_security() {
  set +e
  out=$("$SECURITY" find-generic-password -a "$(acct)" -w -s "$(svc)" 2>/dev/null); rc=$?
  set -e
  if [ "$out" = "$CRED" ]; then m=yes; else m=no; fi
  echo "security read: rc=$rc bytes-match=$m"
}
probe() {
  set +e; "$SECURITY" find-generic-password -a "$(acct)" -s "$(svc)" >/dev/null 2>&1
  echo "existence probe (no -w): rc=$?"; set -e
}
read_claude() {
  local start=$SECONDS
  set +e
  out=$(env -u CLAUDE_SECURESTORAGE_CONFIG_DIR -u CLAUDE_CODE_OAUTH_TOKEN -u CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR -u CLAUDE_CODE_API_KEY_FILE_DESCRIPTOR -u ANTHROPIC_API_KEY -u ANTHROPIC_AUTH_TOKEN CLAUDE_CONFIG_DIR="$PROFILE" claude auth status --json 2>&1); rc=$?
  set -e
  echo "claude auth status: rc=$rc elapsed=$((SECONDS - start))s"
  echo "$out"
}
cleanup() {
  "$SECURITY" delete-generic-password -a "$(acct)" -s "$(svc)" >/dev/null 2>&1 || true
  rm -rf "$PROFILE"; echo "cleaned up"
}
negative_control() {
  "$SECURITY" delete-generic-password -a "$(acct)" -s "$(svc)" >/dev/null 2>&1 || true
  echo "negative control: item deleted, profile kept"
  read_claude
}
# The existence probe against an explicitly locked, throwaway keychain file (never the login
# keychain): answers what `find-generic-password` without -w returns when locked.
locked_probe() {
  lp_dir=$(mktemp -d)
  lp_kc=$lp_dir/r1-locked.keychain
  trap '"$SECURITY" delete-keychain "$lp_kc" >/dev/null 2>&1; rm -rf "$lp_dir"' EXIT
  "$SECURITY" create-keychain -p r1 "$lp_kc"
  "$SECURITY" add-generic-password -a probe -s tagteam-r1 -w x "$lp_kc"
  "$SECURITY" lock-keychain "$lp_kc"
  set +e
  "$SECURITY" show-keychain-info "$lp_kc" >/dev/null 2>&1; echo "locked keychain info: rc=$?"
  "$SECURITY" find-generic-password -a probe -s tagteam-r1 "$lp_kc" >/dev/null 2>&1; echo "probe, present item: rc=$?"
  "$SECURITY" find-generic-password -a missing -s tagteam-r1 "$lp_kc" >/dev/null 2>&1; echo "probe, absent item: rc=$?"
  set -e
}

case "${1:-}" in
  keychain_state|write|read_security|probe|read_claude|cleanup|negative_control|locked_probe) "$1" ;;
  all) keychain_state; write; read_security; probe; read_claude ;;
  *) echo "usage: $0 all|keychain_state|write|read_security|probe|read_claude|cleanup|negative_control|locked_probe" >&2; exit 2 ;;
esac
```

- [ ] **Step 2: Make it executable and lint it**

Run: `chmod +x scripts/spikes/r1-keychain-ssh.sh && bash -n scripts/spikes/r1-keychain-ssh.sh`
Expected: no output, exit 0.

- [ ] **Step 3: Commit the script**

```bash
git add scripts/spikes/r1-keychain-ssh.sh
git commit -m "Add the R1 Keychain-over-SSH spike script"
```

- [ ] **Step 4: Michael runs the scenarios** (Remote Login must be on: System Settings →
  General → Sharing). Watch the Mac's screen for any Keychain dialog during SSH steps.

| # | Where each step runs | Commands |
|---|---|---|
| A | GUI Terminal | `all`, then `negative_control`, then `cleanup` |
| B | `ssh localhost` | `all`, then `negative_control`, then `cleanup` |
| C | GUI `write`; then over SSH | SSH: `read_security`, `probe`, `read_claude`; then `cleanup` |
| D | SSH `write`; then GUI | GUI: `read_security`, `read_claude`; then `cleanup` |
| E | GUI Terminal | `locked_probe` (a throwaway keychain file; the login keychain is never locked) |

For each scenario record: every `rc`, `bytes-match`, `elapsed`, whether a dialog appeared,
and the `claude auth status` JSON (`loggedIn`, `authMethod`, `email`). The output contains no
real secret; still, keep it out of git (paste it into the task report).

- [ ] **Step 5: Decide and record**

R1 passes when, in B and C, `security read` is `rc=0 bytes-match=yes`, `claude auth status`
reports `loggedIn: true` with `email: r1-spike@example.com`, and no dialog appeared. In A and B, `negative_control` reports `loggedIn: false` (proving `claude` authenticated from the spike's item, not from an ambient variable or the unsuffixed default item).

- **Pass:** edit the spec. In §17 change the R1 row's mitigation cell to begin
  `Verified on <date>, macOS <version>, CC <version>: ` followed by the observed behaviour in
  one sentence. In Appendix A.3 add a bullet under the existence probe stating the `rc` the
  probe returned for a present and an absent item in the explicitly locked keychain
  (scenario E), and in the SSH session. This answers the §15.4 locked-Keychain item.
- **Fail:** stop. Report the table to Michael; do not start Task 2.

- [ ] **Step 6: Commit the spec update (pass only)**

```bash
git add docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md
git commit -m "Record the R1 Keychain-over-SSH spike result"
```

---

### Task 2: Workspace scaffold

**Files:**
- Create: `Cargo.toml`, `rust-toolchain.toml`, `LICENSE`, `NOTICE`
- Modify: `.gitignore` (create if absent)
- Create: `crates/tagteam-core/{Cargo.toml,src/lib.rs}`,
  `crates/tagteam-provider/{Cargo.toml,src/lib.rs}`, `crates/tagteam-cc/{Cargo.toml,src/lib.rs}`,
  `crates/tagteam-engine/{Cargo.toml,src/lib.rs}`,
  `crates/tagteam/{Cargo.toml,src/main.rs,src/lib.rs}`
- Test: `crates/tagteam/tests/cli_smoke.rs`

**Interfaces:**
- Produces: `tagteam::main_with_args(args) -> i32` (the binary's entry point; later tasks
  extend it).

- [ ] **Step 1: Write the workspace manifest and toolchain pin**

`Cargo.toml`:
```toml
[workspace]
resolver = "3"
members = ["crates/*"]

[workspace.package]
version = "0.1.0"
edition = "2024"
rust-version = "1.85"
license = "MIT"
repository = "https://github.com/michaelrigart/tagteam"

[workspace.dependencies]
tagteam-core = { path = "crates/tagteam-core", version = "0.1.0" }
tagteam-provider = { path = "crates/tagteam-provider", version = "0.1.0" }
tagteam-cc = { path = "crates/tagteam-cc", version = "0.1.0" }
tagteam-engine = { path = "crates/tagteam-engine", version = "0.1.0" }
clap = { version = "4", features = ["derive"] }
fastrand = "2"
hex = "0.4"
libc = "0.2"
rpassword = "7"
rusqlite = { version = "0.40", features = ["bundled"] }
serde = { version = "1", features = ["derive"] }
serde_json = { version = "1", features = ["preserve_order", "arbitrary_precision"] }
sha2 = "0.10"
thiserror = "2"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
unicode-normalization = "0.1"
uuid = { version = "1", features = ["v7"] }
# test-only
assert_cmd = "2"
predicates = "3"
tempfile = "3"

[workspace.lints.rust]
unsafe_op_in_unsafe_fn = "deny"

[workspace.lints.clippy]
dbg_macro = "deny"
```

`rust-toolchain.toml`:
```toml
[toolchain]
channel = "1.88.0"
components = ["rustfmt", "clippy"]
```

`.gitignore` (append these lines if the file exists):
```
/target
```

- [ ] **Step 2: Write LICENSE and NOTICE**

`LICENSE`: the standard MIT license text with the line `Copyright (c) 2026 Michaël Rigart`.

`NOTICE`:
```
tagteam
Copyright (c) 2026 Michaël Rigart

tagteam is a Rust rewrite of claude-swap (https://github.com/realiti4/claude-swap),
Copyright (c) Onur Cetinkol, released under the MIT license. Its behaviour, constants and
safety rules are derived from claude-swap v0.27.0b1 (commit 9aa6d02).
```

- [ ] **Step 3: Write the five crate manifests and empty libraries**

`crates/tagteam-core/Cargo.toml`:
```toml
[package]
name = "tagteam-core"
description = "Provider-neutral domain types and policy for tagteam"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
hex.workspace = true
sha2.workspace = true
thiserror.workspace = true

[lints]
workspace = true
```

`crates/tagteam-provider/Cargo.toml`:
```toml
[package]
name = "tagteam-provider"
description = "The tagteam Provider trait and the I/O primitives providers share"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[features]
# A directory-backed fake Keychain, for tests that drive the real binary.
file-keychain = []
# Tests against a temporary real keychain via /usr/bin/security (macOS CI).
real_keychain = []

[dependencies]
tagteam-core.workspace = true
fastrand.workspace = true
hex.workspace = true
libc.workspace = true
serde_json.workspace = true
thiserror.workspace = true

[dev-dependencies]
tempfile.workspace = true

[lints]
workspace = true
```

`crates/tagteam-cc/Cargo.toml`:
```toml
[package]
name = "tagteam-cc"
description = "The Claude Code provider for tagteam"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
tagteam-core.workspace = true
tagteam-provider.workspace = true
fastrand.workspace = true
hex.workspace = true
libc.workspace = true
serde_json.workspace = true
sha2.workspace = true
thiserror.workspace = true
unicode-normalization.workspace = true

[dev-dependencies]
tagteam-provider = { workspace = true, features = ["file-keychain"] }
tempfile.workspace = true

[lints]
workspace = true
```

`crates/tagteam-engine/Cargo.toml`:
```toml
[package]
name = "tagteam-engine"
description = "The tagteam store, vault and provider-generic operations"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[features]
# Crash points for the kill tests (§15.2). Never enabled in release builds.
test-hooks = []

[dependencies]
tagteam-core.workspace = true
tagteam-provider.workspace = true
fastrand.workspace = true
rusqlite.workspace = true
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
tracing.workspace = true
uuid.workspace = true

[dev-dependencies]
tagteam-cc.workspace = true
tagteam-provider = { workspace = true, features = ["file-keychain"] }
tempfile.workspace = true

[lints]
workspace = true
```

`crates/tagteam/Cargo.toml`:
```toml
[package]
name = "tagteam"
description = "Multi-account switcher for AI coding agent CLIs"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[features]
# Test support for driving the real binary: file-backed fake Keychain and crash points.
test-support = ["tagteam-provider/file-keychain", "tagteam-engine/test-hooks"]

[dependencies]
tagteam-core.workspace = true
tagteam-provider.workspace = true
tagteam-cc.workspace = true
tagteam-engine.workspace = true
clap.workspace = true
libc.workspace = true
rpassword.workspace = true
serde_json.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true

[dev-dependencies]
tagteam-provider = { workspace = true, features = ["file-keychain"] }
assert_cmd.workspace = true
predicates.workspace = true
tempfile.workspace = true

[lints]
workspace = true
```

Each of `crates/tagteam-core/src/lib.rs`, `crates/tagteam-provider/src/lib.rs`,
`crates/tagteam-cc/src/lib.rs` and `crates/tagteam-engine/src/lib.rs` starts as a single line:
```rust
#![forbid(unsafe_code)]
```
except `tagteam-provider` and `tagteam-cc`, which call libc and start empty instead.

- [ ] **Step 4: Write the failing smoke test**

`crates/tagteam/tests/cli_smoke.rs`:
```rust
use assert_cmd::Command;

#[test]
fn prints_its_version() {
    Command::cargo_bin("tagteam")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout("tagteam 0.1.0\n");
}
```

Run: `cargo test -p tagteam --test cli_smoke`
Expected: FAIL — the binary target does not exist yet.

- [ ] **Step 5: Write the entry point**

`crates/tagteam/src/main.rs`:
```rust
fn main() {
    std::process::exit(tagteam::main_with_args(std::env::args_os()));
}
```

`crates/tagteam/src/lib.rs`:
```rust
use std::ffi::OsString;

use clap::Parser;

#[derive(Parser)]
#[command(name = "tagteam", version, about = "Multi-account switcher for AI coding agent CLIs")]
struct Cli {}

/// Runs the CLI and returns the process exit code.
pub fn main_with_args<I, T>(args: I) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    match Cli::try_parse_from(args) {
        Ok(_) => 0,
        Err(e) => {
            let code = if e.use_stderr() { 2 } else { 0 };
            let _ = e.print();
            code
        }
    }
}
```

- [ ] **Step 6: Run the tests and the lints**

Run: `cargo test --workspace && cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS, no formatting diff, no clippy warnings.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock rust-toolchain.toml LICENSE NOTICE .gitignore crates
git commit -m "Scaffold the Cargo workspace and the tagteam binary"
```

---

### Task 3: Core identifiers and fingerprints

**Files:**
- Create: `crates/tagteam-core/src/ids.rs`, `crates/tagteam-core/src/fingerprint.rs`
- Modify: `crates/tagteam-core/src/lib.rs`

**Interfaces:**
- Produces:
  - `ProviderId::new(impl Into<String>) -> ProviderId`, `.as_str() -> &str`, `Display`;
    `pub const CLAUDE_CODE: &str = "claude-code"`
  - `AccountId::from_string(impl Into<String>) -> AccountId`, `.as_str()`, `Display`, `Ord`
  - `IdentityKey::new(impl Into<String>)`, `.as_str()`
  - `Fingerprint::of_secret(&[u8]) -> Fingerprint`, `Fingerprint::parse(&str) -> Option<Fingerprint>`,
    `.as_str() -> &str` (`"sha256:<64 hex>"`), `.short12() -> &str` (first 12 hex digits)

- [ ] **Step 1: Write the failing tests**

`crates/tagteam-core/src/fingerprint.rs` (tests first; the implementation follows in Step 3):
```rust
#[cfg(test)]
mod tests {
    use super::Fingerprint;

    #[test]
    fn fingerprint_is_prefixed_lowercase_sha256_hex() {
        let fp = Fingerprint::of_secret(b"abc");
        assert_eq!(
            fp.as_str(),
            "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(fp.short12(), "ba7816bf8f01");
    }

    #[test]
    fn parse_accepts_only_the_canonical_form() {
        let fp = Fingerprint::of_secret(b"rt-1");
        assert_eq!(Fingerprint::parse(fp.as_str()), Some(fp));
        assert_eq!(Fingerprint::parse("sha256:ABC"), None);
        assert_eq!(Fingerprint::parse(&"sha256:".repeat(1)), None);
        let upper = Fingerprint::of_secret(b"rt-1").as_str().to_uppercase();
        assert_eq!(Fingerprint::parse(&upper), None);
        assert_eq!(Fingerprint::parse("md5:a33d8c625833429df4658aa6f6940675ca829051a620ed398517039d4a1fc7ec"), None);
    }

    #[test]
    fn different_secrets_differ() {
        assert_ne!(Fingerprint::of_secret(b"a"), Fingerprint::of_secret(b"b"));
    }
}
```

`crates/tagteam-core/src/ids.rs` tests:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_their_strings() {
        assert_eq!(ProviderId::new(CLAUDE_CODE).as_str(), "claude-code");
        assert_eq!(AccountId::from_string("0192").to_string(), "0192");
        assert_eq!(IdentityKey::new("a@b.co\n").as_str(), "a@b.co\n");
    }

    #[test]
    fn account_ids_order_as_strings() {
        let mut v = vec![AccountId::from_string("b"), AccountId::from_string("a")];
        v.sort();
        assert_eq!(v, vec![AccountId::from_string("a"), AccountId::from_string("b")]);
    }
}
```

`crates/tagteam-core/src/lib.rs`:
```rust
#![forbid(unsafe_code)]

pub mod fingerprint;
pub mod ids;

pub use fingerprint::Fingerprint;
pub use ids::{AccountId, CLAUDE_CODE, IdentityKey, ProviderId};
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tagteam-core`
Expected: FAIL to compile — `Fingerprint`, `ProviderId` and friends are not defined.

- [ ] **Step 3: Implement**

Top of `crates/tagteam-core/src/fingerprint.rs`:
```rust
use sha2::{Digest, Sha256};

/// The fingerprint of one credential generation (§2 "Generation"):
/// `sha256:<lowercase hex of sha256(secret)>`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Fingerprint(String);

impl Fingerprint {
    pub fn of_secret(secret: &[u8]) -> Self {
        let digest = Sha256::digest(secret);
        Self(format!("sha256:{}", hex::encode(digest.as_slice())))
    }

    pub fn parse(s: &str) -> Option<Self> {
        let hex = s.strip_prefix("sha256:")?;
        let canonical = hex.len() == 64
            && hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        canonical.then(|| Self(s.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The first 12 hex digits, used in file names (§5).
    pub fn short12(&self) -> &str {
        &self.0[7..19]
    }
}
```

Top of `crates/tagteam-core/src/ids.rs`:
```rust
use std::fmt;

pub const CLAUDE_CODE: &str = "claude-code";

macro_rules! string_id {
    ($name:ident, $ctor:ident) => {
        #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(String);

        impl $name {
            pub fn $ctor(s: impl Into<String>) -> Self {
                Self(s.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

string_id!(ProviderId, new);
string_id!(AccountId, from_string);
string_id!(IdentityKey, new);
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p tagteam-core`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/tagteam-core
git commit -m "Add core identifiers and credential fingerprints"
```

---

### Task 4: Validation rules and account-reference parsing

**Files:**
- Create: `crates/tagteam-core/src/validate.rs`
- Modify: `crates/tagteam-core/src/lib.rs`

**Interfaces:**
- Produces:
  - `is_valid_email(&str) -> bool` — the §10.2 regex `^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$`
  - `normalize_alias(&str) -> Result<String, AliasError>` — lowercases, then enforces §10.3
  - `enum AliasError { Empty, Charset, AllDigits, LeadingDash }` (thiserror, `Display`)
  - `enum AccountRefInput { Position(u32), Text(String) }`;
    `parse_account_ref(&str) -> Option<AccountRefInput>` (`None` for empty input)

- [ ] **Step 1: Write the failing tests**

Append to `crates/tagteam-core/src/validate.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_matches_the_spec_regex() {
        for ok in ["a@b.co", "first.last+tag@sub.example.org", "x_1%y@host-name.io", "a@..co"] {
            assert!(is_valid_email(ok), "{ok}");
        }
        for bad in ["", "a", "a@b", "a@b.c", "@b.co", "a@.co", "a@b.c0", "a@@b.co", "a b@c.co", "a@b.co ", "é@b.co"] {
            assert!(!is_valid_email(bad), "{bad}");
        }
    }

    #[test]
    fn alias_is_lowercased_then_checked() {
        assert_eq!(normalize_alias("Work.Main").unwrap(), "work.main");
        assert_eq!(normalize_alias("a-1_b").unwrap(), "a-1_b");
        assert_eq!(normalize_alias(""), Err(AliasError::Empty));
        assert_eq!(normalize_alias("has space"), Err(AliasError::Charset));
        assert_eq!(normalize_alias("café"), Err(AliasError::Charset));
        assert_eq!(normalize_alias("123"), Err(AliasError::AllDigits));
        assert_eq!(normalize_alias("-x"), Err(AliasError::LeadingDash));
    }

    #[test]
    fn account_refs_split_positions_from_text() {
        assert_eq!(parse_account_ref("3"), Some(AccountRefInput::Position(3)));
        assert_eq!(parse_account_ref("007"), Some(AccountRefInput::Position(7)));
        assert_eq!(parse_account_ref("work"), Some(AccountRefInput::Text("work".into())));
        assert_eq!(parse_account_ref("a@b.co"), Some(AccountRefInput::Text("a@b.co".into())));
        assert_eq!(parse_account_ref(""), None);
        // All digits but too large for a position: treated as text, which never matches an alias.
        assert_eq!(
            parse_account_ref("99999999999"),
            Some(AccountRefInput::Text("99999999999".into()))
        );
    }
}
```

Add `pub mod validate;` to `lib.rs`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tagteam-core validate`
Expected: FAIL to compile — functions not defined.

- [ ] **Step 3: Implement**

Top of `crates/tagteam-core/src/validate.rs`:
```rust
/// `^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$` (§10.2), without a regex engine.
pub fn is_valid_email(s: &str) -> bool {
    let Some((local, domain)) = s.split_once('@') else {
        return false;
    };
    let local_ok = !local.is_empty()
        && local
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._%+-".contains(&b));
    // The TLD is all letters, so it can only follow the last dot.
    let Some((host, tld)) = domain.rsplit_once('.') else {
        return false;
    };
    local_ok
        && !host.is_empty()
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
        && tld.len() >= 2
        && tld.bytes().all(|b| b.is_ascii_alphabetic())
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AliasError {
    #[error("an alias cannot be empty")]
    Empty,
    #[error("an alias may contain only a-z, 0-9, '_', '.' and '-'")]
    Charset,
    #[error("an alias cannot be all digits, which would read as a position")]
    AllDigits,
    #[error("an alias cannot start with '-'")]
    LeadingDash,
}

/// Aliases are lowercase and match `^[a-z0-9_.-]+$`; they cannot be all digits or start
/// with `-` (§10.3).
pub fn normalize_alias(s: &str) -> Result<String, AliasError> {
    let a = s.to_ascii_lowercase();
    if a.is_empty() {
        return Err(AliasError::Empty);
    }
    if !a
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_.-".contains(&b))
    {
        return Err(AliasError::Charset);
    }
    if a.bytes().all(|b| b.is_ascii_digit()) {
        return Err(AliasError::AllDigits);
    }
    if a.starts_with('-') {
        return Err(AliasError::LeadingDash);
    }
    Ok(a)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountRefInput {
    Position(u32),
    Text(String),
}

/// First stage of §10.4: all digits is a position; anything else is an alias or email.
pub fn parse_account_ref(s: &str) -> Option<AccountRefInput> {
    if s.is_empty() {
        return None;
    }
    if s.bytes().all(|b| b.is_ascii_digit()) {
        if let Ok(p) = s.parse::<u32>() {
            return Some(AccountRefInput::Position(p));
        }
    }
    Some(AccountRefInput::Text(s.to_owned()))
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p tagteam-core validate`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/tagteam-core
git commit -m "Add email, alias and account-reference validation"
```

---

### Task 5: Outgoing-credential classification and rotation

**Files:**
- Create: `crates/tagteam-core/src/classify.rs`, `crates/tagteam-core/src/rotation.rs`
- Modify: `crates/tagteam-core/src/lib.rs`

**Interfaces:**
- Produces:
  - `enum OracleVerdict { Unavailable, ThisAccount, OtherIdentity }`
  - `struct OutgoingFacts { bytes_equal_vault: bool, fp_equal_vault: bool, wiped: bool, oracle: OracleVerdict, lacks_refresh_over_complete: bool }`
  - `enum OutgoingClass { Ours, Wiped, OursRotated, Foreign, Unresolved }`
  - `enum OutgoingAction { Nothing, CaptureToVault { backfill_uuid: bool }, Displace }`
  - `decide_outgoing(&OutgoingFacts) -> (OutgoingClass, OutgoingAction)`
  - `next_in_rotation(accounts: &[(u32, bool)], anchor: Option<u32>) -> Option<u32>` —
    `accounts` is `(position, switchable)` sorted by position

- [ ] **Step 1: Write the failing tests**

`crates/tagteam-core/src/classify.rs` tests:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> OutgoingFacts {
        OutgoingFacts {
            bytes_equal_vault: false,
            fp_equal_vault: false,
            wiped: false,
            oracle: OracleVerdict::Unavailable,
            lacks_refresh_over_complete: false,
        }
    }

    #[test]
    fn ours_by_bytes_or_fingerprint() {
        let f = OutgoingFacts { bytes_equal_vault: true, ..facts() };
        assert_eq!(decide_outgoing(&f), (OutgoingClass::Ours, OutgoingAction::Nothing));
        let f = OutgoingFacts { fp_equal_vault: true, ..facts() };
        assert_eq!(decide_outgoing(&f), (OutgoingClass::Ours, OutgoingAction::Nothing));
    }

    #[test]
    fn wiped_keeps_the_vault() {
        let f = OutgoingFacts { wiped: true, oracle: OracleVerdict::ThisAccount, ..facts() };
        assert_eq!(decide_outgoing(&f), (OutgoingClass::Wiped, OutgoingAction::Nothing));
    }

    #[test]
    fn oracle_decides_rotated_or_foreign() {
        let f = OutgoingFacts { oracle: OracleVerdict::ThisAccount, ..facts() };
        assert_eq!(
            decide_outgoing(&f),
            (OutgoingClass::OursRotated, OutgoingAction::CaptureToVault { backfill_uuid: true })
        );
        let f = OutgoingFacts { oracle: OracleVerdict::OtherIdentity, ..facts() };
        assert_eq!(decide_outgoing(&f), (OutgoingClass::Foreign, OutgoingAction::Displace));
    }

    #[test]
    fn no_verdict_is_unresolved_and_captured() {
        assert_eq!(
            decide_outgoing(&facts()),
            (OutgoingClass::Unresolved, OutgoingAction::CaptureToVault { backfill_uuid: false })
        );
    }

    #[test]
    fn a_blob_without_a_refresh_token_never_replaces_a_complete_one() {
        for oracle in [OracleVerdict::ThisAccount, OracleVerdict::Unavailable] {
            let f = OutgoingFacts { oracle, lacks_refresh_over_complete: true, ..facts() };
            assert_eq!(decide_outgoing(&f).1, OutgoingAction::Displace);
        }
    }
}
```

`crates/tagteam-core/src/rotation.rs` tests:
```rust
#[cfg(test)]
mod tests {
    use super::next_in_rotation;

    #[test]
    fn picks_the_next_switchable_position_and_wraps() {
        let a = [(1, true), (2, false), (3, true), (5, true)];
        assert_eq!(next_in_rotation(&a, Some(1)), Some(3));
        assert_eq!(next_in_rotation(&a, Some(3)), Some(5));
        assert_eq!(next_in_rotation(&a, Some(5)), Some(1));
        assert_eq!(next_in_rotation(&a, Some(2)), Some(3));
    }

    #[test]
    fn no_anchor_takes_the_first_switchable() {
        assert_eq!(next_in_rotation(&[(2, false), (4, true)], None), Some(4));
    }

    #[test]
    fn only_the_anchor_switchable_yields_none() {
        assert_eq!(next_in_rotation(&[(1, true), (2, false)], Some(1)), None);
        assert_eq!(next_in_rotation(&[], None), None);
    }
}
```

Update `crates/tagteam-core/src/lib.rs` to:
```rust
#![forbid(unsafe_code)]

pub mod classify;
pub mod fingerprint;
pub mod ids;
pub mod rotation;
pub mod validate;

pub use classify::{OracleVerdict, OutgoingAction, OutgoingClass, OutgoingFacts, decide_outgoing};
pub use fingerprint::Fingerprint;
pub use ids::{AccountId, CLAUDE_CODE, IdentityKey, ProviderId};
pub use rotation::next_in_rotation;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tagteam-core`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

Top of `classify.rs`:
```rust
/// What the identity oracle (§7.6) said about the live credential, already discarded if the
/// live bytes changed after it was asked (§9.4 step 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OracleVerdict {
    Unavailable,
    ThisAccount,
    OtherIdentity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutgoingFacts {
    pub bytes_equal_vault: bool,
    pub fp_equal_vault: bool,
    /// An OAuth blob with both tokens empty: CC's reaction to `invalid_grant`.
    pub wiped: bool,
    pub oracle: OracleVerdict,
    /// The live credential lacks a refresh token while the vault's has one (§6.2).
    pub lacks_refresh_over_complete: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutgoingClass {
    Ours,
    Wiped,
    OursRotated,
    Foreign,
    Unresolved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutgoingAction {
    Nothing,
    CaptureToVault { backfill_uuid: bool },
    Displace,
}

/// The §9.4 step 4 table, with the §6.2 rule that an automatic capture never replaces a
/// refresh token with a credential that lacks one.
pub fn decide_outgoing(f: &OutgoingFacts) -> (OutgoingClass, OutgoingAction) {
    if f.bytes_equal_vault || f.fp_equal_vault {
        return (OutgoingClass::Ours, OutgoingAction::Nothing);
    }
    if f.wiped {
        return (OutgoingClass::Wiped, OutgoingAction::Nothing);
    }
    let (class, backfill_uuid) = match f.oracle {
        OracleVerdict::ThisAccount => (OutgoingClass::OursRotated, true),
        OracleVerdict::OtherIdentity => return (OutgoingClass::Foreign, OutgoingAction::Displace),
        OracleVerdict::Unavailable => (OutgoingClass::Unresolved, false),
    };
    if f.lacks_refresh_over_complete {
        (class, OutgoingAction::Displace)
    } else {
        (class, OutgoingAction::CaptureToVault { backfill_uuid })
    }
}
```

Top of `rotation.rs`:
```rust
/// The next switchable position after `anchor`, wrapping around (§9.3 rotation). With no
/// anchor, the first switchable position. `None` when no other account is switchable.
pub fn next_in_rotation(accounts: &[(u32, bool)], anchor: Option<u32>) -> Option<u32> {
    let mut switchable = accounts.iter().filter(|(_, s)| *s).map(|(p, _)| *p);
    match anchor {
        None => switchable.next(),
        Some(a) => {
            let all: Vec<u32> = switchable.collect();
            all.iter()
                .copied()
                .find(|p| *p > a)
                .or_else(|| all.iter().copied().find(|p| *p != a))
        }
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p tagteam-core`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tagteam-core
git commit -m "Add outgoing-credential classification and rotation"
```

---
### Task 6: Tri-state reads, credentials with provenance, Env and Clock

**Files:**
- Create: `crates/tagteam-provider/src/{read.rs,credential.rs,env.rs,clock.rs}`
- Modify: `crates/tagteam-provider/src/lib.rs`

**Interfaces:**
- Produces:
  - `enum Read<T> { Present(T), Absent, Unreadable(ReadError) }` with `map`, `as_ref`,
    `is_present`, `present() -> Option<T>`; `struct ReadError { what: String, detail: String }`
    with `ReadError::new(what, detail)`
  - `enum Provenance { Fresh, Degraded }`; `struct Credential` with `Credential::fresh(Vec<u8>)`,
    `Credential::degraded(Vec<u8>)`, `.bytes() -> &[u8]`, `.provenance()`, `.is_empty()`,
    `.into_fresh() -> Option<FreshCredential>`; `Debug` never prints the bytes
  - `struct FreshCredential` with `.credential() -> &Credential` (constructible only via
    `into_fresh`)
  - `struct Env { home, user, xdg_config_home, xdg_data_home, xdg_state_home, claude_config_dir,
    claude_securestorage_config_dir }` (all `pub`), `Env::from_process()`, `Env::for_test(&Path)`,
    `.with_forbidden_root(PathBuf)`, `.data_dir()`, `.config_dir()`, `.state_dir()`,
    `.guard(PathBuf) -> PathBuf`, `.inside_run_shell() -> bool`
  - `trait Clock: Send + Sync { fn now_ms(&self) -> i64; }`, `SystemClock`, `FakeClock::new(i64)`
    with `.set(i64)` and `.advance_ms(i64)`

- [ ] **Step 1: Write the failing tests**

`crates/tagteam-provider/src/read.rs` tests:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreadable_is_never_absent() {
        let r: Read<u8> = Read::Unreadable(ReadError::new("keychain", "rc 36"));
        assert!(!r.is_present());
        assert!(matches!(r.map(|v| v + 1), Read::Unreadable(e) if e.detail == "rc 36"));
        let a: Read<u8> = Read::Absent;
        assert!(a.present().is_none());
        assert_eq!(Read::Present(2).map(|v| v * 2).present(), Some(4));
    }
}
```

`crates/tagteam-provider/src/credential.rs` tests:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_prints_secret_bytes() {
        let c = Credential::fresh(b"sk-ant-ort01-secret".to_vec());
        let shown = format!("{c:?}");
        assert!(!shown.contains("secret"), "{shown}");
        assert!(shown.contains("19 bytes"), "{shown}");
    }

    #[test]
    fn degraded_credentials_cannot_become_fresh() {
        assert!(Credential::degraded(b"x".to_vec()).into_fresh().is_none());
        let fresh = Credential::fresh(b"x".to_vec()).into_fresh().unwrap();
        assert_eq!(fresh.credential().bytes(), b"x");
    }
}
```

`crates/tagteam-provider/src/env.rs` tests:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn xdg_defaults_live_under_home() {
        let env = Env::for_test(Path::new("/tmp/fixture"));
        assert_eq!(env.data_dir(), Path::new("/tmp/fixture/home/.local/share/tagteam"));
        assert_eq!(env.config_dir(), Path::new("/tmp/fixture/home/.config/tagteam"));
        assert_eq!(env.state_dir(), Path::new("/tmp/fixture/home/.local/state/tagteam"));
    }

    #[test]
    fn absolute_xdg_overrides_win() {
        let mut env = Env::for_test(Path::new("/tmp/fixture"));
        env.xdg_data_home = Some(PathBuf::from("/data"));
        assert_eq!(env.data_dir(), Path::new("/data/tagteam"));
    }

    #[test]
    #[should_panic(expected = "under the real HOME")]
    fn the_harness_guard_trips() {
        let env = Env::for_test(Path::new("/tmp/fixture"))
            .with_forbidden_root(PathBuf::from("/tmp/fixture/home"));
        let _ = env.data_dir();
    }

    #[test]
    fn a_run_shell_is_detected_from_claude_config_dir() {
        let mut env = Env::for_test(Path::new("/tmp/fixture"));
        assert!(!env.inside_run_shell());
        env.claude_config_dir =
            Some("/tmp/fixture/home/.local/share/tagteam/sessions/0192".into());
        assert!(env.inside_run_shell());
    }
}
```

`crates/tagteam-provider/src/clock.rs` tests:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_clock_moves_only_when_told() {
        let c = FakeClock::new(1_000);
        assert_eq!(c.now_ms(), 1_000);
        c.advance_ms(500);
        assert_eq!(c.now_ms(), 1_500);
        c.set(7);
        assert_eq!(c.now_ms(), 7);
        assert!(SystemClock.now_ms() > 1_700_000_000_000);
    }
}
```

`crates/tagteam-provider/src/lib.rs`:
```rust
pub mod clock;
pub mod credential;
pub mod env;
pub mod read;

pub use clock::{Clock, FakeClock, SystemClock};
pub use credential::{Credential, FreshCredential, Provenance};
pub use env::Env;
pub use read::{Read, ReadError};
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tagteam-provider`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

`read.rs`:
```rust
use std::fmt;

/// Every read of a credential, config, roster or session record (§4.3). `Unreadable` is
/// never collapsed into `Absent` or into an empty value.
#[derive(Debug, Clone)]
pub enum Read<T> {
    Present(T),
    Absent,
    Unreadable(ReadError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadError {
    pub what: String,
    pub detail: String,
}

impl ReadError {
    pub fn new(what: impl Into<String>, detail: impl Into<String>) -> Self {
        Self { what: what.into(), detail: detail.into() }
    }
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} is unreadable: {}", self.what, self.detail)
    }
}

impl<T> Read<T> {
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Read<U> {
        match self {
            Read::Present(v) => Read::Present(f(v)),
            Read::Absent => Read::Absent,
            Read::Unreadable(e) => Read::Unreadable(e),
        }
    }

    pub fn as_ref(&self) -> Read<&T> {
        match self {
            Read::Present(v) => Read::Present(v),
            Read::Absent => Read::Absent,
            Read::Unreadable(e) => Read::Unreadable(e.clone()),
        }
    }

    pub fn is_present(&self) -> bool {
        matches!(self, Read::Present(_))
    }

    pub fn present(self) -> Option<T> {
        match self {
            Read::Present(v) => Some(v),
            _ => None,
        }
    }
}
```

`credential.rs`:
```rust
use std::fmt;

/// `Degraded`: the Keychain lookup failed and the plaintext file covered it, so the bytes may
/// be a superseded generation (§4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provenance {
    Fresh,
    Degraded,
}

#[derive(Clone, PartialEq, Eq)]
pub struct Credential {
    bytes: Vec<u8>,
    provenance: Provenance,
}

impl Credential {
    pub fn fresh(bytes: Vec<u8>) -> Self {
        Self { bytes, provenance: Provenance::Fresh }
    }

    pub fn degraded(bytes: Vec<u8>) -> Self {
        Self { bytes, provenance: Provenance::Degraded }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn provenance(&self) -> Provenance {
        self.provenance
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    pub fn into_fresh(self) -> Option<FreshCredential> {
        (self.provenance == Provenance::Fresh).then_some(FreshCredential(self))
    }
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Credential(<{} bytes>, {:?})", self.bytes.len(), self.provenance)
    }
}

/// The only credential the refresh gate accepts. It cannot be built from a degraded read.
#[derive(Debug, Clone)]
pub struct FreshCredential(Credential);

impl FreshCredential {
    pub fn credential(&self) -> &Credential {
        &self.0
    }
}
```

`env.rs`:
```rust
use std::ffi::OsString;
use std::path::PathBuf;

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
    forbidden_root: Option<PathBuf>,
}

impl Env {
    pub fn from_process() -> Self {
        let abs = |k: &str| std::env::var_os(k).map(PathBuf::from).filter(|p| p.is_absolute());
        Self {
            home: std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| "/".into()),
            user: std::env::var("USER").ok(),
            xdg_config_home: abs("XDG_CONFIG_HOME"),
            xdg_data_home: abs("XDG_DATA_HOME"),
            xdg_state_home: abs("XDG_STATE_HOME"),
            claude_config_dir: std::env::var_os("CLAUDE_CONFIG_DIR"),
            claude_securestorage_config_dir: std::env::var_os("CLAUDE_SECURESTORAGE_CONFIG_DIR"),
            forbidden_root: None,
        }
    }

    /// A fixture environment rooted at `root`, with the harness guard armed against the
    /// real HOME.
    pub fn for_test(root: &std::path::Path) -> Self {
        Self {
            home: root.join("home"),
            user: Some("tester".into()),
            xdg_config_home: None,
            xdg_data_home: None,
            xdg_state_home: None,
            claude_config_dir: None,
            claude_securestorage_config_dir: None,
            forbidden_root: std::env::var_os("HOME").map(PathBuf::from),
        }
    }

    pub fn with_forbidden_root(mut self, root: PathBuf) -> Self {
        self.forbidden_root = Some(root);
        self
    }

    /// Panics in tests when a resolved path falls under the real HOME (§15.1).
    pub fn guard(&self, path: PathBuf) -> PathBuf {
        if let Some(root) = &self.forbidden_root {
            assert!(
                !path.starts_with(root),
                "test harness: resolved path {path:?} is under the real HOME"
            );
        }
        path
    }

    pub fn data_dir(&self) -> PathBuf {
        let base = self.xdg_data_home.clone().unwrap_or_else(|| self.home.join(".local/share"));
        self.guard(base.join("tagteam"))
    }

    pub fn config_dir(&self) -> PathBuf {
        let base = self.xdg_config_home.clone().unwrap_or_else(|| self.home.join(".config"));
        self.guard(base.join("tagteam"))
    }

    pub fn state_dir(&self) -> PathBuf {
        let base = self.xdg_state_home.clone().unwrap_or_else(|| self.home.join(".local/state"));
        self.guard(base.join("tagteam"))
    }

    /// `CLAUDE_CONFIG_DIR` under tagteam's `sessions/`: every command that changes accounts or
    /// the live login refuses there (§9.2, B.32).
    pub fn inside_run_shell(&self) -> bool {
        self.claude_config_dir
            .as_ref()
            .is_some_and(|d| PathBuf::from(d).starts_with(self.data_dir().join("sessions")))
    }
}
```

`clock.rs`:
```rust
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub trait Clock: Send + Sync {
    /// Wall-clock milliseconds since the Unix epoch.
    fn now_ms(&self) -> i64;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
    }
}

#[derive(Debug)]
pub struct FakeClock(AtomicI64);

impl FakeClock {
    pub fn new(ms: i64) -> Self {
        Self(AtomicI64::new(ms))
    }

    pub fn set(&self, ms: i64) {
        self.0.store(ms, Ordering::SeqCst);
    }

    pub fn advance_ms(&self, ms: i64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
}

impl Clock for FakeClock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p tagteam-provider`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tagteam-provider
git commit -m "Add tri-state reads, credential provenance, Env and Clock"
```

---

### Task 7: Atomic writes through symlinks

**Files:**
- Create: `crates/tagteam-provider/src/atomic.rs`
- Modify: `crates/tagteam-provider/src/lib.rs` (`pub mod atomic;`)

**Interfaces:**
- Produces:
  - `resolve_target(&Path) -> io::Result<PathBuf>` — follows a symlink chain (≤ 40 hops);
    a relative target joins the link's parent; a missing final target is returned as-is
  - `write_atomic_with<E: From<io::Error>>(path, bytes, new_mode, before_publish: impl Fn() -> Result<(), E>) -> Result<(), E>`
    — temp file beside the resolved target, `O_EXCL`, mode set before any byte is written,
    fsync, then `before_publish` immediately before the rename; the directory fsync after the
    rename is best-effort, so an error always means nothing was published. An existing file's
    mode is preserved, a new file gets `new_mode`
  - `write_atomic(path, bytes, new_mode) -> io::Result<()>` — the same with no check
  - `remove_target(&Path) -> io::Result<()>` — removes what the path resolves to, never the
    symlink itself; absent is success
  - `ensure_private_dir(&Path) -> io::Result<()>` — `create_dir_all` with mode 0700 for any
    directory it creates

- [ ] **Step 1: Write the failing tests**

Append to `crates/tagteam-provider/src/atomic.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn mode(p: &Path) -> u32 {
        fs::metadata(p).unwrap().permissions().mode() & 0o7777
    }

    #[test]
    fn new_files_get_the_requested_mode() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("creds.json");
        write_atomic(&p, b"{}", 0o600).unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"{}");
        assert_eq!(mode(&p), 0o600);
    }

    #[test]
    fn existing_mode_is_preserved() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.json");
        fs::write(&p, "old").unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();
        write_atomic(&p, b"new", 0o600).unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"new");
        assert_eq!(mode(&p), 0o644);
    }

    #[test]
    fn writes_land_in_the_symlink_target_and_the_link_survives() {
        let d = tempfile::tempdir().unwrap();
        let real = d.path().join("dotfiles/claude.json");
        fs::create_dir_all(real.parent().unwrap()).unwrap();
        fs::write(&real, "old").unwrap();
        let link = d.path().join(".claude.json");
        symlink("dotfiles/claude.json", &link).unwrap(); // relative target
        write_atomic(&link, b"new", 0o600).unwrap();
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(fs::read(&real).unwrap(), b"new");
    }

    #[test]
    fn a_dangling_link_creates_its_target() {
        let d = tempfile::tempdir().unwrap();
        let link = d.path().join("l");
        symlink(d.path().join("t"), &link).unwrap();
        write_atomic(&link, b"x", 0o600).unwrap();
        assert_eq!(fs::read(d.path().join("t")).unwrap(), b"x");
    }

    #[test]
    fn no_temp_files_are_left_behind() {
        let d = tempfile::tempdir().unwrap();
        write_atomic(&d.path().join("a"), b"1", 0o600).unwrap();
        write_atomic(&d.path().join("a"), b"2", 0o600).unwrap();
        let names: Vec<_> = fs::read_dir(d.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, vec![std::ffi::OsString::from("a")]);
    }

    #[test]
    fn a_missing_directory_is_an_error() {
        let d = tempfile::tempdir().unwrap();
        assert!(write_atomic(&d.path().join("nope/a"), b"1", 0o600).is_err());
    }

    #[test]
    fn a_symlink_loop_is_an_error() {
        let d = tempfile::tempdir().unwrap();
        symlink(d.path().join("b"), d.path().join("a")).unwrap();
        symlink(d.path().join("a"), d.path().join("b")).unwrap();
        assert!(resolve_target(&d.path().join("a")).is_err());
    }

    #[test]
    fn removing_through_a_link_keeps_the_link() {
        let d = tempfile::tempdir().unwrap();
        let link = d.path().join("l");
        symlink(d.path().join("t"), &link).unwrap(); // dangling
        write_atomic(&link, b"x", 0o600).unwrap(); // creates the target
        remove_target(&link).unwrap();
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert!(!d.path().join("t").exists());
        remove_target(&link).unwrap(); // absent: still fine
    }

    #[test]
    fn a_failed_pre_publication_check_publishes_nothing() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.json");
        fs::write(&p, "old").unwrap();
        let r = write_atomic_with(&p, b"new", 0o600, || Err(io::Error::other("lock lost")));
        assert!(r.is_err());
        assert_eq!(fs::read(&p).unwrap(), b"old");
        assert_eq!(fs::read_dir(d.path()).unwrap().count(), 1, "no temp file left");
    }

    #[test]
    fn private_dirs_are_0700() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("x/y");
        ensure_private_dir(&p).unwrap();
        assert_eq!(mode(&p), 0o700);
        assert_eq!(mode(&d.path().join("x")), 0o700);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tagteam-provider atomic`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

Top of `atomic.rs`:
```rust
use std::fs::{self, DirBuilder, File, OpenOptions, Permissions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub fn resolve_target(path: &Path) -> io::Result<PathBuf> {
    let mut p = path.to_path_buf();
    for _ in 0..40 {
        match fs::symlink_metadata(&p) {
            Ok(m) if m.file_type().is_symlink() => {
                let target = fs::read_link(&p)?;
                p = if target.is_absolute() {
                    target
                } else {
                    p.parent().unwrap_or(Path::new("/")).join(target)
                };
            }
            Ok(_) => return Ok(p),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(p),
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other(format!("too many levels of symbolic links at {}", path.display())))
}

/// The primitive every tagteam file write goes through (§9.5).
///
/// An error means the target was **not** replaced: everything that can fail happens before
/// the rename, and the directory fsync after it is best-effort. `before_publish` runs
/// immediately before the rename, so a lock holder can re-check ownership at the last moment
/// (§9.1); if it fails, nothing is published.
pub fn write_atomic_with<E: From<io::Error>>(
    path: &Path,
    bytes: &[u8],
    new_mode: u32,
    before_publish: impl Fn() -> Result<(), E>,
) -> Result<(), E> {
    let target = resolve_target(path)?;
    let dir = target
        .parent()
        .ok_or_else(|| io::Error::other(format!("{} has no parent", target.display())))?;
    let name = target
        .file_name()
        .ok_or_else(|| io::Error::other(format!("{} has no file name", target.display())))?;
    let mode = match fs::metadata(&target) {
        Ok(m) => m.permissions().mode() & 0o7777,
        Err(e) if e.kind() == io::ErrorKind::NotFound => new_mode,
        Err(e) => return Err(e.into()),
    };
    let tmp = dir.join(format!(
        ".{}.tagteam-{}-{:08x}",
        name.to_string_lossy(),
        std::process::id(),
        fastrand::u32(..)
    ));
    let mut file = OpenOptions::new().write(true).create_new(true).mode(mode).open(&tmp)?;
    let prepared = (|| {
        // Before any byte is written, so the umask can never widen a secret file.
        file.set_permissions(Permissions::from_mode(mode))?;
        file.write_all(bytes)?;
        file.sync_all()
    })();
    let published = prepared
        .map_err(E::from)
        .and_then(|()| before_publish())
        .and_then(|()| fs::rename(&tmp, &target).map_err(E::from));
    if published.is_err() {
        let _ = fs::remove_file(&tmp);
        return published;
    }
    // Published: from here on nothing may report failure.
    let _ = File::open(dir).and_then(|d| d.sync_all());
    Ok(())
}

/// Removes the file a path resolves to and leaves any symlink in place, the mirror image of
/// how writes land in the link's target (§9.5). Absent is success.
pub fn remove_target(path: &Path) -> io::Result<()> {
    match fs::remove_file(resolve_target(path)?) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// `write_atomic_with` with no pre-publication check.
pub fn write_atomic(path: &Path, bytes: &[u8], new_mode: u32) -> io::Result<()> {
    write_atomic_with(path, bytes, new_mode, || Ok::<(), io::Error>(()))
}

pub fn ensure_private_dir(path: &Path) -> io::Result<()> {
    DirBuilder::new().recursive(true).mode(0o700).create(path)
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p tagteam-provider atomic`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tagteam-provider
git commit -m "Add the symlink-following atomic writer"
```

---

### Task 8: Span-preserving JSON splice

**Files:**
- Create: `crates/tagteam-provider/src/splice.rs`
- Modify: `crates/tagteam-provider/src/lib.rs` (`pub mod splice;`)

**Interfaces:**
- Produces:
  - `enum SpliceError { NotObject, Torn(String) }` (thiserror)
  - `replace_top_level(doc: &[u8], key: &str, value: &serde_json::Value) -> Result<Vec<u8>, SpliceError>`
    — replaces the last occurrence of the top-level key's value span, or inserts the member
    before the closing `}` with 2-space indentation; only that span changes
  - `remove_top_level(doc: &[u8], key: &str) -> Result<Vec<u8>, SpliceError>` — removes the
    member and one adjoining comma; returns the input unchanged when the key is absent
  - `get_top_level(doc: &[u8], key: &str) -> Result<Option<serde_json::Value>, SpliceError>`
  - `render_nested(value: &serde_json::Value, depth: usize) -> String` —
    `JSON.stringify(v, null, 2)` layout, continuation lines indented by `2 * depth` spaces

- [ ] **Step 1: Write the failing tests**

Append to `crates/tagteam-provider/src/splice.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const DOC: &str = "{\n  \"numStartups\": 1e400,\n  \"projects\": {\n    \"/x\": {\n      \"oauthAccount\": \"nested, not top-level\"\n    }\n  },\n  \"oauthAccount\": {\n    \"emailAddress\": \"old@a.co\"\n  },\n  \"tipsHistory\": { \"x\": 0.1000 },\n  \"userID\": \"héllo ✓\"\n}\n";

    #[test]
    fn replace_changes_only_the_value_span() {
        let out = replace_top_level(DOC.as_bytes(), "oauthAccount", &json!({"emailAddress": "new@b.co"})).unwrap();
        let out = String::from_utf8(out).unwrap();
        let expected = DOC.replace(
            "{\n    \"emailAddress\": \"old@a.co\"\n  }",
            "{\n    \"emailAddress\": \"new@b.co\"\n  }",
        );
        assert_eq!(out, expected);
        assert!(out.contains("1e400") && out.contains("0.1000") && out.contains("nested, not top-level"));
    }

    #[test]
    fn a_missing_key_is_inserted_before_the_closing_brace() {
        let out = replace_top_level(b"{\n  \"a\": 1\n}\n", "k", &json!([1, {"b": null}])).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "{\n  \"a\": 1,\n  \"k\": [\n    1,\n    {\n      \"b\": null\n    }\n  ]\n}\n"
        );
    }

    #[test]
    fn an_empty_object_gets_its_first_member() {
        let out = replace_top_level(b"{}", "k", &json!("v")).unwrap();
        assert_eq!(out, b"{\n  \"k\": \"v\"\n}");
    }

    #[test]
    fn crlf_documents_keep_their_other_bytes() {
        let doc = b"{\r\n  \"a\": 1,\r\n  \"k\": 2\r\n}\r\n";
        let out = replace_top_level(doc, "k", &json!(3)).unwrap();
        assert_eq!(out, b"{\r\n  \"a\": 1,\r\n  \"k\": 3\r\n}\r\n");
    }

    #[test]
    fn remove_takes_one_adjoining_comma() {
        let doc = b"{\n  \"a\": 1,\n  \"k\": 2,\n  \"z\": 3\n}";
        assert_eq!(remove_top_level(doc, "k").unwrap(), b"{\n  \"a\": 1,\n  \"z\": 3\n}");
        assert_eq!(remove_top_level(doc, "a").unwrap(), b"{\n  \"k\": 2,\n  \"z\": 3\n}");
        assert_eq!(remove_top_level(doc, "z").unwrap(), b"{\n  \"a\": 1,\n  \"k\": 2\n}");
        assert_eq!(remove_top_level(b"{\"k\": 1}", "k").unwrap(), b"{}");
        assert_eq!(remove_top_level(doc, "missing").unwrap(), doc.to_vec());
        // Every duplicate goes, so an earlier value cannot come back.
        assert_eq!(remove_top_level(b"{\"k\": 1, \"a\": 0, \"k\": 2}", "k").unwrap(), b"{\"a\": 0}");
    }

    #[test]
    fn duplicate_keys_replace_the_last_like_json_parse() {
        let out = replace_top_level(b"{\"k\": 1, \"k\": 2}", "k", &json!(9)).unwrap();
        assert_eq!(out, b"{\"k\": 1, \"k\": 9}");
    }

    #[test]
    fn get_reads_a_top_level_value() {
        assert_eq!(get_top_level(DOC.as_bytes(), "userID").unwrap(), Some(json!("héllo ✓")));
        assert_eq!(get_top_level(DOC.as_bytes(), "nope").unwrap(), None);
    }

    #[test]
    fn torn_and_non_object_documents_are_refused() {
        for torn in [
            &b""[..], b"{", b"{\"a\": 1", b"{\"a\": 1,}", b"{\"a\" 1}", b"{\"a\": tru}", b"{} x", b"{\"a\": \"\x01\"}",
            // A malformed unrelated value refuses the whole write, too.
            b"{\"x\": 01, \"oauthAccount\": {}}", b"{\"x\": \"\\q\", \"oauthAccount\": {}}",
            b"{\"x\": \"\\u12\"}", b"{\"x\": \"\xff\"}",
        ] {
            assert!(matches!(replace_top_level(torn, "k", &json!(1)), Err(SpliceError::Torn(_))), "{:?}", String::from_utf8_lossy(torn));
        }
        for not_obj in [&b"[]"[..], b"\"s\"", b"12", b"null"] {
            assert!(matches!(replace_top_level(not_obj, "k", &json!(1)), Err(SpliceError::NotObject)));
        }
    }

    #[test]
    fn deep_nesting_is_torn_not_a_stack_overflow() {
        let doc = format!("{{\"a\": {}{}}}", "[".repeat(100_000), "]".repeat(100_000));
        assert!(matches!(replace_top_level(doc.as_bytes(), "k", &json!(1)), Err(SpliceError::Torn(_))));
    }

    #[test]
    fn large_documents_splice_quickly() {
        let mut doc = String::from("{\n  \"projects\": {");
        for i in 0..20_000 {
            doc.push_str(&format!("\n    \"/p/{i}\": {{ \"allowedTools\": [], \"history\": [\"x\"] }},"));
        }
        doc.pop();
        doc.push_str("\n  }\n}\n");
        let start = std::time::Instant::now();
        let out = replace_top_level(doc.as_bytes(), "oauthAccount", &json!({"e": 1})).unwrap();
        assert!(start.elapsed() < std::time::Duration::from_millis(500));
        // Everything up to the end of `projects` is untouched; the new member follows it.
        let untouched = &doc.as_bytes()[..doc.len() - "\n}\n".len()];
        assert!(out.starts_with(untouched));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tagteam-provider splice`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

Top of `splice.rs`:
```rust
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SpliceError {
    #[error("the document is not a JSON object")]
    NotObject,
    #[error("the document is torn or not valid JSON: {0}")]
    Torn(String),
}

const MAX_DEPTH: usize = 512;

struct Member {
    key: String,
    start: usize,
    value_start: usize,
    value_end: usize,
}

struct Object {
    open: usize,
    close: usize,
    members: Vec<Member>,
}

struct Scanner<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Scanner<'a> {
    fn torn(&self, what: &str) -> SpliceError {
        SpliceError::Torn(format!("{what} at byte {}", self.i))
    }

    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn expect(&mut self, c: u8) -> Result<(), SpliceError> {
        if self.peek() == Some(c) {
            self.i += 1;
            Ok(())
        } else {
            Err(self.torn(&format!("expected '{}'", c as char)))
        }
    }

    fn string(&mut self) -> Result<(), SpliceError> {
        self.expect(b'"')?;
        while let Some(c) = self.peek() {
            self.i += 1;
            match c {
                b'"' => return Ok(()),
                b'\\' => match self.peek() {
                    Some(b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't') => self.i += 1,
                    Some(b'u') => {
                        let hex = self.b.get(self.i + 1..self.i + 5);
                        if !hex.is_some_and(|h| h.iter().all(u8::is_ascii_hexdigit)) {
                            return Err(self.torn("bad \\u escape"));
                        }
                        self.i += 5;
                    }
                    _ => return Err(self.torn("bad escape")),
                },
                0x00..=0x1f => return Err(self.torn("control character in string")),
                _ => {}
            }
        }
        Err(self.torn("unterminated string"))
    }

    fn number(&mut self) -> Result<(), SpliceError> {
        let start = self.i;
        if self.peek() == Some(b'-') {
            self.i += 1;
        }
        let digits = |s: &mut Self| {
            let d = s.i;
            while s.peek().is_some_and(|c| c.is_ascii_digit()) {
                s.i += 1;
            }
            s.i > d
        };
        let int_start = self.i;
        if !digits(self) {
            return Err(self.torn("bad number"));
        }
        if self.b[int_start] == b'0' && self.i - int_start > 1 {
            return Err(self.torn("leading zero in number"));
        }
        if self.peek() == Some(b'.') {
            self.i += 1;
            if !digits(self) {
                return Err(self.torn("bad fraction"));
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.i += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.i += 1;
            }
            if !digits(self) {
                return Err(self.torn("bad exponent"));
            }
        }
        debug_assert!(self.i > start);
        Ok(())
    }

    fn literal(&mut self, word: &[u8]) -> Result<(), SpliceError> {
        if self.b[self.i..].starts_with(word) {
            self.i += word.len();
            Ok(())
        } else {
            Err(self.torn("bad literal"))
        }
    }

    fn value(&mut self, depth: usize) -> Result<(), SpliceError> {
        if depth > MAX_DEPTH {
            return Err(self.torn("nesting too deep"));
        }
        match self.peek() {
            Some(b'{') => self.container(b'{', b'}', depth, true),
            Some(b'[') => self.container(b'[', b']', depth, false),
            Some(b'"') => self.string(),
            Some(b't') => self.literal(b"true"),
            Some(b'f') => self.literal(b"false"),
            Some(b'n') => self.literal(b"null"),
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => Err(self.torn("expected a value")),
        }
    }

    fn container(&mut self, open: u8, close: u8, depth: usize, keyed: bool) -> Result<(), SpliceError> {
        self.expect(open)?;
        self.ws();
        if self.peek() == Some(close) {
            self.i += 1;
            return Ok(());
        }
        loop {
            self.ws();
            if keyed {
                self.string()?;
                self.ws();
                self.expect(b':')?;
                self.ws();
            }
            self.value(depth + 1)?;
            self.ws();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(c) if c == close => {
                    self.i += 1;
                    return Ok(());
                }
                _ => return Err(self.torn("expected ',' or a closing bracket")),
            }
        }
    }
}

fn scan(doc: &[u8]) -> Result<Object, SpliceError> {
    std::str::from_utf8(doc).map_err(|e| SpliceError::Torn(format!("invalid UTF-8: {e}")))?;
    let mut s = Scanner { b: doc, i: 0 };
    s.ws();
    match s.peek() {
        Some(b'{') => {}
        Some(b'[' | b'"' | b't' | b'f' | b'n' | b'-' | b'0'..=b'9') => {
            // A complete non-object value is "not an object"; anything else is torn.
            s.value(0)?;
            return Err(SpliceError::NotObject);
        }
        _ => return Err(s.torn("expected '{'")),
    }
    let open = s.i;
    s.i += 1;
    let mut members = Vec::new();
    s.ws();
    if s.peek() == Some(b'}') {
        let close = s.i;
        s.i += 1;
        s.ws();
        if s.i != doc.len() {
            return Err(s.torn("trailing data"));
        }
        return Ok(Object { open, close, members });
    }
    loop {
        s.ws();
        let start = s.i;
        s.string()?;
        let key: String = serde_json::from_slice(&doc[start..s.i])
            .map_err(|e| SpliceError::Torn(format!("bad key: {e}")))?;
        s.ws();
        s.expect(b':')?;
        s.ws();
        let value_start = s.i;
        s.value(1)?;
        members.push(Member { key, start, value_start, value_end: s.i });
        s.ws();
        match s.peek() {
            Some(b',') => s.i += 1,
            Some(b'}') => {
                let close = s.i;
                s.i += 1;
                s.ws();
                if s.i != doc.len() {
                    return Err(s.torn("trailing data"));
                }
                return Ok(Object { open, close, members });
            }
            _ => return Err(s.torn("expected ',' or '}'")),
        }
    }
}

/// `JSON.stringify(v, null, 2)` layout, nested at `depth` (continuation lines indented).
pub fn render_nested(value: &Value, depth: usize) -> String {
    let pretty = serde_json::to_string_pretty(value).expect("a Value always serializes");
    pretty.replace('\n', &format!("\n{}", "  ".repeat(depth)))
}

pub fn replace_top_level(doc: &[u8], key: &str, value: &Value) -> Result<Vec<u8>, SpliceError> {
    let obj = scan(doc)?;
    let rendered = render_nested(value, 1);
    let mut out = Vec::with_capacity(doc.len() + rendered.len() + key.len() + 8);
    if let Some(m) = obj.members.iter().rev().find(|m| m.key == key) {
        out.extend_from_slice(&doc[..m.value_start]);
        out.extend_from_slice(rendered.as_bytes());
        out.extend_from_slice(&doc[m.value_end..]);
    } else {
        let key_json = serde_json::to_string(key).expect("a string always serializes");
        match obj.members.last() {
            Some(last) => {
                out.extend_from_slice(&doc[..last.value_end]);
                out.extend_from_slice(format!(",\n  {key_json}: {rendered}").as_bytes());
                out.extend_from_slice(&doc[last.value_end..]);
            }
            None => {
                out.extend_from_slice(&doc[..=obj.open]);
                out.extend_from_slice(format!("\n  {key_json}: {rendered}\n").as_bytes());
                out.extend_from_slice(&doc[obj.close..]);
            }
        }
    }
    Ok(out)
}

/// Removes every occurrence of the key, so an earlier duplicate can never resurface.
pub fn remove_top_level(doc: &[u8], key: &str) -> Result<Vec<u8>, SpliceError> {
    let mut out = doc.to_vec();
    loop {
        let obj = scan(&out)?;
        let Some(i) = obj.members.iter().rposition(|m| m.key == key) else {
            return Ok(out);
        };
        let m = &obj.members[i];
        let (cut_start, cut_end) = if i > 0 {
            (obj.members[i - 1].value_end, m.value_end)
        } else if obj.members.len() > 1 {
            (m.start, obj.members[1].start)
        } else {
            (obj.open + 1, obj.close)
        };
        out.drain(cut_start..cut_end);
    }
}

pub fn get_top_level(doc: &[u8], key: &str) -> Result<Option<Value>, SpliceError> {
    let obj = scan(doc)?;
    obj.members
        .iter()
        .rev()
        .find(|m| m.key == key)
        .map(|m| {
            serde_json::from_slice(&doc[m.value_start..m.value_end])
                .map_err(|e| SpliceError::Torn(format!("bad value: {e}")))
        })
        .transpose()
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p tagteam-provider splice`
Expected: PASS (10 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/tagteam-provider
git commit -m "Add the span-preserving JSON splice"
```

---
### Task 9: Keychain port, fakes, and the `/usr/bin/security` driver

**Files:**
- Create: `crates/tagteam-provider/src/keychain.rs`, `crates/tagteam-provider/src/security.rs`
- Modify: `crates/tagteam-provider/src/lib.rs`
- Test: `crates/tagteam-provider/tests/real_keychain.rs` (feature `real_keychain`, macOS)

**Interfaces:**
- Produces:
  - `trait Keychain: Send + Sync { fn find(&self, service: &str, account: &str) -> Read<Vec<u8>>; fn exists(&self, service: &str, account: &str) -> Read<()>; fn upsert(&self, service: &str, account: &str, data: &[u8]) -> Result<(), KeychainError>; fn delete(&self, service: &str, account: &str) -> Result<(), KeychainError>; }`
    — `delete` of an absent item is `Ok`; `upsert` verifies by reading back
  - `impl<K: Keychain + ?Sized> Keychain for Arc<K>`
  - `struct KeychainError { rc: Option<i32>, detail: String }` (`Display`, `Error`)
  - `FakeKeychain` (in-memory): `new()`, `put`, `get`, `set_locked`, `set_unreadable(svc, acct, bool)`,
    `set_fail_write(svc, bool)`, `set_fail_delete(svc, bool)`, `items()`
  - `FileKeychain` (feature `file-keychain`): `FileKeychain::new(dir)`; a `LOCKED` file in the
    dir makes every call fail as a locked keychain does
  - `enum RunResult { Exited { code: i32, stdout: Vec<u8>, stderr: Vec<u8> }, TimedOut, SpawnFailed(String) }`,
    `trait Runner: Send + Sync { fn run(&self, program: &str, args: &[String], stdin: Option<&[u8]>, timeout: Duration) -> RunResult; }`,
    `ProcessRunner`
  - `SecurityCli::new()`, `SecurityCli::with_runner(runner: Box<dyn Runner>, keychain_file: Option<PathBuf>)`;
    `pub const SECURITY: &str = "/usr/bin/security"`, `pub const LINE_LIMIT: usize = 4032`,
    `pub fn decode_output(Vec<u8>) -> Vec<u8>`

- [ ] **Step 1: Write the failing driver tests**

Append to `crates/tagteam-provider/src/security.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    type Call = (String, Vec<String>, Option<Vec<u8>>);

    #[derive(Default, Clone)]
    struct Scripted {
        calls: Arc<Mutex<Vec<Call>>>,
        results: Arc<Mutex<VecDeque<RunResult>>>,
    }

    impl Scripted {
        fn then(self, r: RunResult) -> Self {
            self.results.lock().unwrap().push_back(r);
            self
        }
        fn calls(&self) -> Vec<Call> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Runner for Scripted {
        fn run(&self, program: &str, args: &[String], stdin: Option<&[u8]>, _t: Duration) -> RunResult {
            self.calls.lock().unwrap().push((program.into(), args.to_vec(), stdin.map(<[u8]>::to_vec)));
            self.results.lock().unwrap().pop_front().expect("unexpected extra call")
        }
    }

    fn ok(stdout: &[u8]) -> RunResult {
        RunResult::Exited { code: 0, stdout: stdout.to_vec(), stderr: vec![] }
    }
    fn rc(code: i32) -> RunResult {
        RunResult::Exited { code, stdout: vec![], stderr: b"boom".to_vec() }
    }
    fn cli(s: &Scripted, file: Option<&str>) -> SecurityCli {
        SecurityCli::with_runner(Box::new(s.clone()), file.map(PathBuf::from))
    }

    #[test]
    fn find_uses_the_absolute_binary_and_strips_one_newline() {
        let s = Scripted::default().then(ok(b"{\"a\":1}\n\n"));
        let r = cli(&s, None).find("svc", "acct");
        assert_eq!(r.present().unwrap(), b"{\"a\":1}\n");
        let (program, args, stdin) = &s.calls()[0];
        assert_eq!(program, "/usr/bin/security");
        assert_eq!(args, &["find-generic-password", "-a", "acct", "-w", "-s", "svc"]);
        assert!(stdin.is_none());
    }

    #[test]
    fn hex_output_is_decoded() {
        assert_eq!(decode_output(b"7b7d\n".to_vec()), b"{}");
        assert_eq!(decode_output(b"sk-ant-api03-abc\n".to_vec()), b"sk-ant-api03-abc");
        assert_eq!(decode_output(b"abc\n".to_vec()), b"abc"); // odd length: not hex
    }

    #[test]
    fn find_maps_return_codes() {
        let s = Scripted::default().then(rc(44)).then(rc(36)).then(RunResult::TimedOut).then(RunResult::SpawnFailed("x".into()));
        let k = cli(&s, None);
        assert!(matches!(k.find("s", "a"), Read::Absent));
        assert!(matches!(k.find("s", "a"), Read::Unreadable(e) if e.detail.contains("rc 36")));
        assert!(matches!(k.find("s", "a"), Read::Unreadable(e) if e.detail.contains("timed out")));
        assert!(matches!(k.find("s", "a"), Read::Unreadable(_)));
    }

    #[test]
    fn exists_never_asks_for_the_secret() {
        let s = Scripted::default().then(ok(b"attributes"));
        assert!(cli(&s, None).exists("svc", "acct").is_present());
        assert!(!s.calls()[0].1.contains(&"-w".to_string()));
    }

    #[test]
    fn a_test_keychain_file_is_the_last_argument() {
        let s = Scripted::default().then(rc(44));
        let _ = cli(&s, Some("/tmp/t.keychain")).find("svc", "acct");
        assert_eq!(s.calls()[0].1.last().unwrap(), "/tmp/t.keychain");
    }

    #[test]
    fn small_writes_go_through_interactive_mode_and_are_verified() {
        let s = Scripted::default().then(ok(b"")).then(ok(b"7b7d\n"));
        cli(&s, None).upsert("svc", "acct", b"{}").unwrap();
        let calls = s.calls();
        assert_eq!(calls[0].1, vec!["-i".to_string()]);
        assert_eq!(
            calls[0].2.as_deref().unwrap(),
            b"add-generic-password -U -a \"acct\" -s \"svc\" -X \"7b7d\"\n"
        );
        assert_eq!(calls[1].1[0], "find-generic-password");
    }

    #[test]
    fn long_writes_fall_back_to_argv() {
        let data = vec![b'x'; 2100]; // 4200 hex digits: over the 4032-byte line limit
        let s = Scripted::default().then(ok(b"")).then(ok(&data));
        cli(&s, None).upsert("svc", "acct", &data).unwrap();
        let (_, args, stdin) = &s.calls()[0];
        assert!(stdin.is_none());
        assert_eq!(&args[..7], ["add-generic-password", "-U", "-a", "acct", "-s", "svc", "-X"]);
        assert_eq!(args[7], hex::encode(&data));
    }

    #[test]
    fn a_write_that_does_not_read_back_fails() {
        let s = Scripted::default().then(ok(b"")).then(ok(b"something else"));
        assert!(cli(&s, None).upsert("svc", "acct", b"{}").is_err());
    }

    #[test]
    fn delete_treats_absent_as_success() {
        let s = Scripted::default().then(rc(44)).then(rc(36));
        let k = cli(&s, None);
        assert!(k.delete("svc", "acct").is_ok());
        assert_eq!(k.delete("svc", "acct").unwrap_err().rc, Some(36));
    }

    #[test]
    fn run_results_never_show_their_output() {
        let secret = b"sk-ant-ort01-SENTINEL".to_vec();
        let shown = format!("{:?}", ok(&secret));
        assert!(!shown.contains("SENTINEL"), "{shown}");
        assert!(!shown.contains(&format!("{:?}", secret)), "{shown}");
    }

    #[test]
    fn quotes_in_names_are_refused_before_spawning() {
        let s = Scripted::default();
        assert!(cli(&s, None).upsert("s\"vc", "acct", b"{}").is_err());
        assert!(s.calls().is_empty());
    }
}
```

Append to `crates/tagteam-provider/src/keychain.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_keychain_models_absent_locked_and_failures() {
        let k = FakeKeychain::new();
        assert!(matches!(k.find("s", "a"), Read::Absent));
        k.upsert("s", "a", b"v").unwrap();
        assert_eq!(k.find("s", "a").present().unwrap(), b"v");
        assert!(k.exists("s", "a").is_present());
        k.set_unreadable("s", "a", true);
        assert!(matches!(k.find("s", "a"), Read::Unreadable(_)));
        k.set_unreadable("s", "a", false);
        k.set_fail_write("s", true);
        assert!(k.upsert("s", "a", b"w").is_err());
        k.set_fail_delete("s", true);
        assert!(k.delete("s", "a").is_err());
        k.set_locked(true);
        assert!(matches!(k.exists("s", "a"), Read::Unreadable(_)));
        k.set_locked(false);
        k.set_fail_delete("s", false);
        k.delete("s", "a").unwrap();
        k.delete("s", "a").unwrap();
        assert!(matches!(k.find("s", "a"), Read::Absent));
    }

    #[cfg(feature = "file-keychain")]
    #[test]
    fn file_keychain_persists_across_instances() {
        let d = tempfile::tempdir().unwrap();
        FileKeychain::new(d.path()).upsert("Claude Code-credentials", "me", b"{}").unwrap();
        let k = FileKeychain::new(d.path());
        assert_eq!(k.find("Claude Code-credentials", "me").present().unwrap(), b"{}");
        std::fs::write(d.path().join("LOCKED"), "").unwrap();
        assert!(matches!(k.find("Claude Code-credentials", "me"), Read::Unreadable(_)));
        assert!(k.upsert("x", "y", b"z").is_err());
    }
}
```

`crates/tagteam-provider/tests/real_keychain.rs`:
```rust
//! Runs `/usr/bin/security` against a throwaway keychain file; never the login keychain.
#![cfg(all(target_os = "macos", feature = "real_keychain"))]

use std::process::Command;

use tagteam_provider::keychain::Keychain;
use tagteam_provider::security::{ProcessRunner, SecurityCli};
use tagteam_provider::Read;

struct TempKeychain(std::path::PathBuf, tempfile::TempDir);

impl TempKeychain {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.keychain");
        let run = |args: &[&str]| assert!(Command::new("/usr/bin/security").args(args).status().unwrap().success());
        run(&["create-keychain", "-p", "pw", path.to_str().unwrap()]);
        run(&["unlock-keychain", "-p", "pw", path.to_str().unwrap()]);
        Self(path, dir)
    }
}

impl Drop for TempKeychain {
    fn drop(&mut self) {
        let _ = Command::new("/usr/bin/security").args(["delete-keychain", self.0.to_str().unwrap()]).status();
    }
}

#[test]
fn round_trips_small_large_and_binary_items() {
    let kc = TempKeychain::new();
    let k = SecurityCli::with_runner(Box::new(ProcessRunner), Some(kc.0.clone()));
    assert!(matches!(k.find("tagteam", "id"), Read::Absent));
    for data in [b"{\"claudeAiOauth\":{}}".to_vec(), vec![b'x'; 5000], vec![0u8, 1, 2, 255]] {
        k.upsert("tagteam", "id", &data).unwrap();
        assert_eq!(k.find("tagteam", "id").present().unwrap(), data);
        assert!(k.exists("tagteam", "id").is_present());
    }
    k.delete("tagteam", "id").unwrap();
    k.delete("tagteam", "id").unwrap();
    assert!(matches!(k.exists("tagteam", "id"), Read::Absent));
}
```

Update `crates/tagteam-provider/src/lib.rs`:
```rust
pub mod keychain;
pub mod security;
pub use keychain::{FakeKeychain, Keychain, KeychainError};
#[cfg(feature = "file-keychain")]
pub use keychain::FileKeychain;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tagteam-provider --features file-keychain`
Expected: FAIL to compile.

- [ ] **Step 3: Implement the port and fakes**

Top of `keychain.rs`:
```rust
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::read::{Read, ReadError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeychainError {
    pub rc: Option<i32>,
    pub detail: String,
}

impl fmt::Display for KeychainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.rc {
            Some(rc) => write!(f, "keychain operation failed (rc {rc}): {}", self.detail),
            None => write!(f, "keychain operation failed: {}", self.detail),
        }
    }
}

impl std::error::Error for KeychainError {}

/// Generic-password items keyed by (service, account). Appendix A.3 semantics.
pub trait Keychain: Send + Sync {
    fn find(&self, service: &str, account: &str) -> Read<Vec<u8>>;
    /// Attributes only: never prompts and never returns the secret.
    fn exists(&self, service: &str, account: &str) -> Read<()>;
    /// Adds or updates in place (`-U`), then verifies by reading back.
    fn upsert(&self, service: &str, account: &str, data: &[u8]) -> Result<(), KeychainError>;
    /// Deleting an absent item succeeds.
    fn delete(&self, service: &str, account: &str) -> Result<(), KeychainError>;
}

impl<K: Keychain + ?Sized> Keychain for Arc<K> {
    fn find(&self, s: &str, a: &str) -> Read<Vec<u8>> {
        (**self).find(s, a)
    }
    fn exists(&self, s: &str, a: &str) -> Read<()> {
        (**self).exists(s, a)
    }
    fn upsert(&self, s: &str, a: &str, d: &[u8]) -> Result<(), KeychainError> {
        (**self).upsert(s, a, d)
    }
    fn delete(&self, s: &str, a: &str) -> Result<(), KeychainError> {
        (**self).delete(s, a)
    }
}

fn locked_read<T>() -> Read<T> {
    Read::Unreadable(ReadError::new("keychain", "rc 36: the keychain is locked"))
}

fn locked_err() -> KeychainError {
    KeychainError { rc: Some(36), detail: "the keychain is locked".into() }
}

type Key = (String, String);

/// In-memory Keychain for tests, with failure injection.
#[derive(Default)]
pub struct FakeKeychain {
    items: Mutex<BTreeMap<Key, Vec<u8>>>,
    locked: AtomicBool,
    unreadable: Mutex<BTreeSet<Key>>,
    fail_write: Mutex<BTreeSet<String>>,
    fail_delete: Mutex<BTreeSet<String>>,
    panic_delete: Mutex<BTreeSet<String>>,
}

fn key(s: &str, a: &str) -> Key {
    (s.to_owned(), a.to_owned())
}

fn toggle(set: &Mutex<BTreeSet<String>>, s: &str, on: bool) {
    let mut g = set.lock().unwrap();
    if on {
        g.insert(s.to_owned());
    } else {
        g.remove(s);
    }
}

impl FakeKeychain {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn put(&self, s: &str, a: &str, data: &[u8]) {
        self.items.lock().unwrap().insert(key(s, a), data.to_vec());
    }
    pub fn get(&self, s: &str, a: &str) -> Option<Vec<u8>> {
        self.items.lock().unwrap().get(&key(s, a)).cloned()
    }
    pub fn items(&self) -> BTreeMap<Key, Vec<u8>> {
        self.items.lock().unwrap().clone()
    }
    pub fn set_locked(&self, on: bool) {
        self.locked.store(on, Ordering::SeqCst);
    }
    pub fn set_unreadable(&self, s: &str, a: &str, on: bool) {
        let mut g = self.unreadable.lock().unwrap();
        if on {
            g.insert(key(s, a));
        } else {
            g.remove(&key(s, a));
        }
    }
    pub fn set_fail_write(&self, s: &str, on: bool) {
        toggle(&self.fail_write, s, on);
    }
    pub fn set_fail_delete(&self, s: &str, on: bool) {
        toggle(&self.fail_delete, s, on);
    }
    /// Panics inside `delete` for this service, to test rollback during unwinding.
    pub fn set_panic_on_delete(&self, s: &str, on: bool) {
        toggle(&self.panic_delete, s, on);
    }
    fn blocked(&self, s: &str, a: &str) -> bool {
        self.locked.load(Ordering::SeqCst) || self.unreadable.lock().unwrap().contains(&key(s, a))
    }
}

impl Keychain for FakeKeychain {
    fn find(&self, s: &str, a: &str) -> Read<Vec<u8>> {
        if self.blocked(s, a) {
            return locked_read();
        }
        match self.get(s, a) {
            Some(v) => Read::Present(v),
            None => Read::Absent,
        }
    }
    fn exists(&self, s: &str, a: &str) -> Read<()> {
        self.find(s, a).map(|_| ())
    }
    fn upsert(&self, s: &str, a: &str, d: &[u8]) -> Result<(), KeychainError> {
        if self.locked.load(Ordering::SeqCst) {
            return Err(locked_err());
        }
        if self.fail_write.lock().unwrap().contains(s) {
            return Err(KeychainError { rc: Some(25), detail: "injected write failure".into() });
        }
        self.put(s, a, d);
        Ok(())
    }
    fn delete(&self, s: &str, a: &str) -> Result<(), KeychainError> {
        if self.panic_delete.lock().unwrap().contains(s) {
            panic!("injected panic deleting {s}");
        }
        if self.locked.load(Ordering::SeqCst) {
            return Err(locked_err());
        }
        if self.fail_delete.lock().unwrap().contains(s) {
            return Err(KeychainError { rc: Some(25), detail: "injected delete failure".into() });
        }
        self.items.lock().unwrap().remove(&key(s, a));
        Ok(())
    }
}

/// A directory-backed fake, so tests can drive the real binary across processes.
#[cfg(feature = "file-keychain")]
pub struct FileKeychain {
    dir: std::path::PathBuf,
}

#[cfg(feature = "file-keychain")]
impl FileKeychain {
    pub fn new(dir: impl Into<std::path::PathBuf>) -> Self {
        Self { dir: dir.into() }
    }
    fn path(&self, s: &str, a: &str) -> std::path::PathBuf {
        self.dir.join(format!("{}.{}", hex::encode(s), hex::encode(a)))
    }
    fn locked(&self) -> bool {
        self.dir.join("LOCKED").exists()
    }
}

#[cfg(feature = "file-keychain")]
impl Keychain for FileKeychain {
    fn find(&self, s: &str, a: &str) -> Read<Vec<u8>> {
        if self.locked() {
            return locked_read();
        }
        match std::fs::read(self.path(s, a)) {
            Ok(v) => Read::Present(v),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Read::Absent,
            Err(e) => Read::Unreadable(ReadError::new("keychain", e.to_string())),
        }
    }
    fn exists(&self, s: &str, a: &str) -> Read<()> {
        self.find(s, a).map(|_| ())
    }
    fn upsert(&self, s: &str, a: &str, d: &[u8]) -> Result<(), KeychainError> {
        if self.locked() {
            return Err(locked_err());
        }
        crate::atomic::ensure_private_dir(&self.dir)
            .and_then(|()| crate::atomic::write_atomic(&self.path(s, a), d, 0o600))
            .map_err(|e| KeychainError { rc: None, detail: e.to_string() })
    }
    fn delete(&self, s: &str, a: &str) -> Result<(), KeychainError> {
        if self.locked() {
            return Err(locked_err());
        }
        match std::fs::remove_file(self.path(s, a)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(KeychainError { rc: None, detail: e.to_string() }),
        }
    }
}
```

- [ ] **Step 4: Implement the driver**

Top of `security.rs`:
```rust
use std::io::{Read as _, Write as _};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::keychain::{Keychain, KeychainError};
use crate::read::{Read, ReadError};

pub const SECURITY: &str = "/usr/bin/security";
/// The 4096-byte `security -i` line limit minus 64. An over-long line truncates silently and
/// leaves the old entry (Appendix A.3).
pub const LINE_LIMIT: usize = 4032;
const TIMEOUT: Duration = Duration::from_secs(5);

/// `stdout` of `find-generic-password -w` is a secret, so `Debug` shows lengths only.
#[derive(Clone)]
pub enum RunResult {
    Exited { code: i32, stdout: Vec<u8>, stderr: Vec<u8> },
    TimedOut,
    SpawnFailed(String),
}

impl std::fmt::Debug for RunResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunResult::Exited { code, stdout, stderr } => f
                .debug_struct("Exited")
                .field("code", code)
                .field("stdout", &format_args!("<{} bytes>", stdout.len()))
                .field("stderr", &format_args!("<{} bytes>", stderr.len()))
                .finish(),
            RunResult::TimedOut => f.write_str("TimedOut"),
            RunResult::SpawnFailed(e) => f.debug_tuple("SpawnFailed").field(e).finish(),
        }
    }
}

pub trait Runner: Send + Sync {
    fn run(&self, program: &str, args: &[String], stdin: Option<&[u8]>, timeout: Duration) -> RunResult;
}

pub struct ProcessRunner;

impl Runner for ProcessRunner {
    fn run(&self, program: &str, args: &[String], stdin: Option<&[u8]>, timeout: Duration) -> RunResult {
        let mut child = match Command::new(program)
            .args(args)
            .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => return RunResult::SpawnFailed(e.to_string()),
        };
        if let (Some(data), Some(mut pipe)) = (stdin, child.stdin.take()) {
            let data = data.to_vec();
            thread::spawn(move || {
                let _ = pipe.write_all(&data);
            });
        }
        let drain = |p: Option<Box<dyn std::io::Read + Send>>| {
            thread::spawn(move || {
                let mut buf = Vec::new();
                if let Some(mut p) = p {
                    let _ = p.read_to_end(&mut buf);
                }
                buf
            })
        };
        let out = drain(child.stdout.take().map(|p| Box::new(p) as _));
        let err = drain(child.stderr.take().map(|p| Box::new(p) as _));
        let deadline = Instant::now() + timeout;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    return RunResult::Exited {
                        code: status.code().unwrap_or(-1),
                        stdout: out.join().unwrap_or_default(),
                        stderr: err.join().unwrap_or_default(),
                    };
                }
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return RunResult::TimedOut;
                }
                Ok(None) => thread::sleep(Duration::from_millis(10)),
                Err(e) => return RunResult::SpawnFailed(e.to_string()),
            }
        }
    }
}

/// Exactly one trailing `\n` is stripped; all-lowercase-hex output of even length means the
/// stored data was non-printable, so it is decoded.
pub fn decode_output(mut out: Vec<u8>) -> Vec<u8> {
    if out.last() == Some(&b'\n') {
        out.pop();
    }
    let hexlike = !out.is_empty()
        && out.len() % 2 == 0
        && out.iter().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b));
    if hexlike {
        if let Ok(decoded) = hex::decode(&out) {
            return decoded;
        }
    }
    out
}

pub struct SecurityCli {
    runner: Box<dyn Runner>,
    keychain_file: Option<PathBuf>,
}

impl SecurityCli {
    pub fn new() -> Self {
        Self { runner: Box::new(ProcessRunner), keychain_file: None }
    }

    /// `keychain_file` targets a specific keychain (tests only); production searches the
    /// default list, as CC does.
    pub fn with_runner(runner: Box<dyn Runner>, keychain_file: Option<PathBuf>) -> Self {
        Self { runner, keychain_file }
    }

    fn run(&self, mut args: Vec<String>, stdin: Option<&[u8]>) -> RunResult {
        if let Some(f) = &self.keychain_file {
            if stdin.is_none() {
                args.push(f.to_string_lossy().into_owned());
            }
        }
        self.runner.run(SECURITY, &args, stdin, TIMEOUT)
    }

    fn tail(&self) -> String {
        self.keychain_file
            .as_ref()
            .map(|f| format!(" \"{}\"", f.to_string_lossy()))
            .unwrap_or_default()
    }
}

impl Default for SecurityCli {
    fn default() -> Self {
        Self::new()
    }
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| (*x).to_owned()).collect()
}

fn unreadable(r: RunResult) -> ReadError {
    match r {
        RunResult::Exited { code, stderr, .. } => {
            ReadError::new("keychain", format!("rc {code}: {}", String::from_utf8_lossy(&stderr).trim()))
        }
        RunResult::TimedOut => ReadError::new("keychain", "security timed out after 5 s"),
        RunResult::SpawnFailed(e) => ReadError::new("keychain", format!("could not run security: {e}")),
    }
}

fn failed(r: RunResult) -> KeychainError {
    match r {
        RunResult::Exited { code, stderr, .. } => KeychainError {
            rc: Some(code),
            detail: String::from_utf8_lossy(&stderr).trim().to_owned(),
        },
        RunResult::TimedOut => KeychainError { rc: None, detail: "security timed out after 5 s".into() },
        RunResult::SpawnFailed(e) => KeychainError { rc: None, detail: format!("could not run security: {e}") },
    }
}

fn check_name(v: &str) -> Result<(), KeychainError> {
    if v.contains(['"', '\\', '\n']) {
        return Err(KeychainError { rc: None, detail: format!("refusing an item name with quotes: {v:?}") });
    }
    Ok(())
}

impl Keychain for SecurityCli {
    fn find(&self, service: &str, account: &str) -> Read<Vec<u8>> {
        match self.run(s(&["find-generic-password", "-a", account, "-w", "-s", service]), None) {
            RunResult::Exited { code: 0, stdout, .. } => Read::Present(decode_output(stdout)),
            RunResult::Exited { code: 44, .. } => Read::Absent,
            other => Read::Unreadable(unreadable(other)),
        }
    }

    fn exists(&self, service: &str, account: &str) -> Read<()> {
        match self.run(s(&["find-generic-password", "-a", account, "-s", service]), None) {
            RunResult::Exited { code: 0, .. } => Read::Present(()),
            RunResult::Exited { code: 44, .. } => Read::Absent,
            other => Read::Unreadable(unreadable(other)),
        }
    }

    fn upsert(&self, service: &str, account: &str, data: &[u8]) -> Result<(), KeychainError> {
        check_name(service)?;
        check_name(account)?;
        let hex = hex::encode(data);
        let line = format!(
            "add-generic-password -U -a \"{account}\" -s \"{service}\" -X \"{hex}\"{}\n",
            self.tail()
        );
        let result = if line.len() <= LINE_LIMIT {
            self.run(s(&["-i"]), Some(line.as_bytes()))
        } else {
            self.run(s(&["add-generic-password", "-U", "-a", account, "-s", service, "-X", &hex]), None)
        };
        match result {
            RunResult::Exited { code: 0, .. } => {}
            other => return Err(failed(other)),
        }
        match self.find(service, account) {
            Read::Present(v) if v == data => Ok(()),
            Read::Present(_) => Err(KeychainError { rc: None, detail: "the item did not read back as written".into() }),
            Read::Absent => Err(KeychainError { rc: None, detail: "the item is missing after writing".into() }),
            Read::Unreadable(e) => Err(KeychainError { rc: None, detail: e.to_string() }),
        }
    }

    fn delete(&self, service: &str, account: &str) -> Result<(), KeychainError> {
        match self.run(s(&["delete-generic-password", "-a", account, "-s", service]), None) {
            RunResult::Exited { code: 0 | 44, .. } => Ok(()),
            other => Err(failed(other)),
        }
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p tagteam-provider --features file-keychain`
Expected: PASS. On a Mac, also run
`cargo test -p tagteam-provider --features real_keychain --test real_keychain` — expected PASS
with no GUI prompt.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-provider
git commit -m "Add the Keychain port, fakes and the security driver"
```

---

### Task 10: Locks and process liveness

**Files:**
- Create: `crates/tagteam-provider/src/{mkdir_lock.rs,flock.rs,process.rs}`
- Modify: `crates/tagteam-provider/src/lib.rs`

**Interfaces:**
- Produces:
  - `enum LockError { Timeout(PathBuf), Compromised(PathBuf), Io(io::Error) }` (thiserror)
  - `struct MkdirLockSpec { path: PathBuf, stale: Duration, acquire_timeout: Duration, touch_every: Duration }`
    with `MkdirLockSpec::new(path, stale, acquire_timeout)` (touch every 3 s)
  - `MkdirLock::acquire(&MkdirLockSpec) -> Result<MkdirLock, LockError>`,
    `MkdirLock::try_acquire(&MkdirLockSpec) -> Result<Option<MkdirLock>, LockError>` (one attempt,
    with the stale takeover), `.check_owned() -> Result<(), LockError>`, `.is_compromised()`,
    `.path()`; `Drop` removes the directory only when still owned
  - `FlockGuard::try_lock(&Path) -> io::Result<Option<FlockGuard>>`,
    `FlockGuard::lock(&Path, Duration) -> Result<FlockGuard, LockError>` (polls every 100 ms)
  - `MutationGuard::acquire(&Env, Duration) -> Result<MutationGuard, LockError>`;
    `MutationGuard::TIMEOUT` (10 s), `MutationGuard::BOOTSTRAP_TIMEOUT` (30 s)
  - `struct ProcessStamp { pid: u32, start: u64 }`, `ProcessStamp::current() -> io::Result<ProcessStamp>`,
    `.is_live() -> bool` (exact pid + start-time match; undetermined counts as live)

- [ ] **Step 1: Write the failing tests**

Append to `mkdir_lock.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    fn spec(dir: &Path, stale_ms: u64, timeout_ms: u64, touch_ms: u64) -> MkdirLockSpec {
        MkdirLockSpec {
            path: dir.join("x.lock"),
            stale: Duration::from_millis(stale_ms),
            acquire_timeout: Duration::from_millis(timeout_ms),
            touch_every: Duration::from_millis(touch_ms),
        }
    }

    #[test]
    fn acquire_creates_and_drop_removes() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 60_000, 100, 3_000);
        let l = MkdirLock::acquire(&s).unwrap();
        assert!(s.path.is_dir());
        drop(l);
        assert!(!s.path.exists());
    }

    #[test]
    fn a_held_lock_times_out() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 60_000, 600, 3_000);
        let _held = MkdirLock::acquire(&s).unwrap();
        assert!(matches!(MkdirLock::acquire(&s), Err(LockError::Timeout(_))));
        assert!(MkdirLock::try_acquire(&s).unwrap().is_none());
    }

    #[test]
    fn a_stale_lock_is_taken_over() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 1_000, 100, 3_000);
        fs::create_dir(&s.path).unwrap();
        set_dir_mtime(&s.path, SystemTime::now() - Duration::from_secs(5)).unwrap();
        assert!(MkdirLock::acquire(&s).is_ok());
    }

    #[test]
    fn the_heartbeat_keeps_the_mtime_fresh() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 60_000, 100, 20);
        let l = MkdirLock::acquire(&s).unwrap();
        let before = fs::metadata(&s.path).unwrap().modified().unwrap();
        std::thread::sleep(Duration::from_millis(150));
        let after = fs::metadata(&s.path).unwrap().modified().unwrap();
        assert!(after > before);
        assert!(l.check_owned().is_ok());
    }

    #[test]
    fn checks_racing_the_heartbeat_never_see_a_false_takeover() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 60_000, 100, 1);
        let l = MkdirLock::acquire(&s).unwrap();
        for _ in 0..2_000 {
            l.check_owned().unwrap();
        }
        assert!(!l.is_compromised());
    }

    #[test]
    fn the_heartbeat_notices_an_external_takeover() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 60_000, 100, 20);
        let l = MkdirLock::acquire(&s).unwrap();
        // Another process replaced the lock: its mtime is no longer the one we set.
        set_dir_mtime(&s.path, SystemTime::now() - Duration::from_secs(30)).unwrap();
        std::thread::sleep(Duration::from_millis(150));
        assert!(l.is_compromised());
        drop(l);
        assert!(s.path.is_dir(), "a compromised guard must not remove the directory");
    }

    #[test]
    fn a_suspended_holder_detects_the_takeover_and_leaves_the_new_lock() {
        let d = tempfile::tempdir().unwrap();
        // Heartbeat far in the future: models a holder that was suspended.
        let s = spec(d.path(), 200, 1_000, 3_600_000);
        let first = MkdirLock::acquire(&s).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let second = MkdirLock::acquire(&s).unwrap();
        assert!(matches!(first.check_owned(), Err(LockError::Compromised(_))));
        drop(first);
        assert!(s.path.is_dir(), "the resumed holder removed its replacement's lock");
        drop(second);
        assert!(!s.path.exists());
    }

    #[test]
    fn a_panic_releases_the_lock() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 60_000, 100, 3_000);
        let r = std::panic::catch_unwind(|| {
            let _l = MkdirLock::acquire(&s).unwrap();
            panic!("boom");
        });
        assert!(r.is_err());
        assert!(!s.path.exists());
    }
}
```

Append to `flock.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn a_second_lock_on_the_same_file_is_refused_until_release() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("sub/x.lock");
        let g = FlockGuard::try_lock(&p).unwrap().unwrap();
        assert_eq!(fs::metadata(p.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
        assert!(FlockGuard::try_lock(&p).unwrap().is_none());
        assert!(matches!(FlockGuard::lock(&p, Duration::from_millis(250)), Err(LockError::Timeout(_))));
        drop(g);
        assert!(FlockGuard::try_lock(&p).unwrap().is_some());
    }

    #[test]
    fn the_mutation_guard_lives_in_the_data_dir() {
        let d = tempfile::tempdir().unwrap();
        let env = Env::for_test(d.path());
        let g = MutationGuard::acquire(&env, Duration::from_millis(100)).unwrap();
        assert!(env.data_dir().join(".mutation.lock").exists());
        assert!(MutationGuard::acquire(&env, Duration::from_millis(150)).is_err());
        drop(g);
        assert!(MutationGuard::acquire(&env, Duration::from_millis(100)).is_ok());
    }
}
```

Append to `process.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_is_live_and_a_changed_start_is_not() {
        let me = ProcessStamp::current().unwrap();
        assert!(me.is_live());
        assert!(!ProcessStamp { start: me.start + 1, ..me }.is_live());
    }

    #[test]
    fn an_exited_child_is_not_live() {
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let stamp = ProcessStamp { pid: child.id(), start: 0 };
        child.wait().unwrap();
        assert!(!stamp.is_live());
    }
}
```

`lib.rs` additions:
```rust
pub mod flock;
pub mod mkdir_lock;
pub mod process;
pub use flock::{FlockGuard, MutationGuard};
pub use mkdir_lock::{LockError, MkdirLock, MkdirLockSpec};
pub use process::ProcessStamp;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tagteam-provider`
Expected: FAIL to compile.

- [ ] **Step 3: Implement the mkdir lock**

Top of `mkdir_lock.rs`:
```rust
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error("timed out waiting for the lock {0}")]
    Timeout(PathBuf),
    #[error("the lock {0} was taken over while held")]
    Compromised(PathBuf),
    #[error("lock I/O failed: {0}")]
    Io(#[from] io::Error),
}

#[derive(Debug, Clone)]
pub struct MkdirLockSpec {
    pub path: PathBuf,
    pub stale: Duration,
    pub acquire_timeout: Duration,
    pub touch_every: Duration,
}

impl MkdirLockSpec {
    pub fn new(path: PathBuf, stale: Duration, acquire_timeout: Duration) -> Self {
        Self { path, stale, acquire_timeout, touch_every: Duration::from_secs(3) }
    }
}

pub(crate) fn set_dir_mtime(path: &Path, t: SystemTime) -> io::Result<SystemTime> {
    File::open(path)?.set_modified(t)?;
    fs::metadata(path)?.modified()
}

struct State {
    last_set: Mutex<SystemTime>,
    compromised: AtomicBool,
    stop: Mutex<bool>,
    wake: Condvar,
}

/// CC's `proper-lockfile` protocol (§9.1): `mkdir` acquires, a stale mtime may be taken
/// over, a heartbeat touches the mtime, and ownership is re-checked before every protected
/// write and before release.
pub struct MkdirLock {
    path: PathBuf,
    state: Arc<State>,
    heartbeat: Option<JoinHandle<()>>,
}

impl MkdirLock {
    pub fn try_acquire(spec: &MkdirLockSpec) -> Result<Option<Self>, LockError> {
        for _ in 0..2 {
            match fs::create_dir(&spec.path) {
                Ok(()) => return Self::start(spec).map(Some),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    match fs::metadata(&spec.path).and_then(|m| m.modified()) {
                        Ok(mtime) => {
                            let age = SystemTime::now().duration_since(mtime).unwrap_or_default();
                            if age <= spec.stale {
                                return Ok(None);
                            }
                            let _ = fs::remove_dir(&spec.path); // stale: take it over
                        }
                        Err(e) if e.kind() == io::ErrorKind::NotFound => {} // vanished: retry
                        Err(e) => return Err(e.into()),
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    // The directory that holds the lock does not exist yet.
                    if let Some(parent) = spec.path.parent() {
                        crate::atomic::ensure_private_dir(parent)?;
                    }
                }
                Err(e) => return Err(e.into()),
            }
        }
        Ok(None)
    }

    pub fn acquire(spec: &MkdirLockSpec) -> Result<Self, LockError> {
        let deadline = Instant::now() + spec.acquire_timeout;
        loop {
            if let Some(lock) = Self::try_acquire(spec)? {
                return Ok(lock);
            }
            if Instant::now() >= deadline {
                return Err(LockError::Timeout(spec.path.clone()));
            }
            thread::sleep(Duration::from_millis(fastrand::u64(250..=500)));
        }
    }

    fn start(spec: &MkdirLockSpec) -> Result<Self, LockError> {
        let set = set_dir_mtime(&spec.path, SystemTime::now())?;
        let state = Arc::new(State {
            last_set: Mutex::new(set),
            compromised: AtomicBool::new(false),
            stop: Mutex::new(false),
            wake: Condvar::new(),
        });
        let (st, path, every) = (state.clone(), spec.path.clone(), spec.touch_every);
        let heartbeat = thread::spawn(move || {
            let mut stop = st.stop.lock().unwrap();
            loop {
                let (guard, _) = st.wake.wait_timeout(stop, every).unwrap();
                stop = guard;
                if *stop {
                    return;
                }
                // Check and touch as one step under the mutex, so a concurrent check never
                // compares a stale timestamp with the heartbeat's fresh one.
                let mut last = st.last_set.lock().unwrap();
                if check_with(&path, &st, *last).is_ok() {
                    match set_dir_mtime(&path, SystemTime::now()) {
                        Ok(t) => *last = t,
                        Err(_) => st.compromised.store(true, Ordering::SeqCst),
                    }
                }
            }
        });
        Ok(Self { path: spec.path.clone(), state, heartbeat: Some(heartbeat) })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_compromised(&self) -> bool {
        self.state.compromised.load(Ordering::SeqCst)
    }

    /// Synchronous ownership check (§9.1): the directory must still carry the mtime this
    /// holder last set. A failure marks the guard compromised for good.
    pub fn check_owned(&self) -> Result<(), LockError> {
        check(&self.path, &self.state)
    }
}

fn check(path: &Path, st: &State) -> Result<(), LockError> {
    let last = st.last_set.lock().unwrap(); // held across the stat
    check_with(path, st, *last)
}

fn check_with(path: &Path, st: &State, last: SystemTime) -> Result<(), LockError> {
    if st.compromised.load(Ordering::SeqCst) {
        return Err(LockError::Compromised(path.to_path_buf()));
    }
    match fs::metadata(path).and_then(|m| m.modified()) {
        Ok(m) if m == last => Ok(()),
        _ => {
            st.compromised.store(true, Ordering::SeqCst);
            Err(LockError::Compromised(path.to_path_buf()))
        }
    }
}

impl Drop for MkdirLock {
    fn drop(&mut self) {
        *self.state.stop.lock().unwrap() = true;
        self.state.wake.notify_all();
        if let Some(h) = self.heartbeat.take() {
            let _ = h.join();
        }
        if self.check_owned().is_ok() {
            let _ = fs::remove_dir(&self.path);
        }
    }
}
```

- [ ] **Step 4: Implement flock and the mutation guard**

Top of `flock.rs`:
```rust
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use crate::env::Env;
use crate::mkdir_lock::LockError;

/// An exclusive `flock` on a file, released by the kernel when the holder exits. The fd is
/// `O_CLOEXEC` (std's default).
#[derive(Debug)]
pub struct FlockGuard {
    _file: File,
    path: PathBuf,
}

impl FlockGuard {
    pub fn try_lock(path: &Path) -> io::Result<Option<Self>> {
        if let Some(dir) = path.parent() {
            crate::atomic::ensure_private_dir(dir)?;
        }
        let file = OpenOptions::new().read(true).write(true).create(true).mode(0o600).open(path)?;
        // SAFETY: `file` owns a valid descriptor for the duration of the call.
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc == 0 {
            return Ok(Some(Self { _file: file, path: path.to_path_buf() }));
        }
        let e = io::Error::last_os_error();
        if e.raw_os_error() == Some(libc::EWOULDBLOCK) {
            Ok(None)
        } else {
            Err(e)
        }
    }

    pub fn lock(path: &Path, timeout: Duration) -> Result<Self, LockError> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(g) = Self::try_lock(path)? {
                return Ok(g);
            }
            if Instant::now() >= deadline {
                return Err(LockError::Timeout(path.to_path_buf()));
            }
            thread::sleep(Duration::from_millis(100));
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// tagteam's mutation lock (§9.1). Provider live locks can only be taken from one.
#[derive(Debug)]
pub struct MutationGuard {
    _lock: FlockGuard,
}

impl MutationGuard {
    pub const TIMEOUT: Duration = Duration::from_secs(10);
    pub const BOOTSTRAP_TIMEOUT: Duration = Duration::from_secs(30);

    pub fn acquire(env: &Env, timeout: Duration) -> Result<Self, LockError> {
        let path = env.data_dir().join(".mutation.lock");
        Ok(Self { _lock: FlockGuard::lock(&path, timeout)? })
    }
}
```

The imports at the top of `flock.rs` are `std::fs::{File, OpenOptions}` (no bare `fs`); the
tests module adds `use std::fs;` itself.

- [ ] **Step 5: Implement process stamps**

Top of `process.rs`:
```rust
use std::io;

/// A pid plus its start time, so a recycled pid is never mistaken for the original (§12.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessStamp {
    pub pid: u32,
    pub start: u64,
}

impl ProcessStamp {
    pub fn current() -> io::Result<Self> {
        let pid = std::process::id();
        let start = start_of(pid)?.ok_or_else(|| io::Error::other("own process not found"))?;
        Ok(Self { pid, start })
    }

    /// Exact pid and start-time match. Anything undeterminable counts as live.
    pub fn is_live(&self) -> bool {
        match start_of(self.pid) {
            Ok(Some(start)) => start == self.start,
            Ok(None) => false,
            Err(_) => true,
        }
    }
}

/// `/proc/<pid>/stat` field 22, counted after the last `)`.
#[cfg(target_os = "linux")]
fn start_of(pid: u32) -> io::Result<Option<u64>> {
    let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let after = stat.rfind(')').map(|i| &stat[i + 1..]).ok_or_else(|| io::Error::other("bad stat"))?;
    // `after` starts at field 3; field 22 is the 20th item.
    after
        .split_whitespace()
        .nth(19)
        .and_then(|f| f.parse().ok())
        .map(Some)
        .ok_or_else(|| io::Error::other("bad stat"))
}

/// `proc_pidinfo(PROC_PIDTBSDINFO)` start time, in microseconds.
#[cfg(target_os = "macos")]
fn start_of(pid: u32) -> io::Result<Option<u64>> {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: `info` is a correctly sized, writable proc_bsdinfo.
    let n = unsafe {
        libc::proc_pidinfo(pid as libc::c_int, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size)
    };
    if n == size {
        return Ok(Some(info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec));
    }
    let e = io::Error::last_os_error();
    if e.raw_os_error() == Some(libc::ESRCH) { Ok(None) } else { Err(e) }
}
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p tagteam-provider`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/tagteam-provider
git commit -m "Add mkdir and flock locks and process liveness"
```

---

### Task 11: The `Provider` trait and its types

**Files:**
- Create: `crates/tagteam-provider/src/provider.rs`
- Modify: `crates/tagteam-provider/src/lib.rs`

**Interfaces:**
- Consumes: `Read`, `Credential`, `Env`, `MutationGuard`, `LockError`, `KeychainError`,
  `ProviderId`, `IdentityKey`, `Fingerprint`
- Produces (the M1 subset of spec §4.5; later milestones add methods):
```rust
pub struct Identity { pub label: String, pub email: Option<String>, pub org_uuid: String,
                      pub org_name: Option<String>, pub account_uuid: Option<String>,
                      pub raw: serde_json::Value }
pub struct StoredLogin { pub kind: String, pub secret: Vec<u8>, pub identity: Identity }  // Debug redacts `secret`
pub struct CapturedLogin { pub identity: Identity, pub kind: String, pub credential: Credential,
                           pub login_expires_at: Option<i64> }
pub struct LiveAuth { pub credential: Read<Credential>, pub managed_key: Read<Vec<u8>> }
pub struct IdentitySurface { pub json_keys: Vec<(PathBuf, Vec<String>)>,
                             pub credential_files: Vec<PathBuf>,
                             pub credential_items: Vec<(String, String)>,
                             pub owned_items: Vec<(String, String)>,
                             pub machine_shared_keys: Vec<&'static str> }
pub trait LiveLockSet: Send { fn check_owned(&self) -> Result<(), LockError>; }
pub struct LiveLocks<'g>   // new(&'g MutationGuard, Box<dyn LiveLockSet + 'g>), check_owned()
pub trait Undo: Send { fn undo(self: Box<Self>, locks: &LiveLocks<'_>) -> Result<(), ProviderError>; fn what(&self) -> String; }
pub enum ProviderError { Unreadable(ReadError), ConfigUnsplicable(PathBuf), ShadowingItem(String),
                         RestoreFailed { cause: Box<ProviderError>, restore: Box<ProviderError> },
                         Lock(LockError), Keychain(KeychainError), Io(io::Error), Invalid(String) }
pub trait Provider: Send + Sync {
    fn id(&self) -> ProviderId;
    fn display_name(&self) -> &'static str;
    fn identity_surface(&self, env: &Env) -> IdentitySurface;
    fn identity_key(&self, id: &Identity) -> IdentityKey;
    fn credential_kinds(&self) -> &'static [&'static str];
    fn parse_identity(&self, raw: &serde_json::Value) -> Result<Identity, ProviderError>;
    fn token_identity(&self, email: &str) -> Identity;
    fn token_secret(&self, token: &str) -> (String, Vec<u8>);   // (kind, secret) for add-token
    fn classify(&self, secret: &[u8]) -> String;
    fn fingerprint(&self, secret: &[u8]) -> Option<Fingerprint>;
    fn has_refresh_token(&self, secret: &[u8]) -> bool;
    fn is_wiped(&self, secret: &[u8]) -> bool;
    fn login_expires_at(&self, secret: &[u8]) -> Option<i64>;
    fn live_identity(&self, env: &Env) -> Read<Identity>;
    fn read_live_auth(&self, env: &Env) -> LiveAuth;
    fn lock_live<'g>(&self, env: &Env, g: &'g MutationGuard) -> Result<LiveLocks<'g>, ProviderError>;
    fn write_credential(&self, env: &Env, locks: &LiveLocks<'_>, target: &StoredLogin, live: &LiveAuth)
        -> Result<Box<dyn Undo>, ProviderError>;
    fn clear_other_axis(&self, env: &Env, locks: &LiveLocks<'_>, kept_kind: &str)
        -> Result<Box<dyn Undo>, ProviderError>;
    fn write_identity(&self, env: &Env, locks: &LiveLocks<'_>, identity: Option<&Identity>)
        -> Result<Box<dyn Undo>, ProviderError>;
    fn uses_file_store(&self, env: &Env) -> bool;
}
```

- [ ] **Step 1: Write the failing test**

Append to `provider.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    struct Held(std::cell::Cell<bool>);
    impl LiveLockSet for Held {
        fn check_owned(&self) -> Result<(), LockError> {
            if self.0.get() { Ok(()) } else { Err(LockError::Compromised("x".into())) }
        }
    }

    #[test]
    fn live_locks_delegate_ownership_checks() {
        let d = tempfile::tempdir().unwrap();
        let env = Env::for_test(d.path());
        let g = MutationGuard::acquire(&env, std::time::Duration::from_millis(100)).unwrap();
        let locks = LiveLocks::new(&g, Box::new(Held(std::cell::Cell::new(true))));
        assert!(locks.check_owned().is_ok());
    }

    #[test]
    fn debug_output_never_contains_secret_bytes() {
        const SENTINEL: &str = "sk-ant-ort01-SENTINEL-7f3a";
        let s = StoredLogin {
            kind: "oauth".into(),
            secret: SENTINEL.as_bytes().to_vec(),
            identity: Identity {
                label: "a@b.co".into(),
                email: Some("a@b.co".into()),
                org_uuid: String::new(),
                org_name: None,
                account_uuid: None,
                raw: serde_json::json!({}),
            },
        };
        assert!(!format!("{s:?}").contains("SENTINEL"));
        let auth = LiveAuth {
            credential: Read::Present(Credential::fresh(SENTINEL.as_bytes().to_vec())),
            managed_key: Read::Present(SENTINEL.as_bytes().to_vec()),
        };
        assert!(!format!("{auth:?}").contains("SENTINEL"));
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p tagteam-provider provider`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

Top of `provider.rs`:
```rust
use std::fmt;
use std::io;
use std::marker::PhantomData;
use std::path::PathBuf;

use serde_json::Value;
use tagteam_core::{Fingerprint, IdentityKey, ProviderId};

use crate::credential::Credential;
use crate::env::Env;
use crate::flock::MutationGuard;
use crate::keychain::KeychainError;
use crate::mkdir_lock::LockError;
use crate::read::{Read, ReadError};

/// A login's identity. `raw` is the provider-owned object stored in `identity_json`
/// (CC: the `oauthAccount` object).
#[derive(Debug, Clone, PartialEq)]
pub struct Identity {
    pub label: String,
    pub email: Option<String>,
    pub org_uuid: String,
    pub org_name: Option<String>,
    pub account_uuid: Option<String>,
    pub raw: Value,
}

#[derive(Clone)]
pub struct StoredLogin {
    pub kind: String,
    pub secret: Vec<u8>,
    pub identity: Identity,
}

impl fmt::Debug for StoredLogin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoredLogin")
            .field("kind", &self.kind)
            .field("secret", &format_args!("<{} bytes>", self.secret.len()))
            .field("identity", &self.identity.label)
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct CapturedLogin {
    pub identity: Identity,
    pub kind: String,
    pub credential: Credential,
    pub login_expires_at: Option<i64>,
}

/// Both auth axes, each tri-state: the credential entry and the managed API key.
#[derive(Clone)]
pub struct LiveAuth {
    pub credential: Read<Credential>,
    pub managed_key: Read<Vec<u8>>,
}

impl fmt::Debug for LiveAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let key = match &self.managed_key {
            Read::Present(k) => format!("<{} bytes>", k.len()),
            Read::Absent => "Absent".into(),
            Read::Unreadable(e) => format!("Unreadable({e})"),
        };
        f.debug_struct("LiveAuth").field("credential", &self.credential).field("managed_key", &key).finish()
    }
}

/// The exact provider-owned state a switch may write (§3). Drives the pinned test (§15.3).
#[derive(Debug, Clone, Default)]
pub struct IdentitySurface {
    /// Files where only these top-level keys may change; every other byte stays identical.
    pub json_keys: Vec<(PathBuf, Vec<String>)>,
    /// Credential files whose account-scoped keys may change; machine-shared keys may not.
    pub credential_files: Vec<PathBuf>,
    /// Keychain credential entries (service, account), compared like `credential_files`.
    pub credential_items: Vec<(String, String)>,
    /// Keychain items the provider may write wholesale (CC: the managed-key item).
    pub owned_items: Vec<(String, String)>,
    pub machine_shared_keys: Vec<&'static str>,
}

pub trait LiveLockSet: Send {
    fn check_owned(&self) -> Result<(), LockError>;
}

/// A provider's live locks. Only constructible from a held `MutationGuard` (§4.3).
pub struct LiveLocks<'g> {
    set: Box<dyn LiveLockSet + 'g>,
    _guard: PhantomData<&'g MutationGuard>,
}

impl<'g> LiveLocks<'g> {
    pub fn new(_guard: &'g MutationGuard, set: Box<dyn LiveLockSet + 'g>) -> Self {
        Self { set, _guard: PhantomData }
    }

    pub fn check_owned(&self) -> Result<(), LockError> {
        self.set.check_owned()
    }
}

/// Restores what one write replaced, for same-process rollback (§9.4 step 10). Every restore
/// re-checks lock ownership first: after a takeover, CC may have written since, and restoring
/// would overwrite it (§9.1).
pub trait Undo: Send {
    fn undo(self: Box<Self>, locks: &LiveLocks<'_>) -> Result<(), ProviderError>;
    fn what(&self) -> String;
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("{0}")]
    Unreadable(ReadError),
    #[error(
        "{0} is torn or not a JSON object; restore it from Claude Code's backups (~/.claude/backups/) or repair it, then retry"
    )]
    ConfigUnsplicable(PathBuf),
    #[error("the credential was written to the file, but the Keychain item {0} that shadows it could not be verified gone")]
    ShadowingItem(String),
    /// A write failed part-way and restoring the previous state failed too: the live state is
    /// partial, and only journal recovery may settle it.
    #[error("{cause}; restoring the previous state also failed: {restore}")]
    RestoreFailed { cause: Box<ProviderError>, restore: Box<ProviderError> },
    #[error(transparent)]
    Lock(#[from] LockError),
    #[error(transparent)]
    Keychain(#[from] KeychainError),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("{0}")]
    Invalid(String),
}

pub trait Provider: Send + Sync {
    fn id(&self) -> ProviderId;
    fn display_name(&self) -> &'static str;
    fn identity_surface(&self, env: &Env) -> IdentitySurface;
    fn identity_key(&self, id: &Identity) -> IdentityKey;
    fn credential_kinds(&self) -> &'static [&'static str];
    fn parse_identity(&self, raw: &Value) -> Result<Identity, ProviderError>;
    /// The identity recorded for a token-only account (`add-token`, §10.2).
    fn token_identity(&self, email: &str) -> Identity;
    /// `(kind, vault bytes)` for a token given to `add-token`.
    fn token_secret(&self, token: &str) -> (String, Vec<u8>);

    fn classify(&self, secret: &[u8]) -> String;
    fn fingerprint(&self, secret: &[u8]) -> Option<Fingerprint>;
    fn has_refresh_token(&self, secret: &[u8]) -> bool;
    fn is_wiped(&self, secret: &[u8]) -> bool;
    fn login_expires_at(&self, secret: &[u8]) -> Option<i64>;

    /// `Absent` means there is no live login.
    fn live_identity(&self, env: &Env) -> Read<Identity>;
    fn read_live_auth(&self, env: &Env) -> LiveAuth;
    fn lock_live<'g>(&self, env: &Env, g: &'g MutationGuard) -> Result<LiveLocks<'g>, ProviderError>;
    /// Composes the target (§9.4 step 5), writes it on its axis, then clears the other axis
    /// (step 7). Refuses when an entry it would overwrite cannot be read fresh.
    fn write_credential(
        &self,
        env: &Env,
        locks: &LiveLocks<'_>,
        target: &StoredLogin,
        live: &LiveAuth,
    ) -> Result<Box<dyn Undo>, ProviderError>;
    /// Clears the auth axis other than `kept_kind`'s (§9.6 finish-forward).
    fn clear_other_axis(&self, env: &Env, locks: &LiveLocks<'_>, kept_kind: &str)
    -> Result<Box<dyn Undo>, ProviderError>;
    /// Splices the identity into the live config; `None` removes it (§9.4 step 8).
    fn write_identity(&self, env: &Env, locks: &LiveLocks<'_>, identity: Option<&Identity>)
    -> Result<Box<dyn Undo>, ProviderError>;
    /// For the post-switch hint (§9.4 "After unlocking").
    fn uses_file_store(&self, env: &Env) -> bool;
}
```

`lib.rs` additions:
```rust
pub mod provider;
pub use provider::{
    CapturedLogin, Identity, IdentitySurface, LiveAuth, LiveLockSet, LiveLocks, Provider, ProviderError,
    StoredLogin, Undo,
};
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p tagteam-provider`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tagteam-provider
git commit -m "Define the Provider trait for M1"
```

---
### Task 12: Claude Code paths and Keychain naming

**Files:**
- Create: `crates/tagteam-cc/src/paths.rs`, `crates/tagteam-cc/src/naming.rs`
- Modify: `crates/tagteam-cc/src/lib.rs`

**Interfaces:**
- Produces:
  - `struct CcPaths { config_home, global_config, secure_storage_dir, credentials_file, refresh_lock, config_lock }`
    (all `PathBuf`, `pub`), `CcPaths::resolve(&Env) -> CcPaths`, `.legacy_lock() -> PathBuf`
  - `enum ItemKind { OAuth, ManagedKey }`; `keychain_service(&Env, ItemKind) -> String`;
    `read_services(&Env, ItemKind) -> Vec<String>` (primary first); `keychain_account(&Env) -> String`
  - `pub(crate) fn nfc(&OsStr) -> String`

- [ ] **Step 1: Write the failing tests**

Append to `paths.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn env(root: &Path) -> Env {
        Env::for_test(root)
    }

    #[test]
    fn defaults_follow_appendix_a1() {
        let d = tempfile::tempdir().unwrap();
        let h = d.path().join("home");
        let p = CcPaths::resolve(&env(d.path()));
        assert_eq!(p.config_home, h.join(".claude"));
        assert_eq!(p.global_config, h.join(".claude.json"));
        assert_eq!(p.secure_storage_dir, h.join(".claude"));
        assert_eq!(p.credentials_file, h.join(".claude/.credentials.json"));
        assert_eq!(p.refresh_lock, h.join(".claude/.oauth_refresh.lock"));
        assert_eq!(p.config_lock, h.join(".claude.json.lock"));
    }

    #[test]
    fn a_legacy_config_json_wins_when_present() {
        let d = tempfile::tempdir().unwrap();
        let claude = d.path().join("home/.claude");
        fs::create_dir_all(&claude).unwrap();
        fs::write(claude.join(".config.json"), "{}").unwrap();
        let p = CcPaths::resolve(&env(d.path()));
        assert_eq!(p.global_config, claude.join(".config.json"));
        assert_eq!(p.config_lock, claude.join(".config.json.lock"));
    }

    #[test]
    fn claude_config_dir_moves_home_and_config_but_empty_means_unset() {
        let d = tempfile::tempdir().unwrap();
        let mut e = env(d.path());
        e.claude_config_dir = Some("/p".into());
        let p = CcPaths::resolve(&e);
        assert_eq!(p.config_home, Path::new("/p"));
        assert_eq!(p.global_config, Path::new("/p/.claude.json"));
        assert_eq!(p.credentials_file, Path::new("/p/.credentials.json"));
        e.claude_config_dir = Some("".into());
        assert_eq!(CcPaths::resolve(&e).config_home, d.path().join("home/.claude"));
    }

    #[test]
    fn secure_storage_dir_anchors_credentials_and_locks() {
        let d = tempfile::tempdir().unwrap();
        let mut e = env(d.path());
        e.claude_config_dir = Some("/p".into());
        e.claude_securestorage_config_dir = Some("".into());
        let p = CcPaths::resolve(&e);
        assert_eq!(p.secure_storage_dir, d.path().join("home/.claude"));
        assert_eq!(p.credentials_file, d.path().join("home/.claude/.credentials.json"));
        e.claude_securestorage_config_dir = Some("/s".into());
        let p = CcPaths::resolve(&e);
        assert_eq!(p.refresh_lock, Path::new("/s/.oauth_refresh.lock"));
        assert_eq!(p.legacy_lock(), Path::new("/s.lock"));
    }

    #[test]
    fn the_legacy_lock_resolves_symlinks() {
        let d = tempfile::tempdir().unwrap();
        let real = d.path().join("real-claude");
        fs::create_dir_all(&real).unwrap();
        fs::create_dir_all(d.path().join("home")).unwrap();
        std::os::unix::fs::symlink(&real, d.path().join("home/.claude")).unwrap();
        let p = CcPaths::resolve(&env(d.path()));
        let mut expected = fs::canonicalize(&real).unwrap().into_os_string();
        expected.push(".lock");
        assert_eq!(p.legacy_lock(), PathBuf::from(expected));
    }
}
```

Append to `naming.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn env() -> Env {
        let mut e = Env::for_test(Path::new("/"));
        e.home = PathBuf::from("/home/tester");
        e
    }

    #[test]
    fn default_items_are_unsuffixed() {
        assert_eq!(keychain_service(&env(), ItemKind::OAuth), "Claude Code-credentials");
        assert_eq!(keychain_service(&env(), ItemKind::ManagedKey), "Claude Code");
    }

    #[test]
    fn a_config_dir_suffixes_with_the_raw_string_hash() {
        let mut e = env();
        e.claude_config_dir = Some("/home/tester/profile".into());
        assert_eq!(keychain_service(&e, ItemKind::OAuth), "Claude Code-credentials-535fa96b");
        assert_eq!(keychain_service(&e, ItemKind::ManagedKey), "Claude Code-535fa96b");
        e.claude_config_dir = Some("/home/tester/profile/".into());
        assert_eq!(keychain_service(&e, ItemKind::OAuth), "Claude Code-credentials-2c60625b");
        e.claude_config_dir = Some("".into());
        assert_eq!(keychain_service(&e, ItemKind::OAuth), "Claude Code-credentials");
    }

    #[test]
    fn names_are_nfc_normalized() {
        let mut e = env();
        e.claude_config_dir = Some("/home/tester/cafe\u{301}".into()); // NFD
        assert_eq!(keychain_service(&e, ItemKind::OAuth), "Claude Code-credentials-3f2ee927");
    }

    #[test]
    fn a_defined_secure_storage_dir_decides_alone() {
        let mut e = env();
        e.claude_config_dir = Some("/home/tester/profile".into());
        e.claude_securestorage_config_dir = Some("".into());
        assert_eq!(keychain_service(&e, ItemKind::OAuth), "Claude Code-credentials");
        e.claude_config_dir = None;
        e.claude_securestorage_config_dir = Some("/home/tester/profile".into());
        assert_eq!(keychain_service(&e, ItemKind::OAuth), "Claude Code-credentials-535fa96b");
    }

    #[test]
    fn an_explicit_default_config_dir_reads_suffixed_then_plain() {
        let mut e = env();
        e.claude_config_dir = Some("/home/tester/.claude".into());
        assert_eq!(
            read_services(&e, ItemKind::OAuth),
            vec!["Claude Code-credentials-b2e2cf9d".to_string(), "Claude Code-credentials".into()]
        );
        e.claude_config_dir = Some("/home/tester/profile".into());
        assert_eq!(read_services(&e, ItemKind::OAuth).len(), 1);
    }

    #[test]
    fn a_symlinked_config_dir_is_also_read_by_its_target() {
        let d = tempfile::tempdir().unwrap();
        let target = d.path().join("real");
        std::fs::create_dir(&target).unwrap();
        let link = d.path().join("link");
        std::os::unix::fs::symlink("real", &link).unwrap(); // relative target
        let mut e = env();
        e.claude_config_dir = Some(link.clone().into_os_string());
        let hash = |p: &Path| hex::encode(Sha256::digest(p.to_str().unwrap().as_bytes()).as_slice())[..8].to_owned();
        assert_eq!(
            read_services(&e, ItemKind::OAuth),
            vec![format!("Claude Code-credentials-{}", hash(&link)), format!("Claude Code-credentials-{}", hash(&target))]
        );
    }

    #[test]
    fn the_account_falls_back_to_claude_code_user() {
        let mut e = env();
        assert_eq!(keychain_account(&e), "tester");
        e.user = Some("bad user".into());
        assert_eq!(keychain_account(&e), "claude-code-user");
        e.user = None;
        let name = keychain_account(&e);
        assert!(!name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)));
    }
}
```

`crates/tagteam-cc/src/lib.rs`:
```rust
pub mod naming;
pub mod paths;

pub use naming::{ItemKind, keychain_account, keychain_service, read_services};
pub use paths::CcPaths;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tagteam-cc`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

Top of `paths.rs`:
```rust
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use tagteam_provider::Env;
use unicode_normalization::UnicodeNormalization;

pub(crate) fn nfc(v: &OsStr) -> String {
    v.to_string_lossy().nfc().collect()
}

fn set_dir(v: Option<&OsStr>) -> Option<PathBuf> {
    v.filter(|v| !v.is_empty()).map(PathBuf::from)
}

fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// Claude Code's path resolution (Appendix A.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CcPaths {
    pub config_home: PathBuf,
    pub global_config: PathBuf,
    pub secure_storage_dir: PathBuf,
    pub credentials_file: PathBuf,
    pub refresh_lock: PathBuf,
    pub config_lock: PathBuf,
}

impl CcPaths {
    pub fn resolve(env: &Env) -> Self {
        let config_dir = set_dir(env.claude_config_dir.as_deref());
        let config_home = config_dir.clone().unwrap_or_else(|| env.home.join(".claude"));
        let legacy = config_home.join(".config.json");
        let global_config = if legacy.exists() {
            legacy
        } else {
            config_dir.unwrap_or_else(|| env.home.clone()).join(".claude.json")
        };
        let secure_storage_dir = match env.claude_securestorage_config_dir.as_deref() {
            Some(v) if v.is_empty() => env.home.join(".claude"),
            Some(v) => PathBuf::from(nfc(v)),
            None => config_home.clone(),
        };
        Self {
            credentials_file: env.guard(secure_storage_dir.join(".credentials.json")),
            refresh_lock: env.guard(secure_storage_dir.join(".oauth_refresh.lock")),
            config_lock: env.guard(with_suffix(&global_config, ".lock")),
            config_home: env.guard(config_home),
            global_config: env.guard(global_config),
            secure_storage_dir: env.guard(secure_storage_dir),
        }
    }

    /// `<realpath(secure-storage dir)>.lock`, or the unresolved path when realpath fails.
    pub fn legacy_lock(&self) -> PathBuf {
        let base = fs::canonicalize(&self.secure_storage_dir).unwrap_or_else(|_| self.secure_storage_dir.clone());
        with_suffix(&base, ".lock")
    }
}
```

Top of `naming.rs`:
```rust
use std::ffi::CStr;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tagteam_provider::Env;

use crate::paths::nfc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    OAuth,
    ManagedKey,
}

/// The directory string whose hash suffixes the item, or `None` for the default items
/// (Appendix A.2: `useDefault`).
fn suffix_source(env: &Env) -> Option<String> {
    match &env.claude_securestorage_config_dir {
        Some(v) if v.is_empty() => None,
        Some(v) => Some(nfc(v)),
        None => env.claude_config_dir.as_deref().filter(|v| !v.is_empty()).map(nfc),
    }
}

fn service_for(kind: ItemKind, source: Option<&str>) -> String {
    let n = match kind {
        ItemKind::OAuth => "-credentials",
        ItemKind::ManagedKey => "",
    };
    match source {
        None => format!("Claude Code{n}"),
        Some(dir) => {
            let hash = hex::encode(Sha256::digest(dir.as_bytes()).as_slice());
            format!("Claude Code{n}-{}", &hash[..8])
        }
    }
}

pub fn keychain_service(env: &Env, kind: ItemKind) -> String {
    service_for(kind, suffix_source(env).as_deref())
}

/// Every item a reader tries, primary first (Appendix A.2): a symlinked directory is also read
/// as the hash of its target (a relative target joins the link's parent), and an explicitly
/// set `CLAUDE_CONFIG_DIR=~/.claude` also falls back to the unsuffixed item. Anything that
/// snapshots, clears or restores the live credential must cover all of them.
pub fn read_services(env: &Env, kind: ItemKind) -> Vec<String> {
    let mut out = vec![keychain_service(env, kind)];
    let mut push = |s: String| {
        if !out.contains(&s) {
            out.push(s);
        }
    };
    if let Some(dir) = suffix_source(env) {
        let link = PathBuf::from(&dir);
        if let Ok(target) = std::fs::read_link(&link) {
            let target = if target.is_absolute() {
                target
            } else {
                link.parent().unwrap_or(Path::new("/")).join(target)
            };
            push(service_for(kind, Some(&nfc(target.as_os_str()))));
        }
    }
    let explicit_default = env.claude_securestorage_config_dir.is_none()
        && env.claude_config_dir.as_deref().is_some_and(|v| {
            let s = v.to_string_lossy();
            PathBuf::from(s.trim_end_matches('/')) == env.home.join(".claude")
        });
    if explicit_default {
        push(service_for(kind, None));
    }
    out
}

fn passwd_name() -> Option<String> {
    // SAFETY: getpwuid returns a pointer into static storage or null; we copy out at once.
    unsafe {
        let pw = libc::getpwuid(libc::geteuid());
        if pw.is_null() {
            return None;
        }
        CStr::from_ptr((*pw).pw_name).to_str().ok().map(str::to_owned)
    }
}

/// `$USER`, else the passwd name, else `claude-code-user`; also `claude-code-user` when the
/// name fails `^[a-zA-Z0-9._-]+$`.
pub fn keychain_account(env: &Env) -> String {
    let name = env.user.clone().filter(|u| !u.is_empty()).or_else(passwd_name);
    match name {
        Some(n) if n.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)) => n,
        _ => "claude-code-user".to_owned(),
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p tagteam-cc`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tagteam-cc
git commit -m "Resolve Claude Code paths and Keychain item names"
```

---

### Task 13: Claude Code credential and identity shapes

**Files:**
- Create: `crates/tagteam-cc/src/shape.rs`
- Modify: `crates/tagteam-cc/src/lib.rs` (`pub mod shape;`)

**Interfaces:**
- Produces:
  - `pub const MACHINE_SHARED_KEYS: [&str; 5]`, `KIND_OAUTH = "oauth"`,
    `KIND_SETUP_TOKEN = "setup_token"`, `KIND_API_KEY = "api_key"`, `KINDS: [&str; 3]`
  - `is_api_key(&[u8]) -> bool`, `classify(&[u8]) -> &'static str`,
    `fingerprint(&[u8]) -> Option<Fingerprint>`, `has_refresh_token(&[u8]) -> bool`,
    `is_wiped(&[u8]) -> bool`, `login_expires_at(&[u8]) -> Option<i64>`
  - `compose(target: &[u8], live: Option<&Map<String, Value>>) -> Result<Vec<u8>, ProviderError>`
  - `machine_shared_only(&Map<String, Value>) -> Map<String, Value>`
  - `setup_token_credential(&str) -> Vec<u8>`
  - `identity_from_oauth_account(&Value) -> Option<Identity>`, `token_identity(&str) -> Identity`

- [ ] **Step 1: Write the failing tests**

Append to `shape.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn oauth(rt: &str) -> Vec<u8> {
        json!({"claudeAiOauth": {"accessToken": "at", "refreshToken": rt, "expiresAt": 1, "refreshTokenExpiresAt": 99}})
            .to_string()
            .into_bytes()
    }

    #[test]
    fn classification_follows_section_7_1() {
        assert_eq!(classify(b"sk-ant-api03-abc"), KIND_API_KEY);
        assert_eq!(classify(b"  sk-ant-api03-abc\n"), KIND_API_KEY);
        assert_eq!(classify(&oauth("rt")), KIND_OAUTH);
        assert_eq!(classify(&setup_token_credential("sk-ant-oat01-x")), KIND_SETUP_TOKEN);
    }

    #[test]
    fn fingerprints_cover_every_kind() {
        assert_eq!(fingerprint(&oauth("rt-1")), Some(Fingerprint::of_secret(b"rt-1")));
        assert_eq!(fingerprint(&setup_token_credential("tok")), Some(Fingerprint::of_secret(b"tok")));
        assert_eq!(fingerprint(b" sk-ant-api03-k\n"), Some(Fingerprint::of_secret(b"sk-ant-api03-k")));
        assert_eq!(fingerprint(b"{}"), None);
        assert_eq!(fingerprint(b"garbage"), None);
    }

    #[test]
    fn refresh_wiped_and_expiry() {
        assert!(has_refresh_token(&oauth("rt")));
        assert!(!has_refresh_token(&setup_token_credential("t")));
        let wiped = json!({"claudeAiOauth": {"accessToken": "", "refreshToken": ""}}).to_string();
        assert!(is_wiped(wiped.as_bytes()));
        assert!(!is_wiped(&oauth("rt")));
        assert!(!is_wiped(b"sk-ant-api03-x"));
        assert_eq!(login_expires_at(&oauth("rt")), Some(99));
    }

    #[test]
    fn compose_takes_machine_shared_keys_from_live_absence_included() {
        let target = json!({
            "claudeAiOauth": {"refreshToken": "target"},
            "trustedDeviceToken": "target-device",
            "mcpOAuth": {"stale": true},
            "futureKey": 1
        });
        let live = json!({
            "claudeAiOauth": {"refreshToken": "outgoing"},
            "mcpOAuth": {"current": true},
            "pluginSecrets": {"p": "s"}
        });
        let out = compose(target.to_string().as_bytes(), live.as_object()).unwrap();
        let out: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(
            out,
            json!({
                "claudeAiOauth": {"refreshToken": "target"},
                "trustedDeviceToken": "target-device",
                "futureKey": 1,
                "mcpOAuth": {"current": true},
                "pluginSecrets": {"p": "s"}
            })
        );
    }

    #[test]
    fn compose_without_live_drops_machine_shared_keys() {
        let target = json!({"claudeAiOauth": {"refreshToken": "t"}, "pluginSecrets": {"old": 1}});
        let out: Value = serde_json::from_slice(&compose(target.to_string().as_bytes(), None).unwrap()).unwrap();
        assert_eq!(out, json!({"claudeAiOauth": {"refreshToken": "t"}}));
    }

    #[test]
    fn machine_shared_only_strips_account_keys() {
        let live = json!({"claudeAiOauth": {}, "trustedDeviceToken": "d", "x": 1, "mcpOAuth": {"m": 1}});
        assert_eq!(Value::Object(machine_shared_only(live.as_object().unwrap())), json!({"mcpOAuth": {"m": 1}}));
    }

    #[test]
    fn setup_tokens_have_the_spec_shape() {
        assert_eq!(
            setup_token_credential("tok"),
            br#"{"claudeAiOauth":{"accessToken":"tok","scopes":["user:inference"]}}"#.to_vec()
        );
    }

    #[test]
    fn identities_parse_from_oauth_account() {
        let id = identity_from_oauth_account(&json!({
            "emailAddress": "a@b.co", "organizationUuid": null, "organizationName": null, "accountUuid": "u-1"
        }))
        .unwrap();
        assert_eq!((id.label.as_str(), id.org_uuid.as_str(), id.account_uuid.as_deref()), ("a@b.co", "", Some("u-1")));
        assert!(identity_from_oauth_account(&json!({"emailAddress": ""})).is_none());
        assert!(identity_from_oauth_account(&json!({})).is_none());
        let t = token_identity("api-key-3@token.local");
        assert_eq!(t.account_uuid, None);
        assert_eq!(t.raw, json!({"emailAddress": "api-key-3@token.local", "accountUuid": "", "organizationUuid": null, "organizationName": null}));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tagteam-cc shape`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

Top of `shape.rs`:
```rust
use serde_json::{Map, Value, json};
use tagteam_core::Fingerprint;
use tagteam_provider::{Identity, ProviderError};

/// Taken from the live credential on activation, absence included (Appendix A.4, B.9).
pub const MACHINE_SHARED_KEYS: [&str; 5] =
    ["mcpOAuth", "mcpOAuthClientConfig", "mcpXaaIdp", "mcpXaaIdpConfig", "pluginSecrets"];
pub const KIND_OAUTH: &str = "oauth";
pub const KIND_SETUP_TOKEN: &str = "setup_token";
pub const KIND_API_KEY: &str = "api_key";
pub const KINDS: [&str; 3] = [KIND_OAUTH, KIND_SETUP_TOKEN, KIND_API_KEY];

/// §7.1: `trim().starts_with("sk-ant-api") && !starts_with('{')`.
pub fn is_api_key(bytes: &[u8]) -> bool {
    let s = String::from_utf8_lossy(bytes);
    let t = s.trim();
    t.starts_with("sk-ant-api") && !t.starts_with('{')
}

fn oauth_obj(bytes: &[u8]) -> Option<Map<String, Value>> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    v.get("claudeAiOauth")?.as_object().cloned()
}

fn token<'a>(o: &'a Map<String, Value>, k: &str) -> Option<&'a str> {
    o.get(k).and_then(Value::as_str).filter(|s| !s.is_empty())
}

pub fn classify(bytes: &[u8]) -> &'static str {
    if is_api_key(bytes) {
        return KIND_API_KEY;
    }
    match oauth_obj(bytes) {
        Some(o) if token(&o, "refreshToken").is_some() => KIND_OAUTH,
        _ => KIND_SETUP_TOKEN,
    }
}

/// §2 "Generation": the refresh token, else the access token, else the key itself.
pub fn fingerprint(bytes: &[u8]) -> Option<Fingerprint> {
    if is_api_key(bytes) {
        let s = String::from_utf8_lossy(bytes);
        return Some(Fingerprint::of_secret(s.trim().as_bytes()));
    }
    let o = oauth_obj(bytes)?;
    token(&o, "refreshToken")
        .or_else(|| token(&o, "accessToken"))
        .map(|t| Fingerprint::of_secret(t.as_bytes()))
}

pub fn has_refresh_token(bytes: &[u8]) -> bool {
    oauth_obj(bytes).is_some_and(|o| token(&o, "refreshToken").is_some())
}

/// Both tokens empty: CC's reaction to `invalid_grant`.
pub fn is_wiped(bytes: &[u8]) -> bool {
    oauth_obj(bytes).is_some_and(|o| token(&o, "accessToken").is_none() && token(&o, "refreshToken").is_none())
}

pub fn login_expires_at(bytes: &[u8]) -> Option<i64> {
    oauth_obj(bytes)?.get("refreshTokenExpiresAt")?.as_i64()
}

/// §9.4 step 5: account-scoped keys from the target, machine-shared keys from the live
/// credential (absence included); with no live JSON credential, none at all.
pub fn compose(target: &[u8], live: Option<&Map<String, Value>>) -> Result<Vec<u8>, ProviderError> {
    let mut out: Map<String, Value> = match serde_json::from_slice::<Value>(target) {
        Ok(Value::Object(o)) => o,
        _ => return Err(ProviderError::Invalid("the stored credential is not a JSON object".into())),
    };
    for k in MACHINE_SHARED_KEYS {
        out.remove(k);
    }
    if let Some(live) = live {
        for k in MACHINE_SHARED_KEYS {
            if let Some(v) = live.get(k) {
                out.insert(k.to_owned(), v.clone());
            }
        }
    }
    Ok(serde_json::to_vec(&Value::Object(out)).expect("a Value always serializes"))
}

pub fn machine_shared_only(live: &Map<String, Value>) -> Map<String, Value> {
    live.iter()
        .filter(|(k, _)| MACHINE_SHARED_KEYS.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

pub fn setup_token_credential(token: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({"claudeAiOauth": {"accessToken": token, "scopes": ["user:inference"]}}))
        .expect("a Value always serializes")
}

pub fn identity_from_oauth_account(v: &Value) -> Option<Identity> {
    let email = v.get("emailAddress")?.as_str().filter(|s| !s.is_empty())?.to_owned();
    let str_field = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    Some(Identity {
        label: email.clone(),
        email: Some(email),
        org_uuid: str_field("organizationUuid").unwrap_or_default(),
        org_name: str_field("organizationName"),
        account_uuid: str_field("accountUuid").filter(|s| !s.is_empty()),
        raw: v.clone(),
    })
}

/// The identity cswap records for token accounts.
pub fn token_identity(email: &str) -> Identity {
    let raw = json!({"emailAddress": email, "accountUuid": "", "organizationUuid": null, "organizationName": null});
    identity_from_oauth_account(&raw).expect("a non-empty email always parses")
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p tagteam-cc shape`
Expected: PASS (8 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/tagteam-cc
git commit -m "Add Claude Code credential and identity shapes"
```

---

### Task 14: The live store — `~/.claude.json`, credential entry and managed key

**Files:**
- Create: `crates/tagteam-cc/src/config.rs`, `crates/tagteam-cc/src/live.rs`
- Modify: `crates/tagteam-cc/src/lib.rs`

**Interfaces:**
- Consumes: `CcPaths`, naming, shape, `Keychain`, `write_atomic`, splice
- Produces:
  - `config::read_bytes(&Path) -> Read<Vec<u8>>`, `config::live_identity(&CcPaths) -> Read<Identity>`,
    `config::get_key(&Path, &str) -> Read<Option<Value>>`,
    `config::splice_key(&Path, &str, Option<&Value>, Fence) -> Result<ConfigUndo, ProviderError>`;
    `ConfigUndo` implements `Undo` (checking ownership first) and a path-only `Debug`
  - `enum Platform { MacOs, Linux }`, `Platform::current()`
  - `pub type Fence<'a> = &'a dyn Fn() -> Result<(), ProviderError>` — checked immediately
    before every protected mutation
  - `LiveStore::new(Arc<dyn Keychain>, Platform)`, `.with_retry_delay(Duration)`,
    `.read_credential(&Env, &CcPaths) -> Read<Credential>`,
    `.read_managed_key(&Env, &CcPaths) -> Read<Vec<u8>>`,
    `.write_credential_entry(&Env, &CcPaths, &[u8], Fence) -> Result<(), ProviderError>`,
    `.clear_credential_account_keys(&Env, &CcPaths, Fence) -> Result<(), ProviderError>`,
    `.write_managed_key(&Env, &CcPaths, &[u8], Fence) -> Result<(), ProviderError>`,
    `.clear_managed_key(&Env, &CcPaths, Fence) -> Result<(), ProviderError>`,
    `.snapshot(&Env, &CcPaths) -> Result<Snapshot, ProviderError>` (covers every read service),
    `.restore(&Env, &CcPaths, &Snapshot, Fence) -> Result<(), ProviderError>`,
    `.file_mode_pinned() -> bool`, `.platform()`; `Snapshot` has no `Debug`

- [ ] **Step 1: Write the failing tests**

`crates/tagteam-cc/tests/live_store.rs`:
```rust
use std::fs;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tagteam_cc::live::{LiveStore, Platform};
use tagteam_cc::{CcPaths, ItemKind, config, keychain_account, keychain_service, read_services};
use tagteam_provider::{
    Env, FakeKeychain, LiveLockSet, LiveLocks, LockError, MutationGuard, ProviderError, Provenance, Read, Undo,
};

struct Fx {
    _dir: tempfile::TempDir,
    env: Env,
    paths: CcPaths,
    kc: Arc<FakeKeychain>,
}

fn fx() -> Fx {
    fx_with(|_| {})
}

fn fx_with(adjust: impl FnOnce(&mut Env)) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let mut env = Env::for_test(dir.path());
    adjust(&mut env);
    fs::create_dir_all(env.home.join(".claude")).unwrap();
    let paths = CcPaths::resolve(&env);
    Fx { _dir: dir, env, paths, kc: Arc::new(FakeKeychain::new()) }
}

fn store(f: &Fx, p: Platform) -> LiveStore {
    LiveStore::new(f.kc.clone(), p).with_retry_delay(Duration::ZERO)
}

/// A fence that always passes: these tests hold no CC locks.
fn open() -> Result<(), ProviderError> {
    Ok(())
}

/// A lock set that is always owned, for undo calls in these tests.
struct Held;

impl LiveLockSet for Held {
    fn check_owned(&self) -> Result<(), LockError> {
        Ok(())
    }
}

fn oauth_svc(f: &Fx) -> (String, String) {
    (keychain_service(&f.env, ItemKind::OAuth), keychain_account(&f.env))
}

fn json_of(b: &[u8]) -> Value {
    serde_json::from_slice(b).unwrap()
}

#[test]
fn mac_reads_the_keychain_first_then_the_file() {
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    assert!(matches!(s.read_credential(&f.env, &f.paths), Read::Absent));
    fs::write(&f.paths.credentials_file, "file").unwrap();
    let c = s.read_credential(&f.env, &f.paths).present().unwrap();
    assert_eq!((c.bytes(), c.provenance()), (&b"file"[..], Provenance::Fresh));
    f.kc.put(&svc, &acct, b"kc");
    assert_eq!(s.read_credential(&f.env, &f.paths).present().unwrap().bytes(), b"kc");
}

#[test]
fn a_failed_keychain_read_covered_by_the_file_is_degraded() {
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, b"kc");
    f.kc.set_unreadable(&svc, &acct, true);
    assert!(matches!(s.read_credential(&f.env, &f.paths), Read::Unreadable(_)));
    fs::write(&f.paths.credentials_file, "file").unwrap();
    assert_eq!(s.read_credential(&f.env, &f.paths).present().unwrap().provenance(), Provenance::Degraded);
}

#[test]
fn an_unreadable_primary_item_is_never_skipped_for_a_fallback() {
    let f = fx_with(|e| e.claude_config_dir = Some(e.home.join(".claude").into_os_string()));
    let s = store(&f, Platform::MacOs);
    let acct = keychain_account(&f.env);
    for kind in [ItemKind::OAuth, ItemKind::ManagedKey] {
        let services = read_services(&f.env, kind);
        f.kc.put(&services[0], &acct, b"newer");
        f.kc.set_unreadable(&services[0], &acct, true);
        f.kc.put(&services[1], &acct, b"older");
    }
    assert!(matches!(s.read_credential(&f.env, &f.paths), Read::Unreadable(_)));
    assert!(matches!(s.read_managed_key(&f.env, &f.paths), Read::Unreadable(_)));
    fs::write(&f.paths.credentials_file, "file").unwrap();
    assert_eq!(s.read_credential(&f.env, &f.paths).present().unwrap().provenance(), Provenance::Degraded);
}

#[test]
fn a_lock_lost_before_publication_publishes_nothing() {
    let f = fx();
    fs::write(&f.paths.global_config, "{\"a\": 1}").unwrap();
    let lost = || Err(ProviderError::Lock(LockError::Compromised("x".into())));
    assert!(config::splice_key(&f.paths.global_config, "oauthAccount", Some(&json!({})), &lost).is_err());
    assert_eq!(fs::read(&f.paths.global_config).unwrap(), b"{\"a\": 1}");
}

#[test]
fn linux_reads_and_writes_only_the_file() {
    let f = fx();
    let s = store(&f, Platform::Linux);
    s.write_credential_entry(&f.env, &f.paths, b"{\"a\":1}", &open).unwrap();
    assert_eq!(fs::read(&f.paths.credentials_file).unwrap(), b"{\"a\":1}");
    assert!(f.kc.items().is_empty());
    assert_eq!(s.read_credential(&f.env, &f.paths).present().unwrap().bytes(), b"{\"a\":1}");
}

#[test]
fn a_keychain_write_bumps_an_existing_file_but_never_creates_one() {
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    s.write_credential_entry(&f.env, &f.paths, b"v1", &open).unwrap();
    assert_eq!(f.kc.get(&svc, &acct).unwrap(), b"v1");
    assert!(!f.paths.credentials_file.exists());
    fs::write(&f.paths.credentials_file, "old").unwrap();
    s.write_credential_entry(&f.env, &f.paths, b"v2", &open).unwrap();
    assert_eq!(fs::read(&f.paths.credentials_file).unwrap(), b"v2");
}

#[test]
fn a_failed_fence_stops_every_write() {
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let lost = || Err(ProviderError::Lock(LockError::Compromised("x".into())));
    assert!(s.write_credential_entry(&f.env, &f.paths, b"v1", &lost).is_err());
    assert!(s.write_managed_key(&f.env, &f.paths, b"sk-ant-api03-zzzz", &lost).is_err());
    assert!(f.kc.items().is_empty());
    assert!(!f.paths.credentials_file.exists() && !f.paths.global_config.exists());
}

#[test]
fn file_fallback_requires_the_shadowing_item_to_be_gone() {
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, b"old");
    f.kc.set_fail_write(&svc, true);
    f.kc.set_fail_delete(&svc, true);
    assert!(s.write_credential_entry(&f.env, &f.paths, b"new", &open).is_err());
    f.kc.set_fail_delete(&svc, false);
    s.write_credential_entry(&f.env, &f.paths, b"new", &open).unwrap();
    assert_eq!(fs::read(&f.paths.credentials_file).unwrap(), b"new");
    assert!(f.kc.get(&svc, &acct).is_none());
    assert!(s.file_mode_pinned());
    // Pinned: the next write goes straight to the file, even with the Keychain healthy again.
    f.kc.set_fail_write(&svc, false);
    s.write_credential_entry(&f.env, &f.paths, b"newer", &open).unwrap();
    assert!(f.kc.get(&svc, &acct).is_none());
}

#[test]
fn clearing_oauth_keeps_machine_shared_keys() {
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    let env_json = json!({"claudeAiOauth": {"refreshToken": "r"}, "trustedDeviceToken": "d", "mcpOAuth": {"m": 1}});
    f.kc.put(&svc, &acct, env_json.to_string().as_bytes());
    fs::write(&f.paths.credentials_file, env_json.to_string()).unwrap();
    s.clear_credential_account_keys(&f.env, &f.paths, &open).unwrap();
    assert_eq!(json_of(&f.kc.get(&svc, &acct).unwrap()), json!({"mcpOAuth": {"m": 1}}));
    assert_eq!(json_of(&fs::read(&f.paths.credentials_file).unwrap()), json!({"mcpOAuth": {"m": 1}}));
    // An entry with nothing machine-shared is deleted outright.
    f.kc.put(&svc, &acct, br#"{"claudeAiOauth":{}}"#);
    fs::write(&f.paths.credentials_file, r#"{"claudeAiOauth":{}}"#).unwrap();
    s.clear_credential_account_keys(&f.env, &f.paths, &open).unwrap();
    assert!(f.kc.get(&svc, &acct).is_none());
    assert!(!f.paths.credentials_file.exists());
}

#[test]
fn every_fallback_item_is_cleared_snapshotted_and_restored() {
    // An explicit CLAUDE_CONFIG_DIR=~/.claude reads the suffixed item, then the plain one.
    let f = fx_with(|e| e.claude_config_dir = Some(e.home.join(".claude").into_os_string()));
    let s = store(&f, Platform::MacOs);
    let acct = keychain_account(&f.env);
    let services = read_services(&f.env, ItemKind::OAuth);
    assert_eq!(services.len(), 2);
    f.kc.put(&services[1], &acct, br#"{"claudeAiOauth":{"refreshToken":"plain"}}"#);
    let snap = s.snapshot(&f.env, &f.paths).unwrap();
    s.clear_credential_account_keys(&f.env, &f.paths, &open).unwrap();
    assert!(f.kc.get(&services[1], &acct).is_none(), "the fallback item must not stay active");
    s.restore(&f.env, &f.paths, &snap, &open).unwrap();
    assert!(f.kc.get(&services[1], &acct).is_some());
}

#[test]
fn managed_keys_record_approval_and_never_leave_a_shadowing_item() {
    let f = fx();
    let s = store(&f, Platform::MacOs);
    fs::write(&f.paths.global_config, "{\n  \"userID\": \"u\"\n}\n").unwrap();
    let key = b"sk-ant-api03-0123456789abcdefghijKLMNOPQRST";
    let tail = "abcdefghijKLMNOPQRST";
    s.write_managed_key(&f.env, &f.paths, key, &open).unwrap();
    let managed = (keychain_service(&f.env, ItemKind::ManagedKey), keychain_account(&f.env));
    assert_eq!(f.kc.get(&managed.0, &managed.1).unwrap(), key);
    let approved = config::get_key(&f.paths.global_config, "customApiKeyResponses").present().unwrap().unwrap();
    assert_eq!(approved["approved"], json!([tail]));
    s.write_managed_key(&f.env, &f.paths, key, &open).unwrap(); // idempotent approval
    let approved = config::get_key(&f.paths.global_config, "customApiKeyResponses").present().unwrap().unwrap();
    assert_eq!(approved["approved"], json!([tail]));
    assert_eq!(s.read_managed_key(&f.env, &f.paths).present().unwrap(), key);

    // The Keychain update fails: the key lands in primaryApiKey and the stale item is removed,
    // so the key CC will actually use is the new one.
    f.kc.set_fail_write(&managed.0, true);
    let other = b"sk-ant-api03-other-key-000000000000";
    s.write_managed_key(&f.env, &f.paths, other, &open).unwrap();
    assert!(f.kc.get(&managed.0, &managed.1).is_none());
    assert_eq!(s.read_managed_key(&f.env, &f.paths).present().unwrap(), other);

    // And when the stale item cannot be removed, the write fails instead of lying.
    f.kc.set_fail_write(&managed.0, false);
    s.write_managed_key(&f.env, &f.paths, key, &open).unwrap();
    f.kc.set_fail_write(&managed.0, true);
    f.kc.set_fail_delete(&managed.0, true);
    assert!(matches!(s.write_managed_key(&f.env, &f.paths, other, &open), Err(ProviderError::ShadowingItem(_))));
    f.kc.set_fail_write(&managed.0, false);
    f.kc.set_fail_delete(&managed.0, false);

    s.clear_managed_key(&f.env, &f.paths, &open).unwrap();
    assert!(f.kc.get(&managed.0, &managed.1).is_none());
    assert_eq!(config::get_key(&f.paths.global_config, "primaryApiKey").present().unwrap(), None);
    // `approved` is append-only and survives.
    assert!(config::get_key(&f.paths.global_config, "customApiKeyResponses").present().unwrap().is_some());
}

#[test]
fn snapshot_and_restore_are_byte_exact() {
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, b"orig");
    fs::write(&f.paths.global_config, "{\"a\": 1}").unwrap();
    let snap = s.snapshot(&f.env, &f.paths).unwrap();
    s.write_credential_entry(&f.env, &f.paths, b"new", &open).unwrap();
    s.write_managed_key(&f.env, &f.paths, b"sk-ant-api03-zzzzzzzzzzzzzzzzzzzz", &open).unwrap();
    s.restore(&f.env, &f.paths, &snap, &open).unwrap();
    assert_eq!(f.kc.get(&svc, &acct).unwrap(), b"orig");
    assert!(f.kc.get(&keychain_service(&f.env, ItemKind::ManagedKey), &acct).is_none());
    assert!(!f.paths.credentials_file.exists());
    assert_eq!(fs::read(&f.paths.global_config).unwrap(), b"{\"a\": 1}");
}

#[test]
fn an_unreadable_entry_cannot_be_snapshotted() {
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, b"x");
    f.kc.set_unreadable(&svc, &acct, true);
    assert!(s.snapshot(&f.env, &f.paths).is_err());
}

#[test]
fn live_identity_reads_oauth_account_and_refuses_torn_files() {
    let f = fx();
    assert!(matches!(config::live_identity(&f.paths), Read::Absent));
    fs::write(&f.paths.global_config, r#"{"userID": "u"}"#).unwrap();
    assert!(matches!(config::live_identity(&f.paths), Read::Absent));
    fs::write(&f.paths.global_config, r#"{"oauthAccount": {"emailAddress": "a@b.co"}}"#).unwrap();
    assert_eq!(config::live_identity(&f.paths).present().unwrap().label, "a@b.co");
    fs::write(&f.paths.global_config, r#"{"oauthAccount": {"emailAddress": "a@b"#).unwrap();
    assert!(matches!(config::live_identity(&f.paths), Read::Unreadable(_)));
}

#[test]
fn splicing_a_symlinked_config_writes_through_the_link() {
    // Review Focus 5.
    let f = fx();
    let real = f.env.home.join("dotfiles/claude.json");
    fs::create_dir_all(real.parent().unwrap()).unwrap();
    fs::write(&real, "{\n  \"userID\": \"u\"\n}\n").unwrap();
    std::os::unix::fs::symlink(&real, &f.paths.global_config).unwrap();
    let undo = config::splice_key(&f.paths.global_config, "oauthAccount", Some(&json!({"emailAddress": "a@b.co"})), &open).unwrap();
    assert!(fs::symlink_metadata(&f.paths.global_config).unwrap().file_type().is_symlink());
    assert!(fs::read_to_string(&real).unwrap().contains("a@b.co"));
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let held = LiveLocks::new(&g, Box::new(Held));
    Box::new(undo).undo(&held).unwrap();
    assert_eq!(fs::read_to_string(&real).unwrap(), "{\n  \"userID\": \"u\"\n}\n");
}

#[test]
fn rolling_back_through_dangling_links_keeps_the_links() {
    let f = fx();
    let s = store(&f, Platform::Linux);
    let dots = f.env.home.join("dotfiles");
    fs::create_dir_all(&dots).unwrap();
    std::os::unix::fs::symlink(dots.join("claude.json"), &f.paths.global_config).unwrap();
    std::os::unix::fs::symlink(dots.join("credentials.json"), &f.paths.credentials_file).unwrap();
    let snap = s.snapshot(&f.env, &f.paths).unwrap(); // both targets absent
    let undo = config::splice_key(&f.paths.global_config, "oauthAccount", Some(&json!({})), &open).unwrap();
    s.write_credential_entry(&f.env, &f.paths, b"{}", &open).unwrap();
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    Box::new(undo).undo(&LiveLocks::new(&g, Box::new(Held))).unwrap();
    s.restore(&f.env, &f.paths, &snap, &open).unwrap();
    for link in [&f.paths.global_config, &f.paths.credentials_file] {
        assert!(fs::symlink_metadata(link).unwrap().file_type().is_symlink(), "{}", link.display());
    }
    assert!(!dots.join("claude.json").exists() && !dots.join("credentials.json").exists());
}

#[test]
fn splicing_a_torn_config_is_refused_and_writes_nothing() {
    let f = fx();
    fs::write(&f.paths.global_config, "{\"a\": ").unwrap();
    match config::splice_key(&f.paths.global_config, "oauthAccount", Some(&json!({})), &open) {
        Err(e) => assert!(e.to_string().contains("backups"), "{e}"),
        Ok(_) => panic!("a torn config must not be spliced"),
    }
    assert_eq!(fs::read(&f.paths.global_config).unwrap(), b"{\"a\": ");
}

#[test]
fn splicing_a_missing_config_creates_it_with_only_that_key() {
    let f = fx();
    config::splice_key(&f.paths.global_config, "oauthAccount", Some(&json!({"emailAddress": "a@b.co"})), &open).unwrap();
    let doc: Value = serde_json::from_slice(&fs::read(&f.paths.global_config).unwrap()).unwrap();
    assert_eq!(doc, json!({"oauthAccount": {"emailAddress": "a@b.co"}}));
}
```

`lib.rs` additions:
```rust
pub mod config;
pub mod live;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tagteam-cc --test live_store`
Expected: FAIL to compile.

- [ ] **Step 3: Implement `config.rs`**

```rust
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;
use tagteam_provider::atomic::{remove_target, write_atomic_with};
use tagteam_provider::splice::{self, render_nested};
use tagteam_provider::{Identity, LiveLocks, ProviderError, Read, ReadError, Undo};

use crate::live::Fence;

use crate::paths::CcPaths;
use crate::shape::identity_from_oauth_account;

pub fn read_bytes(path: &Path) -> Read<Vec<u8>> {
    match fs::read(path) {
        Ok(b) => Read::Present(b),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Read::Absent,
        Err(e) => Read::Unreadable(ReadError::new(path.display().to_string(), e.to_string())),
    }
}

/// `Absent` means no live login: no file, no `oauthAccount`, or no email (Appendix A.6).
pub fn live_identity(paths: &CcPaths) -> Read<Identity> {
    match get_key(&paths.global_config, "oauthAccount") {
        Read::Present(Some(v)) => identity_from_oauth_account(&v).map_or(Read::Absent, Read::Present),
        Read::Present(None) | Read::Absent => Read::Absent,
        Read::Unreadable(e) => Read::Unreadable(e),
    }
}

pub fn get_key(path: &Path, key: &str) -> Read<Option<Value>> {
    match read_bytes(path) {
        Read::Present(b) => match splice::get_top_level(&b, key) {
            Ok(v) => Read::Present(v),
            Err(e) => Read::Unreadable(ReadError::new(path.display().to_string(), e.to_string())),
        },
        Read::Absent => Read::Absent,
        Read::Unreadable(e) => Read::Unreadable(e),
    }
}

/// Restores the exact bytes a splice replaced, or removes a file the splice created. The
/// bytes may hold `primaryApiKey`, so `Debug` shows the path only.
pub struct ConfigUndo {
    path: PathBuf,
    before: Option<Vec<u8>>,
}

impl fmt::Debug for ConfigUndo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConfigUndo").field("path", &self.path).finish_non_exhaustive()
    }
}

impl Undo for ConfigUndo {
    fn undo(self: Box<Self>, locks: &LiveLocks<'_>) -> Result<(), ProviderError> {
        let fence = || locks.check_owned().map_err(ProviderError::from);
        match &self.before {
            Some(b) => write_atomic_with(&self.path, b, 0o600, fence)?,
            None => {
                // The splice created the file, possibly through a dangling symlink: remove
                // what it created and keep the link.
                fence()?;
                remove_target(&self.path)?;
            }
        }
        Ok(())
    }

    fn what(&self) -> String {
        format!("restore {}", self.path.display())
    }
}

/// §9.5: replaces (`Some`) or removes (`None`) one top-level key, changing no other byte.
/// A torn or non-object file is never replaced. `fence` runs immediately before the new file
/// is published (§9.1).
pub fn splice_key(path: &Path, key: &str, value: Option<&Value>, fence: Fence<'_>) -> Result<ConfigUndo, ProviderError> {
    let before = read_bytes(path);
    let unsplicable = |_| ProviderError::ConfigUnsplicable(path.to_path_buf());
    let new = match (&before, value) {
        (Read::Unreadable(_), _) => return Err(ProviderError::ConfigUnsplicable(path.to_path_buf())),
        (Read::Absent, None) => return Ok(ConfigUndo { path: path.to_path_buf(), before: None }),
        (Read::Absent, Some(v)) => {
            let key_json = serde_json::to_string(key).expect("a string always serializes");
            format!("{{\n  {key_json}: {}\n}}\n", render_nested(v, 1)).into_bytes()
        }
        (Read::Present(b), Some(v)) => splice::replace_top_level(b, key, v).map_err(unsplicable)?,
        (Read::Present(b), None) => splice::remove_top_level(b, key).map_err(unsplicable)?,
    };
    let before = before.present();
    if before.as_deref() != Some(new.as_slice()) {
        write_atomic_with(path, &new, 0o600, fence)?;
    }
    Ok(ConfigUndo { path: path.to_path_buf(), before })
}
```

The `(Read::Absent, None)` undo removes nothing that exists, because `undo` ignores
`NotFound`.

- [ ] **Step 4: Implement `live.rs`**

```rust
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use serde_json::{Map, Value, json};
use tagteam_provider::atomic::{ensure_private_dir, remove_target, write_atomic_with};
use tagteam_provider::{Credential, Env, Keychain, ProviderError, Read};

use crate::config::{self, read_bytes};
use crate::naming::{ItemKind, keychain_account, keychain_service, read_services};
use crate::paths::CcPaths;
use crate::shape::machine_shared_only;

/// Checked immediately before every protected mutation (§9.1). Production passes the live
/// locks' ownership check, so a holder that lost its lock stops before writing anything.
pub type Fence<'a> = &'a dyn Fn() -> Result<(), ProviderError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Linux,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") { Platform::MacOs } else { Platform::Linux }
    }
}

/// The exact prior state of every entry a switch may write, including every Keychain item a
/// reader tries (Appendix A.2). It holds secrets, so it has no `Debug`.
#[derive(Clone)]
pub struct Snapshot {
    oauth_items: Vec<(String, Option<Vec<u8>>)>,
    managed_items: Vec<(String, Option<Vec<u8>>)>,
    credentials_file: Option<Vec<u8>>,
    global_config: Option<Vec<u8>>,
}

/// Appendix A.3: reads and writes of the active credential and the managed-key axis.
pub struct LiveStore {
    keychain: Arc<dyn Keychain>,
    platform: Platform,
    file_mode_pinned: AtomicBool,
    retry_delay: Duration,
}

fn present_or_err(r: Read<Vec<u8>>) -> Result<Option<Vec<u8>>, ProviderError> {
    match r {
        Read::Present(v) => Ok(Some(v)),
        Read::Absent => Ok(None),
        Read::Unreadable(e) => Err(ProviderError::Unreadable(e)),
    }
}

/// Removes what the path resolves to; a symlink itself is never deleted (§9.5).
fn remove_if_present(path: &std::path::Path) -> Result<(), ProviderError> {
    Ok(remove_target(path)?)
}

/// Keeps only the machine-shared keys of a credential entry, or `None` when nothing remains.
fn keep_shared(b: &[u8]) -> Option<Vec<u8>> {
    let Ok(Value::Object(o)) = serde_json::from_slice::<Value>(b) else { return None };
    let shared = machine_shared_only(&o);
    (!shared.is_empty()).then(|| serde_json::to_vec(&Value::Object(shared)).expect("a Value always serializes"))
}

impl LiveStore {
    pub fn new(keychain: Arc<dyn Keychain>, platform: Platform) -> Self {
        Self { keychain, platform, file_mode_pinned: AtomicBool::new(false), retry_delay: Duration::from_millis(300) }
    }

    pub fn with_retry_delay(mut self, d: Duration) -> Self {
        self.retry_delay = d;
        self
    }

    pub fn platform(&self) -> Platform {
        self.platform
    }

    pub fn file_mode_pinned(&self) -> bool {
        self.file_mode_pinned.load(Ordering::SeqCst)
    }

    fn mac(&self) -> bool {
        self.platform == Platform::MacOs
    }

    /// The first item that answers, in reader order. An unreadable item stops the search: a
    /// later fallback might be superseded by what the unreadable one holds, so it is never
    /// returned as if it were authoritative.
    fn find_first(&self, services: &[String], acct: &str) -> Read<Vec<u8>> {
        for svc in services {
            match self.keychain.find(svc, acct) {
                Read::Absent => continue,
                other => return other,
            }
        }
        Read::Absent
    }

    /// Keychain first, retried twice 300 ms apart; the file covers an absent item. A failed
    /// Keychain read covered by the file is `Degraded` (§4.3).
    pub fn read_credential(&self, env: &Env, paths: &CcPaths) -> Read<Credential> {
        if !self.mac() {
            return read_bytes(&paths.credentials_file).map(Credential::fresh);
        }
        let services = read_services(env, ItemKind::OAuth);
        let acct = keychain_account(env);
        let mut kc = self.find_first(&services, &acct);
        for _ in 0..2 {
            if !matches!(kc, Read::Unreadable(_)) {
                break;
            }
            thread::sleep(self.retry_delay);
            kc = self.find_first(&services, &acct);
        }
        match kc {
            Read::Present(b) => Read::Present(Credential::fresh(b)),
            Read::Absent => read_bytes(&paths.credentials_file).map(Credential::fresh),
            Read::Unreadable(e) => match read_bytes(&paths.credentials_file) {
                Read::Present(b) => Read::Present(Credential::degraded(b)),
                _ => Read::Unreadable(e),
            },
        }
    }

    pub fn read_managed_key(&self, env: &Env, paths: &CcPaths) -> Read<Vec<u8>> {
        if self.mac() {
            match self.find_first(&read_services(env, ItemKind::ManagedKey), &keychain_account(env)) {
                Read::Present(v) => return Read::Present(v),
                Read::Unreadable(e) => return Read::Unreadable(e),
                Read::Absent => {}
            }
        }
        match config::get_key(&paths.global_config, "primaryApiKey") {
            Read::Present(Some(Value::String(k))) => Read::Present(k.into_bytes()),
            Read::Present(_) | Read::Absent => Read::Absent,
            Read::Unreadable(e) => Read::Unreadable(e),
        }
    }

    /// Deletes every item a reader would try for `kind`, and verifies each one gone.
    fn remove_items(&self, env: &Env, kind: ItemKind, fence: Fence<'_>) -> Result<(), ProviderError> {
        let acct = keychain_account(env);
        for svc in read_services(env, kind) {
            fence()?;
            let _ = self.keychain.delete(&svc, &acct);
            if !matches!(self.keychain.exists(&svc, &acct), Read::Absent) {
                return Err(ProviderError::ShadowingItem(svc));
            }
        }
        Ok(())
    }

    fn write_file(&self, paths: &CcPaths, bytes: &[u8], fence: Fence<'_>) -> Result<(), ProviderError> {
        fence()?;
        ensure_private_dir(&paths.secure_storage_dir)?;
        write_atomic_with(&paths.credentials_file, bytes, 0o600, fence)
    }

    /// Appendix A.3 write, including the verified file fallback.
    pub fn write_credential_entry(&self, env: &Env, paths: &CcPaths, bytes: &[u8], fence: Fence<'_>) -> Result<(), ProviderError> {
        if !self.mac() {
            return self.write_file(paths, bytes, fence);
        }
        if !self.file_mode_pinned() {
            fence()?;
            match self.keychain.upsert(&keychain_service(env, ItemKind::OAuth), &keychain_account(env), bytes) {
                Ok(()) => {
                    if paths.credentials_file.exists() {
                        // Bumps the mtime, so CC reloads (hot reload).
                        write_atomic_with(&paths.credentials_file, bytes, 0o600, fence)?;
                    }
                    return Ok(());
                }
                Err(e) => tracing::warn!("keychain write failed, falling back to the credentials file: {e}"),
            }
        }
        self.write_file(paths, bytes, fence)?;
        self.remove_items(env, ItemKind::OAuth, fence)?;
        self.file_mode_pinned.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// API-key activation: keep only the machine-shared keys of every credential entry a
    /// reader would try; delete an entry when none remain (§9.4 step 7).
    pub fn clear_credential_account_keys(&self, env: &Env, paths: &CcPaths, fence: Fence<'_>) -> Result<(), ProviderError> {
        if self.mac() {
            let acct = keychain_account(env);
            for svc in read_services(env, ItemKind::OAuth) {
                if let Some(b) = present_or_err(self.keychain.find(&svc, &acct))? {
                    fence()?;
                    match keep_shared(&b) {
                        Some(k) => self.keychain.upsert(&svc, &acct, &k)?,
                        None => self.keychain.delete(&svc, &acct)?,
                    }
                }
            }
        }
        if let Some(b) = present_or_err(read_bytes(&paths.credentials_file))? {
            match keep_shared(&b) {
                Some(k) => write_atomic_with(&paths.credentials_file, &k, 0o600, fence)?,
                None => {
                    fence()?;
                    remove_if_present(&paths.credentials_file)?
                }
            }
        }
        Ok(())
    }

    /// Appends the key's last 20 characters to `customApiKeyResponses.approved`, then stores
    /// the key in the managed-key item. When the Keychain refuses, the key goes to
    /// `primaryApiKey`, and every managed-key item is removed and verified gone: CC reads the
    /// Keychain first, so a stale item would stay the effective key.
    pub fn write_managed_key(&self, env: &Env, paths: &CcPaths, key: &[u8], fence: Fence<'_>) -> Result<(), ProviderError> {
        let key_str = String::from_utf8_lossy(key).trim().to_owned();
        let tail: String = key_str.chars().rev().take(20).collect::<Vec<_>>().into_iter().rev().collect();
        let mut responses = match config::get_key(&paths.global_config, "customApiKeyResponses") {
            Read::Present(Some(Value::Object(o))) => o,
            Read::Present(_) | Read::Absent => Map::new(),
            Read::Unreadable(_) => return Err(ProviderError::ConfigUnsplicable(paths.global_config.clone())),
        };
        let appended = match responses.entry("approved").or_insert_with(|| json!([])) {
            Value::Array(list) if !list.iter().any(|v| v.as_str() == Some(tail.as_str())) => {
                list.push(Value::String(tail));
                true
            }
            _ => false,
        };
        if appended {
            config::splice_key(&paths.global_config, "customApiKeyResponses", Some(&Value::Object(responses)), fence)?;
        }
        if self.mac() {
            fence()?;
            match self.keychain.upsert(&keychain_service(env, ItemKind::ManagedKey), &keychain_account(env), key_str.as_bytes()) {
                Ok(()) => return Ok(()),
                Err(e) => tracing::warn!("keychain write failed, storing primaryApiKey instead: {e}"),
            }
        }
        config::splice_key(&paths.global_config, "primaryApiKey", Some(&Value::String(key_str)), fence)?;
        if self.mac() {
            self.remove_items(env, ItemKind::ManagedKey, fence)?;
        }
        Ok(())
    }

    /// Writing OAuth clears the managed key: every managed-key item is deleted (verified) and
    /// `primaryApiKey` is dropped. `approved` is kept (B.10).
    pub fn clear_managed_key(&self, env: &Env, paths: &CcPaths, fence: Fence<'_>) -> Result<(), ProviderError> {
        if self.mac() {
            self.remove_items(env, ItemKind::ManagedKey, fence)?;
        }
        config::splice_key(&paths.global_config, "primaryApiKey", None, fence)?;
        Ok(())
    }

    /// Refuses when any entry a switch may overwrite cannot be read.
    pub fn snapshot(&self, env: &Env, paths: &CcPaths) -> Result<Snapshot, ProviderError> {
        let acct = keychain_account(env);
        let items = |kind| -> Result<Vec<(String, Option<Vec<u8>>)>, ProviderError> {
            if !self.mac() {
                return Ok(vec![]);
            }
            read_services(env, kind)
                .into_iter()
                .map(|svc| Ok((svc.clone(), present_or_err(self.keychain.find(&svc, &acct))?)))
                .collect()
        };
        Ok(Snapshot {
            oauth_items: items(ItemKind::OAuth)?,
            managed_items: items(ItemKind::ManagedKey)?,
            credentials_file: present_or_err(read_bytes(&paths.credentials_file))?,
            global_config: present_or_err(read_bytes(&paths.global_config))?,
        })
    }

    pub fn restore(&self, env: &Env, paths: &CcPaths, snap: &Snapshot, fence: Fence<'_>) -> Result<(), ProviderError> {
        let acct = keychain_account(env);
        for (svc, value) in snap.oauth_items.iter().chain(&snap.managed_items) {
            fence()?;
            match value {
                Some(v) => self.keychain.upsert(svc, &acct, v)?,
                None => self.keychain.delete(svc, &acct)?,
            }
        }
        for (path, value) in [(&paths.credentials_file, &snap.credentials_file), (&paths.global_config, &snap.global_config)] {
            match value {
                Some(v) => write_atomic_with(path, v, 0o600, fence)?,
                None => {
                    fence()?;
                    remove_if_present(path)?
                }
            }
        }
        Ok(())
    }
}
```

Add `tracing.workspace = true` to `crates/tagteam-cc/Cargo.toml` `[dependencies]`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p tagteam-cc`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-cc
git commit -m "Read and write Claude Code's live credential and config"
```

---
### Task 15: CC lock set and `impl Provider for ClaudeCode`

**Files:**
- Create: `crates/tagteam-cc/src/locks.rs`, `crates/tagteam-cc/src/provider.rs`
- Modify: `crates/tagteam-cc/src/lib.rs`
- Test: `crates/tagteam-cc/tests/provider.rs`

**Interfaces:**
- Consumes: everything from Tasks 11–14
- Produces:
  - `locks::CRED_STALE` (60 s), `locks::CONFIG_STALE` (10 s), `locks::ACQUIRE_TIMEOUT` (9 s);
    `locks::acquire(&CcPaths) -> Result<CcLockSet, LockError>`,
    `locks::acquire_with(&CcPaths, Duration)`; `CcLockSet: LiveLockSet`
  - `ClaudeCode::new(Arc<dyn Keychain>, Platform) -> ClaudeCode`,
    `ClaudeCode::with_store(LiveStore) -> ClaudeCode`; `impl Provider for ClaudeCode`

- [ ] **Step 1: Write the failing tests**

Append to `locks.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tagteam_provider::Env;

    fn paths(root: &std::path::Path) -> CcPaths {
        let env = Env::for_test(root);
        fs::create_dir_all(env.home.join(".claude")).unwrap();
        CcPaths::resolve(&env)
    }

    #[test]
    fn takes_all_three_and_releases_them() {
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        let set = acquire(&p).unwrap();
        assert!(p.refresh_lock.is_dir() && p.legacy_lock().is_dir() && p.config_lock.is_dir());
        assert!(set.check_owned().is_ok());
        assert!(!p.config_home.join(".oauth_refresh.lock.owner").exists());
        drop(set);
        assert!(!p.refresh_lock.exists() && !p.legacy_lock().exists() && !p.config_lock.exists());
    }

    #[test]
    fn a_contended_legacy_lock_releases_the_refresh_lock_while_waiting() {
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        fs::create_dir(p.legacy_lock()).unwrap(); // CC holds it, freshly
        assert!(matches!(acquire_with(&p, Duration::from_millis(700)), Err(LockError::Timeout(_))));
        assert!(!p.refresh_lock.exists(), "the refresh lock must not be held while waiting");
        assert!(p.legacy_lock().is_dir(), "CC's lock is left alone");
    }

    #[test]
    fn a_held_refresh_lock_times_out_without_touching_it() {
        // Review Focus 1: CC is mid-refresh.
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        fs::create_dir(&p.refresh_lock).unwrap();
        let start = std::time::Instant::now();
        assert!(matches!(acquire_with(&p, Duration::from_millis(500)), Err(LockError::Timeout(_))));
        assert!(start.elapsed() < Duration::from_secs(2));
        assert!(p.refresh_lock.is_dir());
    }
}
```

`crates/tagteam-cc/tests/provider.rs`:
```rust
use std::fs;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tagteam_cc::live::{LiveStore, Platform};
use tagteam_cc::provider::ClaudeCode;
use tagteam_cc::{CcPaths, ItemKind, keychain_account, keychain_service};
use tagteam_provider::{Env, FakeKeychain, MutationGuard, Provider, Read, StoredLogin};

struct Fx {
    _d: tempfile::TempDir,
    env: Env,
    kc: Arc<FakeKeychain>,
    cc: ClaudeCode,
}

fn fx() -> Fx {
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    fs::create_dir_all(env.home.join(".claude")).unwrap();
    let kc = Arc::new(FakeKeychain::new());
    let cc = ClaudeCode::with_store(LiveStore::new(kc.clone(), Platform::MacOs).with_retry_delay(Duration::ZERO));
    Fx { _d: d, env, kc, cc }
}

fn oauth_item(f: &Fx) -> Option<Value> {
    let svc = keychain_service(&f.env, ItemKind::OAuth);
    f.kc.get(&svc, &keychain_account(&f.env)).map(|b| serde_json::from_slice(&b).unwrap())
}

fn target(f: &Fx, email: &str, rt: &str) -> StoredLogin {
    StoredLogin {
        kind: "oauth".into(),
        secret: json!({"claudeAiOauth": {"accessToken": "at", "refreshToken": rt}}).to_string().into_bytes(),
        identity: f.cc.parse_identity(&json!({"emailAddress": email, "organizationUuid": ""})).unwrap(),
    }
}

#[test]
fn writes_the_composed_credential_and_the_identity_then_undoes_both() {
    let f = fx();
    let paths = CcPaths::resolve(&f.env);
    let before_cfg = "{\n  \"oauthAccount\": {\n    \"emailAddress\": \"old@a.co\"\n  },\n  \"userID\": \"u\"\n}\n";
    fs::write(&paths.global_config, before_cfg).unwrap();
    let svc = keychain_service(&f.env, ItemKind::OAuth);
    let acct = keychain_account(&f.env);
    let live_json = json!({"claudeAiOauth": {"refreshToken": "old"}, "mcpOAuth": {"m": 1}});
    f.kc.put(&svc, &acct, live_json.to_string().as_bytes());

    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let locks = f.cc.lock_live(&f.env, &g).unwrap();
    let live = f.cc.read_live_auth(&f.env);
    let t = target(&f, "new@b.co", "rt-new");
    let u1 = f.cc.write_credential(&f.env, &locks, &t, &live).unwrap();
    let u2 = f.cc.write_identity(&f.env, &locks, Some(&t.identity)).unwrap();

    assert_eq!(oauth_item(&f).unwrap(), json!({"claudeAiOauth": {"accessToken": "at", "refreshToken": "rt-new"}, "mcpOAuth": {"m": 1}}));
    assert_eq!(f.cc.live_identity(&f.env).present().unwrap().label, "new@b.co");
    assert!(fs::read_to_string(&paths.global_config).unwrap().contains("\"userID\": \"u\""));

    u2.undo(&locks).unwrap();
    u1.undo(&locks).unwrap();
    assert_eq!(oauth_item(&f).unwrap(), live_json);
    assert_eq!(fs::read_to_string(&paths.global_config).unwrap(), before_cfg);
}

#[test]
fn an_api_key_target_moves_the_auth_axis() {
    let f = fx();
    let paths = CcPaths::resolve(&f.env);
    fs::write(&paths.global_config, "{}").unwrap();
    let svc = keychain_service(&f.env, ItemKind::OAuth);
    let acct = keychain_account(&f.env);
    f.kc.put(&svc, &acct, br#"{"claudeAiOauth":{"refreshToken":"r"},"pluginSecrets":{"p":1}}"#);
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let locks = f.cc.lock_live(&f.env, &g).unwrap();
    let live = f.cc.read_live_auth(&f.env);
    let key = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz";
    let t = StoredLogin {
        kind: "api_key".into(),
        secret: key.as_bytes().to_vec(),
        identity: f.cc.token_identity("api-key-2@token.local"),
    };
    f.cc.write_credential(&f.env, &locks, &t, &live).unwrap();
    assert_eq!(f.kc.get(&keychain_service(&f.env, ItemKind::ManagedKey), &acct).unwrap(), key.as_bytes());
    assert_eq!(oauth_item(&f).unwrap(), json!({"pluginSecrets": {"p": 1}}));
    // Back to OAuth: the managed key goes, machine-shared keys stay.
    let live = f.cc.read_live_auth(&f.env);
    f.cc.write_credential(&f.env, &locks, &target(&f, "a@b.co", "rt"), &live).unwrap();
    assert!(f.kc.get(&keychain_service(&f.env, ItemKind::ManagedKey), &acct).is_none());
    assert_eq!(oauth_item(&f).unwrap()["pluginSecrets"], json!({"p": 1}));
}

#[test]
fn an_unreadable_live_entry_is_never_overwritten() {
    let f = fx();
    let svc = keychain_service(&f.env, ItemKind::OAuth);
    let acct = keychain_account(&f.env);
    f.kc.put(&svc, &acct, b"{}");
    f.kc.set_unreadable(&svc, &acct, true);
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let locks = f.cc.lock_live(&f.env, &g).unwrap();
    let live = f.cc.read_live_auth(&f.env);
    assert!(f.cc.write_credential(&f.env, &locks, &target(&f, "a@b.co", "rt"), &live).is_err());
    f.kc.set_unreadable(&svc, &acct, false);
    assert_eq!(f.kc.get(&svc, &acct).unwrap(), b"{}");
}

#[test]
fn a_failed_write_restores_what_it_had_already_changed() {
    let f = fx();
    let paths = CcPaths::resolve(&f.env);
    fs::write(&paths.global_config, r#"{"primaryApiKey": "sk-ant-api03-old"}"#).unwrap();
    let acct = keychain_account(&f.env);
    let managed = keychain_service(&f.env, ItemKind::ManagedKey);
    f.kc.put(&managed, &acct, b"sk-ant-api03-old");
    f.kc.set_fail_delete(&managed, true); // clearing the managed key will fail after the OAuth write
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let locks = f.cc.lock_live(&f.env, &g).unwrap();
    let live = f.cc.read_live_auth(&f.env);
    assert!(f.cc.write_credential(&f.env, &locks, &target(&f, "a@b.co", "rt"), &live).is_err());
    assert!(oauth_item(&f).is_none(), "the OAuth write was rolled back");
    assert!(matches!(f.cc.read_live_auth(&f.env).managed_key, Read::Present(_)));
}

#[test]
fn a_restore_that_fails_is_reported_not_hidden() {
    let f = fx();
    let paths = CcPaths::resolve(&f.env);
    fs::write(&paths.global_config, "{}").unwrap();
    let acct = keychain_account(&f.env);
    let (oauth, managed) = (keychain_service(&f.env, ItemKind::OAuth), keychain_service(&f.env, ItemKind::ManagedKey));
    f.kc.put(&managed, &acct, b"sk-ant-api03-old");
    f.kc.set_fail_delete(&managed, true); // clearing the managed key fails after the OAuth write
    f.kc.set_fail_delete(&oauth, true); // and so does restoring the OAuth item's absence
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let locks = f.cc.lock_live(&f.env, &g).unwrap();
    let live = f.cc.read_live_auth(&f.env);
    let err = f.cc.write_credential(&f.env, &locks, &target(&f, "a@b.co", "rt"), &live).err().unwrap();
    assert!(matches!(err, tagteam_provider::ProviderError::RestoreFailed { .. }), "{err}");
}

#[test]
fn a_panic_inside_one_operation_restores_its_first_write() {
    let f = fx();
    let acct = keychain_account(&f.env);
    let (oauth, managed) = (keychain_service(&f.env, ItemKind::OAuth), keychain_service(&f.env, ItemKind::ManagedKey));
    f.kc.put(&managed, &acct, b"sk-ant-api03-old");
    f.kc.set_panic_on_delete(&managed, true); // panics while clearing the managed key, after the OAuth write
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let locks = f.cc.lock_live(&f.env, &g).unwrap();
    let live = f.cc.read_live_auth(&f.env);
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        f.cc.write_credential(&f.env, &locks, &target(&f, "a@b.co", "rt"), &live)
    }));
    assert!(r.is_err());
    f.kc.set_panic_on_delete(&managed, false);
    assert!(f.kc.get(&oauth, &acct).is_none(), "the OAuth write was restored during unwinding");
    assert_eq!(f.kc.get(&managed, &acct).unwrap(), b"sk-ant-api03-old");
}

#[test]
fn identity_surface_names_the_section_3_writes() {
    let f = fx();
    let s = f.cc.identity_surface(&f.env);
    let paths = CcPaths::resolve(&f.env);
    assert_eq!(s.json_keys, vec![(paths.global_config, vec!["oauthAccount".into(), "primaryApiKey".into(), "customApiKeyResponses".into()])]);
    assert_eq!(s.credential_files, vec![paths.credentials_file]);
    assert_eq!(s.machine_shared_keys.len(), 5);
    assert_eq!(f.cc.identity_key(&f.cc.token_identity("a@b.co")).as_str(), "a@b.co\n");
}
```

`lib.rs` additions:
```rust
pub mod locks;
pub mod provider;
pub use provider::ClaudeCode;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tagteam-cc`
Expected: FAIL to compile.

- [ ] **Step 3: Implement `locks.rs`**

```rust
use std::thread;
use std::time::{Duration, Instant};

use tagteam_provider::{LiveLockSet, LockError, MkdirLock, MkdirLockSpec};

use crate::paths::CcPaths;

pub const CRED_STALE: Duration = Duration::from_secs(60);
pub const CONFIG_STALE: Duration = Duration::from_secs(10);
pub const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(9);

/// CC's credential locks and config lock (§9.1). Fields drop in declaration order, so the
/// config lock is released first and the refresh lock last.
pub struct CcLockSet {
    config: MkdirLock,
    legacy: MkdirLock,
    refresh: MkdirLock,
}

impl LiveLockSet for CcLockSet {
    fn check_owned(&self) -> Result<(), LockError> {
        self.refresh.check_owned()?;
        self.legacy.check_owned()?;
        self.config.check_owned()
    }
}

pub fn acquire(paths: &CcPaths) -> Result<CcLockSet, LockError> {
    acquire_with(paths, ACQUIRE_TIMEOUT)
}

/// Refresh lock, then the legacy lock; if the legacy lock is contended the refresh lock is
/// released and the pair retried, as CC does. Then the config lock. tagteam never writes
/// `.oauth_refresh.lock.owner`.
pub fn acquire_with(paths: &CcPaths, timeout: Duration) -> Result<CcLockSet, LockError> {
    let deadline = Instant::now() + timeout;
    let remaining = || deadline.saturating_duration_since(Instant::now());
    loop {
        let refresh = MkdirLock::acquire(&MkdirLockSpec::new(paths.refresh_lock.clone(), CRED_STALE, remaining()))?;
        let legacy_spec = MkdirLockSpec::new(paths.legacy_lock(), CRED_STALE, Duration::ZERO);
        match MkdirLock::try_acquire(&legacy_spec)? {
            Some(legacy) => {
                let config = MkdirLock::acquire(&MkdirLockSpec::new(paths.config_lock.clone(), CONFIG_STALE, remaining()))?;
                return Ok(CcLockSet { config, legacy, refresh });
            }
            None => {
                drop(refresh);
                if Instant::now() >= deadline {
                    return Err(LockError::Timeout(legacy_spec.path));
                }
                thread::sleep(Duration::from_millis(fastrand::u64(250..=500)).min(remaining()));
            }
        }
    }
}
```

- [ ] **Step 4: Implement `provider.rs`**

```rust
use std::sync::Arc;

use serde_json::Value;
use tagteam_core::{CLAUDE_CODE, Fingerprint, IdentityKey, ProviderId};
use tagteam_provider::{
    Env, Identity, IdentitySurface, Keychain, LiveAuth, LiveLocks, MutationGuard, Provider, ProviderError, Read,
    StoredLogin, Undo,
};

use crate::config;
use crate::live::{Fence, LiveStore, Platform, Snapshot};
use crate::locks;
use crate::naming::{ItemKind, keychain_account, read_services};
use crate::paths::CcPaths;
use crate::shape::{self, KIND_API_KEY, KINDS, MACHINE_SHARED_KEYS};

pub struct ClaudeCode {
    live: Arc<LiveStore>,
}

impl ClaudeCode {
    pub fn new(keychain: Arc<dyn Keychain>, platform: Platform) -> Self {
        Self::with_store(LiveStore::new(keychain, platform))
    }

    pub fn with_store(store: LiveStore) -> Self {
        Self { live: Arc::new(store) }
    }
}

struct SnapshotUndo {
    live: Arc<LiveStore>,
    env: Env,
    paths: CcPaths,
    snapshot: Snapshot,
}

impl Undo for SnapshotUndo {
    fn undo(self: Box<Self>, locks: &LiveLocks<'_>) -> Result<(), ProviderError> {
        let fence = || locks.check_owned().map_err(ProviderError::from);
        self.live.restore(&self.env, &self.paths, &self.snapshot, &fence)
    }

    fn what(&self) -> String {
        "restore the live credential and managed key".into()
    }
}

/// Restores the snapshot if the operation unwinds before handing its undo to the engine:
/// a panic between two writes of one operation must not leave the first one in place.
struct Armed<'a, 'l> {
    undo: Option<Box<SnapshotUndo>>,
    locks: &'a LiveLocks<'l>,
}

impl Drop for Armed<'_, '_> {
    fn drop(&mut self) {
        if let Some(undo) = self.undo.take() {
            if let Err(e) = undo.undo(self.locks) {
                tracing::error!("restoring the live credential during unwinding failed: {e}");
            }
        }
    }
}

impl ClaudeCode {
    /// Snapshots first, runs `f` behind the ownership fence, and restores the snapshot itself
    /// when `f` fails or panics part-way. A restore that fails too is reported as
    /// `RestoreFailed`, so the engine keeps its journal instead of believing the rollback
    /// worked.
    fn guarded(
        &self,
        env: &Env,
        locks: &LiveLocks<'_>,
        f: impl FnOnce(&CcPaths, Fence<'_>) -> Result<(), ProviderError>,
    ) -> Result<Box<dyn Undo>, ProviderError> {
        locks.check_owned()?;
        let paths = CcPaths::resolve(env);
        let snapshot = self.live.snapshot(env, &paths)?;
        let undo = Box::new(SnapshotUndo { live: self.live.clone(), env: env.clone(), paths: paths.clone(), snapshot });
        let mut armed = Armed { undo: Some(undo), locks };
        let fence = || locks.check_owned().map_err(ProviderError::from);
        let result = f(&paths, &fence);
        let undo = armed.undo.take().expect("armed until here");
        match result {
            Ok(()) => Ok(undo),
            Err(e) => match undo.undo(locks) {
                Ok(()) => Err(e),
                Err(re) => Err(ProviderError::RestoreFailed { cause: Box::new(e), restore: Box::new(re) }),
            },
        }
    }
}

impl Provider for ClaudeCode {
    fn id(&self) -> ProviderId {
        ProviderId::new(CLAUDE_CODE)
    }

    fn display_name(&self) -> &'static str {
        "Claude Code"
    }

    fn identity_surface(&self, env: &Env) -> IdentitySurface {
        let paths = CcPaths::resolve(env);
        let acct = keychain_account(env);
        let mac = self.live.platform() == Platform::MacOs;
        IdentitySurface {
            json_keys: vec![(
                paths.global_config,
                vec!["oauthAccount".into(), "primaryApiKey".into(), "customApiKeyResponses".into()],
            )],
            credential_files: vec![paths.credentials_file],
            credential_items: if mac {
                read_services(env, ItemKind::OAuth).into_iter().map(|s| (s, acct.clone())).collect()
            } else {
                vec![]
            },
            owned_items: if mac {
                read_services(env, ItemKind::ManagedKey).into_iter().map(|s| (s, acct.clone())).collect()
            } else {
                vec![]
            },
            machine_shared_keys: MACHINE_SHARED_KEYS.to_vec(),
        }
    }

    fn identity_key(&self, id: &Identity) -> IdentityKey {
        IdentityKey::new(format!("{}\n{}", id.email.as_deref().unwrap_or(&id.label), id.org_uuid))
    }

    fn credential_kinds(&self) -> &'static [&'static str] {
        &KINDS
    }

    fn parse_identity(&self, raw: &Value) -> Result<Identity, ProviderError> {
        shape::identity_from_oauth_account(raw)
            .ok_or_else(|| ProviderError::Invalid("the stored oauthAccount has no emailAddress".into()))
    }

    fn token_identity(&self, email: &str) -> Identity {
        shape::token_identity(email)
    }

    fn token_secret(&self, token: &str) -> (String, Vec<u8>) {
        let t = token.trim();
        if shape::is_api_key(t.as_bytes()) {
            (KIND_API_KEY.into(), t.as_bytes().to_vec())
        } else {
            (shape::KIND_SETUP_TOKEN.into(), shape::setup_token_credential(t))
        }
    }

    fn classify(&self, secret: &[u8]) -> String {
        shape::classify(secret).into()
    }

    fn fingerprint(&self, secret: &[u8]) -> Option<Fingerprint> {
        shape::fingerprint(secret)
    }

    fn has_refresh_token(&self, secret: &[u8]) -> bool {
        shape::has_refresh_token(secret)
    }

    fn is_wiped(&self, secret: &[u8]) -> bool {
        shape::is_wiped(secret)
    }

    fn login_expires_at(&self, secret: &[u8]) -> Option<i64> {
        shape::login_expires_at(secret)
    }

    fn live_identity(&self, env: &Env) -> Read<Identity> {
        config::live_identity(&CcPaths::resolve(env))
    }

    fn read_live_auth(&self, env: &Env) -> LiveAuth {
        let paths = CcPaths::resolve(env);
        LiveAuth {
            credential: self.live.read_credential(env, &paths),
            managed_key: self.live.read_managed_key(env, &paths),
        }
    }

    fn lock_live<'g>(&self, env: &Env, g: &'g MutationGuard) -> Result<LiveLocks<'g>, ProviderError> {
        let set = locks::acquire(&CcPaths::resolve(env))?;
        Ok(LiveLocks::new(g, Box::new(set)))
    }

    fn write_credential(
        &self,
        env: &Env,
        locks: &LiveLocks<'_>,
        target: &StoredLogin,
        live: &LiveAuth,
    ) -> Result<Box<dyn Undo>, ProviderError> {
        self.guarded(env, locks, |paths, fence| {
            if target.kind == KIND_API_KEY {
                self.live.write_managed_key(env, paths, &target.secret, fence)?;
                self.live.clear_credential_account_keys(env, paths, fence)
            } else {
                let live_map = match &live.credential {
                    Read::Present(c) => serde_json::from_slice::<Value>(c.bytes()).ok().and_then(|v| v.as_object().cloned()),
                    _ => None,
                };
                let composed = shape::compose(&target.secret, live_map.as_ref())?;
                self.live.write_credential_entry(env, paths, &composed, fence)?;
                self.live.clear_managed_key(env, paths, fence)
            }
        })
    }

    fn clear_other_axis(&self, env: &Env, locks: &LiveLocks<'_>, kept_kind: &str) -> Result<Box<dyn Undo>, ProviderError> {
        self.guarded(env, locks, |paths, fence| {
            if kept_kind == KIND_API_KEY {
                self.live.clear_credential_account_keys(env, paths, fence)
            } else {
                self.live.clear_managed_key(env, paths, fence)
            }
        })
    }

    fn write_identity(&self, env: &Env, locks: &LiveLocks<'_>, identity: Option<&Identity>) -> Result<Box<dyn Undo>, ProviderError> {
        let fence = || locks.check_owned().map_err(ProviderError::from);
        fence()?;
        let paths = CcPaths::resolve(env);
        Ok(Box::new(config::splice_key(&paths.global_config, "oauthAccount", identity.map(|i| &i.raw), &fence)?))
    }

    fn uses_file_store(&self, _env: &Env) -> bool {
        self.live.platform() == Platform::Linux || self.live.file_mode_pinned()
    }
}
```

`ConfigUndo` must be `Send` (it is: `PathBuf` and `Vec<u8>`), and `Snapshot` must be `Clone +
Send` (derive `Clone` in Task 14 already; fields are `Option<Vec<u8>>`).

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p tagteam-cc`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-cc
git commit -m "Implement the Claude Code provider for M1"
```

---
### Task 16: The SQLite store

**Files:**
- Create: `crates/tagteam-engine/src/store/mod.rs`, `crates/tagteam-engine/src/store/schema.sql`
- Modify: `crates/tagteam-engine/src/lib.rs`
- Test: `crates/tagteam-engine/tests/store.rs`

**Interfaces:**
- Produces:
```rust
pub struct Store;                         // Mutex<rusqlite::Connection> inside
impl Store {
    pub fn open(path: &Path) -> Result<Store, StoreError>;               // creates the parent 0700 and migrates
    pub fn open_existing(path: &Path) -> Result<Option<Store>, StoreError>;  // never creates anything
    pub fn schema_version(&self) -> Result<i64, StoreError>;
    pub fn accounts(&self, provider: &ProviderId) -> Result<Vec<AccountRow>, StoreError>;  // by position
    pub fn all_accounts(&self) -> Result<Vec<AccountRow>, StoreError>;                     // by provider, position
    pub fn account(&self, id: &AccountId) -> Result<Option<AccountRow>, StoreError>;
    pub fn find_by_identity_key(&self, provider: &ProviderId, key: &str) -> Result<Option<AccountRow>, StoreError>;
    pub fn find_by_position(&self, provider: &ProviderId, position: u32) -> Result<Option<AccountRow>, StoreError>;
    pub fn find_by_alias(&self, alias: &str) -> Result<Option<AccountRow>, StoreError>;
    pub fn find_by_email(&self, email: &str, provider: Option<&ProviderId>) -> Result<Vec<AccountRow>, StoreError>;
    pub fn next_position(&self, provider: &ProviderId) -> Result<u32, StoreError>;
    pub fn insert_account(&self, a: &NewAccount<'_>) -> Result<(), StoreError>;
    pub fn update_login(&self, id: &AccountId, identity_key: &str, identity: &Identity, kind: &str,
                        login_expires_at: Option<i64>) -> Result<(), StoreError>;   // also clears quarantine
    pub fn begin_replacement(&self, id: &AccountId, fp: &str, meta: &LoginMeta<'_>) -> Result<(), StoreError>; // epoch + 1
    pub fn finish_replacement(&self, id: &AccountId) -> Result<(), StoreError>;   // installs the recorded metadata
    pub fn rollback_replacement(&self, id: &AccountId) -> Result<(), StoreError>;          // epoch - 1
    pub fn backfill_account_uuid(&self, id: &AccountId, uuid: &str) -> Result<(), StoreError>;  // only while NULL
    pub fn set_alias(&self, id: &AccountId, alias: Option<&str>) -> Result<(), StoreError>;
    pub fn set_disabled(&self, id: &AccountId, disabled: bool) -> Result<(), StoreError>;
    pub fn move_to(&self, id: &AccountId, position: u32) -> Result<(), StoreError>;        // swaps if taken
    pub fn delete_account(&self, id: &AccountId) -> Result<(), StoreError>;
    pub fn active(&self, provider: &ProviderId) -> Result<Option<AccountId>, StoreError>;
    pub fn set_active(&self, provider: &ProviderId, id: Option<&AccountId>) -> Result<(), StoreError>;
    pub fn commit_switch(&self, provider: &ProviderId, to: &AccountId, event: &EventRow) -> Result<(), StoreError>;
    pub fn insert_event(&self, e: &EventRow) -> Result<(), StoreError>;
    pub fn events(&self) -> Result<Vec<EventRow>, StoreError>;
    pub fn journal(&self, provider: &ProviderId) -> Result<Option<JournalRow>, StoreError>;
    pub fn journals(&self) -> Result<Vec<JournalRow>, StoreError>;
    pub fn insert_journal(&self, j: &JournalRow) -> Result<(), StoreError>;
    pub fn delete_journal(&self, provider: &ProviderId) -> Result<(), StoreError>;
    pub fn insert_displaced(&self, d: &DisplacedRow) -> Result<(), StoreError>;
}
pub struct AccountRow { pub id: AccountId, pub provider: ProviderId, pub position: u32, pub identity_key: String,
    pub label: String, pub email: Option<String>, pub org_uuid: String, pub org_name: Option<String>,
    pub account_uuid: Option<String>, pub kind: String, pub alias: Option<String>, pub disabled: bool,
    pub identity_json: serde_json::Value, pub login_expires_at: Option<i64>, pub login_epoch: i64,
    pub replacing_fp: Option<String>, pub quarantine_reason: Option<String>, pub quarantine_fp: Option<String>,
    pub added_at: i64 }
pub struct NewAccount<'a> { pub id: &'a AccountId, pub provider: &'a ProviderId, pub position: u32,
    pub identity_key: &'a str, pub identity: &'a Identity, pub kind: &'a str, pub alias: Option<&'a str>,
    pub login_expires_at: Option<i64>, pub added_at: i64 }
pub struct EventRow { pub at: i64, pub provider: ProviderId, pub kind: String, pub from_id: Option<AccountId>,
    pub to_id: Option<AccountId>, pub trigger: Option<String>, pub source: String, pub detail: Option<serde_json::Value> }
pub struct JournalRow { pub provider: ProviderId, pub holder: ProcessStamp, pub from_id: Option<AccountId>,
    pub to_id: AccountId, pub from_fp: Option<String>, pub from_identity: Option<serde_json::Value>,
    pub to_fp: String, pub started_at: i64, pub prior: Option<Box<JournalRow>> }  // `prior`: the row a forced switch superseded
pub struct LoginMeta<'a> { pub identity_key: &'a str, pub identity: &'a Identity, pub kind: &'a str,
    pub login_expires_at: Option<i64> }
pub struct DisplacedRow { pub id: String, pub provider: ProviderId, pub at: i64, pub reason: String,
    pub fingerprint: String, pub identity: Option<serde_json::Value> }
pub enum StoreError { Sqlite(rusqlite::Error), Io(io::Error), Corrupt(String), AliasTaken(String),
    PositionTaken(u32), IdentityTaken, NoSuchAccount }
```
All timestamps are epoch milliseconds.

- [ ] **Step 1: Write the schema**

`crates/tagteam-engine/src/store/schema.sql` — spec §6.1 in full, so later milestones need no
migration for these tables:
```sql
CREATE TABLE accounts (
  id                TEXT PRIMARY KEY,
  provider          TEXT NOT NULL,
  position          INTEGER NOT NULL,
  identity_key      TEXT NOT NULL,
  label             TEXT NOT NULL,
  email             TEXT,
  org_uuid          TEXT NOT NULL DEFAULT '',
  org_name          TEXT,
  account_uuid      TEXT,
  kind              TEXT NOT NULL,
  alias             TEXT UNIQUE COLLATE NOCASE,
  disabled          INTEGER NOT NULL DEFAULT 0,
  identity_json     TEXT NOT NULL,
  login_expires_at  INTEGER,
  login_epoch       INTEGER NOT NULL DEFAULT 0,
  replacing_fp      TEXT,
  replacing_meta    TEXT,
  quarantine_reason TEXT,
  quarantine_fp     TEXT,
  quarantine_at     INTEGER,
  added_at          INTEGER NOT NULL,
  UNIQUE (provider, position),
  UNIQUE (provider, identity_key)
);

CREATE TABLE active_accounts (
  provider   TEXT PRIMARY KEY,
  account_id TEXT REFERENCES accounts(id) ON DELETE SET NULL
);

CREATE TABLE usage_state (
  account_id           TEXT PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
  last_good            TEXT,
  fetched_at           INTEGER,
  last_attempt_at      INTEGER,
  consecutive_failures INTEGER NOT NULL DEFAULT 0,
  last_error           TEXT,
  backoff_until        INTEGER,
  next_poll_at         INTEGER,
  poll_interval_s      INTEGER,
  last_429_at          INTEGER,
  rejected_fp          TEXT
);

CREATE TABLE usage_samples (
  account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  window     TEXT NOT NULL,
  fetched_at INTEGER NOT NULL,
  pct        REAL NOT NULL,
  resets_at  INTEGER,
  PRIMARY KEY (account_id, window, fetched_at)
) WITHOUT ROWID;

CREATE TABLE usage_requests (
  provider     TEXT NOT NULL,
  identity_key TEXT NOT NULL,
  at           INTEGER NOT NULL
);
CREATE INDEX usage_requests_by_identity ON usage_requests (provider, identity_key, at);

CREATE TABLE leases (
  name       TEXT PRIMARY KEY,
  holder     TEXT NOT NULL,
  expires_at INTEGER NOT NULL
);

CREATE TABLE switch_journal (
  provider      TEXT PRIMARY KEY,
  holder_pid    INTEGER NOT NULL,
  holder_start  INTEGER NOT NULL,
  from_id       TEXT,
  to_id         TEXT NOT NULL REFERENCES accounts(id),
  from_fp       TEXT,
  from_identity TEXT,
  to_fp         TEXT NOT NULL,
  started_at    INTEGER NOT NULL,
  prior         TEXT
);

CREATE TABLE autoswitch_state (
  provider          TEXT PRIMARY KEY,
  last_switch_at    INTEGER,
  last_switch_from  TEXT,
  last_switch_to    TEXT,
  left_headroom     REAL,
  left_recovery_at  INTEGER,
  left_trigger      TEXT,
  unhealthy_ticks   INTEGER NOT NULL DEFAULT 0,
  idle_hold_since   INTEGER
);

CREATE TABLE events (
  at       INTEGER NOT NULL,
  provider TEXT NOT NULL,
  kind     TEXT NOT NULL,
  from_id  TEXT,
  to_id    TEXT,
  trigger  TEXT,
  source   TEXT NOT NULL,
  detail   TEXT
);

CREATE TABLE mappings (
  path       TEXT NOT NULL,
  provider   TEXT NOT NULL,
  account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  added_at   INTEGER NOT NULL,
  PRIMARY KEY (path, provider)
);

CREATE TABLE displaced (
  id          TEXT PRIMARY KEY,
  provider    TEXT NOT NULL,
  at          INTEGER NOT NULL,
  reason      TEXT NOT NULL,
  fingerprint TEXT NOT NULL,
  identity    TEXT
);

CREATE TABLE live_identity_cache (
  provider     TEXT PRIMARY KEY,
  path         TEXT,
  mtime_ns     INTEGER,
  size         INTEGER,
  identity_key TEXT,
  label        TEXT,
  account_uuid TEXT
);
```

- [ ] **Step 2: Write the failing tests**

`crates/tagteam-engine/tests/store.rs`:
```rust
use serde_json::json;
use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::store::{EventRow, JournalRow, LoginMeta, NewAccount, Store, StoreError};
use tagteam_provider::{Identity, ProcessStamp};

fn identity(email: &str) -> Identity {
    Identity {
        label: email.into(),
        email: Some(email.into()),
        org_uuid: String::new(),
        org_name: None,
        account_uuid: None,
        raw: json!({"emailAddress": email}),
    }
}

fn add(s: &Store, p: &ProviderId, id: &str, email: &str, pos: u32) -> AccountId {
    let aid = AccountId::from_string(id);
    let key = format!("{email}\n");
    s.insert_account(&NewAccount {
        id: &aid,
        provider: p,
        position: pos,
        identity_key: &key,
        identity: &identity(email),
        kind: "oauth",
        alias: None,
        login_expires_at: None,
        added_at: 1,
    })
    .unwrap();
    aid
}

fn cc() -> ProviderId {
    ProviderId::new("claude-code")
}

#[test]
fn opening_migrates_once_and_open_existing_never_creates() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("data/tagteam/tagteam.db");
    assert!(Store::open_existing(&path).unwrap().is_none());
    assert!(!path.parent().unwrap().exists());
    let s = Store::open(&path).unwrap();
    assert_eq!(s.schema_version().unwrap(), 1);
    drop(s);
    let s = Store::open(&path).unwrap();
    assert_eq!(s.schema_version().unwrap(), 1);
    assert!(Store::open_existing(&path).unwrap().is_some());
}

#[test]
fn positions_are_per_provider_and_gaps_are_not_reused() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("t.db")).unwrap();
    let other = ProviderId::new("fake-agent");
    add(&s, &cc(), "a", "a@x.co", 1);
    add(&s, &cc(), "b", "b@x.co", 4);
    add(&s, &other, "c", "a@x.co", 1); // same email, other provider: another account
    assert_eq!(s.next_position(&cc()).unwrap(), 5);
    assert_eq!(s.next_position(&other).unwrap(), 2);
    assert_eq!(s.next_position(&ProviderId::new("none")).unwrap(), 1);
    let rows = s.accounts(&cc()).unwrap();
    assert_eq!(rows.iter().map(|r| r.position).collect::<Vec<_>>(), vec![1, 4]);
    assert_eq!(rows[0].identity_json, json!({"emailAddress": "a@x.co"}));
}

#[test]
fn uniqueness_is_reported_by_name() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("t.db")).unwrap();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let b = add(&s, &cc(), "b", "b@x.co", 2);
    let dup = NewAccount {
        id: &AccountId::from_string("z"),
        provider: &cc(),
        position: 1,
        identity_key: "z@x.co\n",
        identity: &identity("z@x.co"),
        kind: "oauth",
        alias: None,
        login_expires_at: None,
        added_at: 1,
    };
    assert!(matches!(s.insert_account(&dup), Err(StoreError::PositionTaken(1))));
    s.set_alias(&a, Some("work")).unwrap();
    assert!(matches!(s.set_alias(&b, Some("WORK")), Err(StoreError::AliasTaken(_))));
    assert_eq!(s.find_by_alias("Work").unwrap().unwrap().id, a);
    s.set_alias(&a, None).unwrap();
    assert!(s.find_by_alias("work").unwrap().is_none());
}

#[test]
fn move_swaps_when_the_position_is_taken() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("t.db")).unwrap();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let b = add(&s, &cc(), "b", "b@x.co", 2);
    s.move_to(&a, 2).unwrap();
    assert_eq!(s.account(&a).unwrap().unwrap().position, 2);
    assert_eq!(s.account(&b).unwrap().unwrap().position, 1);
    s.move_to(&a, 7).unwrap();
    assert_eq!(s.account(&a).unwrap().unwrap().position, 7);
}

#[test]
fn replacement_markers_move_the_epoch() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("t.db")).unwrap();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let replacement = Identity { account_uuid: Some("u-new".into()), ..identity("a@x.co") };
    let meta = |kind| LoginMeta { identity_key: "a@x.co\n", identity: &replacement, kind, login_expires_at: Some(42) };
    s.begin_replacement(&a, "sha256:x", &meta("api_key")).unwrap();
    let r = s.account(&a).unwrap().unwrap();
    assert_eq!((r.login_epoch, r.replacing_fp.as_deref(), r.kind.as_str()), (1, Some("sha256:x"), "oauth"));
    // Finishing installs the recorded metadata, kind included.
    s.finish_replacement(&a).unwrap();
    let r = s.account(&a).unwrap().unwrap();
    assert_eq!((r.login_epoch, r.replacing_fp, r.kind.as_str()), (1, None, "api_key"));
    assert_eq!((r.account_uuid.as_deref(), r.login_expires_at), (Some("u-new"), Some(42)));
    s.begin_replacement(&a, "sha256:y", &meta("setup_token")).unwrap();
    s.rollback_replacement(&a).unwrap();
    let r = s.account(&a).unwrap().unwrap();
    assert_eq!((r.login_epoch, r.replacing_fp, r.kind.as_str()), (1, None, "api_key"));
}

#[test]
fn commit_switch_is_one_transaction_and_delete_clears_active() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("t.db")).unwrap();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let b = add(&s, &cc(), "b", "b@x.co", 2);
    let j = JournalRow {
        provider: cc(),
        holder: ProcessStamp { pid: 1, start: 2 },
        from_id: Some(a.clone()),
        to_id: b.clone(),
        from_fp: Some("sha256:a".into()),
        from_identity: Some(json!({"emailAddress": "a@x.co"})),
        to_fp: "sha256:b".into(),
        started_at: 5,
        prior: None,
    };
    s.insert_journal(&j).unwrap();
    assert_eq!(s.journal(&cc()).unwrap().unwrap(), j);
    // A second write replaces the row in one statement, carrying the row it superseded.
    let replaced = JournalRow { started_at: 9, prior: Some(Box::new(j.clone())), ..j.clone() };
    s.insert_journal(&replaced).unwrap();
    assert_eq!(s.journal(&cc()).unwrap().unwrap(), replaced);
    s.insert_journal(&j).unwrap();
    let ev = EventRow {
        at: 6,
        provider: cc(),
        kind: "switch".into(),
        from_id: Some(a.clone()),
        to_id: Some(b.clone()),
        trigger: Some("manual".into()),
        source: "cli".into(),
        detail: None,
    };
    s.commit_switch(&cc(), &b, &ev).unwrap();
    assert_eq!(s.active(&cc()).unwrap(), Some(b.clone()));
    assert!(s.journal(&cc()).unwrap().is_none());
    assert_eq!(s.events().unwrap(), vec![ev]);
    s.delete_account(&b).unwrap();
    assert_eq!(s.active(&cc()).unwrap(), None);
}

#[test]
fn uuid_backfill_only_fills_null() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("t.db")).unwrap();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    s.backfill_account_uuid(&a, "u-1").unwrap();
    s.backfill_account_uuid(&a, "u-2").unwrap();
    assert_eq!(s.account(&a).unwrap().unwrap().account_uuid.as_deref(), Some("u-1"));
}

#[test]
fn find_by_email_spans_providers_unless_narrowed() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("t.db")).unwrap();
    add(&s, &cc(), "a", "a@x.co", 1);
    add(&s, &ProviderId::new("fake-agent"), "b", "a@x.co", 1);
    assert_eq!(s.find_by_email("a@x.co", None).unwrap().len(), 2);
    assert_eq!(s.find_by_email("a@x.co", Some(&cc())).unwrap().len(), 1);
}
```

`crates/tagteam-engine/src/lib.rs`:
```rust
#![forbid(unsafe_code)]

pub mod store;
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p tagteam-engine --test store`
Expected: FAIL to compile.

- [ ] **Step 4: Implement**

`crates/tagteam-engine/src/store/mod.rs`:
```rust
use std::io;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use rusqlite::{Connection, ErrorCode, OptionalExtension, Row, params};
use serde_json::{Value, json};
use tagteam_core::{AccountId, ProviderId};
use tagteam_provider::atomic::ensure_private_dir;
use tagteam_provider::{Identity, ProcessStamp};

const SCHEMA_V1: &str = include_str!("schema.sql");

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("store error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("store I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("the store holds invalid data: {0}")]
    Corrupt(String),
    #[error("the alias {0:?} is already taken")]
    AliasTaken(String),
    #[error("position {0} is already taken")]
    PositionTaken(u32),
    #[error("that login is already stored")]
    IdentityTaken,
    #[error("no such account")]
    NoSuchAccount,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AccountRow {
    pub id: AccountId,
    pub provider: ProviderId,
    pub position: u32,
    pub identity_key: String,
    pub label: String,
    pub email: Option<String>,
    pub org_uuid: String,
    pub org_name: Option<String>,
    pub account_uuid: Option<String>,
    pub kind: String,
    pub alias: Option<String>,
    pub disabled: bool,
    pub identity_json: Value,
    pub login_expires_at: Option<i64>,
    pub login_epoch: i64,
    pub replacing_fp: Option<String>,
    pub quarantine_reason: Option<String>,
    pub quarantine_fp: Option<String>,
    pub added_at: i64,
}

pub struct NewAccount<'a> {
    pub id: &'a AccountId,
    pub provider: &'a ProviderId,
    pub position: u32,
    pub identity_key: &'a str,
    pub identity: &'a Identity,
    pub kind: &'a str,
    pub alias: Option<&'a str>,
    pub login_expires_at: Option<i64>,
    pub added_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EventRow {
    pub at: i64,
    pub provider: ProviderId,
    pub kind: String,
    pub from_id: Option<AccountId>,
    pub to_id: Option<AccountId>,
    pub trigger: Option<String>,
    pub source: String,
    pub detail: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct JournalRow {
    pub provider: ProviderId,
    pub holder: ProcessStamp,
    pub from_id: Option<AccountId>,
    pub to_id: AccountId,
    pub from_fp: Option<String>,
    pub from_identity: Option<Value>,
    pub to_fp: String,
    pub started_at: i64,
    /// The unresolved row a forced switch superseded. If the forced switch never lands, this
    /// is what recovery or rollback puts back, so the unresolved state is never forgotten.
    pub prior: Option<Box<JournalRow>>,
}

/// The metadata an explicit replacement installs with its credential (§12.5). It is recorded
/// with the marker, so the next lock holder can finish a replacement whose vault write landed.
pub struct LoginMeta<'a> {
    pub identity_key: &'a str,
    pub identity: &'a Identity,
    pub kind: &'a str,
    pub login_expires_at: Option<i64>,
}

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
        "started_at": j.started_at,
        "prior": j.prior.as_deref().map(journal_to_json),
    })
}

fn journal_from_json(v: &Value) -> Option<JournalRow> {
    Some(JournalRow {
        provider: ProviderId::new(v["provider"].as_str()?),
        holder: ProcessStamp { pid: v["holder_pid"].as_u64()? as u32, start: v["holder_start"].as_u64()? },
        from_id: v["from_id"].as_str().map(AccountId::from_string),
        to_id: AccountId::from_string(v["to_id"].as_str()?),
        from_fp: v["from_fp"].as_str().map(str::to_owned),
        from_identity: Some(v["from_identity"].clone()).filter(|x| !x.is_null()),
        to_fp: v["to_fp"].as_str()?.to_owned(),
        started_at: v["started_at"].as_i64()?,
        prior: v.get("prior").filter(|x| !x.is_null()).and_then(journal_from_json).map(Box::new),
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct DisplacedRow {
    pub id: String,
    pub provider: ProviderId,
    pub at: i64,
    pub reason: String,
    pub fingerprint: String,
    pub identity: Option<Value>,
}

pub struct Store {
    conn: Mutex<Connection>,
}

const ACCOUNT_COLUMNS: &str = "id, provider, position, identity_key, label, email, org_uuid, org_name, \
    account_uuid, kind, alias, disabled, identity_json, login_expires_at, login_epoch, replacing_fp, \
    quarantine_reason, quarantine_fp, added_at";

fn json_col(r: &Row<'_>, name: &str) -> rusqlite::Result<Option<Value>> {
    let raw: Option<String> = r.get(name)?;
    raw.map(|s| serde_json::from_str(&s))
        .transpose()
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e)))
}

fn account_from_row(r: &Row<'_>) -> rusqlite::Result<AccountRow> {
    Ok(AccountRow {
        id: AccountId::from_string(r.get::<_, String>("id")?),
        provider: ProviderId::new(r.get::<_, String>("provider")?),
        position: r.get("position")?,
        identity_key: r.get("identity_key")?,
        label: r.get("label")?,
        email: r.get("email")?,
        org_uuid: r.get("org_uuid")?,
        org_name: r.get("org_name")?,
        account_uuid: r.get("account_uuid")?,
        kind: r.get("kind")?,
        alias: r.get("alias")?,
        disabled: r.get::<_, i64>("disabled")? != 0,
        identity_json: json_col(r, "identity_json")?.unwrap_or(Value::Null),
        login_expires_at: r.get("login_expires_at")?,
        login_epoch: r.get("login_epoch")?,
        replacing_fp: r.get("replacing_fp")?,
        quarantine_reason: r.get("quarantine_reason")?,
        quarantine_fp: r.get("quarantine_fp")?,
        added_at: r.get("added_at")?,
    })
}

fn journal_from_row(r: &Row<'_>) -> rusqlite::Result<JournalRow> {
    Ok(JournalRow {
        provider: ProviderId::new(r.get::<_, String>("provider")?),
        holder: ProcessStamp { pid: r.get("holder_pid")?, start: r.get::<_, i64>("holder_start")? as u64 },
        from_id: r.get::<_, Option<String>>("from_id")?.map(AccountId::from_string),
        to_id: AccountId::from_string(r.get::<_, String>("to_id")?),
        from_fp: r.get("from_fp")?,
        from_identity: json_col(r, "from_identity")?,
        to_fp: r.get("to_fp")?,
        started_at: r.get("started_at")?,
        prior: json_col(r, "prior")?.as_ref().and_then(journal_from_json).map(Box::new),
    })
}

fn event_from_row(r: &Row<'_>) -> rusqlite::Result<EventRow> {
    Ok(EventRow {
        at: r.get("at")?,
        provider: ProviderId::new(r.get::<_, String>("provider")?),
        kind: r.get("kind")?,
        from_id: r.get::<_, Option<String>>("from_id")?.map(AccountId::from_string),
        to_id: r.get::<_, Option<String>>("to_id")?.map(AccountId::from_string),
        trigger: r.get("trigger")?,
        source: r.get("source")?,
        detail: json_col(r, "detail")?,
    })
}

/// Maps UNIQUE violations to named errors.
fn classify(e: rusqlite::Error, position: u32, alias: Option<&str>) -> StoreError {
    if let rusqlite::Error::SqliteFailure(f, Some(msg)) = &e {
        if f.code == ErrorCode::ConstraintViolation {
            if msg.contains("accounts.alias") {
                return StoreError::AliasTaken(alias.unwrap_or_default().to_owned());
            }
            if msg.contains("accounts.position") {
                return StoreError::PositionTaken(position);
            }
            if msg.contains("accounts.identity_key") {
                return StoreError::IdentityTaken;
            }
        }
    }
    StoreError::Sqlite(e)
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(dir) = path.parent() {
            ensure_private_dir(dir)?;
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_millis(5000))?;
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON; PRAGMA synchronous = NORMAL;")?;
        let store = Self { conn: Mutex::new(conn) };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_existing(path: &Path) -> Result<Option<Self>, StoreError> {
        if path.exists() { Self::open(path).map(Some) } else { Ok(None) }
    }

    fn migrate(&self) -> Result<(), StoreError> {
        let mut c = self.conn.lock().unwrap();
        let version: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version < 1 {
            let tx = c.transaction()?;
            tx.execute_batch(SCHEMA_V1)?;
            tx.execute_batch("PRAGMA user_version = 1;")?;
            tx.commit()?;
        }
        Ok(())
    }

    pub fn schema_version(&self) -> Result<i64, StoreError> {
        Ok(self.conn.lock().unwrap().query_row("PRAGMA user_version", [], |r| r.get(0))?)
    }

    fn query_accounts(&self, where_clause: &str, params: &[&dyn rusqlite::ToSql]) -> Result<Vec<AccountRow>, StoreError> {
        let c = self.conn.lock().unwrap();
        let sql = format!("SELECT {ACCOUNT_COLUMNS} FROM accounts {where_clause}");
        let mut stmt = c.prepare(&sql)?;
        let rows = stmt.query_map(params, account_from_row)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn one(&self, where_clause: &str, params: &[&dyn rusqlite::ToSql]) -> Result<Option<AccountRow>, StoreError> {
        Ok(self.query_accounts(where_clause, params)?.into_iter().next())
    }

    pub fn accounts(&self, provider: &ProviderId) -> Result<Vec<AccountRow>, StoreError> {
        self.query_accounts("WHERE provider = ?1 ORDER BY position", &[&provider.as_str()])
    }

    pub fn all_accounts(&self) -> Result<Vec<AccountRow>, StoreError> {
        self.query_accounts("ORDER BY provider, position", &[])
    }

    pub fn account(&self, id: &AccountId) -> Result<Option<AccountRow>, StoreError> {
        self.one("WHERE id = ?1", &[&id.as_str()])
    }

    pub fn find_by_identity_key(&self, provider: &ProviderId, key: &str) -> Result<Option<AccountRow>, StoreError> {
        self.one("WHERE provider = ?1 AND identity_key = ?2", &[&provider.as_str(), &key])
    }

    pub fn find_by_position(&self, provider: &ProviderId, position: u32) -> Result<Option<AccountRow>, StoreError> {
        self.one("WHERE provider = ?1 AND position = ?2", &[&provider.as_str(), &position])
    }

    pub fn find_by_alias(&self, alias: &str) -> Result<Option<AccountRow>, StoreError> {
        self.one("WHERE alias = ?1", &[&alias])
    }

    pub fn find_by_email(&self, email: &str, provider: Option<&ProviderId>) -> Result<Vec<AccountRow>, StoreError> {
        match provider {
            Some(p) => self.query_accounts("WHERE email = ?1 AND provider = ?2 ORDER BY position", &[&email, &p.as_str()]),
            None => self.query_accounts("WHERE email = ?1 ORDER BY provider, position", &[&email]),
        }
    }

    pub fn next_position(&self, provider: &ProviderId) -> Result<u32, StoreError> {
        let c = self.conn.lock().unwrap();
        let max: Option<u32> =
            c.query_row("SELECT MAX(position) FROM accounts WHERE provider = ?1", [provider.as_str()], |r| r.get(0))?;
        Ok(max.unwrap_or(0) + 1)
    }

    pub fn insert_account(&self, a: &NewAccount<'_>) -> Result<(), StoreError> {
        let c = self.conn.lock().unwrap();
        c.execute(
            "INSERT INTO accounts (id, provider, position, identity_key, label, email, org_uuid, org_name, \
             account_uuid, kind, alias, identity_json, login_expires_at, added_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                a.id.as_str(),
                a.provider.as_str(),
                a.position,
                a.identity_key,
                a.identity.label,
                a.identity.email,
                a.identity.org_uuid,
                a.identity.org_name,
                a.identity.account_uuid,
                a.kind,
                a.alias,
                a.identity.raw.to_string(),
                a.login_expires_at,
                a.added_at,
            ],
        )
        .map_err(|e| classify(e, a.position, a.alias))?;
        Ok(())
    }

    fn exec(&self, sql: &str, p: &[&dyn rusqlite::ToSql]) -> Result<usize, StoreError> {
        Ok(self.conn.lock().unwrap().execute(sql, p)?)
    }

    pub fn update_login(
        &self,
        id: &AccountId,
        identity_key: &str,
        identity: &Identity,
        kind: &str,
        login_expires_at: Option<i64>,
    ) -> Result<(), StoreError> {
        let n = self.exec(
            "UPDATE accounts SET identity_key = ?2, label = ?3, email = ?4, org_uuid = ?5, org_name = ?6, \
             account_uuid = COALESCE(?7, account_uuid), kind = ?8, identity_json = ?9, login_expires_at = ?10, \
             quarantine_reason = NULL, quarantine_fp = NULL, quarantine_at = NULL WHERE id = ?1",
            &[
                &id.as_str(),
                &identity_key,
                &identity.label,
                &identity.email,
                &identity.org_uuid,
                &identity.org_name,
                &identity.account_uuid,
                &kind,
                &identity.raw.to_string(),
                &login_expires_at,
            ],
        )?;
        if n == 0 { Err(StoreError::NoSuchAccount) } else { Ok(()) }
    }

    pub fn begin_replacement(&self, id: &AccountId, fp: &str, meta: &LoginMeta<'_>) -> Result<(), StoreError> {
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
        });
        self.exec(
            "UPDATE accounts SET login_epoch = login_epoch + 1, replacing_fp = ?2, replacing_meta = ?3 WHERE id = ?1",
            &[&id.as_str(), &fp, &meta.to_string()],
        )?;
        Ok(())
    }

    /// The replacement landed: installs its recorded metadata, clears any quarantine and the
    /// marker, all in one transaction.
    pub fn finish_replacement(&self, id: &AccountId) -> Result<(), StoreError> {
        let mut c = self.conn.lock().unwrap();
        let tx = c.transaction()?;
        let meta: Option<String> = tx
            .query_row("SELECT replacing_meta FROM accounts WHERE id = ?1", [id.as_str()], |r| r.get(0))
            .optional()?
            .flatten();
        if let Some(m) = meta {
            let v: Value = serde_json::from_str(&m).map_err(|e| StoreError::Corrupt(e.to_string()))?;
            tx.execute(
                "UPDATE accounts SET identity_key = ?2, label = ?3, email = ?4, org_uuid = ?5, org_name = ?6, \
                 account_uuid = COALESCE(?7, account_uuid), kind = ?8, identity_json = ?9, login_expires_at = ?10, \
                 quarantine_reason = NULL, quarantine_fp = NULL, quarantine_at = NULL WHERE id = ?1",
                params![
                    id.as_str(),
                    v["identity_key"].as_str(),
                    v["label"].as_str(),
                    v["email"].as_str(),
                    v["org_uuid"].as_str().unwrap_or(""),
                    v["org_name"].as_str(),
                    v["account_uuid"].as_str(),
                    v["kind"].as_str(),
                    v["identity_json"].to_string(),
                    v["login_expires_at"].as_i64(),
                ],
            )?;
        }
        tx.execute("UPDATE accounts SET replacing_fp = NULL, replacing_meta = NULL WHERE id = ?1", [id.as_str()])?;
        tx.commit()?;
        Ok(())
    }

    pub fn rollback_replacement(&self, id: &AccountId) -> Result<(), StoreError> {
        self.exec(
            "UPDATE accounts SET login_epoch = login_epoch - 1, replacing_fp = NULL, replacing_meta = NULL \
             WHERE id = ?1 AND replacing_fp IS NOT NULL",
            &[&id.as_str()],
        )?;
        Ok(())
    }

    pub fn backfill_account_uuid(&self, id: &AccountId, uuid: &str) -> Result<(), StoreError> {
        self.exec("UPDATE accounts SET account_uuid = ?2 WHERE id = ?1 AND account_uuid IS NULL", &[&id.as_str(), &uuid])?;
        Ok(())
    }

    pub fn set_alias(&self, id: &AccountId, alias: Option<&str>) -> Result<(), StoreError> {
        self.exec("UPDATE accounts SET alias = ?2 WHERE id = ?1", &[&id.as_str(), &alias])
            .map_err(|e| match e {
                StoreError::Sqlite(e) => classify(e, 0, alias),
                other => other,
            })?;
        Ok(())
    }

    pub fn set_disabled(&self, id: &AccountId, disabled: bool) -> Result<(), StoreError> {
        self.exec("UPDATE accounts SET disabled = ?2 WHERE id = ?1", &[&id.as_str(), &(disabled as i64)])?;
        Ok(())
    }

    pub fn move_to(&self, id: &AccountId, position: u32) -> Result<(), StoreError> {
        let mut c = self.conn.lock().unwrap();
        let tx = c.transaction()?;
        let (provider, from): (String, u32) =
            tx.query_row("SELECT provider, position FROM accounts WHERE id = ?1", [id.as_str()], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()?
                .ok_or(StoreError::NoSuchAccount)?;
        let occupant: Option<String> = tx
            .query_row(
                "SELECT id FROM accounts WHERE provider = ?1 AND position = ?2 AND id != ?3",
                params![provider, position, id.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(other) = &occupant {
            tx.execute("UPDATE accounts SET position = 0 WHERE id = ?1", [other])?;
        }
        tx.execute("UPDATE accounts SET position = ?2 WHERE id = ?1", params![id.as_str(), position])?;
        if let Some(other) = &occupant {
            tx.execute("UPDATE accounts SET position = ?2 WHERE id = ?1", params![other, from])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn delete_account(&self, id: &AccountId) -> Result<(), StoreError> {
        self.exec("DELETE FROM accounts WHERE id = ?1", &[&id.as_str()])?;
        Ok(())
    }

    pub fn active(&self, provider: &ProviderId) -> Result<Option<AccountId>, StoreError> {
        let c = self.conn.lock().unwrap();
        let id: Option<Option<String>> = c
            .query_row("SELECT account_id FROM active_accounts WHERE provider = ?1", [provider.as_str()], |r| r.get(0))
            .optional()?;
        Ok(id.flatten().map(AccountId::from_string))
    }

    pub fn set_active(&self, provider: &ProviderId, id: Option<&AccountId>) -> Result<(), StoreError> {
        self.exec(
            "INSERT INTO active_accounts (provider, account_id) VALUES (?1, ?2) \
             ON CONFLICT(provider) DO UPDATE SET account_id = excluded.account_id",
            &[&provider.as_str(), &id.map(AccountId::as_str)],
        )?;
        Ok(())
    }

    fn insert_event_on(c: &Connection, e: &EventRow) -> rusqlite::Result<usize> {
        c.execute(
            "INSERT INTO events (at, provider, kind, from_id, to_id, trigger, source, detail) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                e.at,
                e.provider.as_str(),
                e.kind,
                e.from_id.as_ref().map(AccountId::as_str),
                e.to_id.as_ref().map(AccountId::as_str),
                e.trigger,
                e.source,
                e.detail.as_ref().map(Value::to_string),
            ],
        )
    }

    /// §9.4 step 9: the active account, the event and the journal row move together.
    pub fn commit_switch(&self, provider: &ProviderId, to: &AccountId, event: &EventRow) -> Result<(), StoreError> {
        let mut c = self.conn.lock().unwrap();
        let tx = c.transaction()?;
        tx.execute(
            "INSERT INTO active_accounts (provider, account_id) VALUES (?1, ?2) \
             ON CONFLICT(provider) DO UPDATE SET account_id = excluded.account_id",
            params![provider.as_str(), to.as_str()],
        )?;
        Self::insert_event_on(&tx, event)?;
        tx.execute("DELETE FROM switch_journal WHERE provider = ?1", [provider.as_str()])?;
        tx.commit()?;
        Ok(())
    }

    pub fn insert_event(&self, e: &EventRow) -> Result<(), StoreError> {
        Self::insert_event_on(&self.conn.lock().unwrap(), e)?;
        Ok(())
    }

    pub fn events(&self) -> Result<Vec<EventRow>, StoreError> {
        let c = self.conn.lock().unwrap();
        let mut stmt = c.prepare("SELECT * FROM events ORDER BY rowid")?;
        let rows = stmt.query_map([], event_from_row)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn journal(&self, provider: &ProviderId) -> Result<Option<JournalRow>, StoreError> {
        let c = self.conn.lock().unwrap();
        Ok(c.query_row("SELECT * FROM switch_journal WHERE provider = ?1", [provider.as_str()], journal_from_row)
            .optional()?)
    }

    pub fn journals(&self) -> Result<Vec<JournalRow>, StoreError> {
        let c = self.conn.lock().unwrap();
        let mut stmt = c.prepare("SELECT * FROM switch_journal ORDER BY provider")?;
        let rows = stmt.query_map([], journal_from_row)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Writes the provider's journal row, atomically replacing any row already there: a
    /// forced switch settles an undecidable row this way without a gap in which neither
    /// row exists (§9.6).
    pub fn insert_journal(&self, j: &JournalRow) -> Result<(), StoreError> {
        self.conn.lock().unwrap().execute(
            "INSERT OR REPLACE INTO switch_journal \
             (provider, holder_pid, holder_start, from_id, to_id, from_fp, from_identity, to_fp, started_at, prior) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                j.provider.as_str(),
                j.holder.pid,
                j.holder.start as i64,
                j.from_id.as_ref().map(AccountId::as_str),
                j.to_id.as_str(),
                j.from_fp,
                j.from_identity.as_ref().map(Value::to_string),
                j.to_fp,
                j.started_at,
                j.prior.as_deref().map(|p| journal_to_json(p).to_string()),
            ],
        )?;
        Ok(())
    }

    pub fn delete_journal(&self, provider: &ProviderId) -> Result<(), StoreError> {
        self.exec("DELETE FROM switch_journal WHERE provider = ?1", &[&provider.as_str()])?;
        Ok(())
    }

    pub fn insert_displaced(&self, d: &DisplacedRow) -> Result<(), StoreError> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO displaced (id, provider, at, reason, fingerprint, identity) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![d.id, d.provider.as_str(), d.at, d.reason, d.fingerprint, d.identity.as_ref().map(Value::to_string)],
        )?;
        Ok(())
    }
}
```

`lib.rs` already declares `pub mod store;`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p tagteam-engine --test store`
Expected: PASS (8 tests).

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-engine
git commit -m "Add the SQLite store with the full schema"
```

---
### Task 17: Engine skeleton — vault, account locks, registry, oracle port, displacement

**Files:**
- Create: `crates/tagteam-engine/src/{error.rs,vault.rs,account_lock.rs,registry.rs,oracle.rs,displace.rs,engine.rs}`
- Modify: `crates/tagteam-engine/src/lib.rs`
- Create: `crates/tagteam-engine/tests/common/mod.rs` (shared fixture)
- Test: `crates/tagteam-engine/tests/vault.rs`, `crates/tagteam-engine/tests/engine_basics.rs`

**Interfaces:**
- Consumes: `Store` (Task 16), the provider crate, `ClaudeCode` (tests only)
- Produces:
```rust
// error.rs
pub enum EngineError { Store(StoreError), Provider(ProviderError), Lock(LockError), Vault(VaultError),
    Unreadable(ReadError), UnknownProvider(String), InsideRunShell, NoLiveLogin, LiveApiKey, DegradedRead,
    OwnerMismatch { expected: String, found: String }, LiveMoved, NeedsConfirmation { position: u32, occupant: String },
    InvalidInput(String), NoSuchAccount(String), Ambiguous { input: String, candidates: Vec<String> },
    InterruptedSwitch(String), RolledBack(String), RollbackFailed { cause: String, failed: String }, Io(io::Error) }
impl EngineError { pub fn kind(&self) -> &'static str }   // stable JSON `error.type`
// vault.rs
pub trait VaultBackend: Send + Sync { fn read(&self, key: &str) -> Read<Vec<u8>>;
    fn write(&self, key: &str, bytes: &[u8]) -> Result<(), VaultError>; fn delete(&self, key: &str) -> Result<(), VaultError>; }
pub struct KeychainVault; impl KeychainVault { pub fn new(Arc<dyn Keychain>) -> Self }   // service "tagteam"
pub struct FileVault;     impl FileVault { pub fn new(PathBuf) -> Self }                  // <dir>/<key>.json
pub struct Vault; impl Vault { pub fn new(Box<dyn VaultBackend>) -> Self;
    pub fn read(&self, id: &AccountId) -> Read<Vec<u8>>; pub fn read_prev(&self, id: &AccountId) -> Read<Vec<u8>>;
    pub fn store(&self, lock: &AccountLock, bytes: &[u8], fp: &dyn Fn(&[u8]) -> Option<Fingerprint>) -> Result<(), VaultError>;
    pub fn delete(&self, lock: &AccountLock) -> Result<(), VaultError>; }
pub enum VaultError { Write(String), Delete(String), Unreadable(ReadError), Verify }
// account_lock.rs
pub struct AccountLock; impl AccountLock { pub const WAIT: Duration /* 15 s */;
    pub fn acquire(env: &Env, id: &AccountId, wait: Duration) -> Result<AccountLock, LockError>;
    pub fn try_acquire(env: &Env, id: &AccountId) -> Result<Option<AccountLock>, LockError>;
    pub fn id(&self) -> &AccountId; }
// registry.rs
pub struct ProviderRegistry; impl ProviderRegistry { pub fn new() -> Self; pub fn with(self, Arc<dyn Provider>) -> Self;
    pub fn get(&self, id: &ProviderId) -> Option<Arc<dyn Provider>>; pub fn all(&self) -> &[Arc<dyn Provider>]; }
// oracle.rs
pub trait Oracle: Send + Sync { fn resolve(&self, provider: &dyn Provider, credential: &Credential) -> Option<Identity>; }
pub struct NoOracle;
pub fn verdict(resolved: Option<&Identity>, account: &AccountRow) -> OracleVerdict;
// engine.rs
pub struct EngineConfig { pub env: Env, pub registry: ProviderRegistry, pub vault: Vault,
    pub oracle: Arc<dyn Oracle>, pub clock: Arc<dyn Clock>, pub default_provider: ProviderId }
pub struct Engine; impl Engine { pub fn new(EngineConfig) -> Self; pub fn env(&self) -> &Env;
    pub fn default_provider(&self) -> &ProviderId; pub fn store(&self) -> Result<Arc<Store>, EngineError>;
    pub fn existing_store(&self) -> Result<Option<Arc<Store>>, EngineError>;
    pub fn provider(&self, id: &ProviderId) -> Result<Arc<dyn Provider>, EngineError>;
    pub fn providers(&self) -> Vec<Arc<dyn Provider>>; pub fn now_ms(&self) -> i64;
    pub fn mutation_guard(&self) -> Result<MutationGuard, EngineError>;
    pub fn lock_account(&self, id: &AccountId) -> Result<AccountLock, EngineError>;          // reconciles §12.5
    pub fn lock_accounts(&self, ids: &[&AccountId]) -> Result<Vec<AccountLock>, EngineError>; // ascending, deduped
    pub(crate) fn refuse_inside_run_shell(&self) -> Result<(), EngineError>;
    pub(crate) fn refuse_if_interrupted(&self, provider: &ProviderId) -> Result<(), EngineError>;
    pub(crate) fn settle_or_refuse(&self, provider: &ProviderId) -> Result<(), EngineError>; }
// displace.rs
pub(crate) fn displace(engine: &Engine, provider: &ProviderId, bytes: &[u8], fp: Option<&Fingerprint>,
    reason: &str, identity: Option<&Value>) -> Result<String, EngineError>;
```

- [ ] **Step 1: Write the shared test fixture**

`crates/tagteam-engine/tests/common/mod.rs`:
```rust
#![allow(dead_code)]

use std::fs;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tagteam_cc::live::{LiveStore, Platform};
use tagteam_cc::{CcPaths, ClaudeCode, ItemKind, keychain_account, keychain_service};
use tagteam_core::{AccountId, CLAUDE_CODE, ProviderId};
use tagteam_engine::oracle::Oracle;
use tagteam_engine::registry::ProviderRegistry;
use tagteam_engine::vault::{FileVault, KeychainVault, Vault};
use tagteam_engine::{Engine, EngineConfig};
use tagteam_provider::splice::replace_top_level;
use tagteam_provider::{Credential, Env, FakeClock, FakeKeychain, Identity, Provider, Read};

/// An oracle that answers whatever the test sets.
#[derive(Default)]
pub struct FixedOracle(pub Mutex<Option<Identity>>);

impl FixedOracle {
    pub fn set(&self, id: Option<Identity>) {
        *self.0.lock().unwrap() = id;
    }
}

impl Oracle for FixedOracle {
    fn resolve(&self, _p: &dyn Provider, _c: &Credential) -> Option<Identity> {
        self.0.lock().unwrap().clone()
    }
}

pub const CLAUDE_JSON: &str = r#"{
  "numStartups": 12,
  "projects": {
    "/work/app": {
      "allowedTools": [],
      "history": ["x"]
    }
  },
  "mcpServers": {
    "local": { "command": "srv" }
  },
  "userID": "user-7",
  "someFutureKey": { "n": 1e400 }
}
"#;

pub struct Fx {
    pub dir: tempfile::TempDir,
    pub env: Env,
    pub platform: Platform,
    pub kc: Arc<FakeKeychain>,
    pub oracle: Arc<FixedOracle>,
    pub clock: Arc<FakeClock>,
    pub cc: Arc<ClaudeCode>,
    pub engine: Engine,
}

impl Fx {
    pub fn new() -> Self {
        Self::with_platform(Platform::MacOs)
    }

    pub fn with_platform(platform: Platform) -> Self {
        Self::with(platform, |_| {})
    }

    /// A fixture whose Env is adjusted before anything is created in it.
    pub fn with(platform: Platform, adjust: impl FnOnce(&mut Env)) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut env = Env::for_test(dir.path());
        adjust(&mut env);
        let claude = env.home.join(".claude");
        fs::create_dir_all(claude.join("projects/-work-app/memory")).unwrap();
        fs::write(claude.join("projects/-work-app/memory/MEMORY.md"), "remember this\n").unwrap();
        fs::create_dir_all(claude.join("skills/s")).unwrap();
        fs::write(claude.join("skills/s/SKILL.md"), "skill\n").unwrap();
        fs::create_dir_all(claude.join("plugins")).unwrap();
        fs::write(claude.join("plugins/installed.json"), "{}\n").unwrap();
        fs::write(claude.join("CLAUDE.md"), "instructions\n").unwrap();
        fs::write(claude.join("history.jsonl"), "{\"display\":\"hi\"}\n").unwrap();
        fs::write(claude.join("settings.json"), "{\"theme\":\"dark\"}\n").unwrap();
        // Wherever this Env's global config resolves (Appendix A.1), not a hard-coded path.
        fs::write(CcPaths::resolve(&env).global_config, CLAUDE_JSON).unwrap();

        let kc = Arc::new(FakeKeychain::new());
        let oracle = Arc::new(FixedOracle::default());
        let clock = Arc::new(FakeClock::new(1_790_000_000_000));
        let cc = Arc::new(ClaudeCode::with_store(LiveStore::new(kc.clone(), platform).with_retry_delay(Duration::ZERO)));
        let vault = match platform {
            Platform::MacOs => Vault::new(Box::new(KeychainVault::new(kc.clone()))),
            Platform::Linux => Vault::new(Box::new(FileVault::new(env.data_dir().join("vault")))),
        };
        let engine = Engine::new(EngineConfig {
            env: env.clone(),
            registry: ProviderRegistry::new().with(cc.clone()),
            vault,
            oracle: oracle.clone(),
            clock: clock.clone(),
            default_provider: ProviderId::new(CLAUDE_CODE),
        });
        Fx { dir, env, platform, kc, oracle, clock, cc, engine }
    }

    pub fn provider(&self) -> ProviderId {
        ProviderId::new(CLAUDE_CODE)
    }

    pub fn paths(&self) -> CcPaths {
        CcPaths::resolve(&self.env)
    }

    pub fn credential_json(email: &str, rt: &str) -> Value {
        json!({
            "claudeAiOauth": {
                "accessToken": format!("at-{rt}"),
                "refreshToken": rt,
                "expiresAt": 1_790_003_600_000i64,
                "refreshTokenExpiresAt": 1_797_000_000_000i64,
                "scopes": ["user:inference", "user:profile"],
                "subscriptionType": if email.starts_with("team") { "team" } else { "max" }
            },
            "mcpOAuth": {"srv": {"token": "machine-shared"}}
        })
    }

    pub fn oauth_account(email: &str) -> Value {
        json!({"emailAddress": email, "organizationUuid": "", "organizationName": null, "accountUuid": format!("uuid-{email}")})
    }

    /// What `claude /login` leaves behind: `oauthAccount` plus the live credential.
    pub fn login(&self, email: &str, rt: &str) {
        let p = self.paths();
        let doc = fs::read(&p.global_config).unwrap();
        fs::write(&p.global_config, replace_top_level(&doc, "oauthAccount", &Self::oauth_account(email)).unwrap()).unwrap();
        self.set_live_credential(Self::credential_json(email, rt).to_string().as_bytes());
    }

    pub fn set_live_credential(&self, bytes: &[u8]) {
        match self.platform {
            Platform::MacOs => self.kc.put(&keychain_service(&self.env, ItemKind::OAuth), &keychain_account(&self.env), bytes),
            Platform::Linux => fs::write(self.paths().credentials_file, bytes).unwrap(),
        }
    }

    pub fn live_credential(&self) -> Option<Value> {
        let bytes = match self.platform {
            Platform::MacOs => self.kc.get(&keychain_service(&self.env, ItemKind::OAuth), &keychain_account(&self.env)),
            Platform::Linux => fs::read(self.paths().credentials_file).ok(),
        }?;
        serde_json::from_slice(&bytes).ok()
    }

    /// CC rotating the live refresh token in place.
    pub fn rotate_live(&self, new_rt: &str) {
        let mut v = self.live_credential().unwrap();
        v["claudeAiOauth"]["refreshToken"] = json!(new_rt);
        self.set_live_credential(v.to_string().as_bytes());
    }

    pub fn live_refresh_token(&self) -> Option<String> {
        self.live_credential()?["claudeAiOauth"]["refreshToken"].as_str().map(str::to_owned)
    }

    pub fn live_email(&self) -> Option<String> {
        match self.cc.live_identity(&self.env) {
            Read::Present(i) => i.email,
            _ => None,
        }
    }

    pub fn vault_bytes(&self, id: &AccountId) -> Option<Vec<u8>> {
        self.engine_vault_read(id)
    }

    fn engine_vault_read(&self, id: &AccountId) -> Option<Vec<u8>> {
        match self.platform {
            Platform::MacOs => self.kc.get("tagteam", id.as_str()),
            Platform::Linux => fs::read(self.env.data_dir().join("vault").join(format!("{id}.json"))).ok(),
        }
    }

    pub fn vault_refresh_token(&self, id: &AccountId) -> Option<String> {
        let v: Value = serde_json::from_slice(&self.vault_bytes(id)?).ok()?;
        v["claudeAiOauth"]["refreshToken"].as_str().map(str::to_owned)
    }

    /// An engine over the same Keychain, oracle and clock, but a different Env.
    pub fn engine_with_env(&self, env: Env) -> Engine {
        Engine::new(EngineConfig {
            env,
            registry: ProviderRegistry::new().with(self.cc.clone()),
            vault: Vault::new(Box::new(KeychainVault::new(self.kc.clone()))),
            oracle: self.oracle.clone(),
            clock: self.clock.clone(),
            default_provider: ProviderId::new(CLAUDE_CODE),
        })
    }
}
```

- [ ] **Step 2: Write the failing tests**

`crates/tagteam-engine/tests/vault.rs`:
```rust
mod common;

use std::sync::Arc;

use tagteam_core::{AccountId, Fingerprint};
use tagteam_engine::account_lock::AccountLock;
use tagteam_engine::vault::{FileVault, KeychainVault, Vault};
use tagteam_provider::{Env, FakeKeychain, Read};

fn fp(b: &[u8]) -> Option<Fingerprint> {
    // Test lineage: everything up to the first '#' is the generation.
    let s = std::str::from_utf8(b).ok()?;
    Some(Fingerprint::of_secret(s.split('#').next()?.as_bytes()))
}

fn backends(env: &Env, kc: Arc<FakeKeychain>) -> Vec<Vault> {
    vec![
        Vault::new(Box::new(KeychainVault::new(kc))),
        Vault::new(Box::new(FileVault::new(env.data_dir().join("vault")))),
    ]
}

#[test]
fn prev_moves_only_when_the_generation_changes() {
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    for v in backends(&env, Arc::new(FakeKeychain::new())) {
        let id = AccountId::from_string("0192-a");
        let lock = AccountLock::acquire(&env, &id, AccountLock::WAIT).unwrap();
        assert!(matches!(v.read(&id), Read::Absent));
        v.store(&lock, b"gen1#a", &fp).unwrap();
        v.store(&lock, b"gen1#b", &fp).unwrap(); // same generation: no .prev
        assert!(matches!(v.read_prev(&id), Read::Absent));
        v.store(&lock, b"gen2", &fp).unwrap();
        assert_eq!(v.read_prev(&id).present().unwrap(), b"gen1#b");
        assert_eq!(v.read(&id).present().unwrap(), b"gen2");
        v.delete(&lock).unwrap();
        assert!(matches!(v.read(&id), Read::Absent));
        assert!(matches!(v.read_prev(&id), Read::Absent));
    }
}

#[test]
fn a_locked_keychain_aborts_the_delete_and_keeps_the_items() {
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    let kc = Arc::new(FakeKeychain::new());
    let v = Vault::new(Box::new(KeychainVault::new(kc.clone())));
    let id = AccountId::from_string("x");
    let lock = AccountLock::acquire(&env, &id, AccountLock::WAIT).unwrap();
    v.store(&lock, b"g", &fp).unwrap();
    kc.set_locked(true);
    assert!(v.delete(&lock).is_err());
    kc.set_locked(false);
    assert!(v.read(&id).is_present());
}

#[test]
fn file_vault_entries_are_private() {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    let v = Vault::new(Box::new(FileVault::new(env.data_dir().join("vault"))));
    let id = AccountId::from_string("y");
    let lock = AccountLock::acquire(&env, &id, AccountLock::WAIT).unwrap();
    v.store(&lock, b"g", &fp).unwrap();
    let meta = std::fs::metadata(env.data_dir().join("vault/y.json")).unwrap();
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
}
```

`crates/tagteam-engine/tests/engine_basics.rs`:
```rust
mod common;

use common::Fx;
use serde_json::json;
use tagteam_core::{AccountId, OracleVerdict};
use tagteam_engine::account_lock::AccountLock;
use tagteam_engine::oracle::verdict;
use tagteam_engine::store::{LoginMeta, NewAccount};
use tagteam_provider::{Identity, Provider};

#[test]
fn reading_commands_never_create_the_store() {
    let fx = Fx::new();
    assert!(fx.engine.existing_store().unwrap().is_none());
    assert!(!fx.env.data_dir().exists());
}

#[test]
fn account_locks_are_taken_in_ascending_order_without_duplicates() {
    let fx = Fx::new();
    let (a, b) = (AccountId::from_string("b-2"), AccountId::from_string("a-1"));
    let locks = fx.engine.lock_accounts(&[&a, &b, &a]).unwrap();
    assert_eq!(locks.iter().map(|l| l.id().as_str()).collect::<Vec<_>>(), vec!["a-1", "b-2"]);
    assert!(AccountLock::try_acquire(&fx.env, &a).unwrap().is_none());
    drop(locks);
    assert!(AccountLock::try_acquire(&fx.env, &a).unwrap().is_some());
}

#[test]
fn a_pending_replacement_is_reconciled_by_the_next_lock_holder() {
    let fx = Fx::new();
    let store = fx.engine.store().unwrap();
    let id = AccountId::from_string("acc");
    let identity = fx.cc.token_identity("t@token.local");
    store
        .insert_account(&NewAccount {
            id: &id,
            provider: &fx.provider(),
            position: 1,
            identity_key: "t@token.local\n",
            identity: &identity,
            kind: "api_key",
            alias: None,
            login_expires_at: None,
            added_at: 1,
        })
        .unwrap();
    // The replacement installs an OAuth login with new metadata.
    let oauth = fx.cc.parse_identity(&Fx::oauth_account("t@token.local")).unwrap();
    let meta = LoginMeta { identity_key: "t@token.local\n", identity: &oauth, kind: "oauth", login_expires_at: Some(7) };
    // A replacement that died before writing the vault: rolled back, metadata untouched.
    store.begin_replacement(&id, "sha256:never-written", &meta).unwrap();
    drop(fx.engine.lock_account(&id).unwrap());
    let row = store.account(&id).unwrap().unwrap();
    assert_eq!((row.login_epoch, row.replacing_fp, row.kind.as_str()), (0, None, "api_key"));
    // One that wrote the vault and then died: finished, with its kind and metadata.
    let cred = Fx::credential_json("t@token.local", "rt-new").to_string().into_bytes();
    fx.kc.put("tagteam", id.as_str(), &cred);
    let fp = fx.cc.fingerprint(&cred).unwrap();
    store.begin_replacement(&id, fp.as_str(), &meta).unwrap();
    drop(fx.engine.lock_account(&id).unwrap());
    let row = store.account(&id).unwrap().unwrap();
    assert_eq!((row.login_epoch, row.replacing_fp, row.kind.as_str()), (1, None, "oauth"));
    assert_eq!((row.account_uuid.as_deref(), row.login_expires_at), (Some("uuid-t@token.local"), Some(7)));
}

#[test]
fn oracle_verdicts_need_a_positive_uuid_match() {
    let fx = Fx::new();
    let row_for = |uuid: Option<&str>| tagteam_engine::store::AccountRow {
        id: AccountId::from_string("a"),
        provider: fx.provider(),
        position: 1,
        identity_key: "a@b.co\n".into(),
        label: "a@b.co".into(),
        email: Some("a@b.co".into()),
        org_uuid: String::new(),
        org_name: None,
        account_uuid: uuid.map(str::to_owned),
        kind: "oauth".into(),
        alias: None,
        disabled: false,
        identity_json: json!({}),
        login_expires_at: None,
        login_epoch: 0,
        replacing_fp: None,
        quarantine_reason: None,
        quarantine_fp: None,
        added_at: 0,
    };
    let id = |email: &str, uuid: &str| Identity {
        label: email.into(),
        email: Some(email.into()),
        org_uuid: String::new(),
        org_name: None,
        account_uuid: Some(uuid.into()),
        raw: json!({}),
    };
    assert_eq!(verdict(None, &row_for(Some("u"))), OracleVerdict::Unavailable);
    assert_eq!(verdict(Some(&id("a@b.co", "u")), &row_for(Some("u"))), OracleVerdict::ThisAccount);
    // Same email, conflicting uuid: a recycled email is another account.
    assert_eq!(verdict(Some(&id("a@b.co", "v")), &row_for(Some("u"))), OracleVerdict::OtherIdentity);
    // No stored uuid yet: email and org must agree.
    assert_eq!(verdict(Some(&id("a@b.co", "v")), &row_for(None)), OracleVerdict::ThisAccount);
    assert_eq!(verdict(Some(&id("z@b.co", "v")), &row_for(None)), OracleVerdict::OtherIdentity);
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p tagteam-engine --test vault --test engine_basics`
Expected: FAIL to compile.

- [ ] **Step 4: Implement `error.rs`**

```rust
use std::io;

use tagteam_provider::{LockError, ProviderError, ReadError};

use crate::store::StoreError;
use crate::vault::VaultError;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Provider(#[from] ProviderError),
    #[error(transparent)]
    Lock(#[from] LockError),
    #[error(transparent)]
    Vault(#[from] VaultError),
    #[error("{0}")]
    Unreadable(ReadError),
    #[error("unknown provider {0:?}")]
    UnknownProvider(String),
    #[error("this command cannot run inside a `tagteam run` session")]
    InsideRunShell,
    #[error("there is no live login to add; log in with `claude` first")]
    NoLiveLogin,
    #[error("the live login is a managed API key; add it with `tagteam add-token`")]
    LiveApiKey,
    #[error("the Keychain could not be read, so the live credential may be out of date; unlock the Keychain and retry")]
    DegradedRead,
    #[error("the live credential belongs to {found}, not {expected}; refusing to add it")]
    OwnerMismatch { expected: String, found: String },
    #[error("the live login changed while it was being checked; run the command again")]
    LiveMoved,
    #[error("position {position} holds {occupant}; confirm to replace it, or pass --yes")]
    NeedsConfirmation { position: u32, occupant: String },
    #[error("{0}")]
    InvalidInput(String),
    #[error("no account matches {0:?}")]
    NoSuchAccount(String),
    #[error("{input:?} matches several accounts: {}", candidates.join(", "))]
    Ambiguous { input: String, candidates: Vec<String> },
    #[error("an interrupted switch for {0} could not be resolved; run `tagteam switch <account> --force` to settle it")]
    InterruptedSwitch(String),
    #[error("the switch failed and was rolled back: {0}")]
    RolledBack(String),
    #[error("the switch failed ({cause}) and rolling back also failed: {failed}")]
    RollbackFailed { cause: String, failed: String },
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl EngineError {
    /// Stable `error.type` for `--json` output (§14).
    pub fn kind(&self) -> &'static str {
        match self {
            EngineError::Store(_) => "store",
            EngineError::Provider(ProviderError::ConfigUnsplicable(_)) => "config-unsplicable",
            EngineError::Provider(ProviderError::Lock(LockError::Timeout(_))) => "lock-timeout",
            EngineError::Provider(ProviderError::RestoreFailed { .. }) => "rollback-failed",
            EngineError::Provider(_) => "provider",
            EngineError::Lock(LockError::Timeout(_)) => "lock-timeout",
            EngineError::Lock(_) => "lock",
            EngineError::Vault(_) => "vault",
            EngineError::Unreadable(_) => "unreadable",
            EngineError::UnknownProvider(_) => "unknown-provider",
            EngineError::InsideRunShell => "inside-run-shell",
            EngineError::NoLiveLogin => "no-live-login",
            EngineError::LiveApiKey => "live-api-key",
            EngineError::DegradedRead => "degraded-read",
            EngineError::OwnerMismatch { .. } => "owner-mismatch",
            EngineError::LiveMoved => "live-moved",
            EngineError::NeedsConfirmation { .. } => "needs-confirmation",
            EngineError::InvalidInput(_) => "invalid-input",
            EngineError::NoSuchAccount(_) => "no-such-account",
            EngineError::Ambiguous { .. } => "ambiguous-account",
            EngineError::InterruptedSwitch(_) => "interrupted-switch",
            EngineError::RolledBack(_) => "rolled-back",
            EngineError::RollbackFailed { .. } => "rollback-failed",
            EngineError::Io(_) => "io",
        }
    }
}
```

- [ ] **Step 5: Implement `vault.rs` and `account_lock.rs`**

`account_lock.rs`:
```rust
use std::time::Duration;

use tagteam_core::AccountId;
use tagteam_provider::{Env, FlockGuard, LockError};

/// The per-account `flock` every vault writer holds (§6.2). The kernel releases it when the
/// holder exits; it never expires under a suspended holder.
#[derive(Debug)]
pub struct AccountLock {
    _guard: FlockGuard,
    id: AccountId,
}

impl AccountLock {
    pub const WAIT: Duration = Duration::from_secs(15);

    fn path(env: &Env, id: &AccountId) -> std::path::PathBuf {
        env.data_dir().join("locks").join(format!("{id}.lock"))
    }

    pub fn acquire(env: &Env, id: &AccountId, wait: Duration) -> Result<Self, LockError> {
        Ok(Self { _guard: FlockGuard::lock(&Self::path(env, id), wait)?, id: id.clone() })
    }

    pub fn try_acquire(env: &Env, id: &AccountId) -> Result<Option<Self>, LockError> {
        Ok(FlockGuard::try_lock(&Self::path(env, id))?.map(|g| Self { _guard: g, id: id.clone() }))
    }

    pub fn id(&self) -> &AccountId {
        &self.id
    }
}
```

`vault.rs`:
```rust
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use tagteam_core::{AccountId, Fingerprint};
use tagteam_provider::atomic::{ensure_private_dir, write_atomic};
use tagteam_provider::{Keychain, Read, ReadError};

use crate::account_lock::AccountLock;

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("vault write failed: {0}")]
    Write(String),
    #[error("vault delete failed: {0}")]
    Delete(String),
    #[error("{0}")]
    Unreadable(ReadError),
    #[error("the vault entry did not read back as written")]
    Verify,
}

pub trait VaultBackend: Send + Sync {
    fn read(&self, key: &str) -> Read<Vec<u8>>;
    fn write(&self, key: &str, bytes: &[u8]) -> Result<(), VaultError>;
    fn delete(&self, key: &str) -> Result<(), VaultError>;
}

pub const SERVICE: &str = "tagteam";

/// macOS: generic passwords, service `tagteam`, account `<id>` / `<id>.prev` (§6.2).
pub struct KeychainVault {
    keychain: Arc<dyn Keychain>,
}

impl KeychainVault {
    pub fn new(keychain: Arc<dyn Keychain>) -> Self {
        Self { keychain }
    }
}

impl VaultBackend for KeychainVault {
    fn read(&self, key: &str) -> Read<Vec<u8>> {
        self.keychain.find(SERVICE, key)
    }
    fn write(&self, key: &str, bytes: &[u8]) -> Result<(), VaultError> {
        self.keychain.upsert(SERVICE, key, bytes).map_err(|e| VaultError::Write(e.to_string()))
    }
    fn delete(&self, key: &str) -> Result<(), VaultError> {
        self.keychain.delete(SERVICE, key).map_err(|e| VaultError::Delete(e.to_string()))
    }
}

/// Linux: `<dir>/<id>.json` and `<id>.prev.json`, 0600, directory 0700 (§6.2).
pub struct FileVault {
    dir: PathBuf,
}

impl FileVault {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }
    fn path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.json"))
    }
}

impl VaultBackend for FileVault {
    fn read(&self, key: &str) -> Read<Vec<u8>> {
        match fs::read(self.path(key)) {
            Ok(b) => Read::Present(b),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Read::Absent,
            Err(e) => Read::Unreadable(ReadError::new("vault", e.to_string())),
        }
    }
    fn write(&self, key: &str, bytes: &[u8]) -> Result<(), VaultError> {
        ensure_private_dir(&self.dir)
            .and_then(|()| write_atomic(&self.path(key), bytes, 0o600))
            .map_err(|e| VaultError::Write(e.to_string()))
    }
    fn delete(&self, key: &str) -> Result<(), VaultError> {
        match fs::remove_file(self.path(key)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(VaultError::Delete(e.to_string())),
        }
    }
}

/// The source of truth for credential bytes. It never parses what it stores; lineage comes
/// from the caller's fingerprint function.
pub struct Vault {
    backend: Box<dyn VaultBackend>,
}

fn prev_key(id: &AccountId) -> String {
    format!("{id}.prev")
}

impl Vault {
    pub fn new(backend: Box<dyn VaultBackend>) -> Self {
        Self { backend }
    }

    pub fn read(&self, id: &AccountId) -> Read<Vec<u8>> {
        self.backend.read(id.as_str())
    }

    pub fn read_prev(&self, id: &AccountId) -> Read<Vec<u8>> {
        self.backend.read(&prev_key(id))
    }

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

    /// Strict: both generations are deleted, errors propagate, and absence is verified. A
    /// locked Keychain that still holds an item aborts the delete (§6.2).
    pub fn delete(&self, lock: &AccountLock) -> Result<(), VaultError> {
        let id = lock.id();
        for key in [id.to_string(), prev_key(id)] {
            self.backend.delete(&key)?;
            match self.backend.read(&key) {
                Read::Absent => {}
                Read::Present(_) => return Err(VaultError::Delete(format!("{key} is still present"))),
                Read::Unreadable(e) => return Err(VaultError::Delete(format!("could not verify {key}: {e}"))),
            }
        }
        Ok(())
    }
}
```

- [ ] **Step 6: Implement `registry.rs`, `oracle.rs` and `displace.rs`**

`registry.rs`:
```rust
use std::sync::Arc;

use tagteam_core::ProviderId;
use tagteam_provider::Provider;

#[derive(Clone, Default)]
pub struct ProviderRegistry {
    providers: Vec<Arc<dyn Provider>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, p: Arc<dyn Provider>) -> Self {
        self.providers.push(p);
        self
    }

    pub fn get(&self, id: &ProviderId) -> Option<Arc<dyn Provider>> {
        self.providers.iter().find(|p| &p.id() == id).cloned()
    }

    pub fn all(&self) -> &[Arc<dyn Provider>] {
        &self.providers
    }
}
```

`oracle.rs`:
```rust
use tagteam_core::OracleVerdict;
use tagteam_provider::{Credential, Identity, Provider};

use crate::store::AccountRow;

/// Resolves who owns a live access token (§7.6). Advisory only; never called under a lock.
pub trait Oracle: Send + Sync {
    fn resolve(&self, provider: &dyn Provider, credential: &Credential) -> Option<Identity>;
}

/// M1 makes no network calls, so the oracle is always unavailable.
pub struct NoOracle;

impl Oracle for NoOracle {
    fn resolve(&self, _: &dyn Provider, _: &Credential) -> Option<Identity> {
        None
    }
}

/// Attribution to an account needs a positive uuid match; with no stored uuid yet, email and
/// org must agree (then `account_uuid` is backfilled).
pub fn verdict(resolved: Option<&Identity>, account: &AccountRow) -> OracleVerdict {
    let Some(r) = resolved else { return OracleVerdict::Unavailable };
    let same = match (&account.account_uuid, &r.account_uuid) {
        (Some(a), Some(b)) => a == b,
        (None, _) => r.email == account.email && r.org_uuid == account.org_uuid,
        (Some(_), None) => false,
    };
    if same { OracleVerdict::ThisAccount } else { OracleVerdict::OtherIdentity }
}
```

`displace.rs`:
```rust
use serde_json::Value;
use tagteam_core::{Fingerprint, ProviderId};
use tagteam_provider::atomic::{ensure_private_dir, write_atomic};

use crate::engine::Engine;
use crate::error::EngineError;
use crate::store::DisplacedRow;

/// Stashes live credential bytes that are about to be overwritten (§6.3). Forensic and
/// write-only; always a plain 0600 file, never a Keychain item.
pub(crate) fn displace(
    engine: &Engine,
    provider: &ProviderId,
    bytes: &[u8],
    fp: Option<&Fingerprint>,
    reason: &str,
    identity: Option<&Value>,
) -> Result<String, EngineError> {
    let dir = engine.env().data_dir().join("displaced");
    ensure_private_dir(&dir)?;
    let now = engine.now_ms();
    let fp12 = fp.map_or_else(|| "000000000000".to_owned(), |f| f.short12().to_owned());
    let rand6: String = (0..6).map(|_| fastrand::alphanumeric().to_ascii_lowercase()).collect();
    let id = format!("{}-{fp12}-{rand6}", now / 1000);
    write_atomic(&dir.join(format!("{id}.json")), bytes, 0o600)?;
    engine.store()?.insert_displaced(&DisplacedRow {
        id: id.clone(),
        provider: provider.clone(),
        at: now,
        reason: reason.to_owned(),
        fingerprint: fp.map(|f| f.as_str().to_owned()).unwrap_or_default(),
        identity: identity.cloned(),
    })?;
    Ok(id)
}
```

- [ ] **Step 7: Implement `engine.rs`**

```rust
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tagteam_core::{AccountId, ProviderId};
use tagteam_provider::{Clock, Env, MutationGuard, Provider, Read};

use crate::account_lock::AccountLock;
use crate::error::EngineError;
use crate::oracle::Oracle;
use crate::registry::ProviderRegistry;
use crate::store::Store;
use crate::vault::Vault;

pub struct EngineConfig {
    pub env: Env,
    pub registry: ProviderRegistry,
    pub vault: Vault,
    pub oracle: Arc<dyn Oracle>,
    pub clock: Arc<dyn Clock>,
    pub default_provider: ProviderId,
}

pub struct Engine {
    pub(crate) env: Env,
    pub(crate) registry: ProviderRegistry,
    pub(crate) vault: Vault,
    pub(crate) oracle: Arc<dyn Oracle>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) default_provider: ProviderId,
    store: Mutex<Option<Arc<Store>>>,
}

impl Engine {
    pub fn new(cfg: EngineConfig) -> Self {
        Self {
            env: cfg.env,
            registry: cfg.registry,
            vault: cfg.vault,
            oracle: cfg.oracle,
            clock: cfg.clock,
            default_provider: cfg.default_provider,
            store: Mutex::new(None),
        }
    }

    pub fn env(&self) -> &Env {
        &self.env
    }

    pub fn default_provider(&self) -> &ProviderId {
        &self.default_provider
    }

    pub fn now_ms(&self) -> i64 {
        self.clock.now_ms()
    }

    fn store_path(&self) -> PathBuf {
        self.env.data_dir().join("tagteam.db")
    }

    /// Opens the store, creating it (and its directory, 0700) when needed.
    pub fn store(&self) -> Result<Arc<Store>, EngineError> {
        let mut slot = self.store.lock().unwrap();
        if let Some(s) = slot.as_ref() {
            return Ok(s.clone());
        }
        let s = Arc::new(Store::open(&self.store_path())?);
        *slot = Some(s.clone());
        Ok(s)
    }

    /// For read-only commands: never creates anything (§5).
    pub fn existing_store(&self) -> Result<Option<Arc<Store>>, EngineError> {
        let mut slot = self.store.lock().unwrap();
        if let Some(s) = slot.as_ref() {
            return Ok(Some(s.clone()));
        }
        match Store::open_existing(&self.store_path())? {
            Some(s) => {
                let s = Arc::new(s);
                *slot = Some(s.clone());
                Ok(Some(s))
            }
            None => Ok(None),
        }
    }

    pub fn provider(&self, id: &ProviderId) -> Result<Arc<dyn Provider>, EngineError> {
        self.registry.get(id).ok_or_else(|| EngineError::UnknownProvider(id.to_string()))
    }

    pub fn providers(&self) -> Vec<Arc<dyn Provider>> {
        self.registry.all().to_vec()
    }

    pub(crate) fn refuse_inside_run_shell(&self) -> Result<(), EngineError> {
        if self.env.inside_run_shell() { Err(EngineError::InsideRunShell) } else { Ok(()) }
    }

    /// Account-changing work refuses while an interrupted switch for the provider is
    /// unresolved (§9.6). Never creates the store.
    pub(crate) fn refuse_if_interrupted(&self, provider: &ProviderId) -> Result<(), EngineError> {
        match self.existing_store()? {
            Some(s) if s.journal(provider)?.is_some() => Err(EngineError::InterruptedSwitch(provider.to_string())),
            _ => Ok(()),
        }
    }

    /// Run before any planning or validation: if an interrupted switch is on record, take the
    /// mutation lock once (which recovers what it can, Task 21), then refuse if it is still
    /// unresolved. Creates nothing when there is no store.
    pub(crate) fn settle_or_refuse(&self, provider: &ProviderId) -> Result<(), EngineError> {
        let pending = match self.existing_store()? {
            Some(s) => s.journal(provider)?.is_some(),
            None => false,
        };
        if pending {
            drop(self.mutation_guard()?);
            self.refuse_if_interrupted(provider)?;
        }
        Ok(())
    }

    /// tagteam's mutation lock. Task 21 adds interrupted-switch recovery here.
    pub fn mutation_guard(&self) -> Result<MutationGuard, EngineError> {
        Ok(MutationGuard::acquire(&self.env, MutationGuard::TIMEOUT)?)
    }

    /// Takes the account lock, then reconciles a pending explicit replacement (§12.5): the
    /// replacer held this lock throughout, so finding its marker means it died.
    pub fn lock_account(&self, id: &AccountId) -> Result<AccountLock, EngineError> {
        let lock = AccountLock::acquire(&self.env, id, AccountLock::WAIT)?;
        self.reconcile_replacement(&lock)?;
        Ok(lock)
    }

    pub fn lock_accounts(&self, ids: &[&AccountId]) -> Result<Vec<AccountLock>, EngineError> {
        let mut sorted: Vec<&AccountId> = ids.to_vec();
        sorted.sort();
        sorted.dedup();
        sorted.into_iter().map(|id| self.lock_account(id)).collect()
    }

    fn reconcile_replacement(&self, lock: &AccountLock) -> Result<(), EngineError> {
        let Some(store) = self.existing_store()? else { return Ok(()) };
        let Some(row) = store.account(lock.id())? else { return Ok(()) };
        let Some(fp) = row.replacing_fp else { return Ok(()) };
        let provider = self.provider(&row.provider)?;
        match self.vault.read(lock.id()) {
            Read::Present(b) if provider.fingerprint(&b).is_some_and(|f| f.as_str() == fp) => {
                store.finish_replacement(lock.id())?
            }
            Read::Present(_) | Read::Absent => store.rollback_replacement(lock.id())?,
            Read::Unreadable(e) => return Err(EngineError::Unreadable(e)),
        }
        Ok(())
    }
}
```

`crates/tagteam-engine/src/lib.rs`:
```rust
#![forbid(unsafe_code)]

pub mod account_lock;
mod displace;
pub mod engine;
pub mod error;
pub mod oracle;
pub mod registry;
pub mod store;
pub mod vault;

pub use engine::{Engine, EngineConfig};
pub use error::EngineError;
```

- [ ] **Step 8: Run the tests to verify they pass**

Run: `cargo test -p tagteam-engine`
Expected: PASS. Dead-code warnings for `displace`, `refuse_inside_run_shell`,
`refuse_if_interrupted` and `settle_or_refuse` are expected until Tasks 18 and 20 use them.

- [ ] **Step 9: Commit**

```bash
git add crates/tagteam-engine
git commit -m "Add the engine skeleton, vault and account locks"
```

---
### Task 18: `add` and `add-token`

**Files:**
- Create: `crates/tagteam-engine/src/lifecycle.rs`
- Modify: `crates/tagteam-engine/src/lib.rs` (`pub mod lifecycle;`)
- Test: `crates/tagteam-engine/tests/add.rs`

**Interfaces:**
- Consumes: `Engine`, `Store`, `Vault`, `Provider`, `Oracle`, `normalize_alias`, `is_valid_email`
- Produces:
```rust
pub struct AddOptions { pub provider: ProviderId, pub position: Option<u32>, pub alias: Option<String>, pub yes: bool }
pub struct AddTokenOptions { pub provider: ProviderId, pub token: String, pub position: Option<u32>,
    pub email: Option<String>, pub alias: Option<String>, pub yes: bool }
pub struct AddOutcome { pub account: AccountRow, pub created: bool, pub notices: Vec<String> }
impl Engine {
    pub fn add_live(&self, opts: AddOptions) -> Result<AddOutcome, EngineError>;
    pub fn add_token(&self, opts: AddTokenOptions) -> Result<AddOutcome, EngineError>;
    pub(crate) fn remove_locked(&self, row: &AccountRow, lock: &AccountLock) -> Result<(), EngineError>;
        // caller holds MutationGuard and the account's lock
    pub(crate) fn event(&self, provider: &ProviderId, kind: &str, from: Option<&AccountId>, to: Option<&AccountId>)
        -> Result<(), EngineError>;
}
pub(crate) fn check_position(position: u32, max_existing: u32) -> Result<(), EngineError>;  // 1..=max(99, max)
pub(crate) fn same_owner(a: &Identity, b: &Identity) -> bool;   // uuid first, org corroborating
```

- [ ] **Step 1: Write the failing tests**

`crates/tagteam-engine/tests/add.rs`:
```rust
mod common;

use common::Fx;
use serde_json::json;
use tagteam_cc::{ItemKind, keychain_account, keychain_service};
use tagteam_engine::EngineError;
use tagteam_engine::lifecycle::{AddOptions, AddTokenOptions};
use tagteam_provider::Provider;

fn add_opts(fx: &Fx) -> AddOptions {
    AddOptions { provider: fx.provider(), position: None, alias: None, yes: false }
}

fn token_opts(fx: &Fx, token: &str) -> AddTokenOptions {
    AddTokenOptions { provider: fx.provider(), token: token.into(), position: None, email: None, alias: None, yes: false }
}

#[test]
fn add_captures_the_live_login() {
    let fx = Fx::new();
    fx.login("me@work.co", "rt-1");
    let out = fx.engine.add_live(AddOptions { alias: Some("Work".into()), ..add_opts(&fx) }).unwrap();
    let a = &out.account;
    assert!(out.created);
    assert_eq!((a.position, a.label.as_str(), a.kind.as_str()), (1, "me@work.co", "oauth"));
    assert_eq!(a.alias.as_deref(), Some("work"));
    assert_eq!(a.account_uuid.as_deref(), Some("uuid-me@work.co"));
    assert_eq!(a.login_expires_at, Some(1_797_000_000_000));
    assert_eq!(fx.vault_refresh_token(&a.id).as_deref(), Some("rt-1"));
    assert_eq!(fx.engine.store().unwrap().active(&fx.provider()).unwrap(), Some(a.id.clone()));
    assert!(out.notices.iter().any(|n| n.contains("could not verify")), "M1 has no oracle");
}

#[test]
fn adding_the_same_login_again_refreshes_it_in_place() {
    let fx = Fx::new();
    fx.login("me@work.co", "rt-1");
    let first = fx.engine.add_live(add_opts(&fx)).unwrap().account;
    fx.rotate_live("rt-2");
    let again = fx.engine.add_live(add_opts(&fx)).unwrap();
    assert!(!again.created);
    assert_eq!(again.account.id, first.id);
    assert_eq!(again.account.login_epoch, first.login_epoch + 1);
    assert_eq!(fx.vault_refresh_token(&first.id).as_deref(), Some("rt-2"));
    let prev = fx.kc.get("tagteam", &format!("{}.prev", first.id)).unwrap();
    assert!(String::from_utf8(prev).unwrap().contains("rt-1"));
}

#[test]
fn add_refuses_what_it_cannot_safely_capture() {
    let fx = Fx::new();
    assert!(matches!(fx.engine.add_live(add_opts(&fx)), Err(EngineError::NoLiveLogin)));

    fx.login("me@work.co", "rt-1");
    let (svc, acct) = (keychain_service(&fx.env, ItemKind::OAuth), keychain_account(&fx.env));
    fx.kc.set_unreadable(&svc, &acct, true);
    std::fs::write(fx.paths().credentials_file, Fx::credential_json("me@work.co", "rt-0").to_string()).unwrap();
    assert!(matches!(fx.engine.add_live(add_opts(&fx)), Err(EngineError::DegradedRead)));
    fx.kc.set_unreadable(&svc, &acct, false);

    fx.kc.put(&keychain_service(&fx.env, ItemKind::ManagedKey), &acct, b"sk-ant-api03-live");
    assert!(matches!(fx.engine.add_live(add_opts(&fx)), Err(EngineError::LiveApiKey)));
    assert!(fx.engine.existing_store().unwrap().is_none_or(|s| s.accounts(&fx.provider()).unwrap().is_empty()));
}

#[test]
fn an_oracle_naming_someone_else_refuses_the_add() {
    let fx = Fx::new();
    fx.login("me@work.co", "rt-1");
    let other = fx.cc.parse_identity(&json!({"emailAddress": "other@x.co", "accountUuid": "uuid-other"})).unwrap();
    fx.oracle.set(Some(other));
    assert!(matches!(fx.engine.add_live(add_opts(&fx)), Err(EngineError::OwnerMismatch { .. })));
    let me = fx.cc.parse_identity(&Fx::oauth_account("me@work.co")).unwrap();
    fx.oracle.set(Some(me));
    let out = fx.engine.add_live(add_opts(&fx)).unwrap();
    assert!(out.notices.is_empty());
}

#[test]
fn an_occupied_position_needs_confirmation_and_then_replaces_its_occupant() {
    let fx = Fx::new();
    fx.login("a@x.co", "rt-a");
    let a = fx.engine.add_live(add_opts(&fx)).unwrap().account;
    fx.login("b@x.co", "rt-b");
    let at1 = AddOptions { position: Some(1), ..add_opts(&fx) };
    assert!(matches!(fx.engine.add_live(at1), Err(EngineError::NeedsConfirmation { position: 1, .. })));
    let b = fx.engine.add_live(AddOptions { position: Some(1), yes: true, ..add_opts(&fx) }).unwrap().account;
    assert_eq!(b.position, 1);
    assert!(fx.engine.store().unwrap().account(&a.id).unwrap().is_none());
    assert!(fx.vault_bytes(&a.id).is_none());
}

#[test]
fn a_replacement_that_cannot_be_written_keeps_its_occupant() {
    let fx = Fx::new();
    fx.login("a@x.co", "rt-a");
    let a = fx.engine.add_live(add_opts(&fx)).unwrap().account;
    fx.login("b@x.co", "rt-b");
    fx.engine.add_live(AddOptions { alias: Some("work".into()), ..add_opts(&fx) }).unwrap();
    fx.login("c@x.co", "rt-c");
    // The alias belongs to someone else: refused before position 1 is touched.
    let clash = AddOptions { position: Some(1), yes: true, alias: Some("work".into()), ..add_opts(&fx) };
    assert!(matches!(fx.engine.add_live(clash), Err(EngineError::InvalidInput(_))));
    // The vault refuses the new credential: the occupant survives.
    fx.kc.set_fail_write("tagteam", true);
    assert!(fx.engine.add_live(AddOptions { position: Some(1), yes: true, ..add_opts(&fx) }).is_err());
    fx.kc.set_fail_write("tagteam", false);
    let store = fx.engine.store().unwrap();
    assert_eq!(store.account(&a.id).unwrap().unwrap().position, 1);
    assert_eq!(fx.vault_refresh_token(&a.id).as_deref(), Some("rt-a"));
    assert_eq!(store.accounts(&fx.provider()).unwrap().len(), 2);
}

#[test]
fn an_unreadable_managed_key_refuses_the_add() {
    let fx = Fx::new();
    fx.login("me@work.co", "rt-1");
    let (svc, acct) = (keychain_service(&fx.env, ItemKind::ManagedKey), keychain_account(&fx.env));
    fx.kc.put(&svc, &acct, b"sk-ant-api03-live");
    fx.kc.set_unreadable(&svc, &acct, true);
    assert!(matches!(fx.engine.add_live(add_opts(&fx)), Err(EngineError::Unreadable(_))));
}

#[test]
fn adding_a_login_updates_an_account_of_another_kind_in_place() {
    // §10.1: an existing (email, org) is refreshed in place, its kind included.
    let fx = Fx::new();
    let token = AddTokenOptions { email: Some("me@work.co".into()), ..token_opts(&fx, "sk-ant-api03-k") };
    let before = fx.engine.add_token(token).unwrap().account;
    fx.login("me@work.co", "rt-1");
    let after = fx.engine.add_live(add_opts(&fx)).unwrap();
    assert!(!after.created);
    assert_eq!((after.account.id, after.account.kind.as_str()), (before.id, "oauth"));
    assert_eq!(after.account.account_uuid.as_deref(), Some("uuid-me@work.co"));
}

#[test]
fn add_token_rechecks_the_kind_after_a_pending_replacement_lands() {
    let fx = Fx::new();
    let token = |t: &str| AddTokenOptions { email: Some("me@work.co".into()), ..token_opts(&fx, t) };
    let x = fx.engine.add_token(token("sk-ant-api03-first")).unwrap().account;
    // An `add` over x with an OAuth login wrote the vault and died before its metadata landed.
    let cred = Fx::credential_json("me@work.co", "rt-1").to_string().into_bytes();
    fx.kc.put("tagteam", x.id.as_str(), &cred);
    let identity = fx.cc.parse_identity(&Fx::oauth_account("me@work.co")).unwrap();
    let meta = tagteam_engine::store::LoginMeta { identity_key: "me@work.co\n", identity: &identity, kind: "oauth", login_expires_at: None };
    fx.engine.store().unwrap().begin_replacement(&x.id, fx.cc.fingerprint(&cred).unwrap().as_str(), &meta).unwrap();
    assert!(matches!(fx.engine.add_token(token("sk-ant-api03-second")), Err(EngineError::InvalidInput(m)) if m.contains("oauth")));
    assert_eq!(fx.vault_refresh_token(&x.id).as_deref(), Some("rt-1"), "the recovered OAuth credential stays");
}

#[test]
fn a_landed_replacement_can_make_add_token_valid() {
    let fx = Fx::new();
    fx.login("me@work.co", "rt-1");
    let x = fx.engine.add_live(add_opts(&fx)).unwrap().account; // an OAuth account
    // An `add` over x with a setup-token login wrote the vault and died before its metadata.
    let setup = br#"{"claudeAiOauth":{"accessToken":"sk-ant-oat01-old","scopes":["user:inference"]}}"#;
    fx.kc.put("tagteam", x.id.as_str(), setup);
    let identity = fx.cc.parse_identity(&Fx::oauth_account("me@work.co")).unwrap();
    let meta = tagteam_engine::store::LoginMeta { identity_key: "me@work.co\n", identity: &identity, kind: "setup_token", login_expires_at: None };
    fx.engine.store().unwrap().begin_replacement(&x.id, fx.cc.fingerprint(setup).unwrap().as_str(), &meta).unwrap();
    let token = AddTokenOptions { email: Some("me@work.co".into()), ..token_opts(&fx, "sk-ant-oat01-new") };
    let out = fx.engine.add_token(token).unwrap();
    assert_eq!((out.account.id, out.account.kind.as_str()), (x.id, "setup_token"));
}

#[test]
fn an_out_of_range_position_creates_nothing() {
    let fx = Fx::new();
    fx.login("me@work.co", "rt-1");
    assert!(fx.engine.add_live(AddOptions { position: Some(100), ..add_opts(&fx) }).is_err());
    assert!(!fx.env.data_dir().exists());
}

#[test]
fn invalid_token_input_creates_nothing() {
    let fx = Fx::new();
    let bad_email = AddTokenOptions { email: Some("nope".into()), ..token_opts(&fx, "sk-ant-api03-x") };
    assert!(fx.engine.add_token(bad_email).is_err());
    let zero = AddTokenOptions { position: Some(0), ..token_opts(&fx, "sk-ant-api03-x") };
    assert!(fx.engine.add_token(zero).is_err());
    assert!(!fx.env.data_dir().exists(), "a command that changes nothing creates nothing");
}

#[test]
fn add_token_stores_api_keys_and_setup_tokens() {
    let fx = Fx::new();
    let k = fx.engine.add_token(token_opts(&fx, "  sk-ant-api03-abc\n")).unwrap().account;
    assert_eq!((k.kind.as_str(), k.label.as_str()), ("api_key", "api-key-1@token.local"));
    assert_eq!(fx.vault_bytes(&k.id).unwrap(), b"sk-ant-api03-abc");
    let s = fx.engine.add_token(token_opts(&fx, "sk-ant-oat01-setup")).unwrap().account;
    assert_eq!((s.kind.as_str(), s.label.as_str()), ("setup_token", "setup-token-2@token.local"));
    let v: serde_json::Value = serde_json::from_slice(&fx.vault_bytes(&s.id).unwrap()).unwrap();
    assert_eq!(v, json!({"claudeAiOauth": {"accessToken": "sk-ant-oat01-setup", "scopes": ["user:inference"]}}));
    assert_eq!(fx.engine.store().unwrap().active(&fx.provider()).unwrap(), None);
}

#[test]
fn add_token_validates_its_inputs() {
    let fx = Fx::new();
    assert!(matches!(fx.engine.add_token(token_opts(&fx, "  ")), Err(EngineError::InvalidInput(_))));
    let bad_email = AddTokenOptions { email: Some("not-an-email".into()), ..token_opts(&fx, "sk-ant-api03-x") };
    assert!(matches!(fx.engine.add_token(bad_email), Err(EngineError::InvalidInput(_))));
    let email = Some("shared@x.co".to_string());
    fx.engine.add_token(AddTokenOptions { email: email.clone(), ..token_opts(&fx, "sk-ant-api03-x") }).unwrap();
    let clash = AddTokenOptions { email, ..token_opts(&fx, "sk-ant-oat01-y") };
    assert!(matches!(fx.engine.add_token(clash), Err(EngineError::InvalidInput(m)) if m.contains("api_key")));
    let bad_alias = AddTokenOptions { alias: Some("123".into()), ..token_opts(&fx, "sk-ant-api03-z") };
    assert!(matches!(fx.engine.add_token(bad_alias), Err(EngineError::InvalidInput(_))));
    let zero = AddTokenOptions { position: Some(0), ..token_opts(&fx, "sk-ant-api03-z") };
    assert!(matches!(fx.engine.add_token(zero), Err(EngineError::InvalidInput(_))));
}

#[test]
fn account_commands_refuse_inside_a_run_shell() {
    let fx = Fx::new();
    fx.login("me@work.co", "rt-1");
    let mut env = fx.env.clone();
    env.claude_config_dir = Some(fx.env.data_dir().join("sessions/x").into_os_string());
    let engine = fx.engine_with_env(env);
    assert!(matches!(engine.add_live(add_opts(&fx)), Err(EngineError::InsideRunShell)));
    assert!(matches!(engine.add_token(token_opts(&fx, "sk-ant-api03-x")), Err(EngineError::InsideRunShell)));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tagteam-engine --test add`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

`crates/tagteam-engine/src/lifecycle.rs`:
```rust
use tagteam_core::validate::{is_valid_email, normalize_alias};
use tagteam_core::{AccountId, ProviderId};
use tagteam_provider::{Credential, Identity, Provenance, Provider, Read};

use crate::account_lock::AccountLock;
use crate::engine::Engine;
use crate::error::EngineError;
use crate::store::{AccountRow, EventRow, LoginMeta, NewAccount, Store, StoreError};

pub struct AddOptions {
    pub provider: ProviderId,
    pub position: Option<u32>,
    pub alias: Option<String>,
    pub yes: bool,
}

pub struct AddTokenOptions {
    pub provider: ProviderId,
    pub token: String,
    pub position: Option<u32>,
    pub email: Option<String>,
    pub alias: Option<String>,
    pub yes: bool,
}

#[derive(Debug)]
pub struct AddOutcome {
    pub account: AccountRow,
    pub created: bool,
    pub notices: Vec<String>,
}

pub(crate) fn alias_arg(alias: Option<&str>) -> Result<Option<String>, EngineError> {
    alias.map(normalize_alias).transpose().map_err(|e| EngineError::InvalidInput(e.to_string()))
}

/// A move or add target must be within 1..=max(99, max(position)) (§6.1 Positions).
pub(crate) fn check_position(position: u32, max_existing: u32) -> Result<(), EngineError> {
    let limit = max_existing.max(99);
    if position == 0 || position > limit {
        return Err(EngineError::InvalidInput(format!("positions run from 1 to {limit}")));
    }
    Ok(())
}

/// Uuid first, org corroborating (§10.1 guard 2).
pub(crate) fn same_owner(a: &Identity, b: &Identity) -> bool {
    match (&a.account_uuid, &b.account_uuid) {
        (Some(x), Some(y)) => x == y && a.org_uuid == b.org_uuid,
        _ => a.email == b.email && a.org_uuid == b.org_uuid,
    }
}

/// The managed-key guard (§10.1 guard 1), with all three read states.
fn no_live_api_key(r: &Read<Vec<u8>>) -> Result<(), EngineError> {
    match r {
        Read::Present(_) => Err(EngineError::LiveApiKey),
        Read::Absent => Ok(()),
        Read::Unreadable(e) => Err(EngineError::Unreadable(e.clone())),
    }
}

fn live_fresh(r: Read<Credential>) -> Result<Credential, EngineError> {
    match r {
        Read::Present(c) if c.provenance() == Provenance::Degraded => Err(EngineError::DegradedRead),
        Read::Present(c) if c.is_empty() => Err(EngineError::NoLiveLogin),
        Read::Present(c) => Ok(c),
        Read::Absent => Err(EngineError::NoLiveLogin),
        Read::Unreadable(e) => Err(EngineError::Unreadable(e)),
    }
}

fn alias_taken(e: StoreError) -> EngineError {
    match e {
        StoreError::AliasTaken(a) => EngineError::InvalidInput(format!("the alias {a:?} is already taken")),
        other => other.into(),
    }
}

/// What a login write will replace, decided before anything is mutated.
struct Prepared {
    existing: Option<AccountRow>,
    occupant: Option<AccountRow>,
    id: AccountId,
}

impl Engine {
    pub(crate) fn event(
        &self,
        provider: &ProviderId,
        kind: &str,
        from: Option<&AccountId>,
        to: Option<&AccountId>,
    ) -> Result<(), EngineError> {
        self.store()?.insert_event(&EventRow {
            at: self.now_ms(),
            provider: provider.clone(),
            kind: kind.to_owned(),
            from_id: from.cloned(),
            to_id: to.cloned(),
            trigger: None,
            source: "cli".into(),
            detail: None,
        })?;
        Ok(())
    }

    /// Deletes the vault entries (strict), then the row (which cascades). The caller holds the
    /// mutation lock and this account's lock. The live login is never touched.
    pub(crate) fn remove_locked(&self, row: &AccountRow, lock: &AccountLock) -> Result<(), EngineError> {
        self.vault.delete(lock)?;
        self.store()?.delete_account(&row.id)?;
        self.event(&row.provider, "remove", Some(&row.id), None)?;
        Ok(())
    }

    /// Pure validation, no mutation: which account is written, which occupant it would
    /// replace, and whether that is allowed.
    #[allow(clippy::too_many_arguments)]
    fn prepare(
        &self,
        store: &Store,
        p: &dyn Provider,
        provider: &ProviderId,
        identity: &Identity,
        position: Option<u32>,
        alias: Option<&str>,
        yes: bool,
    ) -> Result<Prepared, EngineError> {
        let existing = store.find_by_identity_key(provider, p.identity_key(identity).as_str())?;
        let mut occupant = None;
        if let Some(pos) = position {
            check_position(pos, store.next_position(provider)?.saturating_sub(1))?;
            occupant = store.find_by_position(provider, pos)?.filter(|o| existing.as_ref().map(|e| &e.id) != Some(&o.id));
            if let (Some(o), false) = (&occupant, yes) {
                return Err(EngineError::NeedsConfirmation { position: pos, occupant: o.label.clone() });
            }
        }
        if let Some(a) = alias {
            if let Some(owner) = store.find_by_alias(a)? {
                let freed = [existing.as_ref(), occupant.as_ref()].into_iter().flatten().any(|r| r.id == owner.id);
                if !freed {
                    return Err(EngineError::InvalidInput(format!("the alias {a:?} is already taken")));
                }
            }
        }
        let id = match &existing {
            Some(e) => e.id.clone(),
            None => AccountId::from_string(uuid::Uuid::now_v7().to_string()),
        };
        Ok(Prepared { existing, occupant, id })
    }

    fn lock_prepared(&self, prep: &Prepared) -> Result<Vec<AccountLock>, EngineError> {
        let mut ids = vec![&prep.id];
        if let Some(o) = &prep.occupant {
            ids.push(&o.id);
        }
        self.lock_accounts(&ids)
    }

    /// Writes the login under the locks `prep` names. The replacement is persisted before the
    /// occupant it displaces is removed, so a failure never loses the occupant.
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
    ) -> Result<(AccountRow, bool), EngineError> {
        let lock_for = |id: &AccountId| locks.iter().find(|l| l.id() == id).expect("every written account is locked");
        let fp = |b: &[u8]| p.fingerprint(b);
        let key = p.identity_key(identity);
        match &prep.existing {
            Some(row) => {
                // The metadata travels with the marker: if this process dies after the vault
                // write, the next lock holder installs it (§12.5).
                let new_fp = p.fingerprint(secret).map(|f| f.as_str().to_owned()).unwrap_or_default();
                let meta = LoginMeta { identity_key: key.as_str(), identity, kind, login_expires_at: p.login_expires_at(secret) };
                store.begin_replacement(&row.id, &new_fp, &meta)?;
                self.vault.store(lock_for(&row.id), secret, &fp)?;
                store.finish_replacement(&row.id)?;
            }
            None => {
                // At a free position first; it moves once any occupant is gone.
                store.insert_account(&NewAccount {
                    id: &prep.id,
                    provider,
                    position: store.next_position(provider)?,
                    identity_key: key.as_str(),
                    identity,
                    kind,
                    alias: None,
                    login_expires_at: p.login_expires_at(secret),
                    added_at: self.now_ms(),
                })?;
                if let Err(e) = self.vault.store(lock_for(&prep.id), secret, &fp) {
                    store.delete_account(&prep.id)?;
                    return Err(e.into());
                }
            }
        }
        if let Some(occupant) = &prep.occupant {
            self.remove_locked(occupant, lock_for(&occupant.id))?;
        }
        let current = store.account(&prep.id)?.ok_or(StoreError::NoSuchAccount)?;
        if let Some(pos) = position.filter(|pos| *pos != current.position) {
            store.move_to(&prep.id, pos)?;
        }
        if alias.is_some() {
            store.set_alias(&prep.id, alias).map_err(alias_taken)?;
        }
        self.event(provider, "add", None, Some(&prep.id))?;
        Ok((store.account(&prep.id)?.ok_or(StoreError::NoSuchAccount)?, prep.existing.is_none()))
    }

    /// §10.1: captures the live login.
    pub fn add_live(&self, opts: AddOptions) -> Result<AddOutcome, EngineError> {
        self.refuse_inside_run_shell()?;
        self.settle_or_refuse(&opts.provider)?;
        let p = self.provider(&opts.provider)?;
        let alias = alias_arg(opts.alias.as_deref())?;
        if let Some(pos) = opts.position {
            // Validated before anything exists (§5); `prepare` re-checks under the lock.
            let max = match self.existing_store()? {
                Some(s) => s.next_position(&opts.provider)?.saturating_sub(1),
                None => 0,
            };
            check_position(pos, max)?;
        }
        // 1. The live identity, read once.
        let identity = match p.live_identity(&self.env) {
            Read::Present(i) => i,
            Read::Absent => return Err(EngineError::NoLiveLogin),
            Read::Unreadable(e) => return Err(EngineError::Unreadable(e)),
        };
        // 2. The live credential, from the store CC would use; a degraded read is refused.
        let auth = p.read_live_auth(&self.env);
        no_live_api_key(&auth.managed_key)?;
        let cred = live_fresh(auth.credential)?;
        // 3.2 Ownership, advisory and before any lock: never refreshes, never blocks on failure.
        let mut notices = Vec::new();
        match self.oracle.resolve(p.as_ref(), &cred) {
            Some(owner) if !same_owner(&owner, &identity) => {
                return Err(EngineError::OwnerMismatch { expected: identity.label, found: owner.label });
            }
            Some(_) => {}
            None => notices.push(format!("could not verify that the live credential belongs to {}", identity.label)),
        }
        let kind = p.classify(cred.bytes());
        // 4. Write, under the mutation lock, the account locks and then CC's live locks.
        let guard = self.mutation_guard()?;
        self.refuse_if_interrupted(&opts.provider)?;
        let store = self.store()?;
        let prep = self.prepare(&store, p.as_ref(), &opts.provider, &identity, opts.position, alias.as_deref(), opts.yes)?;
        let accounts = self.lock_prepared(&prep)?;
        let live_locks = p.lock_live(&self.env, &guard)?;
        // 3.3 The capture must be the login verified above, on both auth axes: a switch or a
        // recovery may have moved it while this command waited for the locks.
        // The complete identity is compared, not just its key: an `accountUuid` or any other
        // `oauthAccount` field that changed means this is not the login that was verified.
        let now_identity = p.live_identity(&self.env).present();
        let now_auth = p.read_live_auth(&self.env);
        no_live_api_key(&now_auth.managed_key)?;
        let now = live_fresh(now_auth.credential)?;
        if now_identity.as_ref().map(|i| &i.raw) != Some(&identity.raw) || p.fingerprint(now.bytes()) != p.fingerprint(cred.bytes()) {
            return Err(EngineError::LiveMoved);
        }
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
        store.set_active(&opts.provider, Some(&account.id))?;
        Ok(AddOutcome { account, created, notices })
    }

    /// §10.2: stores a token without touching the live login. No network calls. Everything
    /// that can be checked without the lock is checked before anything is created (§5).
    pub fn add_token(&self, opts: AddTokenOptions) -> Result<AddOutcome, EngineError> {
        self.refuse_inside_run_shell()?;
        self.settle_or_refuse(&opts.provider)?;
        let p = self.provider(&opts.provider)?;
        let alias = alias_arg(opts.alias.as_deref())?;
        if opts.token.trim().is_empty() {
            return Err(EngineError::InvalidInput("the token is empty".into()));
        }
        let (kind, secret) = p.token_secret(&opts.token);
        let prefix = if kind == "api_key" { "api-key" } else { "setup-token" };
        let email_for = |position: u32| opts.email.clone().unwrap_or_else(|| format!("{prefix}-{position}@token.local"));
        let next = match self.existing_store()? {
            Some(s) => s.next_position(&opts.provider)?,
            None => 1,
        };
        if let Some(pos) = opts.position {
            check_position(pos, next.saturating_sub(1))?;
        }
        let email = email_for(opts.position.unwrap_or(next));
        if !is_valid_email(&email) {
            return Err(EngineError::InvalidInput(format!("{email:?} is not a valid email address")));
        }
        let _guard = self.mutation_guard()?;
        self.refuse_if_interrupted(&opts.provider)?;
        let store = self.store()?;
        // Under the lock the position is authoritative, so a default email follows it.
        let position = match opts.position {
            Some(pos) => pos,
            None => store.next_position(&opts.provider)?,
        };
        let identity = p.token_identity(&email_for(position));
        let prep = self.prepare(&store, p.as_ref(), &opts.provider, &identity, opts.position, alias.as_deref(), opts.yes)?;
        let accounts = self.lock_prepared(&prep)?;
        // The different-kind collision (§10.2) is decided only now: taking the account lock
        // may have finished a pending replacement and changed the account's kind.
        if let Some(e) = &prep.existing {
            if let Some(existing) = store.account(&e.id)? {
                if existing.kind != kind {
                    return Err(EngineError::InvalidInput(format!(
                        "{} is already stored as a {} account",
                        identity.label, existing.kind
                    )));
                }
            }
        }
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
        )?;
        Ok(AddOutcome { account, created, notices: vec![] })
    }
}
```

Add `pub mod lifecycle;` to `lib.rs`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p tagteam-engine --test add`
Expected: PASS (8 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/tagteam-engine
git commit -m "Add the add and add-token operations"
```

---

### Task 19: Account references, management commands and views

**Files:**
- Create: `crates/tagteam-engine/src/refs.rs`, `crates/tagteam-engine/src/views.rs`
- Modify: `crates/tagteam-engine/src/lifecycle.rs`, `crates/tagteam-engine/src/lib.rs`
- Test: `crates/tagteam-engine/tests/manage.rs`

**Interfaces:**
- Produces:
```rust
impl Engine {
    pub fn candidates(&self, input: &str, provider: Option<&ProviderId>) -> Result<Vec<AccountRow>, EngineError>;
    pub fn resolve(&self, input: &str, provider: Option<&ProviderId>) -> Result<AccountRow, EngineError>;
    pub fn remove(&self, id: &AccountId) -> Result<AccountRow, EngineError>;
    pub fn set_alias(&self, id: &AccountId, alias: Option<&str>) -> Result<AccountRow, EngineError>;
    pub fn set_disabled(&self, id: &AccountId, disabled: bool) -> Result<AccountRow, EngineError>;
    pub fn move_to(&self, id: &AccountId, position: u32) -> Result<AccountRow, EngineError>;
    pub fn accounts(&self, provider: Option<&ProviderId>) -> Result<Vec<ProviderAccounts>, EngineError>;
    pub fn status(&self, provider: &ProviderId) -> Result<StatusView, EngineError>;
}
pub struct AccountView { pub row: AccountRow, pub active: bool }
pub struct ProviderAccounts { pub provider: ProviderId, pub active_position: Option<u32>, pub accounts: Vec<AccountView> }
pub enum StatusView { NoLogin, Unmanaged { email: String }, Managed { account: AccountView, total: usize } }
```

- [ ] **Step 1: Write the failing tests**

`crates/tagteam-engine/tests/manage.rs`:
```rust
mod common;

use common::Fx;
use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::EngineError;
use tagteam_engine::lifecycle::{AddOptions, AddTokenOptions};
use tagteam_engine::store::NewAccount;
use tagteam_engine::views::StatusView;
use tagteam_provider::Provider;

fn add(fx: &Fx, email: &str, rt: &str) -> AccountId {
    fx.login(email, rt);
    fx.engine
        .add_live(AddOptions { provider: fx.provider(), position: None, alias: None, yes: false })
        .unwrap()
        .account
        .id
}

#[test]
fn references_resolve_by_position_alias_and_email() {
    let fx = Fx::new();
    let a = add(&fx, "a@x.co", "rt-a");
    let b = add(&fx, "b@x.co", "rt-b");
    fx.engine.set_alias(&b, Some("Home")).unwrap();
    assert_eq!(fx.engine.resolve("1", None).unwrap().id, a);
    assert_eq!(fx.engine.resolve("HOME", None).unwrap().id, b);
    assert_eq!(fx.engine.resolve("a@x.co", None).unwrap().id, a);
    assert!(matches!(fx.engine.resolve("9", None), Err(EngineError::NoSuchAccount(_))));
    assert!(matches!(fx.engine.resolve("", None), Err(EngineError::NoSuchAccount(_))));
}

#[test]
fn an_email_in_several_providers_is_ambiguous_unless_narrowed() {
    let fx = Fx::new();
    add(&fx, "a@x.co", "rt-a");
    let other = ProviderId::new("fake-agent");
    let identity = fx.cc.token_identity("a@x.co");
    fx.engine
        .store()
        .unwrap()
        .insert_account(&NewAccount {
            id: &AccountId::from_string("fake-1"),
            provider: &other,
            position: 1,
            identity_key: "a@x.co\n",
            identity: &identity,
            kind: "api_key",
            alias: None,
            login_expires_at: None,
            added_at: 0,
        })
        .unwrap();
    assert!(matches!(fx.engine.resolve("a@x.co", None), Err(EngineError::Ambiguous { candidates, .. }) if candidates.len() == 2));
    assert_eq!(fx.engine.candidates("a@x.co", None).unwrap().len(), 2);
    assert_eq!(fx.engine.resolve("a@x.co", Some(&other)).unwrap().id.as_str(), "fake-1");
}

#[test]
fn remove_deletes_vault_and_row_but_never_the_live_login() {
    let fx = Fx::new();
    let a = add(&fx, "a@x.co", "rt-a");
    fx.engine.remove(&a).unwrap();
    assert!(fx.vault_bytes(&a).is_none());
    assert!(fx.engine.store().unwrap().account(&a).unwrap().is_none());
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
}

#[test]
fn alias_disable_and_move() {
    let fx = Fx::new();
    let a = add(&fx, "a@x.co", "rt-a");
    let b = add(&fx, "b@x.co", "rt-b");
    fx.engine.set_alias(&a, Some("work")).unwrap();
    assert!(matches!(fx.engine.set_alias(&b, Some("Work")), Err(EngineError::InvalidInput(_))));
    assert!(matches!(fx.engine.set_alias(&b, Some("12")), Err(EngineError::InvalidInput(_))));
    assert_eq!(fx.engine.set_alias(&a, None).unwrap().alias, None);
    assert!(fx.engine.set_disabled(&a, true).unwrap().disabled);
    assert!(!fx.engine.set_disabled(&a, false).unwrap().disabled);
    assert_eq!(fx.engine.move_to(&a, 2).unwrap().position, 2);
    assert_eq!(fx.engine.store().unwrap().account(&b).unwrap().unwrap().position, 1);
    assert!(matches!(fx.engine.move_to(&a, 100), Err(EngineError::InvalidInput(_))));
    assert!(matches!(fx.engine.move_to(&a, 0), Err(EngineError::InvalidInput(_))));
}

#[test]
fn views_follow_the_live_identity() {
    let fx = Fx::new();
    assert!(matches!(fx.engine.status(&fx.provider()).unwrap(), StatusView::NoLogin));
    let lists = fx.engine.accounts(None).unwrap();
    assert_eq!(lists.len(), 1);
    assert!(lists[0].accounts.is_empty());

    let a = add(&fx, "a@x.co", "rt-a");
    add(&fx, "b@x.co", "rt-b");
    fx.engine
        .add_token(AddTokenOptions {
            provider: fx.provider(),
            token: "sk-ant-api03-k".into(),
            position: None,
            email: None,
            alias: None,
            yes: false,
        })
        .unwrap();
    let list = &fx.engine.accounts(None).unwrap()[0];
    assert_eq!(list.active_position, Some(2));
    assert_eq!(list.accounts.iter().filter(|v| v.active).count(), 1);

    fx.login("a@x.co", "rt-a2"); // CC logged in as `a` directly
    match fx.engine.status(&fx.provider()).unwrap() {
        StatusView::Managed { account, total } => assert_eq!((account.row.id, total), (a, 3)),
        _ => panic!("expected a managed status"),
    }
    fx.login("stranger@x.co", "rt-s");
    assert!(matches!(fx.engine.status(&fx.provider()).unwrap(), StatusView::Unmanaged { email } if email == "stranger@x.co"));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tagteam-engine --test manage`
Expected: FAIL to compile.

- [ ] **Step 3: Implement references**

`crates/tagteam-engine/src/refs.rs`:
```rust
use tagteam_core::ProviderId;
use tagteam_core::validate::{AccountRefInput, normalize_alias, parse_account_ref};

use crate::engine::Engine;
use crate::error::EngineError;
use crate::store::AccountRow;

impl Engine {
    /// §10.4: a position (within `provider`, else the default provider), then an alias
    /// (unique across providers), then an exact email (within `provider` if given).
    pub fn candidates(&self, input: &str, provider: Option<&ProviderId>) -> Result<Vec<AccountRow>, EngineError> {
        let Some(store) = self.existing_store()? else { return Ok(vec![]) };
        match parse_account_ref(input) {
            None => Ok(vec![]),
            Some(AccountRefInput::Position(p)) => {
                let provider = provider.unwrap_or(&self.default_provider);
                Ok(store.find_by_position(provider, p)?.into_iter().collect())
            }
            Some(AccountRefInput::Text(t)) => {
                if let Ok(alias) = normalize_alias(&t) {
                    if let Some(row) = store.find_by_alias(&alias)? {
                        let wrong_provider = provider.is_some_and(|p| p != &row.provider);
                        return Ok(if wrong_provider { vec![] } else { vec![row] });
                    }
                }
                Ok(store.find_by_email(&t, provider)?)
            }
        }
    }

    pub fn resolve(&self, input: &str, provider: Option<&ProviderId>) -> Result<AccountRow, EngineError> {
        let mut found = self.candidates(input, provider)?;
        match found.len() {
            0 => Err(EngineError::NoSuchAccount(input.to_owned())),
            1 => Ok(found.remove(0)),
            _ => Err(EngineError::Ambiguous {
                input: input.to_owned(),
                candidates: found
                    .iter()
                    .map(|r| {
                        let org = r.org_name.clone().unwrap_or_else(|| {
                            if r.org_uuid.is_empty() { "personal".into() } else { r.org_uuid.clone() }
                        });
                        format!("{} #{} {} ({org})", r.provider, r.position, r.label)
                    })
                    .collect(),
            }),
        }
    }
}
```

- [ ] **Step 4: Implement the management commands**

Append to `lifecycle.rs`:
```rust
impl Engine {
    fn managed_row(&self, id: &AccountId) -> Result<AccountRow, EngineError> {
        self.store()?.account(id)?.ok_or_else(|| EngineError::NoSuchAccount(id.to_string()))
    }

    /// §10.3. The live login is never touched.
    pub fn remove(&self, id: &AccountId) -> Result<AccountRow, EngineError> {
        self.refuse_inside_run_shell()?;
        let provider = self.managed_row(id)?.provider;
        self.settle_or_refuse(&provider)?;
        let _guard = self.mutation_guard()?;
        self.refuse_if_interrupted(&provider)?;
        let row = self.managed_row(id)?;
        let lock = self.lock_account(id)?;
        self.remove_locked(&row, &lock)?;
        Ok(row)
    }

    pub fn set_alias(&self, id: &AccountId, alias: Option<&str>) -> Result<AccountRow, EngineError> {
        self.refuse_inside_run_shell()?;
        let alias = alias_arg(alias)?;
        let _guard = self.mutation_guard()?;
        self.managed_row(id)?;
        self.store()?.set_alias(id, alias.as_deref()).map_err(alias_taken)?;
        self.managed_row(id)
    }

    pub fn set_disabled(&self, id: &AccountId, disabled: bool) -> Result<AccountRow, EngineError> {
        self.refuse_inside_run_shell()?;
        let _guard = self.mutation_guard()?;
        self.managed_row(id)?;
        self.store()?.set_disabled(id, disabled)?;
        self.managed_row(id)
    }

    /// Reorders only; if the position is taken, the two accounts swap.
    pub fn move_to(&self, id: &AccountId, position: u32) -> Result<AccountRow, EngineError> {
        self.refuse_inside_run_shell()?;
        let _guard = self.mutation_guard()?;
        let row = self.managed_row(id)?;
        let store = self.store()?;
        check_position(position, store.next_position(&row.provider)?.saturating_sub(1))?;
        store.move_to(id, position)?;
        self.managed_row(id)
    }
}
```

- [ ] **Step 5: Implement the views**

`crates/tagteam-engine/src/views.rs`:
```rust
use tagteam_core::ProviderId;
use tagteam_provider::Read;

use crate::engine::Engine;
use crate::error::EngineError;
use crate::store::AccountRow;

#[derive(Debug, Clone)]
pub struct AccountView {
    pub row: AccountRow,
    /// The live identity wins over the store's active account.
    pub active: bool,
}

#[derive(Debug, Clone)]
pub struct ProviderAccounts {
    pub provider: ProviderId,
    pub active_position: Option<u32>,
    pub accounts: Vec<AccountView>,
}

#[derive(Debug, Clone)]
pub enum StatusView {
    NoLogin,
    Unmanaged { email: String },
    Managed { account: AccountView, total: usize },
}

impl Engine {
    fn view(&self, provider: &ProviderId) -> Result<(ProviderAccounts, Read<String>), EngineError> {
        let p = self.provider(provider)?;
        let rows = match self.existing_store()? {
            Some(s) => s.accounts(provider)?,
            None => vec![],
        };
        let live = p.live_identity(&self.env);
        let live_key = live.as_ref().map(|i| p.identity_key(i).as_str().to_owned());
        let stored_active = match (&live_key, self.existing_store()?) {
            (Read::Unreadable(_), Some(s)) => s.active(provider)?,
            _ => None,
        };
        let accounts: Vec<AccountView> = rows
            .into_iter()
            .map(|row| {
                let active = match &live_key {
                    Read::Present(k) => &row.identity_key == k,
                    Read::Absent => false,
                    Read::Unreadable(_) => stored_active.as_ref() == Some(&row.id),
                };
                AccountView { row, active }
            })
            .collect();
        let active_position = accounts.iter().find(|v| v.active).map(|v| v.row.position);
        let live_label = live.map(|i| i.email.unwrap_or(i.label));
        Ok((ProviderAccounts { provider: provider.clone(), active_position, accounts }, live_label))
    }

    /// Every provider that has accounts, plus the default provider; or just `provider`.
    pub fn accounts(&self, provider: Option<&ProviderId>) -> Result<Vec<ProviderAccounts>, EngineError> {
        let ids: Vec<ProviderId> = match provider {
            Some(p) => vec![p.clone()],
            None => {
                let mut ids = vec![self.default_provider.clone()];
                if let Some(s) = self.existing_store()? {
                    for row in s.all_accounts()? {
                        if !ids.contains(&row.provider) && self.registry.get(&row.provider).is_some() {
                            ids.push(row.provider);
                        }
                    }
                }
                ids
            }
        };
        ids.iter().map(|id| self.view(id).map(|(v, _)| v)).collect()
    }

    pub fn status(&self, provider: &ProviderId) -> Result<StatusView, EngineError> {
        let (list, live) = self.view(provider)?;
        let total = list.accounts.len();
        if let Some(active) = list.accounts.into_iter().find(|v| v.active) {
            return Ok(StatusView::Managed { account: active, total });
        }
        match live {
            Read::Present(email) => Ok(StatusView::Unmanaged { email }),
            Read::Absent => Ok(StatusView::NoLogin),
            Read::Unreadable(e) => Err(EngineError::Unreadable(e)),
        }
    }
}
```

`lib.rs` additions: `mod refs; pub mod views;`.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p tagteam-engine`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/tagteam-engine
git commit -m "Add account references, management commands and views"
```

---
### Task 20: The switch transaction

**Files:**
- Create: `crates/tagteam-engine/src/switch.rs`, `crates/tagteam-engine/src/hooks.rs`
- Modify: `crates/tagteam-engine/src/engine.rs` (the `fail_at` test hook field),
  `crates/tagteam-engine/src/lib.rs`
- Test: `crates/tagteam-engine/tests/switch.rs`, `crates/tagteam-engine/tests/switch_rollback.rs`

**Interfaces:**
- Consumes: everything in the engine so far; `decide_outgoing`, `next_in_rotation`, `verdict`
- Produces:
```rust
pub enum SwitchTarget { Rotation, Account(AccountId) }
pub struct SwitchRequest { pub provider: ProviderId, pub target: SwitchTarget, pub force: bool, pub source: &'static str }
pub enum SwitchReason { Switched, AlreadyActive, Activated, UnmanagedAccount, OnlyOneAccount, NoValidTarget }
impl SwitchReason { pub fn as_str(&self) -> &'static str }  // cswap's reason strings
pub struct SwitchOutcome { pub switched: bool, pub from: Option<AccountRow>, pub to: Option<AccountRow>,
    pub strategy: &'static str, pub reason: SwitchReason, pub message: String, pub warnings: Vec<String>,
    pub file_store: bool, pub unmanaged_email: Option<String> }
impl Engine { pub fn switch(&self, req: SwitchRequest) -> Result<SwitchOutcome, EngineError>; }
// hooks.rs (feature "test-hooks"; no-ops otherwise)
pub(crate) fn point(engine: &Engine, name: &'static str) -> Result<(), EngineError>;
impl Engine { #[cfg(feature = "test-hooks")] pub fn fail_at(&self, name: Option<&'static str>); }
```
Hook points: `planned` (after planning, before any lock), `after-journal`, `after-credential`,
`after-identity`; `add-verified` in `add_live` (after the pre-lock checks, before any lock);
and, from Task 21, `before-mutation-lock` in `mutation_guard` and `recovery-before-commit`
just before a forward recovery's final re-read.

- [ ] **Step 1: Write the failing behaviour tests**

`crates/tagteam-engine/tests/switch.rs`:
```rust
mod common;

use std::fs;

use common::Fx;
use serde_json::json;
use tagteam_cc::{ItemKind, keychain_account, keychain_service};
use tagteam_core::AccountId;
use tagteam_engine::EngineError;
use tagteam_engine::lifecycle::{AddOptions, AddTokenOptions};
use tagteam_engine::switch::{SwitchReason, SwitchRequest, SwitchTarget};
use tagteam_provider::{Keychain, Provider};

fn add(fx: &Fx, email: &str, rt: &str) -> AccountId {
    fx.login(email, rt);
    fx.engine.add_live(AddOptions { provider: fx.provider(), position: None, alias: None, yes: false }).unwrap().account.id
}

fn switch(fx: &Fx, target: SwitchTarget, force: bool) -> Result<tagteam_engine::switch::SwitchOutcome, EngineError> {
    fx.engine.switch(SwitchRequest { provider: fx.provider(), target, force, source: "cli" })
}

fn to(id: &AccountId) -> SwitchTarget {
    SwitchTarget::Account(id.clone())
}

fn displaced_files(fx: &Fx) -> usize {
    fs::read_dir(fx.env.data_dir().join("displaced")).map(|d| d.count()).unwrap_or(0)
}

#[test]
fn rotation_activates_the_next_account_and_keeps_machine_shared_keys() {
    let fx = Fx::new();
    let a = add(&fx, "a@x.co", "rt-a");
    let b = add(&fx, "b@x.co", "rt-b"); // live now: b
    let out = switch(&fx, SwitchTarget::Rotation, false).unwrap();
    assert_eq!((out.switched, out.reason, out.strategy), (true, SwitchReason::Switched, "rotation"));
    assert_eq!(out.to.unwrap().id, a);
    assert_eq!(out.from.unwrap().id, b);
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
    assert_eq!(fx.live_credential().unwrap()["mcpOAuth"], json!({"srv": {"token": "machine-shared"}}));
    let store = fx.engine.store().unwrap();
    assert_eq!(store.active(&fx.provider()).unwrap(), Some(a));
    assert!(store.journal(&fx.provider()).unwrap().is_none());
    assert_eq!(store.events().unwrap().last().unwrap().kind, "switch");
    // CC's lock directories are gone again.
    assert!(!fx.paths().refresh_lock.exists() && !fx.paths().config_lock.exists());
}

#[test]
fn a_rotated_outgoing_credential_is_captured_before_switching() {
    let fx = Fx::new();
    let a = add(&fx, "a@x.co", "rt-a");
    let b = add(&fx, "b@x.co", "rt-b");
    fx.rotate_live("rt-b2"); // CC refreshed b in place
    switch(&fx, to(&a), false).unwrap();
    assert_eq!(fx.vault_refresh_token(&b).as_deref(), Some("rt-b2"));
    let prev = fx.kc.get("tagteam", &format!("{b}.prev")).unwrap();
    assert!(String::from_utf8(prev).unwrap().contains("rt-b"));
}

#[test]
fn a_wiped_outgoing_credential_never_overwrites_the_vault() {
    let fx = Fx::new();
    let a = add(&fx, "a@x.co", "rt-a");
    let b = add(&fx, "b@x.co", "rt-b");
    fx.set_live_credential(br#"{"claudeAiOauth":{"accessToken":"","refreshToken":""}}"#);
    switch(&fx, to(&a), false).unwrap();
    assert_eq!(fx.vault_refresh_token(&b).as_deref(), Some("rt-b"));
}

#[test]
fn a_foreign_outgoing_credential_is_displaced_not_captured() {
    let fx = Fx::new();
    let a = add(&fx, "a@x.co", "rt-a");
    let b = add(&fx, "b@x.co", "rt-b");
    fx.rotate_live("someone-elses-token");
    let stranger = fx.cc.parse_identity(&json!({"emailAddress": "z@x.co", "accountUuid": "uuid-z"})).unwrap();
    fx.oracle.set(Some(stranger));
    let out = switch(&fx, to(&a), false).unwrap();
    assert_eq!(fx.vault_refresh_token(&b).as_deref(), Some("rt-b"));
    assert_eq!(displaced_files(&fx), 1);
    assert!(out.warnings.iter().any(|w| w.contains("displaced")));
}

#[test]
fn an_access_token_only_blob_never_replaces_a_refresh_token() {
    let fx = Fx::new();
    let a = add(&fx, "a@x.co", "rt-a");
    let b = add(&fx, "b@x.co", "rt-b");
    fx.set_live_credential(br#"{"claudeAiOauth":{"accessToken":"only-access"}}"#);
    switch(&fx, to(&a), false).unwrap();
    assert_eq!(fx.vault_refresh_token(&b).as_deref(), Some("rt-b"));
    assert_eq!(displaced_files(&fx), 1);
}

#[test]
fn an_unmanaged_live_login_is_a_noop_unless_forced() {
    let fx = Fx::new();
    let a = add(&fx, "a@x.co", "rt-a");
    fx.login("stranger@x.co", "rt-s");
    let out = switch(&fx, to(&a), false).unwrap();
    assert_eq!((out.switched, out.reason), (false, SwitchReason::UnmanagedAccount));
    assert_eq!(out.unmanaged_email.as_deref(), Some("stranger@x.co"));
    assert_eq!(fx.live_email().as_deref(), Some("stranger@x.co"));
    let out = switch(&fx, to(&a), true).unwrap();
    assert!(out.switched);
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
    assert_eq!(displaced_files(&fx), 1);
}

#[test]
fn a_fresh_machine_activates_the_first_switchable_account() {
    let fx = Fx::new();
    let a = add(&fx, "a@x.co", "rt-a");
    add(&fx, "b@x.co", "rt-b");
    // A new machine: no oauthAccount, no credential.
    fs::write(fx.paths().global_config, common::CLAUDE_JSON).unwrap();
    fx.kc.delete(&keychain_service(&fx.env, ItemKind::OAuth), &keychain_account(&fx.env)).unwrap();
    fx.engine.store().unwrap().set_active(&fx.provider(), None).unwrap();
    let out = switch(&fx, SwitchTarget::Rotation, false).unwrap();
    assert_eq!(out.to.unwrap().id, a);
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
    assert!(fx.live_credential().unwrap().get("mcpOAuth").is_none(), "no live JSON: no machine-shared keys");
}

#[test]
fn a_replacement_finished_by_the_lock_is_switched_away_from_with_its_new_kind() {
    let fx = Fx::new();
    let a = add(&fx, "a@x.co", "rt-a");
    // b was stored as an API key; an `add` over it with b's OAuth login wrote the vault and
    // then died before its metadata landed.
    let b = fx
        .engine
        .add_token(AddTokenOptions {
            provider: fx.provider(),
            token: "sk-ant-api03-b".into(),
            position: None,
            email: Some("b@x.co".into()),
            alias: None,
            yes: false,
        })
        .unwrap()
        .account;
    fx.login("b@x.co", "rt-b");
    let cred = Fx::credential_json("b@x.co", "rt-b").to_string().into_bytes();
    fx.kc.put("tagteam", b.id.as_str(), &cred);
    let identity = fx.cc.parse_identity(&Fx::oauth_account("b@x.co")).unwrap();
    let meta = tagteam_engine::store::LoginMeta { identity_key: "b@x.co\n", identity: &identity, kind: "oauth", login_expires_at: None };
    fx.engine.store().unwrap().begin_replacement(&b.id, fx.cc.fingerprint(&cred).unwrap().as_str(), &meta).unwrap();
    fx.rotate_live("rt-b2"); // then CC refreshed b in place
    switch(&fx, to(&a), false).unwrap();
    assert_eq!(fx.vault_refresh_token(&b.id).as_deref(), Some("rt-b2"), "b's newest generation was captured, not lost");
}

#[test]
fn a_switch_with_no_store_creates_nothing() {
    let fx = Fx::new();
    let out = switch(&fx, SwitchTarget::Rotation, false).unwrap();
    assert_eq!(out.reason, SwitchReason::NoValidTarget);
    fx.login("stranger@x.co", "rt-s");
    let out = switch(&fx, SwitchTarget::Rotation, false).unwrap();
    assert_eq!((out.reason, out.unmanaged_email.as_deref()), (SwitchReason::UnmanagedAccount, Some("stranger@x.co")));
    assert!(!fx.env.data_dir().exists());
}

#[test]
fn a_resolved_self_switch_activates_the_generation_it_captured() {
    let fx = Fx::new();
    let a = add(&fx, "a@x.co", "rt-a");
    add(&fx, "b@x.co", "rt-b");
    switch(&fx, to(&a), false).unwrap();
    fx.rotate_live("rt-a2"); // CC rotated a; the vault still holds rt-a
    fx.oracle.set(Some(fx.cc.parse_identity(&Fx::oauth_account("a@x.co")).unwrap()));
    switch(&fx, to(&a), false).unwrap();
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a2"), "never the superseded rt-a");
}

#[test]
fn trivial_cases_are_noops() {
    let fx = Fx::new();
    let a = add(&fx, "a@x.co", "rt-a");
    assert_eq!(switch(&fx, SwitchTarget::Rotation, false).unwrap().reason, SwitchReason::OnlyOneAccount);
    assert_eq!(switch(&fx, to(&a), false).unwrap().reason, SwitchReason::AlreadyActive);
    let forced = switch(&fx, to(&a), true).unwrap();
    assert_eq!(forced.reason, SwitchReason::Activated);
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
}

#[test]
fn rotation_skips_disabled_accounts_but_direct_targets_may_be_disabled() {
    let fx = Fx::new();
    let a = add(&fx, "a@x.co", "rt-a");
    let b = add(&fx, "b@x.co", "rt-b");
    let c = add(&fx, "c@x.co", "rt-c"); // live: c
    fx.engine.set_disabled(&a, true).unwrap();
    assert_eq!(switch(&fx, SwitchTarget::Rotation, false).unwrap().to.unwrap().id, b);
    assert_eq!(switch(&fx, to(&a), false).unwrap().to.unwrap().id, a);
    let _ = c;
}

#[test]
fn unsafe_live_reads_abort_without_changing_anything() {
    let fx = Fx::new();
    let a = add(&fx, "a@x.co", "rt-a");
    add(&fx, "b@x.co", "rt-b");
    let (svc, acct) = (keychain_service(&fx.env, ItemKind::OAuth), keychain_account(&fx.env));
    let before = fx.kc.get(&svc, &acct).unwrap();

    fx.kc.set_unreadable(&svc, &acct, true);
    assert!(matches!(switch(&fx, to(&a), false), Err(EngineError::Unreadable(_))));
    fs::write(fx.paths().credentials_file, "{}").unwrap();
    assert!(matches!(switch(&fx, to(&a), true), Err(EngineError::DegradedRead)), "--force never overrides an unreadable entry");
    fx.kc.set_unreadable(&svc, &acct, false);
    fs::remove_file(fx.paths().credentials_file).unwrap();

    fx.set_live_credential(b"");
    assert!(matches!(switch(&fx, to(&a), false), Err(EngineError::InvalidInput(m)) if m.contains("empty")));
    fx.set_live_credential(&before);
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
}

#[test]
fn api_key_accounts_move_the_auth_axis_both_ways() {
    let fx = Fx::new();
    let a = add(&fx, "a@x.co", "rt-a");
    let k = fx
        .engine
        .add_token(AddTokenOptions {
            provider: fx.provider(),
            token: "sk-ant-api03-abcdefghijklmnopqrstuvwxyz".into(),
            position: None,
            email: None,
            alias: None,
            yes: false,
        })
        .unwrap()
        .account
        .id;
    let acct = keychain_account(&fx.env);
    let managed = keychain_service(&fx.env, ItemKind::ManagedKey);
    switch(&fx, to(&k), false).unwrap();
    assert_eq!(fx.kc.get(&managed, &acct).unwrap(), b"sk-ant-api03-abcdefghijklmnopqrstuvwxyz");
    assert_eq!(fx.live_credential().unwrap(), json!({"mcpOAuth": {"srv": {"token": "machine-shared"}}}));
    assert_eq!(fx.live_email().as_deref(), Some("api-key-2@token.local"));
    switch(&fx, to(&a), false).unwrap();
    assert!(fx.kc.get(&managed, &acct).is_none());
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
    assert_eq!(fx.live_credential().unwrap()["mcpOAuth"], json!({"srv": {"token": "machine-shared"}}));
}

#[test]
fn the_file_store_reports_itself_for_the_hint() {
    let fx = Fx::with_platform(tagteam_cc::live::Platform::Linux);
    let a = add(&fx, "a@x.co", "rt-a");
    add(&fx, "b@x.co", "rt-b");
    assert!(switch(&fx, to(&a), false).unwrap().file_store);
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
}

#[test]
#[ignore = "waits the full 9 s CC lock timeout; run with --ignored"]
fn cc_holding_its_refresh_lock_blocks_the_switch_and_changes_nothing() {
    // Review Focus 1.
    let fx = Fx::new();
    let a = add(&fx, "a@x.co", "rt-a");
    add(&fx, "b@x.co", "rt-b");
    fs::create_dir(fx.paths().refresh_lock).unwrap();
    let err = switch(&fx, to(&a), false).unwrap_err();
    assert_eq!(err.kind(), "lock-timeout");
    assert!(err.to_string().contains(".oauth_refresh.lock"));
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    assert!(fx.paths().refresh_lock.is_dir(), "CC's lock is left alone");
}
```

`crates/tagteam-engine/tests/switch_rollback.rs`:
```rust
#![cfg(feature = "test-hooks")]

mod common;

use common::Fx;
use tagteam_core::AccountId;
use tagteam_engine::EngineError;
use tagteam_engine::lifecycle::AddOptions;
use tagteam_engine::switch::{SwitchRequest, SwitchTarget};

fn add(fx: &Fx, email: &str, rt: &str) -> AccountId {
    fx.login(email, rt);
    fx.engine.add_live(AddOptions { provider: fx.provider(), position: None, alias: None, yes: false }).unwrap().account.id
}

#[test]
fn an_error_at_any_step_restores_every_byte() {
    for point in ["after-journal", "after-credential", "after-identity"] {
        let fx = Fx::new();
        let a = add(&fx, "a@x.co", "rt-a");
        add(&fx, "b@x.co", "rt-b");
        let cfg_before = std::fs::read(fx.paths().global_config).unwrap();
        let cred_before = fx.live_credential();
        fx.engine.fail_at(Some(point));
        let err = fx
            .engine
            .switch(SwitchRequest { provider: fx.provider(), target: SwitchTarget::Account(a.clone()), force: false, source: "cli" })
            .unwrap_err();
        assert!(matches!(err, EngineError::RolledBack(_)), "{point}: {err}");
        assert_eq!(std::fs::read(fx.paths().global_config).unwrap(), cfg_before, "{point}");
        assert_eq!(fx.live_credential(), cred_before, "{point}");
        let store = fx.engine.store().unwrap();
        assert!(store.journal(&fx.provider()).unwrap().is_none(), "{point}");
        assert_ne!(store.active(&fx.provider()).unwrap(), Some(a), "{point}");
    }
}

#[test]
fn a_rotation_is_replanned_when_positions_move_during_the_wait() {
    let fx = Fx::new();
    let a = add(&fx, "a@x.co", "rt-a");
    let b = add(&fx, "b@x.co", "rt-b");
    add(&fx, "c@x.co", "rt-c"); // live and active at 3: the rotation wraps to position 1
    let other = fx.engine_with_env(fx.env.clone());
    let moved = a.clone();
    fx.engine.on_point("planned", Box::new(move || drop(other.move_to(&moved, 2))));
    let req = SwitchRequest { provider: fx.provider(), target: SwitchTarget::Rotation, force: false, source: "cli" };
    let out = fx.engine.switch(req).unwrap();
    assert_eq!(out.to.unwrap().id, b, "position 1 holds b by the time the locks are held");
}

#[test]
fn a_rotation_down_to_one_switchable_account_becomes_a_noop() {
    let fx = Fx::new();
    add(&fx, "a@x.co", "rt-a");
    let b = add(&fx, "b@x.co", "rt-b");
    let c = add(&fx, "c@x.co", "rt-c");
    let other = fx.engine_with_env(fx.env.clone());
    fx.engine.on_point("planned", Box::new(move || {
        drop(other.set_disabled(&b, true));
        drop(other.set_disabled(&c, true));
    }));
    let req = SwitchRequest { provider: fx.provider(), target: SwitchTarget::Rotation, force: false, source: "cli" };
    let out = fx.engine.switch(req).unwrap();
    assert_eq!(out.reason, tagteam_engine::switch::SwitchReason::OnlyOneAccount);
}

#[test]
fn add_refuses_when_the_identity_changes_while_it_waits() {
    let fx = Fx::new();
    fx.login("me@work.co", "rt-1");
    let path = fx.paths().global_config;
    fx.engine.on_point("add-verified", Box::new(move || {
        let doc = std::fs::read(&path).unwrap();
        let changed = serde_json::json!({"emailAddress": "me@work.co", "organizationUuid": "", "accountUuid": "uuid-recycled"});
        std::fs::write(&path, tagteam_provider::splice::replace_top_level(&doc, "oauthAccount", &changed).unwrap()).unwrap();
    }));
    let opts = AddOptions { provider: fx.provider(), position: None, alias: None, yes: false };
    assert!(matches!(fx.engine.add_live(opts), Err(tagteam_engine::EngineError::LiveMoved)));
}

#[test]
fn a_panic_after_a_live_write_rolls_back_through_drop() {
    for point in ["panic:after-credential", "panic:after-identity"] {
        let fx = Fx::new();
        let a = add(&fx, "a@x.co", "rt-a");
        add(&fx, "b@x.co", "rt-b");
        let cfg_before = std::fs::read(fx.paths().global_config).unwrap();
        let cred_before = fx.live_credential();
        fx.engine.fail_at(Some(point));
        let req = SwitchRequest { provider: fx.provider(), target: SwitchTarget::Account(a.clone()), force: false, source: "cli" };
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fx.engine.switch(req)));
        assert!(r.is_err(), "{point}");
        assert_eq!(std::fs::read(fx.paths().global_config).unwrap(), cfg_before, "{point}");
        assert_eq!(fx.live_credential(), cred_before, "{point}");
        // The journal stays: the unwinding rollback cannot confirm itself, so recovery decides.
        assert!(fx.engine.store().unwrap().journal(&fx.provider()).unwrap().is_some(), "{point}");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tagteam-engine --features test-hooks --test switch --test switch_rollback`
Expected: FAIL to compile.

- [ ] **Step 3: Add the test hooks**

`crates/tagteam-engine/src/hooks.rs`:
```rust
use crate::engine::Engine;
use crate::error::EngineError;

/// A named point in the switch transaction. With the `test-hooks` feature, the environment
/// variable `TAGTEAM_TEST_CRASH_AT=<name>` ends the process there as a kill would: no
/// destructor runs, so no lock guard and no rollback (the kill tests). `Engine::fail_at`
/// injects an error there (`"<name>"`) or a panic (`"panic:<name>"`) for the rollback tests.
/// Without the feature, a no-op.
#[cfg(feature = "test-hooks")]
pub(crate) fn point(engine: &Engine, name: &'static str) -> Result<(), EngineError> {
    if std::env::var("TAGTEAM_TEST_CRASH_AT").as_deref() == Ok(name) {
        std::process::exit(137);
    }
    if let Some((at, callback)) = &*engine.on_point.lock().unwrap() {
        if *at == name {
            callback();
        }
    }
    let injected = *engine.fail_at.lock().unwrap();
    if injected == Some(name) {
        return Err(EngineError::InvalidInput(format!("injected failure at {name}")));
    }
    if injected.and_then(|n| n.strip_prefix("panic:")) == Some(name) {
        panic!("injected panic at {name}");
    }
    Ok(())
}

#[cfg(not(feature = "test-hooks"))]
#[inline(always)]
pub(crate) fn point(_engine: &Engine, _name: &'static str) -> Result<(), EngineError> {
    Ok(())
}
```

In `engine.rs`, add to `struct Engine`:
```rust
    #[cfg(feature = "test-hooks")]
    pub(crate) fail_at: Mutex<Option<&'static str>>,
    #[cfg(feature = "test-hooks")]
    #[allow(clippy::type_complexity)]
    pub(crate) on_point: Mutex<Option<(&'static str, Box<dyn Fn() + Send + Sync>)>>,
```
initialize them in `Engine::new` with `#[cfg(feature = "test-hooks")] fail_at: Mutex::new(None),`
and `#[cfg(feature = "test-hooks")] on_point: Mutex::new(None),`, and add:
```rust
#[cfg(feature = "test-hooks")]
impl Engine {
    pub fn fail_at(&self, name: Option<&'static str>) {
        *self.fail_at.lock().unwrap() = name;
    }

    /// Runs `callback` each time the switch passes the named point: a deterministic barrier
    /// for races that are otherwise timing-dependent.
    pub fn on_point(&self, name: &'static str, callback: Box<dyn Fn() + Send + Sync>) {
        *self.on_point.lock().unwrap() = Some((name, callback));
    }
}
```

- [ ] **Step 4: Implement the switch**

`crates/tagteam-engine/src/switch.rs`:
```rust
use serde_json::Value;
use tagteam_core::{AccountId, OracleVerdict, OutgoingAction, OutgoingFacts, ProviderId, decide_outgoing, next_in_rotation};
use tagteam_provider::{
    Credential, Identity, LiveAuth, LiveLocks, ProcessStamp, Provenance, Provider, ProviderError, Read, StoredLogin, Undo,
};

use crate::account_lock::AccountLock;
use crate::displace::displace;
use crate::engine::Engine;
use crate::error::EngineError;
use crate::hooks;
use crate::lifecycle::same_owner;
use crate::oracle::verdict;
use crate::store::{AccountRow, EventRow, JournalRow, Store};

#[derive(Debug, Clone)]
pub enum SwitchTarget {
    Rotation,
    Account(AccountId),
}

#[derive(Debug, Clone)]
pub struct SwitchRequest {
    pub provider: ProviderId,
    pub target: SwitchTarget,
    pub force: bool,
    pub source: &'static str,
}

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

#[derive(Debug, Clone)]
pub struct SwitchOutcome {
    pub switched: bool,
    pub from: Option<AccountRow>,
    pub to: Option<AccountRow>,
    pub strategy: &'static str,
    pub reason: SwitchReason,
    pub message: String,
    pub warnings: Vec<String>,
    pub file_store: bool,
    pub unmanaged_email: Option<String>,
}

struct Plan {
    target: AccountRow,
    strategy: &'static str,
    self_switch: bool,
}

enum Planned {
    Done(SwitchOutcome),
    Go(Plan),
}

/// The pre-lock oracle answer, valid only for the exact bytes it was asked about (§9.4).
struct OracleHint {
    bytes: Vec<u8>,
    resolved: Option<Identity>,
}

/// Rolls back on unwinding: a panic between the first live write and the commit still runs
/// every undo, through `Drop` (§14). Disarmed once the switch commits or rolls back itself.
struct Rollback<'a, 'l> {
    undos: Vec<Box<dyn Undo>>,
    locks: &'a LiveLocks<'l>,
    armed: bool,
}

impl Drop for Rollback<'_, '_> {
    fn drop(&mut self) {
        if self.armed {
            for undo in self.undos.drain(..).rev() {
                if let Err(e) = undo.undo(self.locks) {
                    tracing::error!("rollback during unwinding failed: {e}");
                }
            }
        }
    }
}

fn strategy_of(target: &SwitchTarget) -> &'static str {
    match target {
        SwitchTarget::Rotation => "rotation",
        SwitchTarget::Account(_) => "direct",
    }
}

/// The live secret on the outgoing account's auth axis: the managed key for an API-key
/// account, the credential entry otherwise.
fn live_secret(kind: &str, auth: &LiveAuth) -> Option<Vec<u8>> {
    if kind == "api_key" {
        auth.managed_key.as_ref().present().cloned()
    } else {
        auth.credential.as_ref().present().map(|c| c.bytes().to_vec())
    }
}

impl Engine {
    fn has_login(&self, row: &AccountRow) -> bool {
        row.identity_json.is_object() && matches!(self.vault.read(&row.id), Read::Present(b) if !b.is_empty())
    }

    /// "Switchable": a vault credential and an identity, and not disabled (§9.3).
    fn is_switchable(&self, row: &AccountRow) -> bool {
        !row.disabled && self.has_login(row)
    }

    fn matches_vault(&self, p: &dyn Provider, row: &AccountRow, live: &[u8]) -> bool {
        match self.vault.read(&row.id) {
            Read::Present(v) => v == live || (p.fingerprint(&v).is_some() && p.fingerprint(&v) == p.fingerprint(live)),
            _ => false,
        }
    }

    fn live_row(&self, p: &dyn Provider, store: &Store, provider: &ProviderId) -> Result<(Read<Identity>, Option<AccountRow>), EngineError> {
        let live = p.live_identity(&self.env);
        let row = match &live {
            Read::Present(i) => store.find_by_identity_key(provider, p.identity_key(i).as_str())?,
            Read::Absent => None,
            Read::Unreadable(e) => return Err(EngineError::Unreadable(e.clone())),
        };
        Ok((live, row))
    }

    /// §9.3 rotation: the next switchable position after the store's active account; on a
    /// fresh machine, the store's active account if switchable, else the first.
    fn rotation_pick(&self, store: &Store, provider: &ProviderId, live_row: Option<&AccountRow>) -> Result<Option<AccountRow>, EngineError> {
        let accounts = store.accounts(provider)?;
        let slots: Vec<(u32, bool)> = accounts.iter().map(|a| (a.position, self.is_switchable(a))).collect();
        let stored_active = store.active(provider)?.and_then(|id| accounts.iter().find(|a| a.id == id).cloned());
        let position = match live_row {
            Some(live) => next_in_rotation(&slots, Some(stored_active.as_ref().unwrap_or(live).position)),
            None => stored_active
                .filter(|a| self.is_switchable(a))
                .map(|a| a.position)
                .or_else(|| next_in_rotation(&slots, None)),
        };
        Ok(position.and_then(|pos| accounts.into_iter().find(|a| a.position == pos)))
    }

    fn plan(&self, p: &dyn Provider, store: &Store, req: &SwitchRequest) -> Result<Planned, EngineError> {
        let strategy = strategy_of(&req.target);
        let (live, live_row) = self.live_row(p, store, &req.provider)?;
        let unmanaged_email = match (&live, &live_row) {
            (Read::Present(i), None) => Some(i.email.clone().unwrap_or_else(|| i.label.clone())),
            _ => None,
        };
        let noop = |reason: SwitchReason, message: String| {
            Planned::Done(SwitchOutcome {
                switched: false,
                from: live_row.clone(),
                to: None,
                strategy,
                reason,
                message,
                warnings: vec![],
                file_store: false,
                unmanaged_email: unmanaged_email.clone(),
            })
        };
        if let (Some(email), false) = (&unmanaged_email, req.force) {
            return Ok(noop(
                SwitchReason::UnmanagedAccount,
                format!("the live login ({email}) is not managed by tagteam; add it first, or use --force"),
            ));
        }
        let accounts = store.accounts(&req.provider)?;
        let target = match &req.target {
            SwitchTarget::Account(id) => {
                store.account(id)?.ok_or_else(|| EngineError::NoSuchAccount(id.to_string()))?
            }
            SwitchTarget::Rotation => {
                let switchable = accounts.iter().filter(|a| self.is_switchable(a)).count();
                if live_row.is_some() && switchable < 2 {
                    return Ok(noop(SwitchReason::OnlyOneAccount, "there is only one switchable account".into()));
                }
                match self.rotation_pick(store, &req.provider, live_row.as_ref())? {
                    Some(a) => a,
                    None => return Ok(noop(SwitchReason::NoValidTarget, "no account can be activated".into())),
                }
            }
        };
        if !self.has_login(&target) {
            return Err(EngineError::InvalidInput(format!(
                "{} cannot be activated: it has no stored credential; log in and run `tagteam add` again",
                target.label
            )));
        }
        let self_switch = live_row.as_ref().is_some_and(|r| r.id == target.id);
        if self_switch && !req.force {
            // A no-op unless the live credential diverged and the oracle resolved it to this
            // account; then a full switch reconciles it (§9.2).
            let auth = p.read_live_auth(&self.env);
            let reconcile = match live_secret(&target.kind, &auth) {
                Some(bytes) if !self.matches_vault(p, &target, &bytes) => {
                    let target_identity = p.parse_identity(&target.identity_json)?;
                    self.oracle
                        .resolve(p, &Credential::fresh(bytes))
                        .is_some_and(|owner| same_owner(&owner, &target_identity))
                }
                _ => false,
            };
            if !reconcile {
                return Ok(noop(SwitchReason::AlreadyActive, format!("{} is already active", target.label)));
            }
        }
        Ok(Planned::Go(Plan { target, strategy, self_switch }))
    }

    fn oracle_hint(&self, p: &dyn Provider, outgoing: Option<&AccountRow>) -> Option<OracleHint> {
        let out = outgoing?;
        let bytes = live_secret(&out.kind, &p.read_live_auth(&self.env))?;
        if bytes.is_empty() || self.matches_vault(p, out, &bytes) {
            return None;
        }
        let resolved = self.oracle.resolve(p, &Credential::fresh(bytes.clone()));
        Some(OracleHint { bytes, resolved })
    }

    /// §9: plan and ask the oracle without any lock, then lock and re-derive every decision.
    /// Anything that moved while this command waited sends it back to planning.
    pub fn switch(&self, req: SwitchRequest) -> Result<SwitchOutcome, EngineError> {
        self.refuse_inside_run_shell()?;
        let p = self.provider(&req.provider)?;
        if !req.force {
            self.settle_or_refuse(&req.provider)?;
        }
        // A command that changes nothing creates nothing (§5). Without a store there is
        // nothing to activate, but an unmanaged live login is still reported as one (§9.2).
        let Some(store) = self.existing_store()? else {
            let unmanaged = match p.live_identity(&self.env) {
                Read::Present(i) => Some(i.email.unwrap_or(i.label)),
                Read::Absent => None,
                Read::Unreadable(e) => return Err(EngineError::Unreadable(e)),
            };
            let (reason, message) = match (&unmanaged, req.force) {
                (Some(email), false) => (
                    SwitchReason::UnmanagedAccount,
                    format!("the live login ({email}) is not managed by tagteam; add it first, or use --force"),
                ),
                _ => (SwitchReason::NoValidTarget, "there are no stored accounts; add one with `tagteam add`".into()),
            };
            return Ok(SwitchOutcome {
                switched: false,
                from: None,
                to: None,
                strategy: strategy_of(&req.target),
                reason,
                message,
                warnings: vec![],
                file_store: false,
                unmanaged_email: unmanaged,
            });
        };
        for _ in 0..3 {
            let plan = match self.plan(p.as_ref(), &store, &req)? {
                Planned::Done(outcome) => return Ok(outcome),
                Planned::Go(plan) => plan,
            };
            let (_, pre_outgoing) = self.live_row(p.as_ref(), &store, &req.provider)?;
            let hint = self.oracle_hint(p.as_ref(), pre_outgoing.as_ref());
            hooks::point(self, "planned")?;
            let guard = self.mutation_guard()?;
            if !req.force {
                self.refuse_if_interrupted(&req.provider)?;
            }
            let (_, outgoing) = self.live_row(p.as_ref(), &store, &req.provider)?;
            let mut ids = vec![&plan.target.id];
            if let Some(o) = &outgoing {
                ids.push(&o.id);
            }
            let accounts = self.lock_accounts(&ids)?;
            let locks = p.lock_live(&self.env, &guard)?;
            // Step 1 (B.47): the live account, the target and the self-switch decision are all
            // re-read under the locks. Any change goes back to planning, locks released.
            let (live, again) = self.live_row(p.as_ref(), &store, &req.provider)?;
            let target = store
                .account(&plan.target.id)?
                .ok_or_else(|| EngineError::NoSuchAccount(plan.target.id.to_string()))?;
            let self_switch = again.as_ref().is_some_and(|r| r.id == target.id);
            let same_pick = match req.target {
                SwitchTarget::Account(_) => true,
                // The whole rotation decision is recomputed from the current roster and anchor
                // (§9.2, §9.3), including the fewer-than-two case.
                SwitchTarget::Rotation => {
                    let switchable = store.accounts(&req.provider)?.iter().filter(|a| self.is_switchable(a)).count();
                    (again.is_none() || switchable >= 2)
                        && self.rotation_pick(&store, &req.provider, again.as_ref())?.map(|r| r.id) == Some(target.id.clone())
                }
            };
            // Account-lock acquisition may have finished a pending replacement (§12.5), changing
            // the outgoing account's kind or identity: compare the rows, not just their IDs.
            let same_outgoing = again.as_ref().map(|r| (&r.id, &r.kind, &r.identity_key))
                == outgoing.as_ref().map(|r| (&r.id, &r.kind, &r.identity_key));
            let unchanged = same_outgoing
                && target.kind == plan.target.kind
                && target.identity_key == plan.target.identity_key
                && self_switch == plan.self_switch
                && same_pick;
            if !unchanged {
                continue;
            }
            // Everything from here on uses the rows read under the locks.
            return self.transact(p.as_ref(), &store, &plan, &target, live, again, hint.as_ref(), &accounts, &locks, &req);
        }
        Err(EngineError::LiveMoved)
    }

    fn read_target(&self, target: &AccountRow) -> Result<Vec<u8>, EngineError> {
        match self.vault.read(&target.id) {
            Read::Present(b) if !b.is_empty() => Ok(b),
            Read::Unreadable(e) => Err(EngineError::Unreadable(e)),
            _ => Err(EngineError::InvalidInput(format!("{} has no stored credential", target.label))),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn transact(
        &self,
        p: &dyn Provider,
        store: &Store,
        plan: &Plan,
        target: &AccountRow,
        live_identity: Read<Identity>,
        outgoing: Option<AccountRow>,
        hint: Option<&OracleHint>,
        account_locks: &[AccountLock],
        locks: &LiveLocks<'_>,
        req: &SwitchRequest,
    ) -> Result<SwitchOutcome, EngineError> {
        let provider = &req.provider;
        let live_identity = live_identity.present();
        if live_identity.is_some() && outgoing.is_none() && !req.force {
            return Err(EngineError::LiveMoved); // became unmanaged since planning
        }
        let target_identity = p.parse_identity(&target.identity_json)?;
        let live = p.read_live_auth(&self.env);

        // Step 3 read rules, with or without --force: never overwrite what we could not read.
        let live_cred: Option<Credential> = match &live.credential {
            Read::Unreadable(e) => return Err(EngineError::Unreadable(e.clone())),
            Read::Present(c) if c.provenance() == Provenance::Degraded => return Err(EngineError::DegradedRead),
            Read::Present(c) if c.is_empty() => {
                return Err(EngineError::InvalidInput(
                    "the live credential read back empty; a Keychain timeout can look empty, so nothing was changed".into(),
                ));
            }
            Read::Present(c) => Some(c.clone()),
            Read::Absent => None,
        };
        if let Read::Unreadable(e) = &live.managed_key {
            return Err(EngineError::Unreadable(e.clone()));
        }

        let mut warnings = Vec::new();
        let direct = outgoing.is_none() || req.force;
        if direct {
            // Step 2: displace whatever is live unless it is byte-identical to the target.
            let target_secret = self.read_target(target)?;
            let reason = if req.force { "forced-activation" } else { "displaced-live-login" };
            let live_bytes = [live_cred.as_ref().map(|c| c.bytes().to_vec()), live.managed_key.as_ref().present().cloned()];
            for bytes in live_bytes.into_iter().flatten().filter(|b| *b != target_secret) {
                match displace(self, provider, &bytes, p.fingerprint(&bytes).as_ref(), reason, live_identity.as_ref().map(|i| &i.raw)) {
                    Ok(id) => warnings.push(format!("the previous live credential was saved as displaced/{id}")),
                    Err(e) if req.force => warnings.push(format!("could not save the previous live credential: {e}")),
                    Err(e) => return Err(e),
                }
            }
        } else if let Some(out) = &outgoing {
            self.settle_outgoing(p, store, out, &live, hint, account_locks, live_identity.as_ref(), &mut warnings)?;
        }
        // Read only now: settling a self-switch may have captured a newer live generation into
        // this very account, and that is the one to activate.
        let target_secret = self.read_target(target)?;

        // Steps 5–10.
        let target_login = StoredLogin { kind: target.kind.clone(), secret: target_secret.clone(), identity: target_identity };
        let from_secret = outgoing.as_ref().and_then(|o| live_secret(&o.kind, &live)).or_else(|| live_cred.as_ref().map(|c| c.bytes().to_vec()));
        // A forced switch may supersede an unresolved row; it is carried along, so a forced
        // switch that never lands puts it back instead of forgetting it (§9.6).
        let prior = if req.force { store.journal(provider)?.map(Box::new) } else { None };
        store.insert_journal(&JournalRow {
            provider: provider.clone(),
            holder: ProcessStamp::current()?,
            from_id: outgoing.as_ref().map(|o| o.id.clone()),
            to_id: target.id.clone(),
            from_fp: from_secret.as_deref().and_then(|b| p.fingerprint(b)).map(|f| f.as_str().to_owned()),
            from_identity: live_identity.as_ref().map(|i| i.raw.clone()),
            to_fp: p.fingerprint(&target_secret).map(|f| f.as_str().to_owned()).unwrap_or_default(),
            started_at: self.now_ms(),
            prior: prior.clone(),
        })?;
        let mut tx = Rollback { undos: Vec::new(), locks, armed: true };
        let applied = self.apply(p, store, target, &target_login, &live, outgoing.as_ref(), locks, req, &mut tx.undos);
        tx.armed = false;
        if let Err(cause) = applied {
            let mut failed = Vec::new();
            if let EngineError::Provider(ProviderError::RestoreFailed { restore, .. }) = &cause {
                failed.push(format!("restoring a partial write: {restore}"));
            }
            for undo in tx.undos.drain(..).rev() {
                let what = undo.what();
                if let Err(e) = undo.undo(locks) {
                    failed.push(format!("{what}: {e}"));
                }
            }
            if failed.is_empty() {
                match &prior {
                    Some(row) => store.insert_journal(row)?,
                    None => store.delete_journal(provider)?,
                }
                return Err(EngineError::RolledBack(cause.to_string()));
            }
            // The journal stays: only recovery may settle a partial state (§9.6).
            return Err(EngineError::RollbackFailed { cause: cause.to_string(), failed: failed.join("; ") });
        }

        let (reason, switched) = if plan.self_switch {
            (SwitchReason::Activated, true)
        } else {
            (SwitchReason::Switched, outgoing.as_ref().map(|o| &o.id) != Some(&target.id))
        };
        Ok(SwitchOutcome {
            switched,
            from: outgoing,
            to: Some(target.clone()),
            strategy: plan.strategy,
            reason,
            message: format!("Switched to {}", target.alias.clone().unwrap_or_else(|| target.label.clone())),
            warnings,
            file_store: p.uses_file_store(&self.env),
            unmanaged_email: None,
        })
    }

    /// Step 4: classify the outgoing credential and act on it.
    #[allow(clippy::too_many_arguments)]
    fn settle_outgoing(
        &self,
        p: &dyn Provider,
        store: &Store,
        out: &AccountRow,
        live: &LiveAuth,
        hint: Option<&OracleHint>,
        account_locks: &[AccountLock],
        live_identity: Option<&Identity>,
        warnings: &mut Vec<String>,
    ) -> Result<(), EngineError> {
        let Some(bytes) = live_secret(&out.kind, live) else { return Ok(()) };
        let vault = match self.vault.read(&out.id) {
            Read::Present(v) => Some(v),
            Read::Absent => None,
            Read::Unreadable(e) => return Err(EngineError::Unreadable(e)),
        };
        let fp_live = p.fingerprint(&bytes);
        let oracle = match hint {
            Some(h) if h.bytes == bytes => verdict(h.resolved.as_ref(), out),
            _ => OracleVerdict::Unavailable,
        };
        let facts = OutgoingFacts {
            bytes_equal_vault: vault.as_deref() == Some(bytes.as_slice()),
            fp_equal_vault: fp_live.is_some() && vault.as_deref().and_then(|v| p.fingerprint(v)) == fp_live,
            wiped: p.is_wiped(&bytes),
            oracle,
            lacks_refresh_over_complete: !p.has_refresh_token(&bytes)
                && vault.as_deref().is_some_and(|v| p.has_refresh_token(v)),
        };
        let (class, action) = decide_outgoing(&facts);
        match action {
            OutgoingAction::Nothing => {}
            OutgoingAction::CaptureToVault { backfill_uuid } => {
                let lock = account_locks.iter().find(|l| l.id() == &out.id).expect("the outgoing account is locked");
                self.vault.store(lock, &bytes, &|b| p.fingerprint(b))?;
                let identity = p.parse_identity(&out.identity_json)?;
                store.update_login(&out.id, &out.identity_key, &identity, &out.kind, p.login_expires_at(&bytes))?;
                if backfill_uuid {
                    if let Some(uuid) = hint.and_then(|h| h.resolved.as_ref()).and_then(|i| i.account_uuid.as_deref()) {
                        store.backfill_account_uuid(&out.id, uuid)?;
                    }
                }
                if matches!(class, tagteam_core::OutgoingClass::Unresolved) {
                    tracing::warn!(position = out.position, "captured an unverified live credential into the vault; .prev keeps the previous one");
                }
            }
            OutgoingAction::Displace => {
                let resolved = hint.and_then(|h| h.resolved.as_ref()).map(|i| &i.raw);
                let id = displace(self, &out.provider, &bytes, fp_live.as_ref(), "displaced-live-login", resolved.or(live_identity.map(|i| &i.raw)))?;
                warnings.push(format!("the live credential did not belong to position {}; it was saved as displaced/{id}", out.position));
            }
        }
        Ok(())
    }

    /// Steps 7–9, recording an undo for each write.
    #[allow(clippy::too_many_arguments)]
    fn apply(
        &self,
        p: &dyn Provider,
        store: &Store,
        target: &AccountRow,
        target_login: &StoredLogin,
        live: &LiveAuth,
        outgoing: Option<&AccountRow>,
        locks: &LiveLocks<'_>,
        req: &SwitchRequest,
        undos: &mut Vec<Box<dyn Undo>>,
    ) -> Result<(), EngineError> {
        hooks::point(self, "after-journal")?;
        undos.push(p.write_credential(&self.env, locks, target_login, live)?);
        hooks::point(self, "after-credential")?;
        undos.push(p.write_identity(&self.env, locks, Some(&target_login.identity))?);
        hooks::point(self, "after-identity")?;
        store.commit_switch(
            &req.provider,
            &target.id,
            &EventRow {
                at: self.now_ms(),
                provider: req.provider.clone(),
                kind: "switch".into(),
                from_id: outgoing.map(|o| o.id.clone()),
                to_id: Some(target.id.clone()),
                trigger: Some(if req.source == "auto" { "auto" } else { "manual" }.into()),
                source: req.source.into(),
                detail: None::<Value>,
            },
        )?;
        Ok(())
    }
}
```

`lib.rs` additions: `mod hooks; pub mod switch;`.

In `lifecycle.rs`, `add_live`, directly before `let guard = self.mutation_guard()?;`, add:
```rust
        crate::hooks::point(self, "add-verified")?;
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p tagteam-engine --features test-hooks`
Expected: PASS. Then once: `cargo test -p tagteam-engine --test switch -- --ignored` — expected
PASS after about 9 s.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-engine
git commit -m "Implement the switch transaction with rollback"
```

---
### Task 21: Interrupted-switch recovery

**Files:**
- Create: `crates/tagteam-engine/src/recover.rs`
- Modify: `crates/tagteam-engine/src/engine.rs` (`mutation_guard`),
  `crates/tagteam-engine/src/switch.rs`, `crates/tagteam-engine/src/lifecycle.rs`,
  `crates/tagteam-engine/src/lib.rs`
- Test: `crates/tagteam-engine/tests/recover.rs`

**Interfaces:**
- Consumes: `JournalRow`, `Provider::clear_other_axis`, `Provider::write_identity`
- Produces:
  - `Engine::mutation_guard()` now recovers every journal row whose holder is dead before
    returning (§9.6). The refusal helpers (`settle_or_refuse`, `refuse_if_interrupted`,
    Task 17) and their calls in `switch`, `add_live`, `add_token` and `remove` (Tasks 18–20)
    already exist; this task makes the recovery behind them real.
  - `switch --force` settles a remaining row: its journal write atomically replaces it
    (Task 16)

- [ ] **Step 1: Write the failing tests**

`crates/tagteam-engine/tests/recover.rs`:
```rust
mod common;

use common::Fx;
use serde_json::Value;
use tagteam_cc::shape::compose;
use tagteam_core::AccountId;
use tagteam_engine::EngineError;
use tagteam_engine::lifecycle::AddOptions;
use tagteam_engine::store::JournalRow;
use tagteam_engine::switch::{SwitchRequest, SwitchTarget};
use tagteam_provider::{ProcessStamp, Provider};

fn add(fx: &Fx, email: &str, rt: &str) -> AccountId {
    fx.login(email, rt);
    fx.engine.add_live(AddOptions { provider: fx.provider(), position: None, alias: None, yes: false }).unwrap().account.id
}

fn dead_holder() -> ProcessStamp {
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    ProcessStamp { pid, start: 0 }
}

/// Leaves a journal row as a switch from `from` to `to` that died after step 6.
fn crashed_switch(fx: &Fx, from: &AccountId, to: &AccountId) {
    let store = fx.engine.store().unwrap();
    let fp = |id: &AccountId| fx.cc.fingerprint(&fx.vault_bytes(id).unwrap()).unwrap().as_str().to_owned();
    let from_row = store.account(from).unwrap().unwrap();
    store
        .insert_journal(&JournalRow {
            provider: fx.provider(),
            holder: dead_holder(),
            from_id: Some(from.clone()),
            to_id: to.clone(),
            from_fp: Some(fp(from)),
            from_identity: Some(from_row.identity_json),
            to_fp: fp(to),
            started_at: 1,
            prior: None,
        })
        .unwrap();
}

/// What step 7 leaves live: the target credential, composed with the live machine-shared keys.
fn write_target_credential(fx: &Fx, to: &AccountId) {
    let live: Value = fx.live_credential().unwrap();
    let composed = compose(&fx.vault_bytes(to).unwrap(), live.as_object()).unwrap();
    fx.set_live_credential(&composed);
}

fn any_mutation(fx: &Fx, id: &AccountId) {
    fx.engine.set_disabled(id, false).unwrap(); // takes the mutation lock, so it recovers
}

#[test]
fn a_landed_credential_finishes_forward() {
    let fx = Fx::new();
    let a = add(&fx, "a@x.co", "rt-a");
    let b = add(&fx, "b@x.co", "rt-b"); // live: b
    crashed_switch(&fx, &b, &a);
    write_target_credential(&fx, &a); // died after writing the credential, before the identity
    any_mutation(&fx, &a);
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
    let store = fx.engine.store().unwrap();
    assert_eq!(store.active(&fx.provider()).unwrap(), Some(a));
    assert!(store.journal(&fx.provider()).unwrap().is_none());
}

#[test]
fn an_unlanded_credential_finishes_backward_without_touching_it() {
    let fx = Fx::new();
    let a = add(&fx, "a@x.co", "rt-a");
    let b = add(&fx, "b@x.co", "rt-b");
    crashed_switch(&fx, &b, &a);
    // The credential rollback succeeded but the identity rollback did not.
    let p = fx.paths();
    let doc = std::fs::read(&p.global_config).unwrap();
    let a_identity = Fx::oauth_account("a@x.co");
    std::fs::write(&p.global_config, tagteam_provider::splice::replace_top_level(&doc, "oauthAccount", &a_identity).unwrap()).unwrap();
    any_mutation(&fx, &a);
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-b"));
    assert!(fx.engine.store().unwrap().journal(&fx.provider()).unwrap().is_none());
}

#[test]
fn a_rotation_or_logout_after_the_crash_is_undecidable() {
    for logout in [false, true] {
        let fx = Fx::new();
        let a = add(&fx, "a@x.co", "rt-a");
        let b = add(&fx, "b@x.co", "rt-b");
        crashed_switch(&fx, &b, &a);
        write_target_credential(&fx, &a);
        if logout {
            use tagteam_provider::Keychain;
            fx.kc.delete(&tagteam_cc::keychain_service(&fx.env, tagteam_cc::ItemKind::OAuth), &tagteam_cc::keychain_account(&fx.env)).unwrap();
        } else {
            fx.rotate_live("rt-a-rotated-by-cc");
        }
        any_mutation(&fx, &a);
        let store = fx.engine.store().unwrap();
        assert!(store.journal(&fx.provider()).unwrap().is_some(), "logout={logout}");
        let req = |force| SwitchRequest { provider: fx.provider(), target: SwitchTarget::Account(b.clone()), force, source: "cli" };
        assert!(matches!(fx.engine.switch(req(false)), Err(EngineError::InterruptedSwitch(_))));
        assert!(matches!(
            fx.engine.add_live(AddOptions { provider: fx.provider(), position: None, alias: None, yes: false }),
            Err(EngineError::InterruptedSwitch(_))
        ));
        fx.engine.switch(req(true)).unwrap();
        assert!(store.journal(&fx.provider()).unwrap().is_none());
        assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    }
}

#[test]
fn a_stale_file_behind_an_unreadable_keychain_is_undecidable() {
    let fx = Fx::new();
    let a = add(&fx, "a@x.co", "rt-a");
    let b = add(&fx, "b@x.co", "rt-b");
    crashed_switch(&fx, &b, &a);
    write_target_credential(&fx, &a); // the Keychain now holds a's credential…
    let (svc, acct) = (tagteam_cc::keychain_service(&fx.env, tagteam_cc::ItemKind::OAuth), tagteam_cc::keychain_account(&fx.env));
    fx.kc.set_unreadable(&svc, &acct, true); // …but cannot be read,
    std::fs::write(fx.paths().credentials_file, Fx::credential_json("b@x.co", "rt-b").to_string()).unwrap(); // and a stale file says b
    any_mutation(&fx, &a);
    assert!(fx.engine.store().unwrap().journal(&fx.provider()).unwrap().is_some(), "never decided from a degraded read");
    fx.kc.set_unreadable(&svc, &acct, false);
}

#[test]
fn a_conflicting_auth_axis_keeps_the_row() {
    let fx = Fx::new();
    let a = add(&fx, "a@x.co", "rt-a");
    let b = add(&fx, "b@x.co", "rt-b");
    crashed_switch(&fx, &b, &a); // b's credential and identity are still live…
    fx.kc.put(&tagteam_cc::keychain_service(&fx.env, tagteam_cc::ItemKind::ManagedKey), &tagteam_cc::keychain_account(&fx.env), b"sk-ant-api03-c");
    any_mutation(&fx, &a); // …but an unrelated API key now authenticates too
    assert!(fx.engine.store().unwrap().journal(&fx.provider()).unwrap().is_some());
}

#[cfg(feature = "test-hooks")]
#[test]
fn a_row_that_appears_while_waiting_for_the_lock_is_recovered() {
    let fx = Fx::new();
    let a = add(&fx, "a@x.co", "rt-a");
    let b = add(&fx, "b@x.co", "rt-b");
    write_target_credential(&fx, &a); // a switch to a had written the credential, then died
    let store = fx.engine.store().unwrap();
    let fp = |id: &AccountId| fx.cc.fingerprint(&fx.vault_bytes(id).unwrap()).unwrap().as_str().to_owned();
    let row = JournalRow {
        provider: fx.provider(),
        holder: dead_holder(),
        from_id: Some(b.clone()),
        to_id: a.clone(),
        from_fp: Some(fp(&b)),
        from_identity: Some(store.account(&b).unwrap().unwrap().identity_json),
        to_fp: fp(&a),
        started_at: 1,
        prior: None,
    };
    // The row is written by "another process" after this command's pre-lock scan found nothing.
    let (writer, pending) = (store.clone(), std::sync::Mutex::new(Some(row)));
    fx.engine.on_point("before-mutation-lock", Box::new(move || {
        if let Some(row) = pending.lock().unwrap().take() {
            writer.insert_journal(&row).unwrap();
        }
    }));
    any_mutation(&fx, &a);
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
    assert!(store.journal(&fx.provider()).unwrap().is_none());
}

#[cfg(feature = "test-hooks")]
#[test]
fn forward_recovery_keeps_the_row_if_the_credential_changes_before_commit() {
    let fx = Fx::new();
    let a = add(&fx, "a@x.co", "rt-a");
    let b = add(&fx, "b@x.co", "rt-b");
    crashed_switch(&fx, &b, &a);
    write_target_credential(&fx, &a);
    // Between writing the identity and the final re-read, the credential becomes someone else's.
    let kc = fx.kc.clone();
    let (svc, acct) = (tagteam_cc::keychain_service(&fx.env, tagteam_cc::ItemKind::OAuth), tagteam_cc::keychain_account(&fx.env));
    fx.engine.on_point("recovery-before-commit", Box::new(move || {
        kc.put(&svc, &acct, Fx::credential_json("c@x.co", "rt-c").to_string().as_bytes());
    }));
    any_mutation(&fx, &a);
    assert!(fx.engine.store().unwrap().journal(&fx.provider()).unwrap().is_some());
}

#[cfg(feature = "test-hooks")]
#[test]
fn a_forced_switch_that_fails_puts_back_the_row_it_superseded() {
    let fx = Fx::new();
    let a = add(&fx, "a@x.co", "rt-a");
    let b = add(&fx, "b@x.co", "rt-b");
    crashed_switch(&fx, &b, &a);
    write_target_credential(&fx, &a);
    fx.rotate_live("rt-a-rotated-by-cc"); // undecidable
    any_mutation(&fx, &a);
    let store = fx.engine.store().unwrap();
    let unresolved = store.journal(&fx.provider()).unwrap().unwrap();
    fx.engine.fail_at(Some("after-journal"));
    let req = SwitchRequest { provider: fx.provider(), target: SwitchTarget::Account(b.clone()), force: true, source: "cli" };
    assert!(matches!(fx.engine.switch(req), Err(EngineError::RolledBack(_))));
    assert_eq!(store.journal(&fx.provider()).unwrap().unwrap(), unresolved, "the unresolved row is back");
}

#[test]
fn a_forced_switch_killed_before_landing_leaves_the_superseded_row() {
    let fx = Fx::new();
    let a = add(&fx, "a@x.co", "rt-a");
    let b = add(&fx, "b@x.co", "rt-b");
    crashed_switch(&fx, &b, &a);
    write_target_credential(&fx, &a);
    fx.rotate_live("rt-a-rotated-by-cc");
    let store = fx.engine.store().unwrap();
    let unresolved = store.journal(&fx.provider()).unwrap().unwrap();
    // `switch b --force` published its own row, carrying the old one, then died.
    let live_fp = fx.cc.fingerprint(fx.live_credential().unwrap().to_string().as_bytes()).unwrap().as_str().to_owned();
    let forced = JournalRow {
        provider: fx.provider(),
        holder: dead_holder(),
        from_id: Some(b.clone()),
        to_id: b.clone(),
        from_fp: Some(live_fp),
        from_identity: Some(store.account(&b).unwrap().unwrap().identity_json),
        to_fp: fx.cc.fingerprint(&fx.vault_bytes(&b).unwrap()).unwrap().as_str().to_owned(),
        started_at: 2,
        prior: Some(Box::new(unresolved.clone())),
    };
    store.insert_journal(&forced).unwrap();
    any_mutation(&fx, &a);
    assert_eq!(store.journal(&fx.provider()).unwrap().unwrap(), unresolved);
}

#[test]
fn a_live_holder_is_left_alone() {
    let fx = Fx::new();
    let a = add(&fx, "a@x.co", "rt-a");
    let b = add(&fx, "b@x.co", "rt-b");
    crashed_switch(&fx, &b, &a);
    let store = fx.engine.store().unwrap();
    let mut row = store.journal(&fx.provider()).unwrap().unwrap();
    row.holder = ProcessStamp::current().unwrap();
    store.delete_journal(&fx.provider()).unwrap();
    store.insert_journal(&row).unwrap();
    write_target_credential(&fx, &a);
    any_mutation(&fx, &a);
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"), "not recovered while its holder lives");
}

#[test]
fn two_engines_never_double_switch() {
    // Review Focus 3.
    let fx = Fx::new();
    let a = add(&fx, "a@x.co", "rt-a");
    let b = add(&fx, "b@x.co", "rt-b");
    let other = fx.engine_with_env(fx.env.clone());
    std::thread::scope(|s| {
        let t1 = s.spawn(|| fx.engine.switch(SwitchRequest { provider: fx.provider(), target: SwitchTarget::Account(a.clone()), force: false, source: "cli" }));
        let t2 = s.spawn(|| other.switch(SwitchRequest { provider: fx.provider(), target: SwitchTarget::Account(b.clone()), force: false, source: "cli" }));
        t1.join().unwrap().unwrap();
        t2.join().unwrap().unwrap();
    });
    // Whatever order they ran in, the live identity and credential name the same account.
    let email = fx.live_email().unwrap();
    let rt = fx.live_refresh_token().unwrap();
    assert!((email == "a@x.co" && rt == "rt-a") || (email == "b@x.co" && rt == "rt-b"), "{email} / {rt}");
    assert!(fx.engine.store().unwrap().journal(&fx.provider()).unwrap().is_none());
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tagteam-engine --test recover`
Expected: FAIL — rows are not recovered and nothing refuses.

- [ ] **Step 3: Implement recovery**

`crates/tagteam-engine/src/recover.rs`:
```rust
use tagteam_core::OracleVerdict;
use tagteam_provider::{Credential, Identity, LiveAuth, MutationGuard, Provenance, Provider, Read};

use crate::engine::Engine;
use crate::error::EngineError;
use crate::oracle::verdict;
use crate::store::{EventRow, JournalRow};

/// The oracle's answer about the live credential, asked before the mutation lock (§9.6).
pub(crate) struct RecoveryHint {
    bytes: Vec<u8>,
    owner: Option<Identity>,
}

/// The fingerprints on both auth axes, or `None` when either cannot be trusted: an unreadable
/// axis, or a degraded credential (the file covering an unreadable Keychain item, which may
/// be stale) leaves recovery undecidable.
fn live_fingerprints(p: &dyn Provider, live: &LiveAuth) -> Option<Vec<String>> {
    let mut out = Vec::new();
    match &live.credential {
        Read::Present(c) if c.provenance() == Provenance::Degraded => return None,
        Read::Present(c) => out.extend(p.fingerprint(c.bytes()).map(|f| f.as_str().to_owned())),
        Read::Absent => {}
        Read::Unreadable(_) => return None,
    }
    match &live.managed_key {
        Read::Present(b) => out.extend(p.fingerprint(b).map(|f| f.as_str().to_owned())),
        Read::Absent => {}
        Read::Unreadable(_) => return None,
    }
    Some(out)
}

/// Every auth surface agrees on one account of `kind` (§9.6): its own axis holds a fresh
/// credential (with fingerprint `fp`, when given), and the other axis carries no
/// authentication at all. A credential entry that only holds machine-shared keys has no
/// fingerprint, so it does not count as authentication.
fn axes_coherent(p: &dyn Provider, live: &LiveAuth, kind: &str, fp: Option<&str>) -> bool {
    let credential = match &live.credential {
        Read::Present(c) if c.provenance() == Provenance::Fresh => Some(c.bytes().to_vec()),
        Read::Present(_) | Read::Unreadable(_) => return false,
        Read::Absent => None,
    };
    let managed = match &live.managed_key {
        Read::Present(b) => Some(b.clone()),
        Read::Absent => None,
        Read::Unreadable(_) => return false,
    };
    let (own, other) = if kind == "api_key" { (managed, credential) } else { (credential, managed) };
    let own_ok = own.as_deref().and_then(|b| p.fingerprint(b)).is_some_and(|f| fp.is_none_or(|want| f.as_str() == want));
    let other_ok = other.as_deref().is_none_or(|b| p.fingerprint(b).is_none());
    own_ok && other_ok
}

impl Engine {
    pub(crate) fn dead_journals(&self) -> Result<Vec<JournalRow>, EngineError> {
        let Some(store) = self.existing_store()? else { return Ok(vec![]) };
        Ok(store.journals()?.into_iter().filter(|j| !j.holder.is_live()).collect())
    }

    pub(crate) fn recovery_hint(&self, row: &JournalRow) -> Option<RecoveryHint> {
        let p = self.provider(&row.provider).ok()?;
        let bytes = p.read_live_auth(&self.env).credential.present()?.bytes().to_vec();
        let owner = self.oracle.resolve(p.as_ref(), &Credential::fresh(bytes.clone()));
        Some(RecoveryHint { bytes, owner })
    }

    /// Finishes one interrupted switch in the direction the live credential decides, and
    /// clears the row only once every surface names the same account. Never writes an old
    /// credential back.
    pub(crate) fn recover_one(&self, guard: &MutationGuard, row: &JournalRow, hint: Option<&RecoveryHint>) -> Result<(), EngineError> {
        let p = self.provider(&row.provider)?;
        let store = self.store()?;
        let mut ids = vec![&row.to_id];
        if let Some(from) = &row.from_id {
            ids.push(from);
        }
        let _accounts = self.lock_accounts(&ids)?;
        let locks = p.lock_live(&self.env, guard)?;
        let live = p.read_live_auth(&self.env);
        let Some(fps) = live_fingerprints(p.as_ref(), &live) else { return Ok(()) };
        let owner_is = |id: &tagteam_core::AccountId| -> bool {
            let (Some(h), Some(account)) = (hint, store.account(id).ok().flatten()) else { return false };
            let same_bytes = live.credential.as_ref().present().is_some_and(|c| c.bytes() == h.bytes.as_slice());
            same_bytes && verdict(h.owner.as_ref(), &account) == OracleVerdict::ThisAccount
        };
        let target_live = fps.contains(&row.to_fp) || owner_is(&row.to_id);
        // The generation established as the target's: the journal's, or the one the oracle
        // attributed. The final re-read must still show exactly it.
        let established = if fps.contains(&row.to_fp) {
            Some(row.to_fp.clone())
        } else {
            hint.and_then(|h| p.fingerprint(&h.bytes)).map(|f| f.as_str().to_owned())
        };
        let outgoing_live =
            row.from_fp.as_ref().is_some_and(|f| fps.contains(f)) || row.from_id.as_ref().is_some_and(|id| owner_is(id));

        if target_live {
            let to = store.account(&row.to_id)?.ok_or_else(|| EngineError::NoSuchAccount(row.to_id.to_string()))?;
            let identity = p.parse_identity(&to.identity_json)?;
            p.clear_other_axis(&self.env, &locks, &to.kind)?;
            p.write_identity(&self.env, &locks, Some(&identity))?;
            crate::hooks::point(self, "recovery-before-commit")?;
            // Commit only while still holding CC's locks, and only when a fresh re-read of every
            // surface agrees on the target and still shows the generation established as its.
            let now_key = p.live_identity(&self.env).present().map(|i| p.identity_key(&i));
            let coherent = axes_coherent(p.as_ref(), &p.read_live_auth(&self.env), &to.kind, established.as_deref());
            if locks.check_owned().is_err() || !coherent || now_key.as_ref().map(|k| k.as_str()) != Some(to.identity_key.as_str()) {
                return Ok(()); // not coherent: keep the row
            }
            store.commit_switch(
                &row.provider,
                &to.id,
                &EventRow {
                    at: self.now_ms(),
                    provider: row.provider.clone(),
                    kind: "switch-recovered".into(),
                    from_id: row.from_id.clone(),
                    to_id: Some(to.id.clone()),
                    trigger: Some("recovery".into()),
                    source: "cli".into(),
                    detail: None,
                },
            )?;
        } else if outgoing_live {
            let from_identity = row.from_identity.as_ref().map(|v| p.parse_identity(v)).transpose()?;
            let current = p.live_identity(&self.env).present();
            if current.as_ref().map(|i| &i.raw) != from_identity.as_ref().map(|i| &i.raw) {
                p.write_identity(&self.env, &locks, from_identity.as_ref())?;
            }
            // Settle only when every surface agrees on the outgoing account: the identity is
            // back, its own axis still holds the outgoing generation, and the other axis
            // carries no authentication. An unmanaged outgoing login is taken as OAuth.
            let from_kind = match &row.from_id {
                Some(id) => store.account(id)?.map(|r| r.kind).unwrap_or_else(|| "oauth".into()),
                None => "oauth".into(),
            };
            let now = p.live_identity(&self.env).present();
            let coherent = axes_coherent(p.as_ref(), &p.read_live_auth(&self.env), &from_kind, row.from_fp.as_deref());
            if coherent && now.as_ref().map(|i| &i.raw) == from_identity.as_ref().map(|i| &i.raw) {
                // A forced switch that never landed puts back the unresolved row it superseded.
                match &row.prior {
                    Some(prior) => store.insert_journal(prior)?,
                    None => store.delete_journal(&row.provider)?,
                }
            }
        }
        // Anything else is undecidable: the row stays until `switch --force` settles it.
        Ok(())
    }
}
```

- [ ] **Step 4: Recover under the mutation lock**

Replace `Engine::mutation_guard` in `engine.rs`:
```rust
    /// tagteam's mutation lock. Before returning it, recovers every interrupted switch whose
    /// holder has died (§9.6). The oracle is asked before the lock is taken.
    pub fn mutation_guard(&self) -> Result<MutationGuard, EngineError> {
        // Oracle hints are gathered without the lock (§7.6)…
        let pending = self.dead_journals()?;
        let hints: Vec<_> = pending.iter().map(|j| (j.clone(), self.recovery_hint(j))).collect();
        crate::hooks::point(self, "before-mutation-lock")?;
        let guard = MutationGuard::acquire(&self.env, MutationGuard::TIMEOUT)?;
        // …but the rows are enumerated again under it: a switch may have died while this
        // command waited, and its row must be recovered now too (without a hint).
        for row in self.dead_journals()? {
            let hint = hints.iter().find(|(r, _)| *r == row).and_then(|(_, h)| h.as_ref());
            if let Err(e) = self.recover_one(&guard, &row, hint) {
                tracing::warn!(provider = %row.provider, "could not recover an interrupted switch: {e}");
            }
        }
        Ok(guard)
    }
```

`lib.rs` addition: `mod recover;`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p tagteam-engine --features test-hooks`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-engine
git commit -m "Recover interrupted switches from the journal"
```

---

### Task 22: The pinned local-state invariant

**Files:**
- Modify: `crates/tagteam-engine/tests/common/mod.rs` (snapshot and comparison)
- Test: `crates/tagteam-engine/tests/invariant.rs`

**Interfaces:**
- Produces (test support): `Fx::snapshot(&self) -> HomeSnapshot`,
  `Fx::assert_only_surface_changed(&self, before: &HomeSnapshot, after: &HomeSnapshot, step: &str)`

- [ ] **Step 1: Add the snapshot and comparison to the fixture**

Append to `crates/tagteam-engine/tests/common/mod.rs`:
```rust
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde_json::Map;
use tagteam_provider::splice::{get_top_level, remove_top_level};

/// Every file under HOME except tagteam's own data dir, plus every Keychain item except the
/// vault's.
pub struct HomeSnapshot {
    files: BTreeMap<PathBuf, Vec<u8>>,
    items: BTreeMap<(String, String), Vec<u8>>,
}

fn walk(dir: &std::path::Path, skip: &std::path::Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let path = e.path();
        if path.starts_with(skip) {
            continue;
        }
        let meta = fs::symlink_metadata(&path).unwrap();
        if meta.is_dir() {
            walk(&path, skip, out);
        } else {
            out.insert(path.clone(), fs::read(&path).unwrap_or_default());
        }
    }
}

/// §3: `customApiKeyResponses.approved` may only grow by appending; nothing else in that
/// object may change.
fn check_api_key_responses(before: Option<&Vec<u8>>, after: Option<&Vec<u8>>, step: &str, path: &std::path::Path) {
    let get = |d: Option<&Vec<u8>>| {
        d.and_then(|d| get_top_level(d, "customApiKeyResponses").unwrap()).unwrap_or_else(|| json!({}))
    };
    let (mut b, mut a) = (get(before), get(after));
    let approved = |v: &mut Value| {
        v.as_object_mut().and_then(|o| o.remove("approved")).and_then(|x| x.as_array().cloned()).unwrap_or_default()
    };
    let (b_list, a_list) = (approved(&mut b), approved(&mut a));
    assert!(a_list.starts_with(&b_list), "{step}: customApiKeyResponses.approved lost or reordered entries in {}", path.display());
    assert_eq!(b, a, "{step}: customApiKeyResponses changed beyond appending to approved in {}", path.display());
}

fn shared_keys(bytes: Option<&Vec<u8>>, keys: &[&str]) -> Map<String, Value> {
    let v: Value = bytes.and_then(|b| serde_json::from_slice(b).ok()).unwrap_or(Value::Null);
    v.as_object()
        .map(|o| o.iter().filter(|(k, _)| keys.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default()
}

impl Fx {
    pub fn snapshot(&self) -> HomeSnapshot {
        let mut files = BTreeMap::new();
        walk(&self.env.home, &self.env.data_dir(), &mut files);
        let items = self.kc.items().into_iter().filter(|((svc, _), _)| svc != "tagteam").collect();
        HomeSnapshot { files, items }
    }

    /// §15.3: every byte outside the identity surface is identical; inside it, only the
    /// declared keys moved, and the machine-shared credential keys kept their values.
    pub fn assert_only_surface_changed(&self, before: &HomeSnapshot, after: &HomeSnapshot, step: &str) {
        let surface = self.cc.identity_surface(&self.env);
        let json_keys: BTreeMap<PathBuf, Vec<String>> = surface.json_keys.iter().cloned().collect();
        let cred_files: BTreeSet<PathBuf> = surface.credential_files.iter().cloned().collect();
        let paths: BTreeSet<&PathBuf> = before.files.keys().chain(after.files.keys()).collect();
        for path in paths {
            let (b, a) = (before.files.get(path), after.files.get(path));
            if let Some(keys) = json_keys.get(path) {
                if keys.iter().any(|k| k == "customApiKeyResponses") {
                    check_api_key_responses(b, a, step, path);
                }
                let strip = |doc: Option<&Vec<u8>>| {
                    doc.map(|d| keys.iter().fold(d.clone(), |acc, k| remove_top_level(&acc, k).unwrap()))
                };
                assert_eq!(strip(b), strip(a), "{step}: {} changed outside {keys:?}", path.display());
            } else if cred_files.contains(path) {
                assert_eq!(
                    shared_keys(b, &surface.machine_shared_keys),
                    shared_keys(a, &surface.machine_shared_keys),
                    "{step}: machine-shared keys changed in {}",
                    path.display()
                );
            } else {
                assert_eq!(b, a, "{step}: {} changed", path.display());
            }
        }
        let owned: BTreeSet<(String, String)> = surface.owned_items.iter().cloned().collect();
        let creds: BTreeSet<(String, String)> = surface.credential_items.iter().cloned().collect();
        let keys: BTreeSet<&(String, String)> = before.items.keys().chain(after.items.keys()).collect();
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
}
```

- [ ] **Step 2: Write the invariant test**

`crates/tagteam-engine/tests/invariant.rs`:
```rust
mod common;

use common::Fx;
use tagteam_cc::live::Platform;
use tagteam_engine::lifecycle::{AddOptions, AddTokenOptions};
use tagteam_engine::switch::{SwitchRequest, SwitchTarget};

fn check(fx: &Fx, step: &str, op: impl FnOnce()) {
    let before = fx.snapshot();
    op();
    let after = fx.snapshot();
    fx.assert_only_surface_changed(&before, &after, step);
}

fn run_every_command(platform: Platform) {
    run_every_command_on(&Fx::with_platform(platform));
}

fn run_every_command_on(fx: &Fx) {
    let add = || AddOptions { provider: fx.provider(), position: None, alias: None, yes: false };
    let sw = |id: &tagteam_core::AccountId, force| SwitchRequest {
        provider: fx.provider(),
        target: SwitchTarget::Account(id.clone()),
        force,
        source: "cli",
    };

    fx.login("a@x.co", "rt-a");
    let mut a = None;
    check(&fx, "add a", || a = Some(fx.engine.add_live(add()).unwrap().account.id));
    let a = a.unwrap();
    fx.login("b@x.co", "rt-b");
    let mut b = None;
    check(&fx, "add b", || b = Some(fx.engine.add_live(add()).unwrap().account.id));
    let b = b.unwrap();
    let mut k = None;
    check(&fx, "add-token", || {
        k = Some(
            fx.engine
                .add_token(AddTokenOptions {
                    provider: fx.provider(),
                    token: "sk-ant-api03-invariant-key-000000000".into(),
                    position: None,
                    email: None,
                    alias: None,
                    yes: false,
                })
                .unwrap()
                .account
                .id,
        )
    });
    let k = k.unwrap();

    check(&fx, "switch b→a", || drop(fx.engine.switch(sw(&a, false)).unwrap()));
    fx.rotate_live("rt-a2"); // CC refreshes while a is live
    check(&fx, "switch a→api key", || drop(fx.engine.switch(sw(&k, false)).unwrap()));
    check(&fx, "switch api key→b", || drop(fx.engine.switch(sw(&b, false)).unwrap()));
    check(&fx, "forced self-switch", || drop(fx.engine.switch(sw(&b, true)).unwrap()));
    check(&fx, "rotation", || {
        drop(fx.engine.switch(SwitchRequest { provider: fx.provider(), target: SwitchTarget::Rotation, force: false, source: "cli" }).unwrap())
    });
    check(&fx, "alias", || drop(fx.engine.set_alias(&a, Some("work")).unwrap()));
    check(&fx, "disable", || drop(fx.engine.set_disabled(&a, true).unwrap()));
    check(&fx, "enable", || drop(fx.engine.set_disabled(&a, false).unwrap()));
    check(&fx, "move", || drop(fx.engine.move_to(&a, 3).unwrap()));
    check(&fx, "remove", || drop(fx.engine.remove(&k).unwrap()));
}

#[test]
fn every_m1_command_writes_only_the_identity_surface_on_macos() {
    run_every_command(Platform::MacOs);
}

#[test]
fn every_m1_command_writes_only_the_identity_surface_on_linux() {
    run_every_command(Platform::Linux);
}

#[test]
fn fallback_keychain_items_stay_within_the_surface() {
    // An explicit CLAUDE_CONFIG_DIR=~/.claude: readers also try the unsuffixed items, so a
    // switch may touch them, and the surface must say so.
    let fx = Fx::with(Platform::MacOs, |e| e.claude_config_dir = Some(e.home.join(".claude").into_os_string()));
    let acct = tagteam_cc::keychain_account(&fx.env);
    fx.kc.put("Claude Code-credentials", &acct, br#"{"mcpOAuth":{"fallback":1}}"#);
    run_every_command_on(&fx);
}

#[test]
fn the_comparison_catches_a_stray_write() {
    let fx = Fx::new();
    let before = fx.snapshot();
    std::fs::write(fx.env.home.join(".claude/CLAUDE.md"), "changed\n").unwrap();
    let after = fx.snapshot();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fx.assert_only_surface_changed(&before, &after, "stray")));
    assert!(r.is_err());
}

#[test]
fn the_comparison_allows_only_appends_to_approved_api_keys() {
    let set = |fx: &Fx, v: serde_json::Value| {
        let path = fx.paths().global_config;
        let doc = std::fs::read(&path).unwrap();
        std::fs::write(&path, tagteam_provider::splice::replace_top_level(&doc, "customApiKeyResponses", &v).unwrap()).unwrap();
    };
    let caught = |fx: &Fx, after_value: serde_json::Value| {
        set(fx, serde_json::json!({"approved": ["a"], "rejected": ["r"]}));
        let before = fx.snapshot();
        set(fx, after_value);
        let after = fx.snapshot();
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fx.assert_only_surface_changed(&before, &after, "api-keys"))).is_err()
    };
    let fx = Fx::new();
    assert!(!caught(&fx, serde_json::json!({"approved": ["a", "b"], "rejected": ["r"]})), "an append is allowed");
    assert!(caught(&fx, serde_json::json!({"approved": [], "rejected": ["r"]})), "a removal is not");
    assert!(caught(&fx, serde_json::json!({"approved": ["a"], "rejected": []})), "touching rejected is not");
}
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p tagteam-engine --test invariant`
Expected: PASS. If a step fails, the message names the file or Keychain item and the step;
fix the production code, never the comparison.

- [ ] **Step 4: Commit**

```bash
git add crates/tagteam-engine
git commit -m "Pin the local-state invariant across every M1 command"
```

---
### Task 23: The CLI

**Files:**
- Create: `crates/tagteam/src/{cli.rs,app.rs,render.rs,prompt.rs,root_guard.rs}`
- Modify: `crates/tagteam/src/lib.rs`
- Test: `crates/tagteam/tests/app.rs` (in-process), `crates/tagteam/tests/cli.rs` (the binary)

**Interfaces:**
- Consumes: the whole engine; `ClaudeCode`, `LiveStore`, `Platform`; `SecurityCli`, `FileKeychain`
- Produces:
```rust
pub mod cli { pub struct Cli { pub json: bool, pub debug: bool, pub no_color: bool,
    pub provider: Option<String>, pub command: Option<Command> } pub enum Command { .. } }
pub mod prompt { pub trait Prompter { fn interactive(&self) -> bool;
    fn confirm(&mut self, question: &str, default_yes: bool) -> bool;
    fn choose(&mut self, question: &str, options: &[String]) -> Option<usize>;
    fn secret(&mut self, question: &str) -> Option<String>; } pub struct TtyPrompter; }
pub mod app { pub struct Context { pub env: Env, pub keychain: Arc<dyn Keychain>, pub platform: Platform }
    impl Context { pub fn from_process() -> Context }
    pub struct Io<'a> { pub out: &'a mut dyn Write, pub err: &'a mut dyn Write, pub prompter: &'a mut dyn Prompter }
    pub fn run(cli: Cli, ctx: Context, io: &mut Io<'_>) -> i32; }
```
With the `test-support` feature, `Context::from_process` honours `TAGTEAM_TEST_KEYCHAIN_DIR`
(a `FileKeychain`) and `TAGTEAM_TEST_PLATFORM` (`macos` | `linux`); release builds contain
neither.

- [ ] **Step 1: Write the failing in-process tests**

`crates/tagteam/tests/app.rs`:
```rust
use std::collections::VecDeque;
use std::fs;
use std::sync::Arc;

use clap::Parser;
use serde_json::{Value, json};
use tagteam::app::{self, Context, Io};
use tagteam::cli::Cli;
use tagteam::prompt::Prompter;
use tagteam_cc::live::Platform;
use tagteam_cc::{ItemKind, keychain_account, keychain_service};
use tagteam_provider::splice::replace_top_level;
use tagteam_provider::{Env, FakeKeychain};

struct Scripted {
    interactive: bool,
    answers: VecDeque<&'static str>,
}

impl Scripted {
    fn none() -> Self {
        Self { interactive: false, answers: VecDeque::new() }
    }
    fn answering(a: &[&'static str]) -> Self {
        Self { interactive: true, answers: a.iter().copied().collect() }
    }
}

impl Prompter for Scripted {
    fn interactive(&self) -> bool {
        self.interactive
    }
    fn confirm(&mut self, _q: &str, default_yes: bool) -> bool {
        match self.answers.pop_front().expect("unexpected prompt") {
            "" => default_yes,
            a => a.starts_with('y'),
        }
    }
    fn choose(&mut self, _q: &str, _o: &[String]) -> Option<usize> {
        self.answers.pop_front().and_then(|a| a.parse().ok())
    }
    fn secret(&mut self, _q: &str) -> Option<String> {
        self.answers.pop_front().map(str::to_owned)
    }
}

struct H {
    _dir: tempfile::TempDir,
    env: Env,
    kc: Arc<FakeKeychain>,
}

impl H {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path());
        fs::create_dir_all(env.home.join(".claude")).unwrap();
        fs::write(env.home.join(".claude.json"), "{\n  \"userID\": \"u\"\n}\n").unwrap();
        H { _dir: dir, env, kc: Arc::new(FakeKeychain::new()) }
    }

    fn login(&self, email: &str, rt: &str) {
        let path = self.env.home.join(".claude.json");
        let doc = fs::read(&path).unwrap();
        let acct = json!({"emailAddress": email, "organizationUuid": "", "accountUuid": format!("uuid-{email}")});
        fs::write(&path, replace_top_level(&doc, "oauthAccount", &acct).unwrap()).unwrap();
        let cred = json!({"claudeAiOauth": {"accessToken": "at", "refreshToken": rt, "refreshTokenExpiresAt": 1_797_000_000_000i64}});
        self.kc.put(&keychain_service(&self.env, ItemKind::OAuth), &keychain_account(&self.env), cred.to_string().as_bytes());
    }

    fn run(&self, args: &[&str], prompter: &mut Scripted) -> (i32, String, String) {
        let cli = Cli::try_parse_from(std::iter::once("tagteam").chain(args.iter().copied())).unwrap();
        let ctx = Context { env: self.env.clone(), keychain: self.kc.clone(), platform: Platform::MacOs };
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = app::run(cli, ctx, &mut Io { out: &mut out, err: &mut err, prompter });
        (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
    }

    fn ok(&self, args: &[&str]) -> String {
        let (code, out, err) = self.run(args, &mut Scripted::none());
        assert_eq!(code, 0, "{args:?}: {err}");
        out
    }

    fn json(&self, args: &[&str]) -> Value {
        let mut v: Value = serde_json::from_str(&self.ok(args)).unwrap();
        normalize_ids(&mut v);
        v
    }
}

fn normalize_ids(v: &mut Value) {
    match v {
        Value::Object(o) => {
            for (k, x) in o.iter_mut() {
                if k == "id" && x.is_string() {
                    *x = json!("[id]");
                } else {
                    normalize_ids(x);
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(normalize_ids),
        _ => {}
    }
}

#[test]
fn an_empty_list_says_how_to_start() {
    let h = H::new();
    assert_eq!(h.ok(&["list"]), "No accounts yet. Log in with `claude`, then run `tagteam add`.\n");
    assert_eq!(h.ok(&[]), "No accounts yet. Log in with `claude`, then run `tagteam add`.\n");
}

#[test]
fn add_list_status_and_switch_read_like_this() {
    let h = H::new();
    h.login("a@x.co", "rt-a");
    assert_eq!(h.ok(&["add", "--alias", "work"]), "Added work (a@x.co) at position 1.\n");
    h.login("b@x.co", "rt-b");
    assert_eq!(h.ok(&["add"]), "Added b@x.co at position 2.\n");
    assert_eq!(h.ok(&["list"]), "  1  work (a@x.co)\n* 2  b@x.co\n");
    assert_eq!(h.ok(&["status"]), "Live: b@x.co (position 2 of 2)\n");
    assert_eq!(
        h.ok(&["switch", "work"]),
        "Switched to work (a@x.co) (position 1).\nClaude Code picks this up within about 30 s; restart it to apply now.\n"
    );
    assert_eq!(h.ok(&["ls"]), "* 1  work (a@x.co)\n  2  b@x.co\n");
    assert_eq!(h.ok(&["switch"]), "Switched to b@x.co (position 2).\nClaude Code picks this up within about 30 s; restart it to apply now.\n");
    assert_eq!(h.ok(&["switch", "2"]), "b@x.co is already active\n");
}

#[test]
fn list_json_is_cswap_compatible() {
    let h = H::new();
    h.login("a@x.co", "rt-a");
    h.ok(&["add"]);
    h.ok(&["add-token", "sk-ant-api03-key", "--alias", "ci"]);
    h.ok(&["disable", "ci"]);
    assert_eq!(
        h.json(&["list", "--json"]),
        json!({
            "schemaVersion": 1,
            "activeAccountNumber": 1,
            "activeByProvider": {"claude-code": 1},
            "accounts": [
                {"number": 1, "position": 1, "id": "[id]", "provider": "claude-code", "email": "a@x.co",
                 "organizationName": null, "organizationUuid": "", "isOrganization": false, "active": true,
                 "usageStatus": "unavailable", "usage": null, "lastGoodUsage": null, "lastGoodFetchedAt": null,
                 "lastGoodAgeSeconds": null, "usageError": "no-data", "usageRetryAt": null,
                 "loginExpiresAt": 1_797_000_000_000i64},
                {"number": 2, "position": 2, "id": "[id]", "provider": "claude-code", "email": "api-key-2@token.local",
                 "organizationName": null, "organizationUuid": "", "isOrganization": false, "active": false,
                 "usageStatus": "api_key", "usage": null, "lastGoodUsage": null, "lastGoodFetchedAt": null,
                 "lastGoodAgeSeconds": null, "alias": "ci", "disabled": true}
            ]
        })
    );
}

#[test]
fn status_and_switch_json() {
    let h = H::new();
    assert_eq!(h.json(&["status", "--json"]), json!({"schemaVersion": 1, "active": null}));
    h.login("stranger@x.co", "rt-s");
    assert_eq!(h.json(&["status", "--json"]), json!({"schemaVersion": 1, "active": {"email": "stranger@x.co", "managed": false}}));
    h.login("a@x.co", "rt-a");
    h.ok(&["add"]);
    h.login("b@x.co", "rt-b");
    h.ok(&["add"]);
    assert_eq!(
        h.json(&["switch", "1", "--json"]),
        json!({"schemaVersion": 1, "provider": "claude-code", "switched": true, "from": 2, "to": 1, "strategy": "direct",
               "reason": "switched", "message": "Switched to a@x.co", "warnings": []})
    );
    let status = h.json(&["status", "--json"]);
    assert_eq!(status["active"]["email"], "a@x.co");
    assert_eq!(status["active"]["managed"], true);
    assert_eq!(status["totalManagedAccounts"], 2);
}

#[test]
fn errors_are_one_json_object_with_a_stable_type() {
    let h = H::new();
    let (code, out, _) = h.run(&["switch", "9", "--json"], &mut Scripted::none());
    assert_eq!(code, 1);
    assert_eq!(
        serde_json::from_str::<Value>(&out).unwrap(),
        json!({"schemaVersion": 1, "error": {"type": "no-such-account", "message": "no account matches \"9\""}})
    );
    let (code, out, err) = h.run(&["switch", "9"], &mut Scripted::none());
    assert_eq!((code, out.as_str()), (1, ""));
    assert_eq!(err, "tagteam: no account matches \"9\"\n");
}

#[test]
fn an_unmanaged_login_is_offered_for_adding_on_a_terminal() {
    let h = H::new();
    h.login("a@x.co", "rt-a");
    h.ok(&["add"]);
    h.login("stranger@x.co", "rt-s");
    let (code, out, _) = h.run(&["switch", "1"], &mut Scripted::answering(&[""]));
    assert_eq!(code, 0);
    assert!(out.starts_with("Added stranger@x.co at position 2.\nSwitched to a@x.co (position 1).\n"), "{out}");
}

#[test]
fn prompts_never_block_a_non_interactive_caller() {
    // Review Focus 2.
    let h = H::new();
    h.login("a@x.co", "rt-a");
    h.ok(&["add"]);
    h.login("stranger@x.co", "rt-s");
    let (code, _, err) = h.run(&["switch", "1"], &mut Scripted::none());
    assert_eq!(code, 1);
    assert!(err.contains("tagteam add") && err.contains("--force"), "{err}");
    let noop = h.json(&["switch", "1", "--json"]);
    assert_eq!((noop["switched"].clone(), noop["reason"].clone()), (json!(false), json!("unmanaged-account")));
    let (code, _, err) = h.run(&["add", "--position", "1"], &mut Scripted::none());
    assert_eq!(code, 1);
    assert!(err.contains("--yes"), "{err}");
    let (code, _, _) = h.run(&["add", "--position", "1"], &mut Scripted::answering(&["y"]));
    assert_eq!(code, 0);
    assert_eq!(h.ok(&["list"]), "* 1  stranger@x.co\n");
    let (code, _, err) = h.run(&["add-token"], &mut Scripted::none());
    assert_eq!(code, 1);
    assert!(err.contains("`-`"), "{err}");
}

#[test]
fn alias_move_remove_and_usage_errors() {
    let h = H::new();
    h.login("a@x.co", "rt-a");
    h.ok(&["add"]);
    h.ok(&["add-token", "sk-ant-api03-key"]);
    assert_eq!(h.ok(&["alias", "1", "Work"]), "Position 1 is now work.\n");
    assert_eq!(h.ok(&["alias"]), "work  1  a@x.co\n");
    assert_eq!(h.ok(&["alias", "work", "--unset"]), "Position 1 has no alias now.\n");
    assert_eq!(h.ok(&["move", "1", "2"]), "a@x.co is now at position 2.\n");
    assert_eq!(h.ok(&["remove", "api-key-2@token.local"]), "Removed api-key-2@token.local (position 1).\n");
    let (code, _, _) = h.run(&["alias", "1"], &mut Scripted::none());
    assert_eq!(code, 2);
    let (code, out, _) = h.run(&["alias", "1", "--json"], &mut Scripted::none());
    assert_eq!(code, 2);
    assert_eq!(serde_json::from_str::<Value>(&out).unwrap()["error"]["type"], "usage");
}
```

`crates/tagteam/tests/cli.rs`:
```rust
//! Drives the real binary. Needs `--features test-support`.
#![cfg(feature = "test-support")]

use std::path::Path;

use assert_cmd::Command;
use serde_json::{Value, json};

fn cmd(root: &Path) -> Command {
    let mut c = Command::cargo_bin("tagteam").unwrap();
    c.env_clear()
        .env("HOME", root.join("home"))
        .env("USER", "tester")
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("TAGTEAM_TEST_KEYCHAIN_DIR", root.join("keychain"))
        .env("TAGTEAM_TEST_PLATFORM", "macos");
    c
}

#[test]
fn a_fresh_machine_lists_nothing_and_creates_nothing() {
    // Review Focus 4.
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("home")).unwrap();
    let out = cmd(d.path()).args(["list", "--json"]).assert().success().get_output().stdout.clone();
    assert_eq!(
        serde_json::from_slice::<Value>(&out).unwrap(),
        json!({"schemaVersion": 1, "activeAccountNumber": null, "activeByProvider": {"claude-code": null}, "accounts": []})
    );
    cmd(d.path()).args(["status", "--json"]).assert().success().stdout("{\"schemaVersion\":1,\"active\":null}\n");
    assert!(std::fs::read_dir(d.path().join("home")).unwrap().next().is_none(), "HOME must stay empty");
    cmd(d.path()).arg("add").assert().code(1).stderr(predicates::str::contains("no live login"));
}

#[test]
fn usage_errors_exit_2_and_keep_the_json_contract() {
    let d = tempfile::tempdir().unwrap();
    cmd(d.path()).args(["move", "1"]).assert().code(2);
    cmd(d.path()).arg("frobnicate").assert().code(2);
    let out = cmd(d.path()).args(["frobnicate", "--json"]).assert().code(2).get_output().stdout.clone();
    assert_eq!(serde_json::from_slice::<Value>(&out).unwrap()["error"]["type"], "usage");
}

#[test]
fn add_token_reads_a_line_from_stdin() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("home")).unwrap();
    cmd(d.path()).args(["add-token", "-"]).write_stdin("sk-ant-api03-from-stdin\n").assert().success();
    let out = cmd(d.path()).args(["list", "--json"]).assert().success().get_output().stdout.clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["accounts"][0]["usageStatus"], "api_key");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tagteam --features test-support`
Expected: FAIL to compile.

- [ ] **Step 3: Write the clap surface**

`crates/tagteam/src/cli.rs`:
```rust
use clap::{Parser, Subcommand};

/// No `Debug`: `add-token` carries a secret, and a derived `Debug` would print it.
#[derive(Parser)]
#[command(name = "tagteam", version, about = "Multi-account switcher for AI coding agent CLIs")]
pub struct Cli {
    /// Print exactly one JSON object on stdout
    #[arg(long, global = true)]
    pub json: bool,
    /// Log diagnostics to stderr
    #[arg(long, global = true)]
    pub debug: bool,
    /// Disable colour
    #[arg(long = "no-color", global = true)]
    pub no_color: bool,
    /// The agent CLI to act on (default: claude-code)
    #[arg(short = 'p', long, global = true, value_name = "PROVIDER")]
    pub provider: Option<String>,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// List stored accounts
    #[command(visible_alias = "ls")]
    List,
    /// Show the live account
    Status,
    /// Switch to the next account, or to ACCOUNT
    Switch {
        account: Option<String>,
        /// Activate even over an unmanaged live login, displacing it
        #[arg(long)]
        force: bool,
    },
    /// Store the current Claude Code login
    Add {
        #[arg(long)]
        position: Option<u32>,
        #[arg(long)]
        alias: Option<String>,
        /// Replace an account already at --position without asking
        #[arg(long)]
        yes: bool,
    },
    /// Store an API key or a setup token (`-` reads it from stdin)
    AddToken {
        token: Option<String>,
        #[arg(long)]
        position: Option<u32>,
        #[arg(long)]
        email: Option<String>,
        #[arg(long)]
        alias: Option<String>,
        #[arg(long)]
        yes: bool,
    },
    /// Delete a stored account (never the live login)
    #[command(visible_alias = "rm")]
    Remove { account: String },
    /// Hold an account out of automatic selection
    Disable { account: String },
    /// Return an account to automatic selection
    Enable { account: String },
    /// Set (ACCOUNT NAME), clear (ACCOUNT --unset) or list aliases
    Alias {
        account: Option<String>,
        name: Option<String>,
        #[arg(long)]
        unset: bool,
    },
    /// Move an account to POSITION, swapping if it is taken
    Move { account: String, position: u32 },
}
```

- [ ] **Step 4: Write the prompter and the root guard**

`crates/tagteam/src/prompt.rs`:
```rust
use std::io::{BufRead, IsTerminal, Write};

pub trait Prompter {
    /// True only when a person can answer: stdin and stderr are both terminals.
    fn interactive(&self) -> bool;
    fn confirm(&mut self, question: &str, default_yes: bool) -> bool;
    fn choose(&mut self, question: &str, options: &[String]) -> Option<usize>;
    fn secret(&mut self, question: &str) -> Option<String>;
}

pub struct TtyPrompter;

fn read_line() -> String {
    let mut s = String::new();
    let _ = std::io::stdin().lock().read_line(&mut s);
    s.trim().to_owned()
}

impl Prompter for TtyPrompter {
    fn interactive(&self) -> bool {
        std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
    }

    fn confirm(&mut self, question: &str, default_yes: bool) -> bool {
        eprint!("{question} {} ", if default_yes { "[Y/n]" } else { "[y/N]" });
        let _ = std::io::stderr().flush();
        match read_line().to_ascii_lowercase().as_str() {
            "" => default_yes,
            a => a == "y" || a == "yes",
        }
    }

    fn choose(&mut self, question: &str, options: &[String]) -> Option<usize> {
        for (i, o) in options.iter().enumerate() {
            eprintln!("  {}) {o}", i + 1);
        }
        eprint!("{question} [1-{}] ", options.len());
        let _ = std::io::stderr().flush();
        read_line().parse::<usize>().ok().filter(|n| (1..=options.len()).contains(n)).map(|n| n - 1)
    }

    fn secret(&mut self, question: &str) -> Option<String> {
        rpassword::prompt_password(question).ok()
    }
}
```

`crates/tagteam/src/root_guard.rs`:
```rust
use std::path::Path;

const MARKERS: [&str; 5] = ["docker", "lxc", "containerd", "kubepods", "overlay"];

fn mentions_container(text: &str) -> bool {
    MARKERS.iter().any(|m| text.contains(m))
}

fn in_container() -> bool {
    std::env::var_os("CONTAINER").is_some()
        || std::env::var_os("container").is_some()
        || Path::new("/.dockerenv").exists()
        || ["/proc/1/cgroup", "/proc/self/mountinfo"]
            .iter()
            .any(|p| std::fs::read_to_string(p).is_ok_and(|s| mentions_container(&s)))
}

/// §5: refuse to run as root outside a container, so no root-owned files land in the user's
/// directories (cswap's heuristics).
pub fn refuse_root() -> Result<(), String> {
    // SAFETY: geteuid has no preconditions and cannot fail.
    let root = unsafe { libc::geteuid() } == 0;
    if root && !in_container() {
        return Err("tagteam refuses to run as root outside a container; run it as your own user".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::mentions_container;

    #[test]
    fn container_markers_are_recognised() {
        assert!(mentions_container("0::/kubepods/besteffort/pod1"));
        assert!(mentions_container("overlay / overlay rw"));
        assert!(!mentions_container("0::/user.slice/user-1000.slice"));
    }
}
```

- [ ] **Step 5: Write the renderers**

`crates/tagteam/src/render.rs`:
```rust
use serde_json::{Value, json};
use tagteam_engine::store::AccountRow;
use tagteam_engine::switch::SwitchOutcome;
use tagteam_engine::views::{AccountView, ProviderAccounts, StatusView};

pub fn name(r: &AccountRow) -> String {
    let email = r.email.clone().unwrap_or_else(|| r.label.clone());
    match &r.alias {
        Some(a) => format!("{a} ({email})"),
        None => email,
    }
}

fn usage_status(r: &AccountRow) -> &'static str {
    if r.kind == "api_key" {
        "api_key"
    } else if r.quarantine_reason.is_some() {
        "relogin_required"
    } else {
        "unavailable"
    }
}

/// One `list` row (§13.2). Until M2 fetches usage, every row reports no data.
pub fn row_json(v: &AccountView) -> Value {
    let r = &v.row;
    let status = usage_status(r);
    let mut o = json!({
        "number": r.position,
        "position": r.position,
        "id": r.id.as_str(),
        "provider": r.provider.as_str(),
        "email": r.email.clone().unwrap_or_else(|| r.label.clone()),
        "organizationName": r.org_name,
        "organizationUuid": r.org_uuid,
        "isOrganization": !r.org_uuid.is_empty(),
        "active": v.active,
        "usageStatus": status,
        "usage": null,
        "lastGoodUsage": null,
        "lastGoodFetchedAt": null,
        "lastGoodAgeSeconds": null,
    });
    if status == "unavailable" {
        o["usageError"] = json!("no-data");
        o["usageRetryAt"] = Value::Null;
    }
    if let Some(a) = &r.alias {
        o["alias"] = json!(a);
    }
    if r.disabled {
        o["disabled"] = json!(true);
    }
    if let Some(e) = r.login_expires_at {
        o["loginExpiresAt"] = json!(e);
    }
    o
}

pub fn list_json(lists: &[ProviderAccounts]) -> Value {
    let active_by: serde_json::Map<String, Value> =
        lists.iter().map(|l| (l.provider.to_string(), json!(l.active_position))).collect();
    let rows: Vec<Value> = lists.iter().flat_map(|l| l.accounts.iter().map(row_json)).collect();
    json!({
        "schemaVersion": 1,
        "activeAccountNumber": lists.first().and_then(|l| l.active_position),
        "activeByProvider": active_by,
        "accounts": rows,
    })
}

pub fn list_human(lists: &[ProviderAccounts], display_names: &dyn Fn(&str) -> String) -> String {
    if lists.iter().all(|l| l.accounts.is_empty()) {
        return "No accounts yet. Log in with `claude`, then run `tagteam add`.\n".into();
    }
    let shown: Vec<&ProviderAccounts> = lists.iter().filter(|l| !l.accounts.is_empty()).collect();
    let mut s = String::new();
    for l in &shown {
        if shown.len() > 1 {
            s.push_str(&format!("{}\n", display_names(l.provider.as_str())));
        }
        for v in &l.accounts {
            let r = &v.row;
            let mut line = format!("{} {}  {}", if v.active { "*" } else { " " }, r.position, name(r));
            if let Some(org) = &r.org_name {
                line.push_str(&format!("  [{org}]"));
            }
            match r.kind.as_str() {
                "api_key" => line.push_str("  api key"),
                "setup_token" => line.push_str("  setup token"),
                _ => {}
            }
            if r.disabled {
                line.push_str("  disabled");
            }
            s.push_str(&line);
            s.push('\n');
        }
    }
    s
}

pub fn status_json(s: &StatusView) -> Value {
    match s {
        StatusView::NoLogin => json!({"schemaVersion": 1, "active": null}),
        StatusView::Unmanaged { email } => json!({"schemaVersion": 1, "active": {"email": email, "managed": false}}),
        StatusView::Managed { account, total } => {
            let mut row = row_json(account);
            row["managed"] = json!(true);
            json!({"schemaVersion": 1, "active": row, "totalManagedAccounts": total})
        }
    }
}

pub fn status_human(s: &StatusView) -> String {
    match s {
        StatusView::NoLogin => "No live login.\n".into(),
        StatusView::Unmanaged { email } => format!("Live: {email} (not managed by tagteam)\n"),
        StatusView::Managed { account, total } => {
            format!("Live: {} (position {} of {total})\n", name(&account.row), account.row.position)
        }
    }
}

pub fn switch_json(o: &SwitchOutcome, provider: &str) -> Value {
    json!({
        "schemaVersion": 1,
        "provider": provider,
        "switched": o.switched,
        "from": o.from.as_ref().map(|r| r.position),
        "to": o.to.as_ref().map(|r| r.position),
        "strategy": o.strategy,
        "reason": o.reason.as_str(),
        "message": o.message,
        "warnings": o.warnings,
    })
}

pub fn switch_human(o: &SwitchOutcome) -> String {
    match (&o.to, o.switched) {
        (Some(to), true) => {
            let hint = if o.file_store {
                "Active on your next message."
            } else {
                "Claude Code picks this up within about 30 s; restart it to apply now."
            };
            format!("Switched to {} (position {}).\n{hint}\n", name(to), to.position)
        }
        _ => format!("{}\n", o.message),
    }
}

pub fn account_json(row: &AccountRow, created: Option<bool>) -> Value {
    let mut v = json!({"schemaVersion": 1, "ok": true, "account": row_json(&AccountView { row: row.clone(), active: false })});
    if let Some(c) = created {
        v["created"] = json!(c);
    }
    v
}
```

- [ ] **Step 6: Write the application layer**

`crates/tagteam/src/app.rs`:
```rust
use std::io::{BufRead, Write};
use std::sync::Arc;

use serde_json::json;
use tagteam_cc::ClaudeCode;
use tagteam_cc::live::Platform;
use tagteam_core::{CLAUDE_CODE, ProviderId};
use tagteam_engine::lifecycle::{AddOptions, AddTokenOptions};
use tagteam_engine::oracle::NoOracle;
use tagteam_engine::registry::ProviderRegistry;
use tagteam_engine::store::AccountRow;
use tagteam_engine::switch::{SwitchReason, SwitchRequest, SwitchTarget};
use tagteam_engine::vault::{FileVault, KeychainVault, Vault};
use tagteam_engine::{Engine, EngineConfig, EngineError};
use tagteam_provider::security::SecurityCli;
use tagteam_provider::{Env, Keychain, SystemClock};

use crate::cli::{Cli, Command};
use crate::prompt::Prompter;
use crate::{render, root_guard};

pub struct Context {
    pub env: Env,
    pub keychain: Arc<dyn Keychain>,
    pub platform: Platform,
}

#[cfg(feature = "test-support")]
fn test_overrides() -> (Option<Arc<dyn Keychain>>, Option<Platform>) {
    let kc = std::env::var_os("TAGTEAM_TEST_KEYCHAIN_DIR")
        .map(|d| Arc::new(tagteam_provider::FileKeychain::new(std::path::PathBuf::from(d))) as Arc<dyn Keychain>);
    let platform = match std::env::var("TAGTEAM_TEST_PLATFORM").as_deref() {
        Ok("linux") => Some(Platform::Linux),
        Ok("macos") => Some(Platform::MacOs),
        _ => None,
    };
    (kc, platform)
}

#[cfg(not(feature = "test-support"))]
fn test_overrides() -> (Option<Arc<dyn Keychain>>, Option<Platform>) {
    (None, None)
}

impl Context {
    pub fn from_process() -> Self {
        let (kc, platform) = test_overrides();
        Self {
            env: Env::from_process(),
            keychain: kc.unwrap_or_else(|| Arc::new(SecurityCli::new())),
            platform: platform.unwrap_or_else(Platform::current),
        }
    }
}

pub struct Io<'a> {
    pub out: &'a mut dyn Write,
    pub err: &'a mut dyn Write,
    pub prompter: &'a mut dyn Prompter,
}

fn build_engine(ctx: Context) -> Engine {
    let cc = Arc::new(ClaudeCode::new(ctx.keychain.clone(), ctx.platform));
    let vault = match ctx.platform {
        Platform::MacOs => Vault::new(Box::new(KeychainVault::new(ctx.keychain))),
        Platform::Linux => Vault::new(Box::new(FileVault::new(ctx.env.data_dir().join("vault")))),
    };
    Engine::new(EngineConfig {
        env: ctx.env,
        registry: ProviderRegistry::new().with(cc),
        vault,
        oracle: Arc::new(NoOracle),
        clock: Arc::new(SystemClock),
        default_provider: ProviderId::new(CLAUDE_CODE),
    })
}

fn init_logging(debug: bool, color: bool) {
    let level = if debug { tracing::Level::DEBUG } else { tracing::Level::ERROR };
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(color)
        .with_target(false)
        .with_max_level(level)
        .try_init();
}

enum Failure {
    Engine(EngineError),
    Usage(String),
    Message(&'static str, String),
}

impl From<EngineError> for Failure {
    fn from(e: EngineError) -> Self {
        Failure::Engine(e)
    }
}

struct App<'a, 'b> {
    engine: Engine,
    json: bool,
    provider_flag: Option<ProviderId>,
    io: &'a mut Io<'b>,
}

pub fn run(cli: Cli, ctx: Context, io: &mut Io<'_>) -> i32 {
    let color = !cli.no_color && std::env::var_os("NO_COLOR").is_none();
    init_logging(cli.debug, color);
    let json = cli.json;
    if let Err(msg) = root_guard::refuse_root() {
        return fail(io, json, "root", &msg);
    }
    let mut app = App { engine: build_engine(ctx), json, provider_flag: cli.provider.map(ProviderId::new), io };
    if let Some(p) = &app.provider_flag {
        if let Err(e) = app.engine.provider(p) {
            return fail(app.io, json, e.kind(), &e.to_string());
        }
    }
    let result = app.dispatch(cli.command.unwrap_or(Command::List));
    match result {
        Ok(()) => 0,
        Err(Failure::Engine(e)) => fail(app.io, json, e.kind(), &e.to_string()),
        Err(Failure::Message(kind, m)) => fail(app.io, json, kind, &m),
        Err(Failure::Usage(m)) => {
            fail(app.io, json, "usage", &m);
            2
        }
    }
}

fn fail(io: &mut Io<'_>, json: bool, kind: &str, message: &str) -> i32 {
    if json {
        let _ = writeln!(io.out, "{}", json!({"schemaVersion": 1, "error": {"type": kind, "message": message}}));
    } else {
        let _ = writeln!(io.err, "tagteam: {message}");
    }
    1
}

impl App<'_, '_> {
    fn provider(&self) -> ProviderId {
        self.provider_flag.clone().unwrap_or_else(|| self.engine.default_provider().clone())
    }

    fn print(&mut self, human: &str, json: serde_json::Value) {
        if self.json {
            let _ = writeln!(self.io.out, "{json}");
        } else {
            let _ = write!(self.io.out, "{human}");
        }
    }

    fn notices(&mut self, notices: &[String]) {
        for n in notices {
            let _ = writeln!(self.io.err, "note: {n}");
        }
    }

    /// §10.4, with a terminal prompt for an ambiguous email.
    fn resolve(&mut self, input: &str) -> Result<AccountRow, Failure> {
        match self.engine.resolve(input, self.provider_flag.as_ref()) {
            Err(EngineError::Ambiguous { .. }) if !self.json && self.io.prompter.interactive() => {
                let found = self.engine.candidates(input, self.provider_flag.as_ref())?;
                let labels: Vec<String> =
                    found.iter().map(|r| format!("{} #{} {}", r.provider, r.position, render::name(r))).collect();
                match self.io.prompter.choose("Which account?", &labels) {
                    Some(i) => Ok(found[i].clone()),
                    None => Err(Failure::Message("cancelled", "cancelled".into())),
                }
            }
            other => Ok(other?),
        }
    }

    fn dispatch(&mut self, command: Command) -> Result<(), Failure> {
        match command {
            Command::List => {
                let lists = self.engine.accounts(self.provider_flag.as_ref())?;
                let names = |id: &str| {
                    self.engine.provider(&ProviderId::new(id)).map_or_else(|_| id.to_owned(), |p| p.display_name().to_owned())
                };
                let human = render::list_human(&lists, &names);
                self.print(&human, render::list_json(&lists));
            }
            Command::Status => {
                let s = self.engine.status(&self.provider())?;
                self.print(&render::status_human(&s), render::status_json(&s));
            }
            Command::Switch { account, force } => self.switch(account, force)?,
            Command::Add { position, alias, yes } => self.add(position, alias, yes)?,
            Command::AddToken { token, position, email, alias, yes } => {
                let token = match token.as_deref() {
                    Some("-") => {
                        let mut line = String::new();
                        std::io::stdin().lock().read_line(&mut line).map_err(EngineError::Io)?;
                        line
                    }
                    Some(t) => t.to_owned(),
                    None if !self.json && self.io.prompter.interactive() => self
                        .io
                        .prompter
                        .secret("Token: ")
                        .ok_or_else(|| Failure::Message("cancelled", "cancelled".into()))?,
                    None => {
                        return Err(Failure::Message(
                            "invalid-input",
                            "pass the token as an argument, or `-` to read it from stdin".into(),
                        ));
                    }
                };
                let out = self.engine.add_token(AddTokenOptions { provider: self.provider(), token, position, email, alias, yes })?;
                let human = format!("Added {} at position {}.\n", render::name(&out.account), out.account.position);
                self.print(&human, render::account_json(&out.account, Some(out.created)));
            }
            Command::Remove { account } => {
                let row = self.resolve(&account)?;
                let row = self.engine.remove(&row.id)?;
                self.print(&format!("Removed {} (position {}).\n", render::name(&row), row.position), render::account_json(&row, None));
            }
            Command::Disable { account } => {
                let row = self.resolve(&account)?;
                let row = self.engine.set_disabled(&row.id, true)?;
                self.print(&format!("{} is disabled.\n", render::name(&row)), render::account_json(&row, None));
            }
            Command::Enable { account } => {
                let row = self.resolve(&account)?;
                let row = self.engine.set_disabled(&row.id, false)?;
                self.print(&format!("{} is enabled.\n", render::name(&row)), render::account_json(&row, None));
            }
            Command::Alias { account, name, unset } => match (account, name, unset) {
                (None, None, false) => {
                    let lists = self.engine.accounts(self.provider_flag.as_ref())?;
                    let rows: Vec<&AccountRow> = lists.iter().flat_map(|l| l.accounts.iter().map(|v| &v.row)).filter(|r| r.alias.is_some()).collect();
                    let human: String = rows
                        .iter()
                        .map(|r| format!("{}  {}  {}\n", r.alias.as_deref().unwrap_or_default(), r.position, r.email.clone().unwrap_or_else(|| r.label.clone())))
                        .collect();
                    let json = json!({"schemaVersion": 1, "aliases": rows.iter().map(|r| json!({"alias": r.alias, "number": r.position, "provider": r.provider.as_str()})).collect::<Vec<_>>()});
                    self.print(&human, json);
                }
                (Some(account), Some(name), false) => {
                    let row = self.resolve(&account)?;
                    let row = self.engine.set_alias(&row.id, Some(&name))?;
                    let alias = row.alias.clone().unwrap_or_default();
                    self.print(&format!("Position {} is now {alias}.\n", row.position), render::account_json(&row, None));
                }
                (Some(account), None, true) => {
                    let row = self.resolve(&account)?;
                    let row = self.engine.set_alias(&row.id, None)?;
                    self.print(&format!("Position {} has no alias now.\n", row.position), render::account_json(&row, None));
                }
                _ => return Err(Failure::Usage("alias takes ACCOUNT NAME, ACCOUNT --unset, or no arguments".into())),
            },
            Command::Move { account, position } => {
                let row = self.resolve(&account)?;
                let row = self.engine.move_to(&row.id, position)?;
                let email = row.email.clone().unwrap_or_else(|| row.label.clone());
                self.print(&format!("{email} is now at position {}.\n", row.position), render::account_json(&row, None));
            }
        }
        Ok(())
    }

    fn add(&mut self, position: Option<u32>, alias: Option<String>, yes: bool) -> Result<(), Failure> {
        let provider = self.provider();
        let opts = |yes| AddOptions { provider: provider.clone(), position, alias: alias.clone(), yes };
        let out = match self.engine.add_live(opts(yes)) {
            Err(EngineError::NeedsConfirmation { position, occupant }) if !self.json && self.io.prompter.interactive() => {
                if !self.io.prompter.confirm(&format!("Position {position} holds {occupant}. Replace it?"), false) {
                    return Err(Failure::Message("cancelled", "cancelled".into()));
                }
                self.engine.add_live(opts(true))?
            }
            other => other?,
        };
        self.notices(&out.notices);
        let verb = if out.created { "Added" } else { "Updated" };
        let human = format!("{verb} {} at position {}.\n", render::name(&out.account), out.account.position);
        self.print(&human, render::account_json(&out.account, Some(out.created)));
        Ok(())
    }

    fn switch(&mut self, account: Option<String>, force: bool) -> Result<(), Failure> {
        let (target, provider) = match &account {
            Some(a) => {
                let row = self.resolve(a)?;
                (SwitchTarget::Account(row.id.clone()), row.provider)
            }
            None => (SwitchTarget::Rotation, self.provider()),
        };
        let req = || SwitchRequest { provider: provider.clone(), target: target.clone(), force, source: "cli" };
        let mut outcome = self.engine.switch(req())?;
        if outcome.reason == SwitchReason::UnmanagedAccount && !self.json {
            let email = outcome.unmanaged_email.clone().unwrap_or_default();
            if !self.io.prompter.interactive() {
                return Err(Failure::Message(
                    "unmanaged-account",
                    format!("the live login ({email}) is not managed by tagteam; run `tagteam add` first, or pass --force"),
                ));
            }
            if !self.io.prompter.confirm(&format!("Add the current login ({email}) first?"), true) {
                return Err(Failure::Message("cancelled", "cancelled".into()));
            }
            self.add(None, None, false)?;
            outcome = self.engine.switch(req())?;
        }
        for w in &outcome.warnings {
            let _ = writeln!(self.io.err, "warning: {w}");
        }
        self.print(&render::switch_human(&outcome), render::switch_json(&outcome, provider.as_str()));
        Ok(())
    }
}
```

`crates/tagteam/src/lib.rs`:
```rust
use std::ffi::OsString;

use clap::Parser;

pub mod app;
pub mod cli;
pub mod prompt;
mod render;
mod root_guard;

/// Runs the CLI and returns the process exit code.
pub fn main_with_args<I, T>(args: I) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let args: Vec<OsString> = args.into_iter().map(Into::into).collect();
    let json = args.iter().any(|a| a == "--json");
    let cli = match cli::Cli::try_parse_from(&args) {
        Ok(c) => c,
        // `--json` promises one JSON object on stdout even for a usage error (B.36).
        Err(e) if json && e.use_stderr() => {
            let message = e.kind().to_string();
            println!("{}", serde_json::json!({"schemaVersion": 1, "error": {"type": "usage", "message": message}}));
            return 2;
        }
        Err(e) => {
            let code = if e.use_stderr() { 2 } else { 0 };
            let _ = e.print();
            return code;
        }
    };
    let mut prompter = prompt::TtyPrompter;
    let (mut out, mut err) = (std::io::stdout().lock(), std::io::stderr().lock());
    app::run(cli, app::Context::from_process(), &mut app::Io { out: &mut out, err: &mut err, prompter: &mut prompter })
}
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test -p tagteam --features test-support`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/tagteam
git commit -m "Add the tagteam command line for M1"
```

---
### Task 24: Kill tests through the real binary

**Files:**
- Test: `crates/tagteam/tests/kill.rs`

**Interfaces:**
- Consumes: the `test-support` feature (`TAGTEAM_TEST_KEYCHAIN_DIR`, `TAGTEAM_TEST_PLATFORM`,
  `TAGTEAM_TEST_CRASH_AT`), `CcPaths`, `FileKeychain`

- [ ] **Step 1: Write the tests**

`crates/tagteam/tests/kill.rs`:
```rust
//! §15.2: kill the process at each switch step, then prove the next command recovers it.
#![cfg(feature = "test-support")]

use std::fs;
use std::time::{Duration, SystemTime};

use assert_cmd::Command;
use serde_json::{Value, json};
use tagteam_cc::{CcPaths, ItemKind, keychain_account, keychain_service};
use tagteam_provider::splice::{get_top_level, replace_top_level};
use tagteam_provider::{Env, FileKeychain, Keychain};

struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("home/.claude")).unwrap();
        fs::write(dir.path().join("home/.claude.json"), "{\n  \"userID\": \"u\"\n}\n").unwrap();
        Home { dir }
    }

    fn env(&self) -> Env {
        Env::for_test(self.dir.path())
    }

    fn kc(&self) -> FileKeychain {
        FileKeychain::new(self.dir.path().join("keychain"))
    }

    fn oauth_item(&self) -> (String, String) {
        (keychain_service(&self.env(), ItemKind::OAuth), keychain_account(&self.env()))
    }

    fn cmd(&self) -> Command {
        let mut c = Command::cargo_bin("tagteam").unwrap();
        c.env_clear()
            .env("HOME", self.dir.path().join("home"))
            .env("USER", "tester")
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("TAGTEAM_TEST_KEYCHAIN_DIR", self.dir.path().join("keychain"))
            .env("TAGTEAM_TEST_PLATFORM", "macos");
        c
    }

    fn login(&self, email: &str, rt: &str) {
        let path = self.env().home.join(".claude.json");
        let doc = fs::read(&path).unwrap();
        let acct = json!({"emailAddress": email, "organizationUuid": "", "accountUuid": format!("uuid-{email}")});
        fs::write(&path, replace_top_level(&doc, "oauthAccount", &acct).unwrap()).unwrap();
        self.set_live_rt(rt);
    }

    fn set_live_rt(&self, rt: &str) {
        let (svc, acct) = self.oauth_item();
        let cred = json!({"claudeAiOauth": {"accessToken": "at", "refreshToken": rt}});
        self.kc().upsert(&svc, &acct, cred.to_string().as_bytes()).unwrap();
    }

    fn live(&self) -> (String, String) {
        let doc = fs::read(self.env().home.join(".claude.json")).unwrap();
        let email = get_top_level(&doc, "oauthAccount").unwrap().unwrap()["emailAddress"].as_str().unwrap().to_owned();
        let (svc, acct) = self.oauth_item();
        let cred: Value = serde_json::from_slice(&self.kc().find(&svc, &acct).present().unwrap()).unwrap();
        (email, cred["claudeAiOauth"]["refreshToken"].as_str().unwrap().to_owned())
    }

    /// A killed process leaves CC's mkdir locks behind; age them past their staleness.
    fn age_cc_locks(&self) {
        let p = CcPaths::resolve(&self.env());
        for lock in [p.refresh_lock.clone(), p.legacy_lock(), p.config_lock.clone()] {
            if lock.exists() {
                fs::File::open(&lock).unwrap().set_modified(SystemTime::now() - Duration::from_secs(120)).unwrap();
            }
        }
    }

    fn two_accounts(&self) {
        self.login("a@x.co", "rt-a");
        self.cmd().arg("add").assert().success();
        self.login("b@x.co", "rt-b");
        self.cmd().arg("add").assert().success();
    }
}

#[test]
fn a_kill_at_any_step_is_recovered_by_the_next_command() {
    for (point, lands) in [("after-journal", false), ("after-credential", true), ("after-identity", true)] {
        let h = Home::new();
        h.two_accounts();
        h.cmd().args(["switch", "1"]).env("TAGTEAM_TEST_CRASH_AT", point).assert().failure();
        h.age_cc_locks();
        h.cmd().args(["disable", "2"]).assert().success(); // any mutating command recovers
        let expected = if lands { ("a@x.co", "rt-a") } else { ("b@x.co", "rt-b") };
        let (email, rt) = h.live();
        assert_eq!((email.as_str(), rt.as_str()), expected, "{point}");
        let list: Value = serde_json::from_slice(&h.cmd().args(["list", "--json"]).assert().success().get_output().stdout).unwrap();
        assert_eq!(list["activeAccountNumber"], json!(if lands { 1 } else { 2 }), "{point}");
        h.cmd().args(["enable", "2"]).assert().success();
        h.cmd().arg("switch").assert().success(); // no interrupted switch is left behind
    }
}

#[test]
fn a_cc_rotation_after_the_kill_needs_force() {
    let h = Home::new();
    h.two_accounts();
    h.cmd().args(["switch", "1"]).env("TAGTEAM_TEST_CRASH_AT", "after-credential").assert().failure();
    h.age_cc_locks();
    h.set_live_rt("rt-a-rotated-by-cc");
    h.cmd().args(["switch", "2"]).assert().code(1).stderr(predicates::str::contains("interrupted switch"));
    h.cmd().args(["switch", "2", "--force"]).assert().success();
    assert_eq!(h.live(), ("b@x.co".to_owned(), "rt-b".to_owned()));
}
```

- [ ] **Step 2: Run the tests**

Run: `cargo test -p tagteam --features test-support --test kill`
Expected: PASS. Everything they need was built in Tasks 20, 21 and 23, so a failure here is
a real recovery bug: fix the engine, not the test.

- [ ] **Step 3: Commit**

```bash
git add crates/tagteam/tests/kill.rs
git commit -m "Kill the binary at each switch step and verify recovery"
```

---

### Task 25: Final verification

**Files:**
- Modify: this plan's `**Status:**` line

- [ ] **Step 1: Format, lint and test everything**

Run:
```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo test --workspace --features tagteam/test-support
cargo test -p tagteam-engine --test switch -- --ignored
```
Expected: no diff, no warnings, every test passes. On a Mac, also run
`cargo test -p tagteam-provider --features real_keychain --test real_keychain` (PASS, no GUI
prompt).

- [ ] **Step 2: Build the release binary and check it read-only against the real login**

Run: `cargo build --release && ./target/release/tagteam status && ./target/release/tagteam list`
Expected: `status` names the live Claude Code login as not managed (or `No live login.`);
`list` says there are no accounts; neither command creates `~/.local/share/tagteam`
(`ls ~/.local/share/tagteam` fails). Adding or switching real accounts is Michael's call, not
part of this task.

- [ ] **Step 3: Mark the plan implemented**

Set this plan's `**Status:**` to `Implemented`, citing the merge request once one exists (the
repository has no remote yet; cite the branch head instead until it does). The spec stays
`In progress` until M5 lands.

```bash
git add docs/superpowers/plans/2026-09-27-tagteam-m1-foundation-switching.md
git commit -m "Mark the M1 plan implemented"
```
