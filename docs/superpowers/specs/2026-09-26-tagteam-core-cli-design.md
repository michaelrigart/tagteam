# tagteam — sub-project 1: Core + CLI

**Status:** In progress
**Date:** 2026-09-26
**Scope:** Sub-project 1 of 4, plus a provider extension point. The daemon (2), TUI (3) and
macOS menu bar (4) get their own specs and must not require reshaping anything defined here.
The same holds for further agent CLIs added as providers (5+: Codex first, then others such as
Gemini CLI or Grok).

**Reference implementation.** [realiti4/claude-swap](https://github.com/realiti4/claude-swap)
(MIT) at commit `9aa6d02` (v0.27.0b1). Citations take the form `cswap:<file>:<line>` and are
relative to `src/claude_swap/`. Claude Code facts were verified against Claude Code
**2.1.283**, and re-verified against **2.1.286** on 2026-10-01 for parallel sessions (§12,
Appendix A), except where marked *inferred*.

---

## 1. Purpose

tagteam is a multi-account switcher for AI coding agent CLIs, starting with Claude Code. It
stores several logins per agent, switches the active one, runs accounts side by side, and
switches automatically before a rate limit.

The engine is **provider-neutral**. Every agent-specific behaviour lives behind the `Provider`
trait (§4.5), and Claude Code is the first and, in this sub-project, only implementation.

For Claude Code, tagteam is a Rust rewrite of claude-swap ("cswap"), aiming for:

- **A single static binary** with instant startup and no Python runtime.
- **A cleaner architecture**: bounded crates, one transactional store, and read/provenance
  discipline enforced in the type system.
- **Better performance**: store-served views, lazy secret reads, and no process-per-surface
  polling.
- **New capabilities**: usage history, a Claude Code statusline, encrypted export, `doctor`,
  and a shared daemon (sub-project 2).

tagteam is public OSS under the MIT license, built for macOS and Linux.

### 1.1 Success criteria

1. A switch changes only the account identity. All other local Claude Code state is untouched
   (§3).
2. Every refresh-token successor tagteam receives is persisted, to the vault or to `rescue/`,
   or to `displaced/` when the token endpoint says it belongs to another account (§7.4). A
   degraded or superseded generation is never sent to the token endpoint (§7). The one
   exception, every write failing, is reported rather than silent (§7.3).
3. Polling stays within the usage endpoint's budget across all tagteam processes on a machine
   (§8).
4. The core commands match cswap on macOS and Linux (Appendix C), with JSON that existing
   cswap scripts can consume.
5. `tagteam statusline` completes in ≤ 10 ms p95. `tagteam list` completes in ≤ 50 ms p95 when
   no usage fetch is due. Both are measured on Apple Silicon.
6. Each target ships as one static binary. The only runtime dependencies are `/usr/bin/security`
   (macOS) and `claude`.

### 1.2 Non-goals for this sub-project

- **Windows.** Platform code sits behind traits so a port stays possible, but nothing is built
  or tested for Windows.
- **Later sub-projects.** The daemon, socket API and notifications (#2), the TUI (#3) and the
  menu bar (#4).
- **Migrating a cswap store.** Only cswap *export files* can be imported.
- **Self-upgrade and update checks.** Package managers handle upgrades.
- **Providers other than Claude Code.** Codex, Gemini CLI, Grok and others each get their own
  sub-project and spec. Before that spec, their storage, identity, refresh, usage and locking
  must be verified against the real CLI. This sub-project only guarantees they can be added
  without reshaping the store, the engine or the CLI.

## 2. Terminology

| Term | Meaning |
|---|---|
| CC | Claude Code |
| Provider | An agent CLI that tagteam manages accounts for, identified by a `ProviderId` such as `claude-code` (§4.5) |
| Identity surface | The exact set of provider-owned state a provider's switch may write (§3) |
| Live login | The credential and `oauthAccount` that CC currently uses for the default profile |
| Global config | CC's resolved global config file (Appendix A.1): `~/.claude/.config.json` where that legacy file exists, otherwise `~/.claude.json`. Written as `~/.claude.json` throughout |
| Secure-storage dir | The directory CC keeps credentials and their locks under (Appendix A.1). Normally the config home |
| Account | A login that tagteam stores and manages |
| Position | The number users type to refer to an account. It is display and rotation order only |
| Generation | One state of a credential. Its fingerprint is `sha256:<hex(sha256(secret))>`, where the secret is the refresh token when the credential has one, otherwise its access token (setup tokens) or the key itself (API keys). Every credential kind therefore has a distinct fingerprint, and for OAuth it identifies the refresh-token lineage |
| Login epoch | A per-account counter that every explicit login replacement increments (§12.5) |
| Vault | tagteam's per-account secret storage |
| Profile | A per-account CC config dir used by `tagteam run` |
| Profile marker | `<profile>/.tagteam-profile.json`: the profile's account, its exported config-dir spelling, and the outer home it shares from (§12.2) |
| Run shell | A process whose `CLAUDE_CONFIG_DIR` names a directory holding a profile marker: `claude` under `tagteam run`, and everything it spawns (§12.8) |
| Launch reservation | tagteam's own record of a `run` in progress, written before `claude` starts and removed after exit handling (§12.5) |
| Quiescent | A profile with no live launch reservation, and no live or unreadable session record |
| Session-owned | An account whose profile is not quiescent |
| Activation epoch | The login epoch an account had when tagteam last made it the live login, kept with the store's active account (§6.1, §12.5) |
| Account lock | The per-account `flock` that every vault write for that account holds (§6.2) |

## 3. The local-state invariant (hard requirement)

Switching accounts must never affect an agent's local state: memory, instruction files,
history, projects, settings, skills, agents, commands, plugins or MCP configuration.

**This holds for every provider.** Each provider declares its **identity surface**: the exact,
closed list of provider-owned state that its switch, refresh and `run` paths may write.
Everything outside that list is off-limits. The pinned test in §15.3 is generic: it runs once
per provider, against that provider's fixture home and declared surface.

The Claude Code identity surface is the complete list of writes tagteam makes to CC-owned
state:

| What is written | Written by |
|---|---|
| The account-scoped keys of the active credential entry (Keychain `Claude Code-credentials[-hash]` or `<secure-storage dir>/.credentials.json`), and the managed-key item `Claude Code[-hash]`. The entry's machine-shared keys (Appendix A.4) are carried over unchanged | switch, auto-switch, active-token refresh |
| The `oauthAccount` value in `~/.claude.json` | switch, auto-switch |
| `primaryApiKey`, and the `customApiKeyResponses.approved` list (append only), in `~/.claude.json` | activating or deactivating an API-key account |
| The `projects` and `mcpServers` subtrees of `~/.claude.json`, by three-way merge (§12.4) | `tagteam run` on exit |
| `~/.claude/projects/` and `~/.claude/history.jsonl`, created empty and **only when absent**, as CC itself would create them | `tagteam run`, before linking (§12.2) |

Nothing else under `~/.claude/`, and no other key of `~/.claude.json`, is ever written. Each
rewrite of `~/.claude.json` replaces only the byte span of the affected value (§9.5), so every
other byte stays identical.

A test pins this invariant (§15.3).

## 4. Architecture

### 4.1 Crates (Cargo workspace, edition 2024)

| Crate | Responsibility | I/O |
|---|---|---|
| `tagteam-core` | Pure, provider-neutral domain types and policy: `ProviderId`, identities, generic usage windows, headroom, poll planning, backoff, pace and projection, outgoing-credential classification, and auto-switch `decide()` | None; time is passed in |
| `tagteam-provider` | The `Provider` trait (§4.5) and the I/O primitives that providers share: `Read<T>` and provenance, `FreshCredential`, the atomic write-through-symlink writer, the span-preserving JSON splice, the `/usr/bin/security` driver, `mkdir` and `flock` lock helpers, pid liveness, and the `Http` and `Clock` ports | Primitives only |
| `tagteam-cc` | `impl Provider for ClaudeCode`: everything whose shape CC dictates — path and store resolution, Keychain naming, reads and writes of the active credential (OAuth and managed-key axes), CC's `proper-lockfile` lock protocol, `~/.claude.json` handling, endpoints, poll budget, session records and profile handling | Yes, through `tagteam-provider` |
| `tagteam-engine` | The SQLite store, the vault, the `ureq`-backed `Http` implementation, the provider registry, and the provider-generic operations: switch, refresh gate, usage collector, auto-switch loop, sessions, export/import, doctor | Yes |
| `tagteam` | The CLI binary (clap), human and JSON rendering, and the prompts | Thin |
| `tagteam-fake` | Test-only (`publish = false`): the `FakeAgent` provider (§15.2). It obeys the provider dependency rules below, so it also proves that a new provider is one crate plus one registry line | Its own fixture home only |

**Dependency rules.**
- A provider crate depends only on `tagteam-core` and `tagteam-provider`, never on
  `tagteam-engine`.
- The engine reaches a provider only through the trait. Nothing in `tagteam-engine` names a CC
  path, key or endpoint.
- A future provider is one new crate, such as `tagteam-codex`, plus one line in the registry.

Later sub-projects add crates that depend on `tagteam-engine`: `tagteam-daemon`, `tagteam-tui`
and `tagteam-menubar`.

### 4.2 Engine surface

`Engine` is built from an injected `Env`, a `ProviderRegistry`, and a set of ports: `Keychain`,
`Clock`, `Http`, `ProcessProbe`, and a cancel token that the CLI's signal handler sets (§14.1).
Every account-scoped operation resolves the account's
provider from the registry. Operations that take no account take a `ProviderId`, which
defaults to `default_provider` (§6.4). Its operations return typed outcomes:

```rust
impl Engine {
    fn accounts(&self) -> Result<Vec<AccountView>>;            // store-served
    fn status(&self) -> Result<StatusView>;
    fn collect_usage(&self, mode: CollectMode) -> Result<CollectReport>;
    fn switch(&self, req: SwitchRequest) -> Result<SwitchOutcome>;
    fn add_live(&self, opts: AddOptions) -> Result<AddOutcome>;
    fn add_token(&self, opts: AddTokenOptions) -> Result<AddOutcome>;
    fn remove(&self, acct: AccountRef) -> Result<()>;
    fn set_alias / set_disabled / move_to(...) -> Result<()>;
    fn auto(&self, provider: &ProviderId, cfg: AutoConfig) -> Result<AutoEngine>; // holds the engine lock (§11.1)
    // AutoEngine::tick(&mut self, sink: &dyn EventSink) -> TickOutcome; next_delay(...) (§11.4)
    fn run_session(&self, req: RunRequest) -> Result<ExitStatus>;
    fn export / import / history / doctor / purge / displaced(...);
}
```

`EventSink` receives typed events: switch, poll, quarantine, sleep, error and so on. The CLI's
`--json` JSONL output is one sink. The daemon's socket broadcast will be another.

### 4.3 Type-level rules

- **Every read is tri-state.** Any read of a credential, config, roster or session record
  returns `Read<T> = Present(T) | Absent | Unreadable(ReadError)`. "Unreadable" is never
  collapsed into "absent" or into an empty value.
- **Credentials carry provenance.** A credential is tagged `Fresh` or `Degraded`. A `Degraded`
  read is one where the Keychain lookup failed and the plaintext file covered it, so the bytes
  may be a superseded generation. The refresh gate accepts only `FreshCredential`, which cannot
  be built from a degraded read.
- **Lock order is carried by the guards.** A provider's live locks come in two stages. Its
  credential locks (`CredLocks`) can only be acquired from a held `MutationGuard`, and its
  config lock (`ConfigLock`) only from held `CredLocks`. `LiveLocks` is the pair: a switch and
  a recovery take both, while the active-token refresh (§7.5) holds only the credential locks
  across its request and takes the config lock after it. For Claude Code the credential locks
  are the refresh and legacy locks, and the config lock is `~/.claude.json.lock` (§9.1). CC's
  storage-write lock is a leaf: it is taken only under the credential locks, held only around
  one credential entry's write, and nothing else is taken while it is held (§9.1).
  Order: tagteam mutation lock → account locks (ascending account ID) → provider live locks
  (credential locks → config lock). Any prefix may be skipped, but a lock is never taken while
  holding one that comes later: in particular, `MutationGuard` is never taken while holding an
  account lock. The one exception is a standalone config lock for profile seeding and
  merge-back (§12.4), which take no
  credential lock while holding it; taking a lone lock cannot invert the order. The auto-switch
  engine lock (§11.1) is outside the order altogether: it is only ever tried, never waited
  for, and is held for an engine's lifetime. tagteam's settings lock (§6.4) and the log's
  rotation lock (§14.2) are outside it too: each is taken alone, and nothing else is taken
  while it is held. The displaced lock (§6.3) is a leaf, like CC's storage-write lock: it may
  be taken under any other lock, and nothing is taken while it is held.
- **No network while holding a contended lock.** There are two exceptions, both bounded:
  - the refresh gate holds the account lock across its token request (§7.3, 10 s), which is
    what makes it single-flight;
  - the active-token refresh holds CC's credential locks across its request (§7.5, 6 s).

### 4.4 Runtime choices

- **HTTP.** Blocking `ureq` with rustls and `rustls-platform-verifier` (the OS trust store, so
  corporate TLS proxies work). Fetches for several accounts run in parallel on scoped threads,
  each start staggered by 250 ms. There is no async runtime in sub-project 1.
  - The `Http` port (in `tagteam-provider`) sends one request with its own timeout and returns
    a response or a transport error. The error is `PreSend` only when the request provably
    never left the machine: a DNS, connect or TLS-handshake failure. Everything else,
    including a timeout or a reset after connecting, is `Ambiguous`. Misfiling a pre-send
    failure as ambiguous is harmless, because both retry the same generation (§7.3); the
    reverse would not be.
  - The port sets `User-Agent: tagteam/<version>` on every request, so no provider can omit
    it, and caps response bodies at 1 MiB. A request's `Debug` output never shows its
    `Authorization` header or its body.
- **SQLite.** `rusqlite` with the `bundled` feature.
- **JSON.** `serde_json` with `preserve_order` and `arbitrary_precision` for anything
  round-tripped. The `~/.claude.json` splice is span-based and never re-serializes the file
  (§9.5).
- **Secrets.** Keychain access goes through `/usr/bin/security` only. The Security.framework
  is never used for Keychain items (§17, R1). The HTTP stack's `rustls-platform-verifier` does
  link it on macOS, but only for certificate trust evaluation, which touches no Keychain item.
- **Proxies.** The HTTP adapter honours the standard proxy environment variables (`HTTPS_PROXY`,
  `ALL_PROXY`, `NO_PROXY`, …). When a proxy applies to a request, tagteam does not resolve the
  host itself; the proxy does, and a failure to reach the proxy is `PreSend`. Tests never
  inherit the environment's proxy.
- **Encryption.** The `age` crate: passphrase via scrypt, or X25519 and SSH recipients.
- **Logging.** `tracing` to a rolling file, controlled by `--debug` or `TAGTEAM_LOG` (§14.2).
  Log lines identify accounts by position and ID, never by email, because users paste logs
  into public issues.

### 4.5 The `Provider` trait

The trait is the only way the engine touches agent-specific state. The sketch below shows its
shape; exact signatures are settled in the plan.

```rust
pub trait Provider: Send + Sync {
    // Identity and capabilities
    fn id(&self) -> ProviderId;                        // "claude-code"
    fn display_name(&self) -> &'static str;           // "Claude Code"
    fn capabilities(&self) -> Capabilities;           // usage, refresh, api_keys, sessions, statusline
    fn identity_surface(&self, env: &Env) -> IdentitySurface;   // §3; drives the pinned test

    // Identity and stored shapes (§6.1)
    fn identity_key(&self, id: &Identity) -> IdentityKey;   // CC: email + org uuid
    fn credential_kinds(&self) -> &'static [&'static str];   // CC: oauth, setup_token, api_key
    fn kind_traits(&self, kind: &str) -> KindTraits;        // refreshable? managed-key axis? default-email prefix
    fn primary_long_window(&self) -> Option<&'static str>;  // a window key; CC: "7d"; ranks consume-first (§11.2)
    fn export_login / import_login(...);                    // provider-owned export payload (§13.3)

    // The live login
    fn live_identity(&self, env: &Env) -> Read<LiveIdentity>;
    fn read_active(&self, env: &Env) -> Read<Credential>;        // carries provenance
    fn lock_credentials<'g>(&self, env: &Env, g: &'g MutationGuard) -> Result<CredLocks<'g>>;
    fn lock_config<'c>(&self, env: &Env, cred: &'c CredLocks<'_>) -> Result<ConfigLock<'c>>;
    fn lock_live<'g>(&self, env: &Env, g: &'g MutationGuard) -> Result<LiveLocks<'g>>; // both; one budget (§9.1)
    fn activate(&self, env: &Env, locks: &LiveLocks, target: &StoredLogin,
                live: Read<&Credential>) -> Result<ActivationUndo>;   // compose + write surface
    fn capture(&self, env: &Env) -> Result<CapturedLogin>;             // for `add`

    // Credential semantics
    fn classify(&self, bytes: &[u8]) -> CredentialKind;
    fn fingerprint(&self, cred: &Credential) -> Fingerprint;          // §2 "Generation"; every kind
    fn expiry(&self, cred: &Credential) -> Expiry;                     // access-token expiry (§7.2)

    // Network (the engine supplies the Http port and owns locks, leases, CAS and rescue)
    fn refresh(&self, http: &dyn Http, cred: &FreshCredential) -> RefreshResult;
        // Refreshed { successor, identity hint } | Dead(reason) | Systemic | Transient(kind)
    fn resolve_owner(&self, http: &dyn Http, cred: &Credential) -> Option<Identity>;
        // None, with no request, when the credential has no token to show (§7.6)
    fn fetch_usage(&self, http: &dyn Http, cred: &Credential) -> UsageResult; // generic windows
    fn poll_budget(&self) -> PollBudget;              // the §8.6 constants and hourly request cap

    // Parallel sessions
    fn launch_command(&self) -> &'static str;         // "claude"
    fn session_env(&self, profile: &ProfileDir) -> SessionEnv;  // vars to set and to scrub (§12.5)
    fn outer_home(&self, env: &Env) -> OuterHome;     // the home vars a profile records (§12.2, §12.8)
    fn share_policy(&self, env: &Env) -> SharePolicy;      // source home + allowlist + private list (§12.2)
    fn seed_profile / capture_profile / merge_back(...);   // §12.3–12.5
    fn session_records(&self, profile: &ProfileDir) -> Read<Vec<SessionRecord>>;
    fn validate_profile(&self, profile: &ProfileDir) -> Validity;  // §12.3 step 8's outcomes

    // Diagnostics
    fn doctor_checks(&self, env: &Env) -> Vec<Check>;    // §13.6; Check { id, status, message, fix }
}
```

**Capabilities are explicit.** A provider may lack a usage endpoint or refreshable tokens, and
the engine degrades rather than failing:
- Without `usage`: accounts show `usageStatus: unsupported`, and auto-switch can only fail over
  on authentication failure.
- Without `sessions`: `run` refuses for that provider.
- Without `statusline`: the `statusline` command refuses for that provider.

**Usage windows are generic:** `Window { key, label, kind, pct, resets_at, period_s, detail }`,
where `kind` is one of `Short | Long | Spend | Scoped` and `detail` is optional provider-owned
JSON (CC's spend amounts, for example). The store persists usage only in this form (§8.2).
- Claude Code maps `5h` → `Short`, `7d` → `Long` (period 604800 s), `spend` → `Spend`, and each
  per-model limit → `Scoped(name)`.
- A provider need not have a `Long` window. Without a `primary_long_window`, consume-first is
  unavailable for that provider. `auto --strategy consume-first` with `--provider` naming it is
  a usage error (exit 2). Otherwise a `consume-first` strategy, from the flag or from
  `autoswitch.strategy`, makes that provider's engine run `best` instead, with one
  `config-warning`.
- Pace (§8.7: the average fallback, `expectedPct`, `aheadOfPace`) applies to `Long` and
  `Scoped` windows that have a known period. The regression rate and the projections
  (`projectedExhaustionAt`, `willLastToReset`) apply to every window.
- Headroom and decisions (§8.2, §11) are defined over "relevant windows" rather than fixed
  names.

**The engine owns every cross-provider guarantee.** Account locks, leases, the refresh gate's
CAS and rescue, quarantine, the vault, the store, backoff, the request budget, the auto-switch
policy, the switch transaction's ordering, journal and rollback, launch reservations, and the
local-state test are all generic. A provider supplies only the
agent-specific facts.

- **`refresh`** builds the token request, parses the response, and composes the successor's
  bytes (for CC, the token fields replaced inside the stored JSON, with every other key kept).
  It classifies the response as §7.3 step 7 does, except for the lineage re-read, which is the
  engine's. It never touches the vault, a lock or `rescue/`.
- **Kind traits.** The engine and the CLI never name a provider's credential kinds.
  `kind_traits` tells them whether a kind refreshes, whether it lives on a separate
  managed-key axis, and which prefix a defaulted `add-token` email uses (§10.2).

This trait is internal API, not a public plugin interface. It is expected to change when the
second provider lands (§17, R7).

## 5. Platforms and paths

All tagteam paths follow XDG on both macOS and Linux. They honour `XDG_*` variables that are
set to absolute paths; otherwise they use the XDG defaults. Directories are created lazily with
mode 0700, so a command that changes nothing creates nothing.

| Purpose | Path |
|---|---|
| Settings | `$XDG_CONFIG_HOME/tagteam/config.toml` |
| Store | `$XDG_DATA_HOME/tagteam/tagteam.db` (plus `-wal`, `-shm`) |
| Vault (Linux) | `$XDG_DATA_HOME/tagteam/vault/<id>.json`, `<id>.prev.json` |
| Rescued successors | `$XDG_DATA_HOME/tagteam/rescue/<id>-<epoch>-<fp12>.json` |
| Displaced foreign credentials | `$XDG_DATA_HOME/tagteam/displaced/<epoch>-<fp12>-<rand6>.json` |
| Session profiles | `$XDG_DATA_HOME/tagteam/sessions/<id>/`, with its marker in `<profile>/.tagteam-profile.json` (§12.2), its provenance (login epoch and seed generation) in `<profile>/.tagteam-seed.json`, and the links tagteam made in `<profile>/.tagteam-links.json` |
| Launch reservations | `<profile>/.tagteam-launch/<pid>.lock` |
| Mutation lock | `$XDG_DATA_HOME/tagteam/.mutation.lock` |
| Account locks | `$XDG_DATA_HOME/tagteam/locks/<id>.lock` |
| Auto-switch engine locks | `$XDG_DATA_HOME/tagteam/locks/autoswitch-<provider>.lock`, holding the engine's pid and start time (§11.1) |
| Settings lock | `$XDG_DATA_HOME/tagteam/locks/config.lock` (§6.4) |
| Displaced lock | `$XDG_DATA_HOME/tagteam/locks/displaced.lock` (§6.3) |
| Log | `$XDG_STATE_HOME/tagteam/tagteam.log`, rotated to `.1` and `.2` (1 MiB × 3), with its rotation lock `tagteam.log.lock` (§14.2) |

Every file that contains secrets is created with mode 0600 at creation time (`O_EXCL`, then
write, then rename). It is never chmod'ed afterwards.

**`HOME` must be usable.** Every default path derives from it, so tagteam refuses to run, with
exit 1 and kind `env`, when `HOME` is unset, empty or not absolute; a fallback would put state
under `/` or the working directory. `statusline` prints nothing instead.

tagteam refuses to run as root unless `/` is an overlay mount: the visible root mount in
`/proc/self/mountinfo` has filesystem type `overlay`, as in a Docker container. On macOS, which
has no `/proc`, it always refuses. Containers whose root is not an overlay (LXC on ext4, for
example) are deliberately refused too. This prevents root-owned files in user directories.

CC path and store resolution is specified in Appendix A.

## 6. Data model

### 6.1 Store (`tagteam.db`)

The store opens in WAL mode with `busy_timeout = 5000`, `foreign_keys = ON` and
`synchronous = NORMAL`. Migrations are embedded, forward-only and tracked by
`PRAGMA user_version`. The database file is created with mode 0600, since it names every
account; SQLite gives its `-wal` and `-shm` files the same mode.

Transactions are short. No transaction is held across a network call or a `security` spawn.

```sql
CREATE TABLE accounts (
  id               TEXT PRIMARY KEY,          -- UUIDv7, immutable storage key (unique across providers)
  provider         TEXT NOT NULL,             -- ProviderId, e.g. 'claude-code'
  position         INTEGER NOT NULL,          -- user-facing number, >= 1, per provider
  identity_key     TEXT NOT NULL,             -- provider-derived (CC: email + org uuid); the account's identity
  label            TEXT NOT NULL,             -- display name (CC: the email)
  email            TEXT,                      -- NULL for providers whose logins have none
  org_uuid         TEXT NOT NULL DEFAULT '',  -- org/workspace id; '' = personal or none
  org_name         TEXT,
  account_uuid     TEXT,                      -- provider account id; NULL until known; backfilled only while NULL
  kind             TEXT NOT NULL,             -- one of the provider's credential_kinds() (CC: oauth | setup_token | api_key)
  alias            TEXT UNIQUE COLLATE NOCASE,  -- unique across providers, so an alias alone is unambiguous
  disabled         INTEGER NOT NULL DEFAULT 0,
  identity_json    TEXT NOT NULL,             -- provider-owned identity object (CC: the oauthAccount object only)
  login_expires_at INTEGER,                   -- epoch ms (CC: refreshTokenExpiresAt)
  login_epoch      INTEGER NOT NULL DEFAULT 0,  -- bumped by every explicit login replacement (§12.5)
  replacing_fp     TEXT,                      -- set only while an explicit replacement is in flight (§12.5)
  replacing_meta   TEXT,                      -- JSON snapshot of the incoming login's metadata; installed on landing, discarded otherwise (§12.5)
  quarantine_reason TEXT,                     -- 'invalid_grant' | 'no_refresh_token' | 'identity_conflict' | 'successor_lost'
  quarantine_fp    TEXT,                      -- fingerprint the quarantine is bound to
  quarantine_at    INTEGER,
  added_at         INTEGER NOT NULL,
  UNIQUE (provider, position),
  UNIQUE (provider, identity_key)
);

CREATE TABLE active_accounts (  -- the store's active account per provider (§9.4 step 9); the live identity wins if they disagree
  provider   TEXT PRIMARY KEY,
  account_id TEXT REFERENCES accounts(id) ON DELETE SET NULL,
  login_epoch INTEGER           -- activation epoch: account_id's login_epoch when tagteam made it live, or before a replacement superseded the live login (§12.5); NULL only with account_id
);

CREATE TABLE usage_state (
  account_id        TEXT PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
  last_good         TEXT,     -- JSON array of generic windows (§4.5, §8.2); never cleared by a failure
  fetched_at        INTEGER,  -- epoch s, success only
  last_attempt_at   INTEGER,
  consecutive_failures INTEGER NOT NULL DEFAULT 0,
  last_error        TEXT,     -- kind token: http-<code> | pre-send | ambiguous | bad-response | refresh-failed | over-budget | ...
  backoff_until     INTEGER,
  next_poll_at      INTEGER,
  poll_interval_s   INTEGER,
  last_429_at       INTEGER,  -- never cleared by success
  rejected_fp       TEXT      -- access-token fingerprint a live session had refused (§8.1)
);

CREATE TABLE usage_samples (
  account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  window     TEXT NOT NULL,   -- provider window key; CC: '5h' | '7d' | 'spend' | 'scoped:<model display name>'
  fetched_at INTEGER NOT NULL,
  pct        REAL NOT NULL,
  resets_at  INTEGER,
  PRIMARY KEY (account_id, window, fetched_at)
) WITHOUT ROWID;

CREATE TABLE usage_requests (   -- one row per usage request slot, retries included (§8.6)
  provider     TEXT NOT NULL,
  identity_key TEXT NOT NULL,   -- not an account FK: removing and re-adding an account keeps its history
  at           INTEGER NOT NULL -- epoch s of the reservation; rows older than 3660 s are pruned on insert
);

CREATE TABLE leases (
  name       TEXT PRIMARY KEY,  -- 'usage:<id>', plus internal markers such as the daily sample prune
  holder     TEXT NOT NULL,     -- random UUID per acquisition
  expires_at INTEGER NOT NULL   -- epoch ms, wall clock
);

CREATE TABLE switch_journal (   -- a row exists only while a switch is between its first live write and its commit (§9.4)
  provider     TEXT PRIMARY KEY,
  holder_pid   INTEGER NOT NULL,
  holder_start INTEGER NOT NULL, -- process start time, for liveness (§12.6)
  from_id      TEXT,             -- NULL when the outgoing login was unmanaged
  to_id        TEXT NOT NULL REFERENCES accounts(id),
  from_fp      TEXT,             -- fingerprint of the live credential being replaced
  from_identity TEXT,            -- the outgoing identity object (CC: oauthAccount), for recovery; not a secret
  to_fp        TEXT NOT NULL,    -- fingerprint of the credential being written; no secret is stored
  to_epoch     INTEGER,          -- the target's login_epoch when the row was written; the activation epoch a forward finish records (§9.6)
  started_at   INTEGER NOT NULL,
  prior        TEXT              -- the row a forced switch superseded; restored if this one never lands (§9.6)
);

CREATE TABLE autoswitch_state (       -- one row per provider; auto-switch never crosses providers; one writer (§11.1)
  provider          TEXT PRIMARY KEY,
  last_switch_at    INTEGER,
  last_switch_from  TEXT,
  last_switch_to    TEXT,
  left_headroom     REAL,
  left_recovery_at  INTEGER,
  left_trigger      TEXT,
  unhealthy_ticks   INTEGER NOT NULL DEFAULT 0,
  idle_hold_since   INTEGER           -- unused: §11.2 has no idle-hold (kept to avoid a migration)
);

CREATE TABLE events (
  at        INTEGER NOT NULL,
  provider  TEXT NOT NULL,
  kind      TEXT NOT NULL,      -- 'switch' | 'quarantine' | 'unquarantine' | 'add' | 'remove' | ...
  from_id   TEXT, to_id TEXT,
  trigger   TEXT,               -- 'manual' | 'proactive' | 'at-limit' | 'failover' | 'consume-first'
  source    TEXT NOT NULL,      -- 'cli' | 'auto' | 'daemon'
  detail    TEXT                -- JSON
);

CREATE TABLE mappings (         -- one directory can map to one account per provider
  path       TEXT NOT NULL,     -- canonical absolute directory path
  provider   TEXT NOT NULL,
  account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  added_at   INTEGER NOT NULL,
  PRIMARY KEY (path, provider)
);

CREATE TABLE displaced (
  id          TEXT PRIMARY KEY, -- file stem
  provider    TEXT NOT NULL,
  at          INTEGER NOT NULL,
  reason      TEXT NOT NULL,    -- 'displaced-live-login' | 'forced-activation' | 'identity-conflict' | ...
  fingerprint TEXT NOT NULL,
  identity    TEXT              -- provider-owned identity JSON, if known (CC: {email, orgUuid, accountUuid})
);

CREATE TABLE live_identity_cache (   -- speeds up statusline and list; the file wins if it disagrees
  provider  TEXT PRIMARY KEY,
  path TEXT, mtime_ns INTEGER, size INTEGER, identity_key TEXT, label TEXT, account_uuid TEXT
);
```

**Identity.** An account's identity is `(provider, identity_key)`. The provider derives the key
(CC: email and org uuid) and the engine never parses it. `account_uuid` corroborates it:
attributing a credential to a *different* account requires a positive uuid match, so a
key match whose uuid conflicts is a different account, for example a recycled email. The same
email under two providers is two accounts.

**Stored shapes are provider-neutral.** No column, constraint or JSON shape in the store assumes
Claude Code: identities are keys, credential kinds are validated by the provider, and usage is
stored as generic windows. A new provider needs no migration.

**Positions** are numbered per provider. A new account gets `max(position) + 1` within its
provider, and gaps are never reused. `move` only reorders, since storage is keyed by `id`. A
move target must be ≤ `max(99, max(position))` within the provider.

**Leases.** A lease is taken with a single statement:

```sql
INSERT INTO leases(name, holder, expires_at) VALUES (?1, ?2, ?3)
ON CONFLICT(name) DO UPDATE SET holder = excluded.holder, expires_at = excluded.expires_at
WHERE leases.expires_at <= ?now
```

The lease is held if `changes() = 1`. A result is recorded only if the lease row still shows the
same holder.

Leases only bound work whose overlap is harmless, such as a duplicate usage fetch. They never
protect a refresh token: a lease can expire under a suspended holder, which
then resumes believing it still holds it. Credentials are protected by account locks (§6.2),
which the kernel holds for as long as the holder lives.

**History retention.** Samples older than `usage.history_retention_days` are pruned, at most
once a day, on the write path.

### 6.2 Vault

The vault holds credential bytes and is the source of truth for them. The store holds metadata
only.

| Platform | Current generation | Previous generation |
|---|---|---|
| macOS | Keychain generic password, service `tagteam`, account `<id>` | account `<id>.prev` |
| Linux | `vault/<id>.json` (0600, directory 0700) | `vault/<id>.prev.json` |

- **Contents.** The bytes are stored raw, in whatever form the provider defines. For CC that is
  the credential JSON, or an `sk-ant-api…` string for API-key accounts. The vault is
  provider-agnostic: it never parses what it stores.
- **Account locks.** Every write or delete of an account's vault entries holds that account's
  lock: `flock(LOCK_EX)` on `locks/<id>.lock`, fd `O_CLOEXEC`, released by the kernel when the
  holder exits. Each writer compares and replaces inside one hold, so no writer can overwrite a
  generation it has not seen. The writers are the refresh gate, the switch's outgoing capture,
  `run` capture, lazy capture, active-token adoption, `add`, `add-token`, `import` and `remove`.
  The refresh gate only tries the lock and returns `Busy` if it is held. Every other writer
  waits up to 15 s, which covers the longest hold, a refresh request of up to 10 s.
- **Writes.** A write moves the current generation to `.prev` only when the lineage fingerprint
  changes.
- **What automatic captures may write.** A capture that tagteam makes on its own (the switch's
  outgoing capture, `run` capture, lazy capture, and active-token adoption) never writes a
  degraded credential, and never replaces a credential that has a refresh token with one that
  lacks it. A capture from a profile (`run` capture, lazy capture, and the gate's profile
  adoption) also requires the profile to be quiescent, and is decided by the profile's
  provenance (§12.5), never by expiry. A stale-marked profile is never captured: its login was
  replaced by an explicit command, whether or not a session was running, and the profile's
  other lineage must never undo that. The default home is held to the same rule through its
  activation epoch: a capture from the live store (the switch's outgoing capture, recovery's
  capture, and active-token adoption) never takes a live store that is stale-marked (§12.5).
- **Pending replacements first.** Every holder of an account lock first reconciles a pending
  explicit replacement for that account (§12.5), before it refreshes, captures, bootstraps or
  activates anything. Only the explicit commands `add`, `add-token` and `import` replace a login wholesale.
- **Pending rescues before activation.** Anything that activates an account's vault
  generation, whether a switch in either branch or a profile bootstrap, first settles that
  account's `rescue/` entries under its account lock. A rescue has consumed the vault's
  generation, so activating the vault alone would hand CC a used refresh token.
  - A rescue whose `predecessorFp` is the vault's current fingerprint is adopted, as §7.3 step 3
    adopts it, and the adopted generation is the one activated.
  - If a rescue file for the account (named by its filename, §5) cannot be read or parsed, or
    its adoption fails, the activation is refused.
- **Deletes are strict.** Both generations are deleted on both backends. Errors propagate, and
  the deletion is verified with a tri-state read. A locked Keychain that still holds an item
  aborts the delete.
- **Keychain calls.** Every macOS vault operation uses the same `security` driver as CC
  interop (Appendix A.3).

### 6.3 Rescue and displaced storage

These are always plain files with mode 0600, never Keychain items. Their writes must not share
the Keychain's failure modes.

- **`rescue/`** holds a refreshed successor whose vault write failed. The next pass of the
  refresh gate for that account adopts it automatically (§7.3), and the file is deleted only
  after a verified vault write.
  - Each file is a JSON envelope: `{"format": "tagteam-rescue", "version": 1, "accountId",
    "loginEpoch", "predecessorFp", "credential"}`. `credential` holds the credential bytes
    verbatim, as a UTF-8 string. `predecessorFp` is the fingerprint of the generation that was
    sent, which is how the gate tells that a rescue succeeds the vault's current generation.
    The filename's `<fp12>` is the successor's fingerprint.
  - A rescue file that cannot be read or parsed makes the gate return `Transient` with kind
    `rescue-unreadable`, without sending a request. It also blocks activating the account
    until it is settled (§6.2).
  - A `rescue` path that is not a directory, or cannot be listed, leaves every account's
    rescues unknown, so it counts as an unreadable rescue for every account. `remove` refuses,
    naming the path, rather than guess which entries were the account's; `doctor` fails the
    check (§13.6). Only a full `purge` deletes it (§10.5).
  - A rescue file whose account no longer exists is reported by `doctor` and deleted by
    `purge`. `remove` deletes the removed account's own.
- **`displaced/`** holds credentials that were not tagteam's to keep: a live login that a
  switch overwrote, a successor the token endpoint attributed to another account, and so on
  (the row's `reason`).
  - Each file is `<id>.json`, with `id = <epoch s>-<fp12>-<rand6>`, and holds the credential
    bytes verbatim. The file is written first and its row second, so a failed insert leaves a
    file with no row, never a row that names nothing.
  - Both are written under the displaced lock (`locks/displaced.lock`, §5): a leaf `flock`,
    held only around one entry's file and row, with nothing else taken while it is held
    (§4.3). `displaced --purge` holds it around each deletion, so it never deletes a file
    whose row is still to be inserted.
  - A row's `identity` is the identity the displacing code attributed the bytes to, or null.
    A secret found on the other auth axis (§9.4 step 7) is never attributed to the outgoing
    login: its row carries an identity only when the bytes themselves name one.
  - tagteam never reads a displaced credential back. Restoring one is manual; the listing
    names the file.

**`tagteam displaced [--json]`** lists the entries, newest first, joining rows and files: the
ID, provider, time, reason, identity (and the position of the managed account it names, if
any) and the fingerprint's first 12 hex digits. A file with no row is listed as `unrecorded`,
with the time its name carries; a row whose file is gone is listed as `file missing`. It never
prints a credential, and it reads only the store and the directory, under no lock.

```json
{ "schemaVersion": 1, "dir": "…/displaced",
  "displaced": [ { "id": "…", "provider": "claude-code", "at": "…Z", "reason": "displaced-live-login",
                   "fingerprint": "sha256:…", "identity": { … }, "account": 3,
                   "file": "present", "recorded": true } ] }
```

`identity` and `account` are null when unknown; `file` is `present` or `missing`.

**`tagteam displaced --purge ID... [--yes]`** deletes each named entry: the file first,
verified gone, then the row. Every ID is checked before anything is deleted; an unknown one is
an error that names it. It asks for confirmation on a terminal. Without a terminal, or with
`--json`, it requires `--yes` and otherwise fails with `needs-confirmation`. Each deletion
holds the displaced lock, waited for up to 5 s.

### 6.4 Settings (`config.toml`)

`tagteam config list|get|set|unset|path [--json]` reads and edits the file with `toml_edit`,
which preserves comments and formatting. `set` writes only the key it is given, so defaults are
never frozen into the file.

**One registry.** Every key in the table below is declared once, with its type, default, valid
values and whether a provider table may override it. Reads, `set` and `unset`, `config list`,
`doctor` (§13.6) and completions (§13.7) all use that declaration, so none of them accepts a
key or value that another rejects.

- **Reads are forgiving.** A corrupt file or an invalid value falls back to the default, with a
  warning. An unknown key is ignored; `config list` and `doctor` report it.
- **`set` and `unset` are strict.** They refuse to write to a corrupt file. `set` refuses an
  unknown key, a key that a provider table cannot override (`provider.<id>.ui.color`), a value
  of the wrong type and a value outside its range, each with `invalid-input`; it never clamps.
- **Values on the command line.** Numbers are written as typed. **Booleans** parse only
  `true/false/1/0/yes/no` and are written as TOML booleans. **Lists** are comma-separated; each
  item is trimmed, and an empty item is refused. An empty argument (`''`) writes an empty
  list, which is how a provider table overrides a non-empty global list. `all` in
  `autoswitch.models` must stand alone.
- **Key-specific rules** apply on `set` exactly as on reads: `default_provider` must name a
  registered provider; `statusline.format` takes §13.5's placeholders only; each
  `run.share_extra` item must be a single entry name (no `/`, not `.` or `..`, not
  `.tagteam-*`) and not on the known-private list (§12.2), where a read only ignores it with a
  warning.

**Provider overrides.** The `[autoswitch]` table holds defaults for every provider. A
`[provider.<id>.autoswitch]` table overrides individual keys for one provider, for example
`tagteam config set provider.claude-code.autoswitch.models Fable`. Keys that only make sense for
one provider (`models`, `statusline.*`, `run.share_extra`) are read from that provider's
table first, then from the global table. `provider.<id>.<key>` and `<key> --provider <id>` name
the same entry in every `config` command.

**Commands.**
- **`config list [--provider P]`** shows every key with its effective value for the provider
  (`default_provider` when none is given) and its source: `default`, `global` or `provider`.
  Unknown keys found in the file follow, flagged.
- **`config get KEY [--provider P]`** prints the effective value alone, for scripts: a list
  comma-separated, or as an array under `--json`.
- **`config set KEY VALUE`** and **`config unset KEY`** change one entry. `unset` of an absent
  key changes nothing and succeeds. `unset` also removes a table it leaves empty, unless the
  table holds a comment.
- **`config path`** prints the file's path, whether or not it exists.

**Writing.** `set` and `unset` hold the settings lock (`locks/config.lock`, §5): a standalone
`flock`, outside the lock order (§4.3), waited for up to 5 s, with each wait a cancellation
point (§14.1). Under it they read the file, edit it with `toml_edit`, and replace it with the
atomic writer (§9.5), through a symlink to its target. A new file is created 0600, as every
tagteam file is, so its mode never depends on the umask; an existing file keeps its mode. Only
`set` creates the file. `config` works inside a run shell (§12.8): settings are not accounts.

JSON shapes: `list` returns `{schemaVersion, path, provider, keys: [{key, value, default,
source}], unknown: [key]}`; `get` returns `{schemaVersion, key, provider, value, source}`;
`set` and `unset` return `{schemaVersion, ok, key, value, changed}`; `path` returns
`{schemaVersion, path, exists}`.

| Key | Default | Valid | Per provider |
|---|---|---|---|
| `default_provider` | `claude-code` | a registered `ProviderId` | — |
| `autoswitch.threshold` | 90.0 | 50–99.9 | yes |
| `autoswitch.interval_seconds` | 60 | 15–3600 | yes |
| `autoswitch.cooldown_seconds` | 300 | 0–86400 | yes |
| `autoswitch.hysteresis_pct` | 10.0 | 0–50 | yes |
| `autoswitch.strategy` | `best` | `best`, `consume-first` | yes |
| `autoswitch.include_api_key_accounts` | false | bool | yes |
| `autoswitch.unhealthy_ticks` | 3 | 1–100 | yes |
| `autoswitch.models` | `[]` | list of model display names, or `["all"]` alone; duplicates collapse (case-insensitively) | yes |
| `usage.history_retention_days` | 180 | 1–3650 | — |
| `statusline.format` | `"{account} · 5h {5h}% · 7d {7d}%{stale}"` | placeholders listed in §13.5 only; `{model:<name>}` needs a trimmed, non-empty name without braces | yes |
| `run.share_extra` | `[]` | entry names of the source home to share into profiles, besides the provider's allowlist (§12.2); known-private names are ignored with a warning | yes |
| `ui.color` | `auto` | `auto`, `always`, `never` (`NO_COLOR` and `FORCE_COLOR` also honoured) | — |

CLI flags override settings for a single invocation and are clamped to the same ranges. A
running `auto` re-reads the file whenever its mtime changes (§11.4); its flags still win.
Nothing else caches settings across commands.

## 7. Credentials and the refresh gate

### 7.1 Credential shapes

The CC-owned credential shapes and the machine-shared and account-scoped key sets are listed in
Appendix A.4.

- **OAuth accounts** store the CC credential JSON.
- **Setup-token accounts** store `{"claudeAiOauth":{"accessToken":<tok>,"scopes":["user:inference"]}}`,
  with no refresh token and no expiry.
- **API-key accounts** store the raw `sk-ant-api…` string. A value is classified as an API key
  when `trim().starts_with("sk-ant-api") && !starts_with('{')`.

### 7.2 Expiry

- **Expired** means `now_ms + 5 min ≥ expiresAt`. A non-numeric `expiresAt` counts as
  not expired.
- **Freshen before activation.** A target whose token expires within **10 min** is refreshed
  before it is activated. That is twice CC's buffer, so CC's own re-read under its lock aborts
  its refresh rather than consuming the same generation. The refresh goes through the gate
  (§7.3) after the target is resolved and before the switch takes `MutationGuard`. Auto-switch
  acts on the gate's outcome as §11.2 step 10 says; a manual `switch` acts on it as follows:

  | Gate outcome | Manual switch |
  |---|---|
  | Refreshed, or already refreshed by another process | Activate the new generation |
  | Dead or `identity_conflict` | Quarantine the account (§7.4). A direct switch refuses and says to log in again. A bare rotation moves on to the next candidate |
  | `Busy` | Proceed. The switch waits for the account lock, and under it settles pending rescues (§6.2) and re-reads the vault, which picks up the other process's refresh whether it reached the vault or `rescue/` |
  | Transient or Systemic, other than the rows below | Proceed with the vault's generation and a warning. Nothing was consumed, or it was lost either way; once the account is live, the gate leaves its refresh to CC (§7.3 step 2) |
  | Transient with `rescued: true`, Transient `rescue-unreadable`, or `Unpersisted` | Refuse: the vault's generation has been consumed |
  | `Owned` by the live login or by a journal row | Proceed with a warning. The gate cannot tell a live journal row from an interrupted one, so the mutation lock decides: `guard_or_refuse` waits for a live holder, and recovers or refuses a dead one (§9.6). An account that became the live login re-plans as a self-switch |
  | `Owned` by a session, or `Conflict` | The matching §9.2 refusal: `session-owned` or `profile-conflict` |

  A quarantined target is never refreshed (§7.4). If it would need freshening, a switch to it
  is refused as Dead is. Otherwise it is activated with a warning that it needs a new login.

### 7.3 Refresh gate (stored tokens)

This procedure is the only place a stored refresh token is ever sent to the token endpoint.

1. **Account lock.** Try the account's lock (§6.2). If another process holds it, return
   `Busy`. The lock is held until step 6 completes, across the request, so there is at most one
   refresh per account in flight. A suspended holder keeps it, and is never preempted.
2. **Ownership.** Under the lock, return `Owned` if the account is the live login (only §7.5
   refreshes that token), is session-owned (§12.5), or is named in an unresolved
   `switch_journal` row (§9.6): until recovery decides, either account of an interrupted switch
   may be what CC is running on. tagteam makes an account the live login
   or session-owned only while holding this lock, so neither can happen before the request is
   sent. The live login is always the default home's, inside a run shell too (§12.8).
3. **Re-read the vault** (`Read<Credential>`).
   - `Unreadable` returns `Transient`. `Absent` returns `Transient` (the account was
     removed).
   - If a `rescue/` entry for this account holds a successor of the current fingerprint (its
     `predecessorFp`, §6.3), adopt it: write it to the vault, verify, delete the rescue file,
     and use it.
   - If the account's profile is quiescent, apply its provenance (§12.5). If the profile
     rotated since its seed, adopt its generation into the vault first. If provenance reports a
     conflict, return `Conflict` without making a request.
4. **Check whether someone already refreshed.** If the vault's access token differs from the
   caller's snapshot and is not expired, return it without making a request.
5. **POST** to the token endpoint (Appendix A.5), with a 10 s timeout.
6. **Persist, compare-and-swap style.** Re-read the vault. With every writer under the account
   lock this comparison cannot fail; it stays as a defence.
   - If the response names another account (§7.4 `identity_conflict`), the successor is not
     this account's. It is displaced (reason `identity-conflict`), never written to the vault
     or to `rescue/`. The account is quarantined, bound to the fingerprint that was sent.
   - If the fingerprint moved since step 3, write the new successor to `rescue/`, log at ERROR,
     and return the vault's newer credential.
   - Otherwise write the vault (the old generation becomes `.prev`) and update
     `login_expires_at`.
   - If the vault write fails, write to `rescue/` and return `Transient { credential, rescued:
     true }`. **The caller must not activate a credential in this state.**
   - If the rescue write (or the displacement) also fails, return `Unpersisted`, log at ERROR,
     and print a stderr notice naming the account position. The vault's generation has been
     consumed, so the account is also quarantined (`successor_lost`), bound to the fingerprint
     that was sent. That store write is best effort: if it fails too, the loss is reported
     only. A successor that belonged to another account keeps its `identity_conflict`
     quarantine instead: that is the account's real problem, and both reasons are bound to the
     same fingerprint.
   - Any other error after the response is received rescues the successor first, and reports
     `Unpersisted` if that rescue fails.
   - A panic after the response is received does the same as it unwinds. It cannot return
     `Unpersisted`, so a loss it cannot prevent is logged at ERROR, which reaches stderr, and
     quarantines the account (`successor_lost`), best effort.
7. **Classify the result:**

   | Response | Verdict |
   |---|---|
   | 400/401/403 with top-level JSON `error == "invalid_grant"` | **Dead**, but only after re-reading the source that was sent. If its lineage moved in the meantime, record `refresh-failed` instead |
   | No `refreshToken` in a structurally complete OAuth credential | Dead |
   | The server rejects the request itself: a top-level `error == "invalid_client"` (RFC 6749), or a 400 whose nested `error.type` is `invalid_request_error` (what the endpoint returns for an unknown client id, Appendix A.5) | Systemic. Never counts as a strike. The message quotes the server's text |
   | No request sent: DNS, connect or TLS failure | Transient, kind `pre-send` |
   | Request sent, no response read: timeout or reset | Transient, kind `ambiguous` |
   | Anything else, including unparseable bodies | Transient |

The rule behind this procedure: **a successor that tagteam has received is never discarded.**
It is either persisted to the vault or rescued, and if both writes fail, the loss is reported
(`Unpersisted`). A successor the server issued but tagteam never received, because the
response was lost after the request was sent, cannot be recovered by any client.

**After an `ambiguous` failure, the next attempt retries the same generation.** If the server
did consume it, the retry gets `invalid_grant` with the lineage unchanged, so the account is
quarantined and shows `relogin_required`. That is the true state, and it is the same end
state a forced re-login would reach. If the server never processed the request, the retry
simply succeeds.

### 7.4 Quarantine

- **One strike quarantines an account.** A Dead verdict sets `quarantine_reason` and binds
  `quarantine_fp` to the fingerprint that was actually sent.
- **`identity_conflict`.** The token endpoint's `account.uuid` or organization disagreeing with
  the account also quarantines it. Either one alone is enough: an organization is compared when
  both sides name one, and a uuid when both do. The successor is displaced, not stored (§7.3
  step 6).
- **`successor_lost`.** A refresh whose successor could be stored nowhere (§7.3 `Unpersisted`)
  quarantines the account, because the vault's generation has been consumed.
- **Quarantined accounts** are never fetched, refreshed or auto-activated. They show
  `relogin_required`.
- **Clearing a quarantine.**
  - Any vault write that changes the fingerprint clears it; no special clear path exists.
  - `add`, `add-token` and `import` clear it explicitly.
  - A plain `import` may replace a quarantined account without `--force`.
  - Every clear records one `unquarantine` event, whichever path clears it, the switch's
    outgoing capture included. Its reason is `account-replaced` when the account's
    `login_epoch` moved, else `credentials-replaced` (§11.4).
- **The active account's quarantine** holds while *either* the live credential or the vault
  matches `quarantine_fp`.

### 7.5 Active-token refresh

The active token normally belongs to CC, and tagteam leaves it alone. tagteam refreshes it only
when the token has expired, or when the server returned 401 on a token that is still valid
locally (a sibling machine revoked it). The procedure:

1. Take a `MutationGuard`, then the account lock, then CC's credential locks (§4.3 order).
2. Re-read the live credential. It must be `Fresh`, and the live identity must still match the
   account. If the account's activation epoch is stale (§12.5), stop: an explicit command
   replaced the account's login while CC kept the old one, and adopting or refreshing that
   lineage would undo the replacement. Nothing is read further or sent. The outcome is
   `Replaced`; a usage fetch reports it as `unavailable`, with a warning to run
   `tagteam switch <N> --force`. CC goes on refreshing its own copy.
3. **Reconcile before any request.** The account can have up to three copies: the live store,
   the vault (with its `.prev`), and a `rescue/` successor. CC rotates the live copy on its own,
   so the live store is where the lineage advances. Access-token expiry says nothing about
   which generation is newest, and is never used to decide it.

   | Live credential | Meaning | Action |
   |---|---|---|
   | The vault's generation | In step, unless a rescue is pending | If a rescue succeeds this generation, publish it: vault, then live store |
   | The vault's `.prev` | An earlier pass persisted to the vault but failed the live write | Self-heal: write the vault generation to the live store |
   | A rescue's generation | Published, but the vault write failed | Write the rescue to the vault |
   | Any other full token pair | CC rotated it: this is the newest generation | Adopt it into the vault, expired access token or not; retire any rescue, which it supersedes |

   A rescue file is deleted only after its writes are verified, or once it is superseded, and
   is never published over any other live generation. Attribution comes from the live
   identity matched in step 2, corroborated by the oracle when the access token is still
   valid. Never copy an access-token-only blob over the vault's refresh token.
4. **Refresh only when still needed.** A request is made only if, after reconciliation, the
   live access token is expired, or is exactly the token the server rejected with 401
   (`rejected_fp`, §8.1).
5. Otherwise revalidate CC's locks (§9.1) and POST, bounded at 6 s. Persist the successor to
   the vault first, or to `rescue/` if that fails. Then write it to the live store in either
   case, so CC always holds the newest generation; the config lock is taken only around the
   live write, and the storage-write lock around the credential entry's write (§9.1). If CC's
   locks turn out to be compromised when the response arrives, the
   successor is still persisted to tagteam's own storage, but the live store is not written;
   the next pass reconciles it (step 3). A successor that ends up in none of the vault,
   `rescue/` or the live store is lost exactly as in §7.3 step 6: `Unpersisted`, with its
   `successor_lost` quarantine.

Refreshing from a degraded read is never allowed.

### 7.6 Profile oracle

`GET /api/oauth/profile` (Appendix A.5) resolves who owns a live access token. It is:

- **advisory only**: a failure never blocks an operation and never raises an error. It
  resolves to no answer and is logged at DEBUG
- **never called while holding a lock**
- **resolved** only when `account.uuid` is a non-empty string
- **asked only where the answer can change the outcome:**
  - by `switch`, when the live credential is not the outgoing account's vault generation (§9.4)
  - by a self-switch whose live credential diverged from the vault (§9.2)
  - by `add`, for an OAuth login (§10.1)
  - by recovery, when fingerprints alone cannot decide a row or settle an entry it clears (§9.6)
  - by the active-token refresh, when the live access token is still valid (§7.5)
- **asked at most once per process for a given credential**, keyed by a hash of its exact
  bytes. A new access token under the same refresh token is a different credential, so a
  provider's skip for one access token never answers for another
- **never asked without a token it can show.** The provider returns no answer, and sends no
  request, for a credential that cannot be resolved. For Claude Code these are an expired
  access token, a setup token (its only scope is `user:inference`) and an API key
- **never asked by a metadata command** (`alias`, `disable`, `enable`, `move`), so those make no
  network call (§9.6)

## 8. Usage

### 8.1 Fetch

- **Request.** `GET https://api.anthropic.com/api/oauth/usage` with
  `Authorization: Bearer <accessToken>`, `anthropic-beta: oauth-2025-04-20`,
  `User-Agent: tagteam/<version>`, and a 5 s timeout.
- **No access token.** The error kind is `no-access-token`.
- **Inactive account with an expired token and a refresh token.** Refresh through the gate
  first.
  - A Dead verdict returns immediately and never hits the usage endpoint.
  - A deterministic refusal returns immediately: `Busy`, `Owned`, `Conflict`,
    `invalid_client`, or rescue unreadable.
- **401 on an inactive account.** Refresh once, then retry once. The retry is a request like
  any other and needs its own slot in the hourly budget (§8.6).
- **Active account.** Usage fetches never refresh it; only §7.5 does that.
  - An expired live access token is handed to §7.5 (`Expired`) before the fetch. The fetch
    goes on with the live token, read fresh, when §7.5 leaves a usable one: `Refreshed`,
    `PersistedNotPublished`, `PublishedOnly` or `NotNeeded`. `Dead` reports
    `relogin_required`; any other outcome, or an error, reports `unavailable` with a warning.
  - A 401 on an access token that is still valid locally stamps `rejected_fp` with that
    token's fingerprint and hands the account to §7.5 (`Rejected`). If §7.5 then leaves a
    usable token, the fetch retries once, with its own budget slot.
  - A live access token whose fingerprint equals `rejected_fp` is never sent again. The
    fetch hands the account to §7.5 (`Rejected`) first, as after a new 401, and goes on only
    if §7.5 leaves a different, usable token.
- **A fetch that ends before sending** (a refusal or failure while getting a token) is
  recorded as a failure, and gives back the budget slot it reserved (§8.3).
- **Session-owned account** (§12.5). The fetch is read-only and uses the profile's token. CC
  in the session owns that token, so tagteam never refreshes it, never writes the profile, and
  takes no lock to read it.
  - The token is read the way CC reads it: the profile's hashed Keychain item for its recorded
    spelling (§12.2), then `<profile>/.credentials.json`. A degraded read may be used, since an
    access token sent to the usage endpoint consumes nothing. An unreadable credential reports
    `keychain_unavailable` or `unavailable`, as for any other account.
  - An expired access token reports `token_expired` without a request. It is recorded as a
    fetch that ends before sending, so its budget slot is given back (§8.3). CC refreshes the
    token on its next API call.
  - A 401 stamps `rejected_fp` with the access-token fingerprint and reports `token_expired`.
  - The same bytes are not sent again until they change.
  - A profile whose identity drifted (§12.5) is not used: the account reports `unavailable`.
- **A token that cannot be refreshed** (a setup token): a 401 is an ordinary failure,
  recorded as `http-401` on every path, whether the refusal is new or remembered through
  `rejected_fp`.
- **Retry-After** is parsed in its seconds form only.

### 8.2 Normalization

A fetch is normalized into generic windows (§4.5), stored as `last_good`. The rendering layer
turns them back into each provider's output shape (for CC, cswap's, §13.2). Claude Code's
windows, read from the response shape recorded in Appendix A.5:

| Window | Source | Kind, period | Values |
|---|---|---|---|
| `5h` | `five_hour` | `Short`, 18000 s | `pct` = `utilization`; `resets_at` from the ISO 8601 string, as epoch seconds |
| `7d` | `seven_day` | `Long`, 604800 s | as `5h` |
| `scoped:<name>` | each `limits[]` item with a `scope.model.display_name` and a numeric `percent` | `Scoped`; 604800 s when `group` is `weekly`, otherwise no period | `pct` = `percent`, `resets_at` |
| `spend` | `spend`, when `enabled` is true and `used` and `limit` both carry `amount_minor` | `Spend` | `detail {used, limit, currency}`, each amount `amount_minor / 10^exponent`; `pct = used / limit · 100`. A zero `limit` leaves the window out |
| `spend` | `extra_usage`, only when there is no `spend` object, `is_enabled` is true, and `used_credits`, `monthly_limit` and `currency` are non-null | `Spend` | `used = used_credits / 10^decimal_places`, `limit = monthly_limit / 10^decimal_places` (`decimal_places` defaults to 2); `detail {used, limit, currency}`; `pct = used / limit · 100`. A zero `limit` leaves the window out |

- Every other field is ignored: the per-model `seven_day_*` objects, the code-named windows,
  `seven_day_breakdown`. A per-model window that becomes populated is expected in `limits[]`
  as a `weekly_scoped` item.
- A missing or null source leaves its window out. A non-finite `pct` drops that window. A
  `pct` above 100 is kept, so headroom goes negative (at the limit).
- A body that is not JSON, or a known key with the wrong type (`five_hour` as a string, say),
  is the failure `bad-response`.
- An empty result normalizes to `None`.

Each provider declares which of its windows are relevant. For Claude Code, the **relevant
windows** for decisions are 5h and 7d, plus any scoped windows whose names
match `autoswitch.models` case-insensitively (`all` matches every scoped window). Spend never
counts.

- **Headroom** is `100 − max(relevant pct)`.
- **≤ 0** means at the limit. **Unknown** headroom is never auto-skipped.

### 8.3 Collector (three phases)

1. **Reserve.** In a short transaction, check eligibility and take the lease `usage:<id>`
   (90 s TTL).
   - Eligible means: not quarantined, not in backoff, no live lease, either due or stale, and
     **within the hourly budget** (§8.6). The same transaction inserts the `usage_requests`
     row that reserves the slot, so the budget holds across every process.
   - On-demand callers (`list`, `status`, `switch`) also require the reading to be older than
     180 s *and* either a poll to be due or no plan to exist.
   - Scheduled collection (an auto tick, §8.6) requires a poll to be due, or no reading yet.
   - A re-check (consume-first's re-fetch, §11.2 step 8) requires only the reading to be older
     than 180 s; it ignores the plan. Quarantine, backoff, the lease and the budget still
     apply.
   - A fetch that ends before sending is recorded in phase 3 like any failure. The same
     fenced transaction deletes the `usage_requests` row it inserted, so the slot returns to
     the budget. The lease expires as it does after any record.
2. **Fetch**, with no lock held other than the ones the gate (§7.3) or §7.5 take for their
   own refresh.
3. **Record.** In a transaction fenced by lease holder and account identity; a late or
   superseded result is dropped.
   - **Success** writes `last_good` and `fetched_at`, resets the failure fields, stores the
     next plan, **and inserts `usage_samples` rows**.
   - **Failure** never touches `last_good` or `fetched_at`.

**Who collects.**
- `list` collects every eligible account, and `status` only the live one. The accounts are
  collected in parallel, one thread each, and the command waits for all of them. Each request
  is bounded by its own timeout, so one account costs at most a gate refresh, a fetch and one
  retry.
- `switch` collects only for the strategies that rank by usage (`best`, `next-available`,
  §9.3): on demand, for the live account and every switchable candidate. After any switch that
  activated an account, it re-plans polls from the last readings
  without fetching. The incoming account's `next_poll_at` gets the active-account policy, and
  the outgoing account's gets the candidate policy (§8.6).
- An auto tick collects as scheduled (§8.6), and consume-first re-checks before switching
  (§11.2 step 8). Its fetches run in parallel like `list`'s.
- A usage failure is never a command error. It shows as the row's `usageStatus` (§13.2), and
  its kind is stored in `usage_state.last_error` using §6.1's tokens (`http-<code>`,
  `pre-send`, `ambiguous`, `bad-response`, `refresh-failed`, `over-budget`), plus
  `no-access-token` (§8.1). A successor lost while collecting (`Unpersisted`, §7.3 step 6) is also a
  warning on stderr that names the account, which is quarantined and shows `relogin_required`.
- An error while collecting one account (a store or lock error, for example) is a warning on
  stderr that names the account; the other accounts' results stand, and the command
  succeeds. A never-sent slot is given back on every such path.

### 8.4 Trust (can a reading drive a decision?)

- A reading is decision-grade when its age is ≤ **300 s**.
- **A reading stamped more than 60 s in the future** (clock skew) has no usable age: it is
  never decision-grade, counts as unread for on-demand eligibility, and a re-plan treats
  its time as now. Likewise a `next_poll_at` further ahead than the budget's count window, or a
  `backoff_until` further ahead than the 429 cap (4500 s), each plus 60 s, can only come from
  a skewed clock and is ignored.
- **Extended trust.** Trust extends to ≤ **3600 s** while failures are being retried, while a
  scheduled plan is in force, or while a live lease exists.
- **After a 429.** `last_good` is trusted until the earliest relevant window reset, capped at
  `fetched_at + 7200 s`. Usage only rises within a window, so the old reading is a valid lower
  bound.

### 8.5 Failure backoff

- **Base:** `min(30 · 2^(n−1), 600)` seconds, with the exponent clamped at 32.
- **429 with `Retry-After` below 1 s** (`0` included): at least 300 s.
- **429 with `Retry-After` > 600:** the value plus 900 s of margin, because retrying at the
  deadline re-arms the block.
- **Caps:** `min(asked, 4500)` for 429s, `min(asked, 3600)` otherwise.
- **Result:** `max(asked, computed)`.
- **Non-finite values** (`inf`, huge numbers) are clamped and never stored.

### 8.6 Poll policy

These numbers are ported from `cswap:poll_policy.py`; the hourly budget and the post-jitter
floor are tagteam's. The endpoint allows roughly 28–30 requests per rolling hour per identity
for non-first-party User-Agents.

**The hourly budget is enforced, not targeted.** No identity is sent more than **20** usage
requests in any rolling hour. Requests are counted in `usage_requests` across all tagteam
processes, keyed by `(provider, identity_key)`, so removing and re-adding an account does not
reset its count. Every request counts: scheduled polls, on-demand fetches, forced re-checks
(consume-first's re-fetch), and the retry after a 401.
- A slot is reserved before sending, and is valid for 60 s. A sender that has not sent within
  60 s of reserving, for example after a suspend, discards the slot and reserves again.
- The count covers the last 3660 s (the hour plus the slot validity), so send times, not just
  reservation times, stay within the budget.
- A request that would exceed the budget is not sent. The
fetch reports `over-budget`, the account's `next_poll_at` moves to when the oldest counted
request leaves the hour, and the existing reading keeps whatever trust §8.4 gives it. The
schedule below decides *when* to ask; the budget decides *whether* a request may be sent.

| Constant | Value |
|---|---|
| Hourly request budget, per identity | 20 |
| Floor / serve TTL | 180 s |
| Urgent (active account, moving, within 15 points of threshold, no recent 429) | 60 s |
| Active max / candidate default / candidate max | 300 / 300 / 600 s |
| Exhausted | 600 s |
| Movement delta | 1.0 point |
| Jitter | ±10% |
| Edge backoff / post-429 minimum interval / "recent 429" window | 300 / 360 / 3600 s |
| Post-429 AIMD multiplier / maximum | ×1.5 / 1800 s |
| Escalation margin | 15 points |
| Reset slack | 60 s |

`plan_after_fetch` works as follows:

- **Unknown pct:** use the default (180 s for the active account, 300 s for others).
- **Movement ≥ 1 point:** `max(180, base/2)`. **No movement:** `max(180, base · 1.5)`. Both
  are capped at the role's maximum (300 s active, 600 s candidate); the rules below are
  not. A non-finite pct counts as unknown.
- **Urgent:** 60 s.
- **Recent 429:** `min(1800, max(interval, max(base · 1.5, 360)))`. A 429 counts as recent from
  when its backoff lifts, not from when the 429 arrived.
- **Exhausted:** at least 600 s.
- **Then:** apply jitter, then clamp: never below the 180 s floor (60 s when urgent), and never
  later than the next relevant reset + 60 s.

**Auto-switch scheduling** is O(1) per tick, in two phases:
1. **The active account**, if it is due. An expired or refused live token goes to §7.5 first,
   as for any collection of the active account (§8.1).
2. **Candidates**, chosen from the store as phase 1 left it: the single stalest due candidate
   (never fetched first, then the oldest `fetched_at`, ties to the lower position). The tick
   **escalates** to every due candidate when the active account's max relevant pct is within
   15 points of the threshold (≥ threshold − 15), or its headroom is still unknown (§8.4).

The candidates are the provider's switchable accounts (§9.3) other than the live one. A
candidate that is not due keeps its reading and its trust (§8.4).

### 8.7 History and projection

`usage_samples` feeds `tagteam history` and the projections.

- **Rate (regression).** Least-squares slope over the samples of the current window instance
  (same `resets_at` ± 60 s) from the last 48 h. It requires ≥ 3 samples spanning ≥ 2 h and a
  positive slope.
- **Rate (fallback).** cswap's average pace, for `Long` and `Scoped` windows with a known
  period (§4.5; CC's 7d and weekly scoped windows: `period` = 604800 s):
  - `elapsed = period − ((reset − fetched_at) mod period)`
  - suppressed when `elapsed < 86400 s`
  - `expected = min(100, elapsed / period · 100)`
  - `rate = pct / elapsed`
- **Projections**, measured from the reading's `fetched_at`, since `pct` is as of then:
  - `projectedExhaustionAt = fetched_at + (100 − pct) / rate` (`fetched_at` when `pct ≥ 100`)
  - `willLastToReset = pct + rate · (reset − fetched_at) ≤ 100`
  - `aheadOfPace = pct − expected ≥ 15`, for the windows the fallback covers (never `Short`)
- **Where they appear.** `list` marks a window that is ahead of pace (`▲ pace`, §13.1).
  `history` shows the ETA. JSON carries all fields plus the additive `projectionMethod: "regression" | "average"`.

## 9. Switch

The engine owns the transaction's shape: special cases, classification, displacement, ordering,
commit and rollback. The provider supplies the live locks (`lock_live`), the reads, and the
writes of its identity surface (`activate`). Everything below is written for the Claude Code
provider, whose `activate` is steps 5, 7 and 8 of §9.4. Another provider reuses the same transaction
with its own locks and surface.

### 9.1 Locks

| Lock | Type | Path | Staleness | Acquire timeout |
|---|---|---|---|---|
| tagteam mutation lock | `flock(LOCK_EX)`, fd `O_CLOEXEC`, polled every 100 ms | `$XDG_DATA_HOME/tagteam/.mutation.lock` | n/a | 10 s (30 s for `run` bootstrap) |
| CC OAuth refresh lock | `mkdir` directory lock | `<secure-storage dir>/.oauth_refresh.lock`, symlinks not resolved | 60 s | 9 s |
| CC legacy credential lock | `mkdir` | `<realpath(secure-storage dir)>.lock` (`~/.claude.lock`); the unresolved path if `realpath` fails | 60 s | 9 s |
| CC config lock | `mkdir` | `<global config path>.lock` (`~/.claude.json.lock`) | 10 s | 9 s |
| CC storage-write lock | `mkdir` | `<secure-storage dir>/.storage-write`, symlinks not resolved | 15 s | 9 s |

Both credential locks and the storage-write lock are anchored at the secure-storage dir, not
the config home; the two differ when `CLAUDE_SECURESTORAGE_CONFIG_DIR` is set (Appendix A.1).

**The storage-write lock** is CC's serialization of every credential write (CC 2.1.286,
Appendix A.3). CC takes it for each write to its secure storage, including writes that take no
refresh lock, such as MCP OAuth updates and its dead-token marking. tagteam takes it for every
write or delete of a CC credential entry: the OAuth entry (Keychain item or
`.credentials.json`) and the managed-key item. That covers the switch (§9.4 steps 7 and 10),
recovery (§9.6), the active-token refresh's live write (§7.5) and profile bootstrap (§12.3).
- It is a leaf lock. It is taken only while the credential locks are held (for a profile, the
  profile's own), held around one entry's write, and never across a network call; no other
  lock is taken while it is held. Its wait is a cancellation point (§14.1).
- Under it, the entry is read again. Its account-scoped keys must still equal what the writer
  last read or wrote under the credential locks; otherwise the write aborts, and a switch rolls
  back. CC changes those keys under the credential locks only by refreshing. Outside them it
  changes them only by its dead-token marking, which writes both tokens empty and `expiresAt`
  0 (the `Wiped` shape, §9.4 step 4, Appendix A.3).
  - **A marking is no conflict.** An entry that differs from what the writer last read or
    wrote only by such a marking does not abort the write, which goes ahead over it. The
    marking holds no secret. The writer holds the generation CC marked, or a newer one: §7.5's
    successor or a switch's target.
  - The machine-shared keys are taken from this read, so a CC write made since the earlier
    read is never lost.
  - **A rollback's restore is stricter** (§9.4 step 10). It puts an entry back byte for byte,
    and only while nothing has written any of the entry's places since tagteam first changed
    them. A marking or any other write since leaves the whole entry as it is.

When a switch or a recovery takes the three CC locks together, one 9 s budget covers all
three. The active-token refresh takes the credential locks with that budget and, after its
request, the config lock with a fresh 9 s budget (§7.5).

The CC locks follow the `proper-lockfile` protocol:

- `mkdir` acquires the lock.
- On `EEXIST`, if `now − mtime > staleness`, `rmdir` it and retry. Otherwise sleep a jittered
  250–500 ms.
- While the lock is held, a thread touches the directory's mtime every 3 s, through a
  directory fd opened when the lock was acquired (`futimens`), never by path. CC touches every
  5 s; both are well inside the staleness windows.
- **Compromise detection.** An ownership check confirms that the path still names the
  directory tagteam created (the same device and inode as the held fd) and that it still
  carries the mtime tagteam last set. If it doesn't, the lock has been taken over, and the
  guard is marked compromised. A directory that replaced tagteam's after a long stall, as a
  suspended `auto` can meet, is therefore never touched or removed. The check runs:
  - on each touch;
  - **synchronously, immediately before every write the lock protects**, and before a token
    request made under the lock;
  - before release.

  A thread that resumes from a suspension therefore finds out before it acts; it does not wait
  for the heartbeat's flag. A compromised guard aborts the protected write, and `Drop` leaves
  the directory alone, since it now belongs to someone else. This is `proper-lockfile`'s
  `onCompromised`, which CC also honours.
- `Drop` releases an uncompromised lock with `rmdir`, including on panic.
- If the refresh lock is acquired but the legacy lock is contended, the refresh lock is
  released and the pair retried, as CC does.
- tagteam **never writes `.oauth_refresh.lock.owner`**. An owner-less lock can be taken over
  only by the 60 s staleness rule, which is safe.

### 9.2 Special cases

These are decided before locking and re-checked afterwards.

- **Inside a run shell** (§12.8): every command that changes accounts or the live login is
  refused.
- **No live identity** (fresh machine): activate the target directly (§9.4, direct branch).
- **Live login not managed by tagteam:**
  - On a terminal, prompt `Add the current login (<email>) first? [Y/n]`, then continue.
  - With `--json`, return a no-op with reason `unmanaged-account`.
  - With `--force`, displace the live login and activate.
- **Fewer than 2 switchable accounts**, for a bare `switch`: a no-op with reason
  `only-one-account`, counted as §9.3 says.
- **Self-switch** (the target is the live account):
  - A no-op, unless the live credential diverged from the vault and the oracle resolved its
    owner. In that case, reconcile by running a full switch.
  - With `--force`, re-activate the vault generation. The live credential is displaced first,
    not written to the vault.
- **Session-owned target** (§12.5): refuse with reason `session-owned`, with or without
  `--force`. Activating it would give the default profile and the running session two
  independently locked copies of one single-use refresh token. The message names the account's
  `tagteam run` session and says to exit it first.
  - If the profile is quiescent and rotated since its seed, its generation is adopted into the
    vault before activation (lazy capture, §12.5). If its provenance reports a conflict, the
    switch refuses with reason `profile-conflict`, because the vault's generation may be
    consumed.
- **An interrupted switch** for the provider is recovered first (§9.6).

### 9.3 Manual strategies

| Invocation | Strategy | Anchor | Behaviour |
|---|---|---|---|
| `switch` (bare) | rotation | the live account if it is managed; otherwise the store's active account | With a managed live anchor: the next switchable position after it. Otherwise: the anchor itself if it is switchable, else the first switchable position |
| `switch --strategy next-available` | next-available | as rotation | The rotation walk, skipping candidates whose known headroom is ≤ 0 (the message names each one's binding window). Unknown headroom is never skipped (§8.2). If the walk skips every candidate: `candidates-exhausted` |
| `switch --strategy best` | best | the live account | The candidate with the most known headroom, ties to the lower position. Switch only if it has strictly more headroom than the live account; otherwise `already-best`. If the live account's headroom is unknown, or there is no live login, switch to it with a warning. With a managed live login whose headroom is unknown, a candidate known to be at its limit (headroom ≤ 0) is never picked; if every known candidate is, the result is `candidates-exhausted`. No candidate with known headroom: `usage-unavailable` |
| `switch <ACCOUNT>` | direct | — | Disabled accounts are allowed as explicit targets |

"Switchable" means the account has a vault credential and an `identity_json`, and is not
disabled or quarantined. A direct target needs only the credential and the `identity_json`:
disabled accounts are allowed, and quarantined ones follow §7.2. Every strategy works within
one provider: a switch never crosses providers.

**Reading the vault lazily.** A bare `switch` counts its candidates from the store: enabled,
unquarantined rows with an `identity_json`. With fewer than two, the result is
`only-one-account`. Otherwise it walks the positions from the anchor and reads each account's
vault only until it finds one with a credential. If the walk finds none, the result is again
`only-one-account`, or `no-valid-target` when there is no live login to anchor on (a fresh
machine, §9.2). Under the locks, only the chosen account is read again.
- An `Unreadable` vault met before the pick could have been the pick, so the switch fails and
  names that account (position and label). Accounts after the pick are never read.
- A Dead verdict while freshening the pick quarantines it, and the walk continues (§7.2).

**Strategies that rank by usage** (`best`, `next-available`):
- Before planning, they collect on demand (§8.3) for the live account and every switchable
  candidate, then rank decision-grade readings only (§8.4). Relevant windows follow §8.2 and
  `autoswitch.models`, or `--model` (a comma-separated list, or `all`) for this invocation.
  `--model` without `--strategy` is a usage error.
- `best` never picks a candidate whose headroom is unknown. When some were unknown, a warning
  says how many.
- Before candidates are counted, a quarantine that no longer binds is released: the vault's
  fingerprint has moved past `quarantine_fp` (§7.4), as auto-switch's step 1 does (§11.2).
- Both read vaults lazily in their own order, as rotation does: an `Unreadable` vault met
  before the pick fails the switch and names the account.
- **Under the locks**, the ranking is not recomputed, since no network is used there. The pick
  stands while it is still switchable and the live account is unchanged; otherwise the
  strategy plans again from the store's readings. If the live account under the locks is
  already the pick, another process has done this command's work: the result is a no-op with
  reason `already-active`. The same holds for a bare rotation.
- A Dead verdict while freshening the pick quarantines it, and the strategy plans again
  without it (§7.2).

Every strategy, rotation included, skips session-owned candidates (§12.5). `--force` acts on
the strategies as on a bare `switch` (§9.2).

### 9.4 Transaction

**Before locking:**
- Resolve the target and handle the §9.2 cases.
- Read the live credential. If it is not the outgoing account's vault generation (compared by
  bytes, then fingerprint), call the profile oracle (§7.6).

**Taking the locks.** Take `MutationGuard`. Then read the live account from `~/.claude.json`'s
`oauthAccount` and take the account locks of that outgoing account and the target, in
ascending ID order. Then take CC's credential locks and config lock. No network is used from
here on.

1. **Recompute the live account**, and re-check that the target is not session-owned. Launch
   reservations are written under `MutationGuard` and refreshes run under the account lock, so
   neither can change until the locks are released. If CC itself changed the live login since
   the account locks were chosen, release every lock except `MutationGuard` and take them again
   for the new pair. Taking one more account lock now could invert the lock order. After three
   attempts, abort.

   Re-read the target's quarantine too. A refresh that finished while this switch waited for
   the account lock may have quarantined it (§7.4 `successor_lost`). A target quarantined since
   planning then follows §7.2's quarantined-target rule, and a bare rotation or a usage
   strategy plans again without it.

   An auto-switch re-checks its preconditions here, before anything is written (§11.2 step
   11).
2. **Direct branch** (no live identity, unmanaged live login, or `--force`):
   - Settle the target's pending rescues (§6.2), then read it from the vault.
   - Read the live credential and config under step 3's rules.
   - Displace the live credential unless it is byte-identical to the target. A failed
     displacement aborts, except under `--force`.
   - Continue at step 5.
3. **Read the live credential.** These rules hold with or without `--force`, because tagteam
   never overwrites a credential entry it could not read fresh; `--force` only overrides whose
   credential it is.
   - `Unreadable` → abort.
   - `Present("")` → abort: never back up an empty value, because a Keychain timeout can look
     empty.
   - `Degraded` → abort. The Keychain item that could not be read may hold a newer generation
     than the file, plus the current machine-shared keys, and overwriting it would lose them.

   `Unreadable` aborts for every other live entry that step 7 overwrites or deletes as well,
   including one it destroys only if the Keychain refuses the write.
4. **Classify the outgoing credential**, using the pre-lock oracle result only if the live bytes
   haven't changed since it was taken:

   | Class | Condition | Action |
   |---|---|---|
   | `Ours` | Bytes or fingerprint equal the vault's | Nothing |
   | `Superseded` | Bytes or fingerprint equal the vault's `.prev`: an active-token refresh stored a newer generation it could not publish (§7.5) | Nothing; the vault keeps the newer generation, and capturing this one would put a consumed token back |
   | `Wiped` | An OAuth blob with both tokens empty (CC's reaction to `invalid_grant`), or a credential with no token at all | Nothing; the vault keeps its refresh token |
   | `OursRotated` | The oracle resolved the token to this account (uuid-positive, org agreeing) | Write to the vault; the old generation becomes `.prev`. Backfill `account_uuid` if NULL |
   | `Foreign` | The oracle resolved it to another identity, known or not | **Displace**. This must succeed, or the switch aborts |
   | `Unresolved` | No oracle verdict, or the bytes moved since the oracle call | Write to the vault; `.prev` keeps the old generation recoverable. Log at WARN |

   `OursRotated` and `Unresolved` are automatic captures, bound by §6.2: a live credential
   without a refresh token never replaces a vault credential that has one, and a live
   credential whose activation epoch is stale (§12.5) never replaces the replacement that made
   it stale. Either is displaced instead.

5. **Compose the target credential.** The target's pending rescues are settled first (§6.2),
   unless step 2 already did so.
   - The account-scoped keys come from the vault: `claudeAiOauth`, `trustedDeviceToken`, and
     unknown sibling keys.
   - The machine-shared keys come from the **live** credential (Appendix A.4), and so does
     their absence: a key the machine no longer holds is not resurrected.
   - With no live JSON credential, the target is used without its machine-shared keys: the
     machine holds none, so none are taken from the vault.
6. **Journal.** In one store transaction, write the provider's `switch_journal` row: this
   process's pid and start time, `from_id`, `to_id`, the fingerprints of the live and target
   credentials, the target's `login_epoch`, and the outgoing `oauthAccount` object. It holds
   no secret.
7. **Write the active credential** (Appendix A.3). Each entry is written under the
   storage-write lock (§9.1). The target's credential is always written
   first, and the other auth axis cleared after it, so a switch interrupted in between leaves
   either the outgoing credential intact or the target's in place (§9.6). The auth axis is
   single:
   - **Writing OAuth** deletes the managed-key item and drops `primaryApiKey`. The `approved`
     list is kept.
   - **Writing an API key** appends the key's last 20 characters to
     `customApiKeyResponses.approved`, stores the key in `Claude Code[-hash]` (or
     `primaryApiKey` when the Keychain is unavailable), and clears OAuth. Clearing OAuth
     removes the account-scoped keys (`claudeAiOauth`, `trustedDeviceToken` and unknown
     siblings) from the live credential entry and keeps its machine-shared keys, in the
     Keychain item and in `.credentials.json` alike. An entry is deleted only when no
     machine-shared key remains in it.

   Before the journal row, every live entry on either axis that step 7 will overwrite or
   delete, and that holds an account-scoped secret, is displaced unless its generation is
   already kept. Kept means held in the current or `.prev` vault generation of the outgoing
   account or the target, or settled by step 2 or 4. The provider names these entries under
   the locks. An entry that step 7 destroys only if the Keychain refuses the write (Appendix
   A.3) is displaced as the fallback begins, not before. A failed displacement aborts, except
   under `--force`. §9.6 recovery applies the same rule before it clears the other axis. There,
   the accounts are the two the row names, and the outgoing generation the row journaled
   counts as settled.
8. **Splice** the target's `oauthAccount` into `~/.claude.json` (§9.5).
9. **Commit** in one store transaction: set the active account and its activation epoch (the
   target's `login_epoch`, which cannot move while its account lock is held), insert an
   `events` row (`source` = `cli` or `auto`), and delete the journal row. An auto-switch also writes its
   `autoswitch_state` record in this transaction (§11.2 step 11).
10. **Rollback.** Any failure in steps 7–9 restores, in reverse order, the original
    `~/.claude.json` bytes and the original live credential, then restores the journal row's
    `prior` if it carried one — a forced switch's superseded row (§9.6) — or deletes the row
    otherwise. Writing the original credential back is safe here, and only here: CC's credential
    locks have been held throughout, so CC cannot have rotated it.
    - CC can still write the entry without those locks, under the storage-write lock: its
      dead-token marking, an MCP token update (§9.1).
    - So each credential entry is put back byte for byte, to what its places held just before
      tagteam first changed them, and only while nothing has written them since. That is
      re-read under the storage-write lock.
    - Otherwise the entry is left exactly as it is. The rollback never merges, and never moves
      keys between places.

    The operation fails with "rolled back", or with "rollback also failed" listing what could
    not be restored; the journal row then stays for recovery (§9.6), which decides from the
    live credential. This covers errors and panics, through `Drop`. A
    killed process runs no `Drop`: §9.6 covers that.

**After unlocking:**
- Replan the new active account's poll: `next_poll_at = max(now, fetched_at + 180)`, interval
  180 s.
- The human output prints the result and a hint:
  - Keychain: "applies within ~30 s; restart Claude Code to apply now"
  - File store: "active on your next message"

### 9.5 The `~/.claude.json` splice

A minimal JSON scanner locates the byte span of the top-level key's value, and only that span
is replaced.

- **Missing key:** it is inserted before the closing `}`, with CC's 2-space indentation.
- **The new value** is serialized with 2-space indentation, matching `JSON.stringify(v, null, 2)`
  and nested at the correct depth.
- **Missing file:** it is created containing only the new key, with mode 0600.
- **Unreadable, torn, or not a JSON object:** abort without writing. The error says to restore
  the file from Claude Code's backups (`~/.claude/backups/`) or repair it, then retry. tagteam
  never replaces a file it cannot splice, because that would drop everything else in it (§3).
  This departs from cswap, which saved a salvage copy and replaced the file.
- **Atomic write through symlinks.** The temp file is created beside the *resolved* target, then
  fsynced and renamed. The mode is preserved, or 0600 for new files. The temp file is named
  `.<name>.tagteam-<pid>-<hex8>`, so one left by a killed writer is recognizable, along with
  whether its writer still runs (§13.6).

The same primitive writes `.credentials.json` and every other file tagteam writes.

### 9.6 Interrupted-switch recovery

A `switch_journal` row whose holder is not live (by the pid and start-time rules of §12.6)
means a switch died between its first live write and its commit, or failed its own rollback.
The next command that takes `MutationGuard` recovers it before doing anything else, under the
same locks as a switch, using an oracle result taken before locking. Until then, neither
account named in the row is refreshed by the gate or activated (§7.3). `doctor` reports such a
row.

**The credential decides.** It is what CC acts on, so recovery reads it first and then makes
every other surface agree with it. The surfaces are the credential entry, the managed-key axis
(`Claude Code[-hash]`, `primaryApiKey`), `oauthAccount`, and the store's active account. Rows
are checked in order:

| Live credential (either auth axis) | Meaning | Action |
|---|---|---|
| The target's is present (`to_fp`), or the oracle resolves it to `to_id` | The switch landed; CC may have rotated the credential since | Finish forward: clear the other auth axis (§9.4 step 7), splice the target's `oauthAccount`, and commit (step 9). The activation epoch recorded is the row's `to_epoch` (for a row written before that column, `to_id`'s current `login_epoch`), so a replacement that landed on `to_id` since leaves the live store stale-marked |
| The outgoing one is present (`from_fp`), or the oracle resolves it to `from_id` | The switch never landed, or its credential rollback succeeded | Finish backward, without touching the credential: splice `from_identity` back into `oauthAccount` if the live object names a different identity (by identity key; a CC-updated object for the same identity is kept), and keep the store's active account |
| Anything else, including no credential on either axis | Undecidable. For example, CC rotated the credential while the oracle is unavailable; or it rotated it and then logged out, so absence does not prove the switch never published | Keep the row. Account-changing commands for the provider refuse with `interrupted-switch` until recovery can decide; `switch --force` resolves it by displacing any live credential and activating the chosen account. The vault is never re-activated on absence alone |

**Account-changing commands** here are the ones that read or write credentials or the live
login: `switch` (without `--force`), `add`, `add-token`, `import` and `remove`. `alias`,
`disable`, `enable` and `move` change only store metadata and proceed regardless. They still
attempt recovery, but from fingerprints alone: they never ask the oracle (§7.6). `purge`
recovers what it can and otherwise deletes the row with a warning (§10.5), and `export` treats
the accounts the row names as broken (§13.3).

**What a forward finish does with the entries it clears.** Clearing the other auth axis follows
§9.4 step 7's rule, with the generation the row journaled (`from_fp`) counting as settled. An
entry holding any other generation is classified as §9.4 step 4 would classify it, but without
the `Unresolved` capture:
- If the pre-lock oracle resolved it to `from_id`, and its bytes have not changed since, it is
  written to `from_id`'s vault under the account lock recovery already holds. §6.2 bounds the
  write as it does `OursRotated`, and `account_uuid` is backfilled if NULL.
- Otherwise it is displaced.

The switch's `Unresolved` capture relies on the live login naming the outgoing account while
the switch holds the locks. Recovery can run long after the crash, even after a re-login, so
that inference no longer holds.

**Forced-switch put-back.** An undecidable row is not discarded by `switch --force`: the forced
switch's journal write replaces it (`INSERT OR REPLACE`) with a new row carrying the superseded
row in `prior`. If that forced switch is itself rolled back (§9.4 step 10), or a later crash's
recovery finishes backward (the row above), `prior` is restored as the journal row instead of
being deleted — putting the undecidable case back for a future switch to settle. When the row
being replaced had no `prior`, rollback and backward recovery just delete the row as before.

The row is deleted only after every surface has been re-read and found coherent: all of them
name the same account. Recovery never writes an old credential back. CC may have rotated the
live one after the crash released CC's locks, and restoring would overwrite that rotation.

## 10. Account lifecycle commands

### 10.1 `add [--position N] [--alias A] [--yes]`

Captures the live login.

1. **Read the live identity once.** Read `(email, org_uuid, account_uuid, oauthAccount)` from
   `~/.claude.json` in a single read.
2. **Read the live credential** from the store CC would use in this environment (Appendix A.2).
   A degraded read is refused.
3. **Guards,** in order:
   1. A live managed API key: refuse, and point to `add-token`.
   2. A credential owned by someone else: use the oracle (uuid first, org corroborating).
      - Mismatch: refuse.
      - An expired access token or an oracle failure: register with a "could not verify"
        notice. Never refresh here.
   3. Re-read after verification. If the credential's lineage moved during verification, or the
      identity changed since step 1, refuse.
4. **Write.**
   - **Existing `(email, org)` with no `--position`:** refresh that account's vault and
     metadata in place.
   - **`--position` occupied by another account:** confirm, or use `--yes`.
   - **The same account at another position:** move it.
   - In every case, clear its quarantine.
   - The live store now holds exactly what the vault holds, so the store's active account
     becomes this account, with its current `login_epoch` as the activation epoch (§12.5),
     written in the same transaction as the account's last store write (for a replacement, the
     one that clears `replacing_fp`).

### 10.2 `add-token <TOKEN|-> [--position N] [--email E] [--alias A]`

- **Token source.** `-` reads one line from stdin; no argument prompts without echo.
- **Classification.** `sk-ant-api…` creates an `api_key` account. Anything else is a setup token
  (`setup_token`).
- **Default email:** `api-key-<N>@token.local` or `setup-token-<N>@token.local`, where N is the
  target position, or the next higher N whose identity no account holds. A defaulted email never
  names an existing account, so only an explicit `--email` replaces a token account in place, and
  `--position` over another account needs confirmation or `--yes` as in §10.1.
- **Validation.** A supplied email is validated with
  `^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$`. A collision with an account of a different
  kind under the same `(email, '')` is refused.
- **No network calls.**

### 10.3 Other commands

- **`remove <ACCOUNT>`** deletes, in this order: the vault entries (strict), the account's
  rescue files (§6.3), the session profile, and last the store row, which cascades to the
  mappings and usage rows. For the profile it deletes the profile's hashed Keychain item
  first, named from the spelling its marker records (§12.2), then the directory. The
  directory's links are removed as links; nothing they point to is touched.
  - **The order is what makes a `remove` that stops part-way safe.** The vault goes first, so
    no generation older than a rescue or a rotated profile can outlive it: an account
    without a vault credential is never switched to, refreshed or launched (§7.3 step 3,
    §9.3, §12.3). The row goes last, so the account stays listed, and running `remove` again
    finishes, since every delete treats an absent item as done.
- **`disable` / `enable <ACCOUNT>`** hold an account out of automatic selection. It stays a
  valid explicit `switch` target.
- **`alias <ACCOUNT> <NAME>` / `alias <ACCOUNT> --unset` / `alias`** (list).
  - Aliases are lowercase and match `^[a-z0-9_.-]+$`.
  - They cannot be all digits or start with `-`, and they are unique case-insensitively.
- **`move <ACCOUNT> <POSITION>`.** If the target position is taken, the two accounts swap
  positions.
- **Guard.** Destructive commands (`remove`, `purge`, `add` over an occupied position, and
  profile bootstrap) refuse while an affected account is session-owned: a live launch
  reservation, or a session record that is live **or unreadable**. `move` is not destructive:
  positions are display order only, and a profile is keyed by the account's ID.

### 10.4 Account references

An `ACCOUNT` argument is resolved in this order:

1. All digits: a position within the chosen provider (§13.1).
2. An alias. Aliases are unique across providers, so an alias alone identifies the provider.
3. An exact email, within `--provider` if given, otherwise across all providers.

If an email matches several orgs, or several providers, it is ambiguous:
- On a terminal, prompt with the candidates.
- With `--json`, or with no terminal, fail with an error listing the candidates.

An empty alias never matches.

### 10.5 `purge [--provider P] [--yes]`

Deletes tagteam's data: every account of the provider, or with no `--provider`, everything
tagteam stores. It never deletes or replaces a provider's live login. It writes the default
home only to finish an interrupted switch, as every command that takes `MutationGuard` does
(§9.6).

**Order.** Inside a run shell, purge refuses at once (§12.8). Otherwise:
1. On macOS, the Keychain lock check (Appendix A.3).
2. The summary and confirmation, before any lock is taken, so none is held while the user
   reads: the accounts by position and label, their profiles, orphaned profiles (step 6),
   pending rescues (refreshed tokens not yet in the vault), displaced credentials, and for a
   full purge the store and the log.
   Purge asks on a terminal. Without one, or with `--json`, it requires `--yes`, and otherwise
   fails with `needs-confirmation`.
3. Each affected provider's engine lock (§11.1), tried and held to the end, so no auto-switch
   engine starts while purge runs. A lock that is held refuses the purge, naming the engine's
   pid from its record.
4. `MutationGuard`, held to the end. Every command that adds an account, replaces a login,
   launches a session, captures or switches takes it, so nothing is created behind purge;
   such a command run meanwhile may fail with `lock-timeout`.
5. Recovery of an interrupted switch (§9.6). If recovery cannot decide it, purge deletes the
   journal row with a warning that the live login may be incoherent (its credential and
   `oauthAccount` may name different accounts), and leaves the live login as it is: purge is
   the way out of a state tagteam cannot repair.
6. The refusals that need the guard:
   - an affected account that is session-owned (§10.3's guard), naming its session;
   - an orphaned profile that is not quiescent: a profile directory under `sessions/` whose
     marker names no store account, or cannot be read, and that has a live launch
     reservation or a session record that is live or unreadable (§12.5, §12.6). An orphan
     counts as affected when its marker names the provider being purged, or cannot be read,
     or the purge is full;
   - a set of affected accounts that differs from the one confirmed, because another command
     added or removed one in between, asking to run purge again.
7. The accounts, one at a time, each under its account lock, re-checking that it is not
   session-owned, exactly as `remove` deletes one (§10.3), vault first. In a full purge a
   `rescue` path that cannot be listed (§6.3) does not stop it: the accounts' rescue step is
   skipped, and the whole path goes at step 9, after every vault. A `--provider` purge refuses
   on one, as `remove` would.
8. The orphaned profiles that count as affected, each after deleting the hashed item its
   marker's spelling names, or, when the marker cannot be read, the item its canonical path
   names.
9. The rest of the provider's data, or of all data (below).

**A purge that stops part-way,** interrupted or failing on one item, can leave an account
partly deleted. Since each account's vault goes first (§10.3), such an account has no vault
credential, so it is never switched to, refreshed or launched, and no consumed generation is
left where a newer one was deleted. `doctor` reports it (§13.6), and running `purge` or
`remove` again finishes it, since every delete treats an absent item as done.

**With `--provider P`,** purge then deletes P's remaining rows: its displaced entries (each
file before its row, §6.3), events, `autoswitch_state`, `active_accounts`, `live_identity_cache`
and `switch_journal`. Its `usage_requests` rows stay. They age out within the hour, so the
budget (§8.6) does not reset.

**Without `--provider`,** purge then deletes:
- every Keychain item of service `tagteam`, by deleting by service until none is left
  (Appendix A.3). This catches items whose store rows are gone, as after a store deleted by
  hand;
- `vault/` (Linux), the `rescue` path whatever it is, `displaced/`, and the atomic writer's
  temp files (§9.5) whose writer is gone, in tagteam's own directories only: one beside a CC
  file is outside the identity surface (§3), so `doctor` names it for the user to delete;
- the store's contents: every row of every table, in one transaction with `secure_delete` on,
  followed by a WAL checkpoint that truncates the WAL, so no deleted row survives in either
  file. The store file itself stays, with its schema, so a process that opened it before the
  purge goes on with a valid, empty store rather than a deleted file;
- the log and its rotations (§14.2).

It keeps `config.toml`, which the user writes, and the lock files, which hold no data:
deleting a lock file while another process waits on it would let two holders in. A full purge
resets the usage budget along with the store, the one exception to invariant 41; the
endpoint's own limit and the 429 backoff (§8.5) still apply.

**Result.** Purge reports what it deleted and anything it could not, and exits 1 if anything
could not be deleted. JSON: `{schemaVersion, ok, provider, accounts: [{number, id, email}],
displaced, rescues, storeEmptied, failures: [{what, message}]}`.

## 11. Auto-switch

### 11.1 Structure

- **`tagteam-core`:** `decide(snapshot, state, config, now) -> Decision`. It is pure: no
  clock, no I/O. It is built from named predicates (`below_threshold`, `landing_ok`,
  `beats_by_hysteresis`, `recovered_since_departure`, `recovery_axis_useful`, …), each unit
  tested. The scheduled-collection pick (§8.6) and the loop delay (§11.4) are pure functions
  there too.
- **`tagteam-engine`:** fetch, freshen, switch, record and emit.
- **Per provider.** `tagteam auto` runs one independent engine per provider that has at least
  two switchable accounts, or only the provider given with `--provider`. Each has its own
  `autoswitch_state` row, engine lock, settings (§6.4) and poll budget. Nothing on one provider
  ever triggers a switch on another. Every JSONL event carries an additive `provider` field.
  One process ticks its providers on independent schedules.
- **One engine per provider per machine.** An engine holds its provider's engine lock,
  `locks/autoswitch-<provider>.lock` (§5), for its whole life. The lock is a `flock` that is
  only ever tried, never waited for, and that the kernel releases when the holder dies.
  `autoswitch_state` therefore has a single writer, so no unhealthy tick or departure snapshot
  is counted twice.
  - A loop skips a provider whose lock another process holds, with a warning. If it gets no
    provider's lock at all, it exits 1, saying auto-switch already runs for them.
  - `--once` reports `no-switch` with reason `engine-running` for such a provider (NO_ACTION).
  - `--dry-run` takes no engine lock and writes no auto-switch state: its state lives in
    memory, it releases no quarantine (§11.2 step 1), and it freshens and switches nothing
    (step 10). Its usage collection is the ordinary one (§8.3). Like `list`, it can finish
    the recovery of an interrupted switch when it takes `MutationGuard` (§9.6): recovery
    repairs a switch that has already reached the live store, and decides nothing.
  - The daemon (sub-project 2) takes the same lock, so `auto` and the daemon never both drive
    one provider.
  - Once it holds the lock, the engine writes its pid and start time into the lock file, the
    start time taken as for the `switch_journal` holder (§12.6). The record is never used for
    exclusion. `doctor` reads it to tell whether an engine runs, by an exact pid and
    start-time match (§13.6), instead of trying the lock, which an `auto` starting at that
    instant would then find taken.
- **Where it runs.** Like every command that changes the live login, `auto` refuses inside a
  `tagteam run` shell (§9.2), except with `--dry-run`. On macOS it runs the Keychain lock check
  before its first tick (Appendix A.3).
- **`--once` with several providers** exits with the most severe outcome: `1` if any provider
  errored, else `0` if any switched, else `3` if any was blocked, else `2`.

### 11.2 Tick

1. **Load state.** Except in dry-run, release any quarantine that no longer binds (§7.4):
   neither the vault's fingerprint nor, for the live account, the live credential's equals
   `quarantine_fp`. Emit `account-unquarantined` for each. Quarantines that other processes
   set or cleared since the previous tick are reported too, as `account-quarantined` or
   `account-unquarantined`.
2. **Recover, then check the live account.**
   - An unresolved switch journal for the provider is recovered first, under `MutationGuard`
     (§9.6); its events carry `source` = `auto`. If recovery cannot decide it yet: `no-switch`
     with reason `interrupted-switch` (BLOCKED), whose detail carries recovery's reason.
   - No managed live account: `no-switch` with reason `unmanaged-active-account` or
     `no-active-account`; outcome NO_ACTION. tagteam never acts on a login it doesn't manage.
3. **Collect usage** as scheduled (§8.6) and emit `poll`. Check that the configured model
   names exist, once per engine and again whenever the setting changes, emitting
   `config-warning` if not.
4. **Active account is an API key.** With `include_api_key_accounts` false: `active-api-key`
   (NO_ACTION). With it true, the tick looks for a way back to OAuth: the active account counts
   as headroom 0, the trigger is `proactive`, and the landing must be below the threshold even
   when every account is above it (step 8's exception does not apply).
5. **Decide the trigger:**
   - **A quarantined active account** (`relogin_required`) → trigger `failover` at once,
     whatever its last reading says. Neither tagteam nor CC can refresh its token.
   - **Known headroom** resets the unhealthy-tick counter.
     - With `best`: usage below the threshold → `below-threshold` (NO_ACTION).
     - With `consume-first`: usage below the threshold → trigger `consume-first`.
     - Headroom ≤ 0 → `at-limit`. Otherwise → `proactive`.
   - **Otherwise unknown:** increment `unhealthy_ticks`. At `autoswitch.unhealthy_ticks`,
     trigger `failover`; before that, `active-usage-unknown n/N`.

   There is no idle-hold. An expired live token never leaves the usage unknown by itself: the
   tick's collection hands it to §7.5 (§8.1), so unknown usage means that refresh, or the
   fetch, failed.
6. **Cooldown.** `proactive` and `consume-first` within `cooldown_seconds` of the last switch →
   `cooldown`. `at-limit` and `failover` bypass it.
7. **Candidates** must be switchable, not the current account, not quarantined, and not
   session-owned. API-key accounts qualify only when `include_api_key_accounts` is true, and
   then only as a last resort (step 10), never for `consume-first`. No candidates →
   `no-candidates` (BLOCKED); for a `consume-first` trigger, whose active account is healthy,
   `below-threshold` (NO_ACTION) instead.
8. **Rank** the OAuth candidates:
   - **Skip** unknown headroom, headroom ≤ 0, and the barred account (§11.3).
   - **Landing rule.** For `proactive` and `consume-first`, the landing account must be below
     the threshold, unless *every* account is above it.
   - **`best`:** candidate headroom − active headroom ≥ `hysteresis_pct`. Order by most
     headroom; ties go to the lower position.
   - **`consume-first`:** ranked on the provider's `primary_long_window()` (CC: 7d); a
     provider without one runs `best` instead (§4.5). The target's reset of that window must
     be strictly sooner than the active account's (unknown → skip). Order by soonest reset,
     then most headroom. Before switching, re-check the current account and all candidates
     (§8.3) and re-rank; the target's reading must be ≤ 180 s old, else `stale-usage`.
   - **Every account above the threshold:** for each pair, pick the recovery axis when both the
     active account and the candidate are within 3 points of headroom, or either one resets
     within 4 h. Otherwise use the headroom axis.
     - The recovery axis requires the candidate's binding-window recovery to be ≥ 300 s sooner
       than the active account's.
     - The headroom axis requires ≥ 2 × the active account's headroom.
     - Select the binding window first, then its reset. A past or unknown reset sorts last.
   - **`at-limit` and `failover`** skip every anti-flap gate.
9. **Nothing ranked.** With an `at-limit` or `failover` trigger and API-key candidates
   (step 7), go to step 10 with those. Otherwise:

   | Situation | Outcome |
   |---|---|
   | No readable candidate | `no-comparison` (BLOCKED) |
   | `consume-first` found nothing | `reset-unknown` or `already-consuming-soonest` (NO_ACTION) |
   | Candidates not all known to be exhausted | `no-qualifying-candidate` (BLOCKED, normal cadence) |
   | Every candidate exhausted | `all-exhausted {earliestResetAt}` (BLOCKED); sleep until the earliest recovery + 60 s, capped at 600 s |

10. **For each ranked target, in order:**
    - Dry-run: emit `switch` with `dryRun: true` and write nothing.
    - Otherwise freshen it (§7.2):

      | Freshen result | Next step |
      |---|---|
      | `identity_conflict` or Dead | Quarantine it and emit `account-quarantined`; try the next target |
      | Transient or systemic | Try the next target |
      | `Owned` (session-owned) | Try the next target |
      | OK | Perform the switch |

    When every OAuth target has failed and the trigger is `at-limit` or `failover`, the
    API-key candidates are tried next, in position order. They do not refresh, so freshening
    passes them.
11. **Perform.** Call `switch` (direct) with the tick's preconditions. The switch re-checks
    them under its locks, before its first write (§9.4 step 1):
    - The live account is still the one this tick decided on. Otherwise: `no-switch` with
      reason `live-changed` (NO_ACTION), and the next tick decides afresh. A manual switch made
      meanwhile is never overridden.
    - For `proactive` and `consume-first`, the cooldown still allows a switch, judged from
      `autoswitch_state` as read under the lock. Otherwise: `cooldown`.
    - The target is still a candidate (step 7): switchable, so neither disabled nor
      quarantined, and not session-owned. A direct switch would accept a disabled or
      quarantined target (§9.3, §7.2), so auto checks for itself. Otherwise the switch writes
      nothing, and the tick moves on to its next target (step 10).

    The switch's commit (§9.4 step 9) records `last_switch_*`, `left_headroom`,
    `left_recovery_at` and `left_trigger` together with the switch. The check and the record
    are both made under `MutationGuard`, which every switch holds, so no two switches can pass
    one cooldown. Then emit `switch`.
12. **Every target failed:**
    - Transient or systemic failures: `error` "could not freshen…" (ERROR).
    - Otherwise: `no-viable-target` (BLOCKED).

### 11.3 No-return rule

After the engine switches A → B, a `proactive` or `consume-first` move back to A is barred while
the engine is still on B, unless A has *recovered* against its departure snapshot. Recovery
means any of:

- headroom is ≥ 3 points higher
- the binding-window reset is ≥ 300 s sooner
- dominance: A's headroom > 2 × the active account's + 3

A failover departure is judged on the landing and recovery legs. The bar is lifted only when the
barred ranking is empty *and* A has recovered; the real ranking is then re-run without the bar.
A missing departure snapshot lifts the bar.

### 11.4 Loop, exit codes, and events

**Loop delay:**

| Outcome | Delay |
|---|---|
| BLOCKED with a known reset | `min(max(until − now, interval), 600)` |
| BLOCKED otherwise | `max(interval, 300)` |
| Anything else | `interval × U(0.9, 1.1)`, shortened (never lengthened) to the active account's `next_poll_at`, floored at 60 s |

A `sleep` event is emitted when the delay is more than 1.5 × `interval`.

- **Sleeping.** The loop sleeps toward a wall-clock deadline, in slices of at most 1 s, and
  checks the cancel token in each slice (§14.1). A machine that was suspended therefore ticks
  as soon as it wakes past the deadline. macOS's monotonic clock stops during sleep, so a
  monotonic sleep would add the suspended time to the delay.
- **Settings** are re-read before a tick whenever `config.toml`'s mtime has changed. Flags
  still override them (§6.4).
- **Errors.** A tick that fails emits `error`, and the loop goes on after the normal delay.
  With `--once`, the command exits 1.
- **Signals.** SIGINT, SIGTERM and SIGHUP stop the loop at its next cancellation point, after
  any critical span in flight has finished (§14.1). The loop then exits 0. An interrupted
  `--once` exits as §14.1 says.
- **Human output.** Each tick prints one line on stdout: the local time, the active account
  (position and label), its relevant usage, and the outcome (the reason, or the switch).
  Quarantine changes and long sleeps get lines of their own. Errors and config warnings go to
  stderr.

**`--once` exit codes:** `0` switched · `1` error · `2` no action · `3` blocked.

**`--json`** emits one JSON object per line, with the envelope
`{"schemaVersion":1,"event":<kind>,"ts":"…Z",…}`. Kinds and fields match cswap, and consumers
must ignore unknown kinds and fields.

| Event | Fields |
|---|---|
| `poll` | `active`, `headroomPct`, `threshold`, `fetchErrors?`, `windowsPct?` |
| `switch` | `trigger`, `from`, `to`, `warnings`, `dryRun` |
| `no-switch` | `reason`, `detail` |
| `account-quarantined` / `account-unquarantined` | `number`, `email`, `reason` |
| `all-exhausted` | `earliestResetAt` |
| `sleep` | `seconds`, `until` |
| `error` | `message`, `transient` |
| `config-warning` | `message` |

- `no-switch` reasons are cswap's, plus `engine-running`, `live-changed` and
  `interrupted-switch`. cswap's `active-idle` is never emitted (§11.2 step 5).
- `account-unquarantined`'s reason is `account-replaced` when the account's `login_epoch`
  moved (an explicit replacement, §12.5), else `credentials-replaced`.

The flags `--once`, `--dry-run`, `--json`, `--threshold`, `--interval`, `--cooldown`,
`--strategy`, `--model` and `--include-api-key-accounts` override the settings.

### 11.5 Simulation tests

A proptest harness drives `decide()` through multi-day synthetic traces: 2–6 accounts, burn
rates, 5h and 7d resets, 429s, dead tokens, unknown readings, and API-key accounts. It asserts:

- no A→B→A return within a cooldown unless A recovered
- never landing on an account with headroom ≤ 0
- never idling at the limit while a viable candidate exists
- never landing on an API-key account unless the trigger is `at-limit` or `failover` and no
  OAuth target was viable; a return from an API-key account always lands below the threshold
- a quarantined active account fails over on the first tick that has a viable candidate
- `--once` exit codes consistent with the outcome
- deterministic results for a given seed

The tick itself is tested in the engine against both providers (§15.2). `FakeAgent` names no
`primary_long_window`, so a `consume-first` setting runs `best` there.

## 12. Parallel sessions: `tagteam run`

### 12.1 Invocation

`tagteam run [ACCOUNT] [--provider P] [--require-session] [-- <agent args>]`

The command launched is the provider's `launch_command` (`claude` for Claude Code). The rest of
this section describes the Claude Code provider; the sharing, liveness and capture rules are
engine-generic, with the provider supplying paths, the share lists and the env vars.

- **With no `ACCOUNT`,** use the nearest mapped ancestor of the current directory's canonical
  path, for the provider given with `--provider` (default: `default_provider`).
  - A mapping to a removed account: warn and run plain `claude`.
  - No mapping: run plain `claude` with an untouched environment.
- **API-key accounts** are refused.
- **The target is already the live default login:** run plain `claude`, so there are never two
  copies of one rotating token.
- **`--require-session`** refuses wherever `run` would otherwise run plain `claude`: no
  mapping, a mapping to a removed account, or a target that is the live login.
- **Plain `claude`** means `exec`: tagteam replaces itself with the launch command, with the
  environment it was given. No reservation is made and no parent stays resident, so signals,
  the terminal and the exit status are `claude`'s own. Inside a run shell, the outer home's
  variables are first restored from the profile marker (§12.8), so plain `claude` runs in the
  default home, as it would have outside.
- **The launch command** is looked up on `PATH` before any lock is taken. If it is missing,
  `run` fails with exit 1 and changes nothing.
- **`--json`** covers only `run`'s own errors before the launch, as the usual error envelope.
  Once the launch command starts, stdout is its own.
- **Inside a run shell** (§12.8), `run` resolves the outer home from the profile marker and
  launches as it would from there, so a session can start a session of another account.

### 12.2 Profile layout

The profile is `$XDG_DATA_HOME/tagteam/sessions/<id>/`.

**One spelling.** `CLAUDE_CONFIG_DIR` is set to the profile's canonical path: resolved with
`realpath`, NFC-normalized, absolute, with no trailing slash. CC names the profile's Keychain
item from exactly that string and, since 2.1.286, tries no other spelling (Appendix A.2). The
spelling is recorded in the profile marker, and every operation on the profile's credential
(bootstrap, validation, capture, the session-owned usage read, and the hashed-item deletion in
`remove` and `purge`) uses the recorded spelling, never one derived again. A profile whose
canonical path no longer matches its recorded spelling, because the data directory moved,
needs a bootstrap (§12.3), which also deletes the item under the old spelling.

**Profile marker.** `<profile>/.tagteam-profile.json` holds `{"format": "tagteam-profile",
"version": 1, "provider", "accountId", "configDir", "outer"}`. `configDir` is the exported
spelling. `outer` is the provider's record of the home the profile shares from, as `run` found
it (§4.5 `outer_home`); for Claude Code, whether `CLAUDE_CONFIG_DIR` and
`CLAUDE_SECURESTORAGE_CONFIG_DIR` were defined, and their values. It holds no secret. A
profile's first launch creates it. Every quiescent launch then updates `outer` before it syncs
links; only a bootstrap changes `configDir`, once it has deleted the item under the spelling it
replaces (§12.3 step 5). The marker is what makes a process a run shell (§12.8).

**Shared by allowlist.** The provider names the entries of the source home that a profile
shares (§4.5 `share_policy`). Each one present in the source home is symlinked into the profile,
pointing at the *fully resolved* source: CC follows at most one link when it writes through one
(Appendix A.1, anthropics/claude-code#78162). Every other entry stays private to the profile,
and CC creates its own copy there when it needs one. For Claude Code:

| Shared | Holds |
|---|---|
| `projects/`, `history.jsonl` | transcripts, auto-memory and prompt history (must-share, below) |
| `CLAUDE.md`, `settings.json`, `keybindings.json` | the user's instructions and settings |
| `agents/`, `commands/`, `skills/`, `plugins/`, `hooks/`, `output-styles/`, `themes/`, `rules/`, `workflows/` | the user's customizations |
| `file-history/`, `paste-cache/`, `shell-snapshots/`, `session-env/` | per-session state keyed by session ID, which a session resumed in the other home needs |

`run.share_extra` (§6.4) adds entry names to the allowlist, for a user's own files that hooks
or settings reach through the profile. A known-private name there is ignored with a warning.

**Unknown entries are private.** An entry on neither list stays private to the profile. Sharing
by default would share any per-account state a later CC version adds, as 2.1.286 already holds
entitlement caches, org policy and a credential-using daemon in its config home. The first
launch that meets an unknown entry prints one notice naming it, and records it in
`.tagteam-links.json` so the notice is not repeated; `doctor` lists them all.

**Must-share entries.** `projects/` (transcripts and auto-memory) and `history.jsonl` hold the
memory and history that §3 protects, and CC creates both on demand. If either is absent from
the default home, tagteam first creates it empty there (the create-only row of §3), so the
profile always links to it and CC never starts a private copy. If a profile holds a real
`projects/` or `history.jsonl` in place of tagteam's link, `run` refuses and names both
paths, for the user to merge by hand. Splitting memory or history silently is never an
option.

**Shared files can split.** CC writes most files by renaming a temporary file over the path,
which replaces a symlink with a regular file; only `.claude.json` and the user `settings.json`
are written through a link (Appendix A.1). A shared directory is therefore always safe, since
CC writes inside it. Of the shared files, `history.jsonl` is appended to and `settings.json` is
written through its link; `CLAUDE.md` and `keybindings.json` are normally edited by the user.
A launch that finds a shared file replaced by a regular file in the profile refuses for a
must-share entry, as above, and otherwise warns, naming both paths. tagteam never merges or
replaces either copy.

**Private to the profile** (the known-private list, so that `doctor` can tell known entries
from unknown ones):

| Entry | Why it's private |
|---|---|
| `.credentials.json` | the profile's own credential |
| `.claude.json`, and its variants under non-production OAuth settings (`.claude-*-oauth.json`) | the profile's own config; see §12.4 |
| `.config.json` | CC's legacy global config, which CC prefers over `.claude.json` when it exists (Appendix A.1). Shared, it would make the profile read and write the default identity and config |
| `sessions/`, `ide/`, `jobs/` | per-profile process records and background jobs |
| `daemon/`, `daemon.json`, `daemon.lock`, `daemon.log`, `daemon.status.json`, `daemon.scheduled.status.json`, `daemon-auth-cooldown`, `daemon-auth-status.json` | CC's background daemon, which reads and refreshes the profile's credential (Appendix A.7) |
| `backups/` | CC's config backups; restoring from a shared one could cross profiles |
| `cache/`, `mcp-needs-auth-cache.json`, `stats-cache.json` | per-account model entitlements, connector state and caches |
| `policy-limits.json` (and `.signature*`, `.stamp`), `remote-settings.json`, `remote-settings-consent.json`, `remote-settings-helper-consent` | an organization's managed policy, which must not reach another account's sessions |
| `.session_ingress_token`, `hfi-auth.json` | tokens |
| `state/`, `seed-admin/`, `bridge-spawn/`, `chrome/`, `debug/`, `feedback/`, `routines/`, `settings.local.json`, `.last-cleanup`, `.cc-writes/` | per-home consents, uploads, logs and housekeeping |
| `.device-keys.json` | unused in a profile: CC keeps device keys machine-wide (Appendix A.7) |
| `*.lock`, `*.lock.owner`, `.storage-write`, `.*_auth_refresh-*` | lock directories, owner records and refresh coordination files |
| `.tagteam-*` | tagteam's own files |

**Sync runs on every launch.**
- Create any missing links.
- Remove only links that tagteam created (tracked in `<profile>/.tagteam-links.json`) whose
  source has disappeared, whose entry has left the allowlist, or whose source is no longer in
  the outer home that the marker records. A link to a moved source is created again.
- A launch that joins a running session only creates missing links: the links a running
  session uses never change under it.
- A real file or directory where a link belongs is never replaced; tagteam reports it (and
  refuses, for a must-share entry).
- Real history directories are never deleted.
- `.tagteam-*` entries are tagteam's own and are never linked.

### 12.3 Bootstrap and validation

This runs within a launch (§12.5), under `MutationGuard` and the account lock, and only when
the profile is quiescent and is missing, invalid, stale-marked, holding a credential other than
the vault's current generation, or recorded under a spelling that is no longer its canonical
path (§12.2). Both locks are held through validation, so other commands may wait up to their
own lock timeouts while a bootstrap runs.

1. Refresh the vault credential through the gate first, before the launch takes its locks.
   - `Transient` with `rescued`, and `Unpersisted`, abort with advice.
   - A plain `Transient` continues with the stored credential.
2. Read the profile's current credential the way CC would (the hashed Keychain item for the
   recorded spelling, then `<profile>/.credentials.json`). An unreadable or degraded read
   aborts the launch: tagteam never overwrites a credential it could not read. A new profile
   has none.
3. If the profile is stale-marked, displace that credential (§6.3): it may be a live
   generation of the login that was replaced.
4. **Compose and write** `<profile>/.credentials.json` (0600), under the profile's own
   credential locks and storage-write lock (§9.1), as an activation composes (§9.4 step 5)
   with the profile as the live store:
   - the account-scoped keys come from the vault's current generation;
   - the machine-shared keys (MCP OAuth tokens, plugin secrets; Appendix A.4) come from the
     profile's own credential read in step 2, and their absence too. A new profile starts with
     none: its MCP servers authenticate once in the profile, and a rotating MCP token never has
     a copy in two homes.

   CC moves the file into its own hashed item on its next credential write, and then deletes
   the file (Appendix A.3). tagteam never writes that item.
5. **Then always** delete the profile's hashed Keychain item (macOS), whatever the reason for
   the bootstrap, and verify it `Absent` with the existence probe (Appendix A.3). When the
   spelling changed, the item under the old spelling is deleted and verified too. CC reads the
   Keychain first, so an item left behind, such as the consumed generation from a
   refresh-driven re-bootstrap, would stay authoritative over the file. If the item cannot be
   verified absent, the launch aborts.

   The file is written before the item is deleted, so the machine-shared keys the item holds
   are already on disk when it goes. A bootstrap that stops between the two leaves the old item
   authoritative and the seed unchanged, so the next launch bootstraps again and reads the same
   keys from the same item.
6. **Verify the effective credential.** Re-read the profile's credential the way CC would
   (Keychain first, then the file), and check that its account-scoped keys are the vault's
   current generation. Only then record the profile's provenance in
   `<profile>/.tagteam-seed.json` (the account's current `login_epoch`, and that generation's
   fingerprint as the seed) and, when the spelling changed, the new spelling in the profile
   marker (§12.2). A mismatch aborts the launch.
7. Seed `<profile>/.claude.json` (§12.4).
8. Validate with `claude auth status --json` (JSON is its default output since 2.1.286, and
   `--json` is still accepted), in exactly the session environment (§12.5) and in the
   directory `claude` will run in, with a 10 s timeout:

   | Outcome | When | Action |
   |---|---|---|
   | `valid` | `rc == 0`, `loggedIn === true`, `authMethod` is the account's own login (`claude.ai` for CC's OAuth accounts, and for setup-token accounts, *inferred*), `configDirectory` equals the recorded spelling, the `email` matches, and the `orgId` matches when both are present | Launch |
   | `invalid` | Not logged in (`authMethod` `none`), or logged in to `claude.ai` as another email or org | Delete the profile, and refuse with advice; the next `run` bootstraps afresh. **Only `invalid` deletes a profile** |
   | `overridden` | Logged in by another method: `api_key`, `api_key_helper`, `oauth_token` or `third_party` | Refuse and keep the profile. The message names the method, and `apiKeySource` when present: something outside the profile, such as an `apiKeyHelper` or `env` entry in the shared `settings.json`, or a workload-identity profile, takes precedence over the account's login |
   | `drifted` | `configDirectory` differs from the recorded spelling | Refuse and keep the profile: CC resolves another config dir than tagteam computes (Appendix A.1) |
   | `unknown` | A timeout, or output that does not parse | Refuse and keep the profile. Step 6 verified the credential CC will read, but not that it is the source CC will use, so a login that cannot be confirmed is not launched. The message names the cause |
   | `unreachable` | The launch command could not be spawned | Refuse |

**Every launch is checked.** A login can be overridden after a profile was bootstrapped: by an
`apiKeyHelper` or `env` entry added to the shared settings, by a project's own settings in the
directory `claude` runs in, or by a workload-identity profile. So a launch that did not
bootstrap runs the same command after it releases its locks and before the spawn, with its
reservation held (§12.5); it costs about 0.1 s. `valid` launches. `overridden`, `drifted`,
`unknown` and `unreachable` refuse as above. `invalid` refuses, keeps the profile, and records
in its seed file that it needs a bootstrap, which the next launch performs. A launch refused
after its reservation exists runs its exit handling (§12.5) as if `claude` had exited at once,
and exits 1.

### 12.4 Profile `.claude.json`: seed and merge-back

**Seed, on every launch into a quiescent profile.** A launch into a profile that already has a
live session joins it without seeding, so a running session's changes and its baseline are
never overwritten (Appendix B.28). If a baseline is left over from a session whose merge-back
never ran (its `tagteam` parent was killed), that merge-back runs first, and if it fails, the
launch aborts with the profile and its baseline untouched: seeding over them would discard the
unmerged changes. Then start from the profile's current file, or `{}` if there is none, and:
- copy `projects` and top-level `mcpServers` from `~/.claude.json`
- set `oauthAccount` from the account
- set `hasCompletedOnboarding: true`, and set `theme` if absent (from the default file, else
  `"dark"`)
- write `<profile>/.tagteam-baseline.json`, a snapshot of the seeded `projects` and `mcpServers`

`hasCompletedOnboarding` is CC's only onboarding gate. Project trust (`hasTrustDialogAccepted`)
and the per-project MCP approvals travel inside `projects` (Appendix A.6). Nothing else is
copied: the profile keeps the per-home identifiers CC creates on its first start (`userID`,
`machineID` and the like), as any second config dir would.

The write takes the profile's own config lock.

**Merge back, when the last session exits** (§12.5). This runs inside the exit handling,
under `MutationGuard`, and takes the default profile's config lock (`~/.claude.json.lock`) on
its own. If `~/.claude.json` cannot be spliced (§9.5) or written, the merge-back fails: it
warns, and keeps the profile's changes and the baseline, so it is retried before any re-seed.

1. Diff the profile's `projects.<path>.<key>` and `mcpServers.<name>` against the baseline.
2. Apply each changed or removed key to `~/.claude.json` with the §9.5 splice (`projects` and
   `mcpServers` subtrees only).
3. If the default file also changed a key since the baseline, the default wins. tagteam
   prints one summary line on stderr (how many keys changed on both sides, and that the
   default file's values were kept), and names each key in the log.

Account-specific fields are never merged back.

### 12.5 Process model

`tagteam` stays resident: it spawns `claude` and waits for it. While it waits, it holds no lock
that anything waits on; the reservation lock below is only ever tested, never waited for.

**Launch reservation.** CC writes its own session record only some time after it starts, and
removes it before its process ends. The reservation covers both gaps, and survives the parent's
death.
- It is the file `<profile>/.tagteam-launch/<pid>.lock`, holding the parent's pid and
  `startedAt` for diagnostics.
- The parent creates it under a temporary name, takes `flock(LOCK_EX)` on it, and renames it
  into place, so it never appears unlocked. The locked fd is passed to `claude` without
  `O_CLOEXEC`, so the kernel holds the lock for as long as the parent **or** the child lives.
- **A reservation is live while its file is locked.** Others test it with a non-blocking
  `flock`, and never wait on it. No pid heuristics are involved, and a parent killed while
  `claude` runs leaves a reservation that is still live.
- After exit handling, the parent unlinks the file. Any descendant of `claude` that still holds
  the inherited fd no longer matters, because liveness is judged by the path. If the parent was
  killed and a background process started by `claude` outlives it, the account stays
  session-owned until that process exits too. That is the safe direction, and `doctor` lists
  such reservations.
- A reservation is created, and a dead one removed, only under `MutationGuard` and the account
  lock. Launch and exit are therefore serialized with every other ownership change.

**Launch**, under `MutationGuard` (30 s timeout), then the account lock:
1. **Re-check the fast path.** If the target has become the live default login since
   `run` decided (§12.1), release the locks and run plain `claude` instead, or refuse under
   `--require-session`. Otherwise one token would get a default copy and a profile copy.
2. Remove this profile's dead reservations. If the profile is quiescent and a baseline is left
   from a session that never merged back, merge it back now (§12.4); a failure aborts the
   launch.
3. Sync links (§12.2). If the profile is quiescent:
   - apply the profile's provenance (below). If the profile rotated, capture it. If the vault
     moved on, or the profile is stale-marked, bootstrap it (§12.3). If there is a conflict,
     refuse the launch. A profile credential that is unreadable or degraded also aborts the
     launch, because tagteam never overwrites a credential it could not read;
   - bootstrap for any other §12.3 reason, then seed (§12.4).

   If the profile is not quiescent, join the running session without seeding; the sync only
   creates missing links (§12.2).
4. Create this process's reservation.
5. Release `MutationGuard` and the account lock.
6. Unless this launch bootstrapped, check the session's login (§12.3, "Every launch is
   checked"). A refusal runs the exit handling below as if `claude` had exited at once.
7. Spawn `claude` with the reservation fd. It stays in the terminal's foreground process group
   (§14.1).

**Signals** (§14.1 sets the general rules; these are `run`'s):
- **Before the spawn**, every lock wait is a cancellation point, and so is the wait for the
  login check (§12.3), whose process is then killed. The token is checked once more right
  before the spawn. An interrupted launch exits as §14.1 says and launches nothing; if its
  reservation already exists, its exit handling runs first, as for a refused launch. A signal
  recorded after that last check, SIGINT included, is sent to `claude` as soon as it is
  spawned: the terminal cannot deliver it to a process that did not exist yet.
- **While `claude` runs**, the parent ignores SIGINT and SIGQUIT (the child owns the terminal,
  and the terminal sends them to the child too) and forwards SIGTERM and SIGHUP to the child.
  None of these cancels the exit handling below: the child's exit starts it.
- **After `claude` exits**, a new SIGINT, SIGTERM or SIGHUP cancels the exit handling at its
  next cancellation point, a lock wait. Its writes (the capture, the merge-back's splice and
  the reservation's unlink) are critical spans. Exit handling that is cancelled or fails prints
  a notice on stderr: its work is left to lazy capture and the next launch, which loses
  nothing, because the reservation dies with this process.
- **The exit code** is always the child's (`128 + signal` if the child was killed by a signal),
  whatever happened in exit handling.

**Environment.** These variables are scrubbed from the session environment, with a warning
naming each one that was set, because each supplies or redirects the login, or renames CC's
config file or Keychain item (Appendix A.1, A.7):
- `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, `CLAUDE_CODE_OAUTH_TOKEN`,
  `CLAUDE_CODE_OAUTH_REFRESH_TOKEN`, `CLAUDE_CODE_OAUTH_SCOPES`, `CLAUDE_CODE_OAUTH_CLIENT_ID`,
  and every `CLAUDE_CODE_*_FILE_DESCRIPTOR`;
- `CLAUDE_CODE_ACCOUNT_UUID`, `CLAUDE_CODE_USER_EMAIL`, `CLAUDE_CODE_ORGANIZATION_UUID`;
- `ANTHROPIC_PROFILE`, `ANTHROPIC_CONFIG_DIR`, `ANTHROPIC_FEDERATION_RULE_ID`,
  `ANTHROPIC_IDENTITY_TOKEN`, `ANTHROPIC_IDENTITY_TOKEN_FILE`;
- `CLAUDE_CODE_CUSTOM_OAUTH_URL`, `USE_LOCAL_OAUTH`, `USE_STAGING_OAUTH`;
- `CLAUDE_SECURESTORAGE_CONFIG_DIR`.

They are not scrubbed on the plain-`claude` paths. A pre-set `CLAUDE_CONFIG_DIR` is
overridden, with a warning. A login source the environment cannot express, such as a
workload-identity profile file, is caught by validation as `overridden` (§12.3).

Removing `CLAUDE_SECURESTORAGE_CONFIG_DIR` makes the secure-storage dir resolve to the profile
(Appendix A.1). Set to an empty string, it would send CC to the default `~/.claude` credentials;
set to anything else, it would redirect them. Every profile credential operation (bootstrap,
validation, capture, the session-owned usage read, and the hashed-item deletion in `remove`)
resolves paths with this same environment and the recorded spelling (§12.2).

**When the child exits**, under `MutationGuard`, then the account lock:
- If the profile is quiescent apart from this process's own reservation, this is the last
  session out:
  1. **Capture.** Adopt the profile's credential into the vault if its provenance (below) says
     it rotated, and it is the same identity. The comparison and write happen under the
     account lock (§6.2), with no network.
  2. **Merge back** `.claude.json` (§12.4).
- Otherwise another session still runs: another `run` of the account, or a background session
  that `claude` left behind (a `bg` or `daemon` session record, §12.6). Capture and merge-back
  then wait for the last of them, through lazy capture and the next launch.
- In either case, unlink this process's reservation last.

**Lazy capture.** If tagteam itself was killed, or a background session outlived the last
`run`, the same adoption runs once the profile is quiescent: at the next launch, the switch
pre-check (§9.2), or the refresh gate (§7.3 step 3), which a usage collection reaches whenever
the account needs a refresh. Each runs under the account lock. The conditions are the same:
quiescent, rotated according to its provenance, and the same identity. The unmerged baseline
is merged back at the next launch (step 2 above).

**Profile provenance.** `<profile>/.tagteam-seed.json` records two things:
- the login epoch the profile was bootstrapped under;
- the **seed**: the fingerprint of the generation the profile last agreed on with the vault.

Bootstrap writes both, and a capture moves the seed to the captured generation. A profile is
**stale-marked** when its recorded epoch differs from the account's `login_epoch`. Every
comparison between a profile's credential P and the vault's V is made against the seed S,
never by expiry, because a rotated token need not expire later than the one it replaced:

| State | Meaning | Outcome |
|---|---|---|
| P = V | In step | Nothing; the seed is set to V if it differs |
| P ≠ V, V = S | The profile rotated; the vault has not moved since they agreed | **Capture** P into the vault (unless stale-marked); the seed becomes P |
| P ≠ V, P = S | The vault moved on (a refresh, a switch capture, or a replacement); P is older and may be consumed | Never captured; the next launch re-bootstraps from V |
| P ≠ V, P ≠ S, V ≠ S, stale-marked | Both moved, and the vault's move was an explicit replacement | The replacement wins: never captured; the next launch displaces P and re-bootstraps |
| P ≠ V, P ≠ S, V ≠ S, not stale-marked | Both moved, in an unknown order | **Conflict.** Nothing is captured, refreshed or overwritten for the account: the gate returns `Conflict`, `run` and `switch` refuse, and `doctor` fails the check. An explicit replacement resolves it |

**Explicit replacements are recoverable.** `add` over an existing account, `add-token` and
`import` hold the account lock throughout, and:
1. in one store transaction, increment `login_epoch` and set `replacing_fp` to the new
   credential's fingerprint, with `replacing_meta` holding that login's metadata (identity,
   kind, expiry, and whether it was taken from the live login, as `add` takes it). When the
   live identity or `active_accounts` names the account, the same transaction records the
   default home's evidence (below);
2. write the vault;
3. in one store transaction, clear `replacing_fp` and `replacing_meta`.

The epoch moves first, so no profile, running or not, is ever captured over a replacement that
landed. The crash gap this ordering opens is closed by recovery. Any holder of the account lock
that finds `replacing_fp` set knows the replacer died, since the replacer held that lock
throughout, and reconciles before doing anything else:
- if the vault holds `replacing_fp`, the replacement landed: the recorded `replacing_meta` is
  installed onto the account (identity, kind, expiry; any quarantine is cleared with it), and
  the marker cleared. A replacement taken from the live login also records the account's
  `login_epoch` as the activation epoch, as `add` would have (§10.1);
- otherwise it never landed: `login_epoch` is decremented and the marker cleared. That restores
  the profile's eligibility, so a rotation it holds is captured rather than stranded;
- if the replacement landed but `replacing_meta` cannot be parsed, it cannot be installed:
  the holder refuses with `replacement-unreadable`, naming the account. `remove` and `purge`
  are the exceptions, since they delete the account either way, and `doctor` fails the check
  with `remove` as the fix (§13.6).

A running profile is never touched: it is simply stale-marked, and is re-bootstrapped at its
next quiescent launch, which writes the new epoch and seed.

**The default home's activation epoch.** The live store is held to the same rule. An `import`
or `add-token` that replaces the live account's login writes the vault only; CC keeps the old
login until `tagteam switch <N> --force` activates the new one (§13.3). Until then the live
store holds a lineage the replacement superseded, and adopting it back would undo the
replacement as surely as a stale profile's capture.
- `active_accounts` records the **activation epoch**: the account's `login_epoch` when tagteam
  made it the live login. A switch's commit (§9.4 step 9), recovery's forward finish (§9.6)
  and `add` (§10.1) write it.
- The live store is **stale-marked** when `active_accounts` names the account and its
  activation epoch differs from the account's `login_epoch`.
- **A replacement records its own evidence.** Its first transaction (step 1 above) sets
  `active_accounts` to the account, with the `login_epoch` it had before the increment,
  whenever the live identity or `active_accounts` names the account, unless `active_accounts`
  already names it with an epoch. So the live store is stale-marked from the moment the
  replacement begins, whoever activated it and whatever the row held before. `add`, whose new
  login is the live one, records the new epoch in its last transaction (§10.1).
- The migration that adds the column fills it with each named account's current
  `login_epoch`: the live store is taken as current, the best evidence there is.
- A stale-marked live store is never captured: the switch's outgoing capture and recovery's
  capture displace it instead (§9.4 step 4, §9.6), and the active-token refresh neither adopts
  nor refreshes it (§7.5 `Replaced`). CC goes on using and refreshing it, which is its own
  business.
- `tagteam switch <N> --force` re-activates the vault generation, as a forced self-switch
  does (§9.2), and its commit records the current epoch.

**Identity drift.** If the profile's `oauthAccount` email, or its org when both are set,
differs from the account's, the profile is ignored for that account.

### 12.6 Process liveness

Session records are read from `<profile>/sessions/*.json`. CC 2.1.286 writes one when a
session starts, before its first prompt, and removes it when it exits gracefully, SIGINT,
SIGTERM and SIGHUP included; SIGKILL leaves it behind. Its fields include `pid`, `procStart`,
`startedAt` and `kind` (Appendix A.7).

A pid is live if `kill(pid, 0)` succeeds or returns `EPERM`, **and** it still belongs to the
record's writer, as judged from `procStart`:

- **`ps lstart` text**, which CC 2.1.286 writes on macOS and Linux alike (`LC_ALL=C`, `TZ=UTC`,
  for example `Wed Oct  1 12:34:56 2026`): the process's start time from the OS
  (`proc_pidinfo(PROC_PIDTBSDINFO)` on macOS; the boot time plus `/proc/<pid>/stat` field 22,
  counted after the last `)`, on Linux) must equal it to the second, within ±1 s. Any other
  start time means the pid was recycled.
- **All digits**, as older CC versions wrote on Linux: `/proc/<pid>/stat` field 22 must equal
  it.
- **Absent or unparseable** (CC omits it when `ps` fails): cswap's rule applies. Get the start
  time and the arguments (`proc_pidinfo` and `sysctl(KERN_PROCARGS2)` on macOS,
  `/proc/<pid>/stat` and `cmdline` on Linux). The pid is treated as recycled **only if** the
  process started more than 120 s after the record's `startedAt` **and** neither its
  executable name nor its arguments contain `claude`.

Records of every `kind` count, `bg` and `daemon` included. CC's background daemon reads and
refreshes the profile's credential (Appendix A.7), so the account stays session-owned for as
long as it runs.

**The `switch_journal` holder** is recorded with its start time taken from the same source it
is later compared against (`/proc/<pid>/stat` field 22 on Linux, `proc_pidinfo` on macOS).
Liveness is then an exact match of pid and start time, with no heuristic. Launch reservations
need no pid at all: they are live while their file is locked (§12.5).

Anything that can't be determined counts as live. A malformed record (non-object JSON, a huge
pid, deep nesting, invalid UTF-8) counts as **unreadable**, and unreadable records block
destructive operations.

### 12.7 Mappings and the shell wrapper

- **`tagteam map [ACCOUNT] [PATH]`** maps a directory for the account's provider. `PATH`
  defaults to the current directory and is stored as its canonical absolute path. With no
  arguments, `map` lists the mappings. A directory can hold one mapping per provider.
- **`tagteam unmap [PATH] [--provider P]`** removes a mapping (all providers' mappings for that
  path if `--provider` is omitted).
- Subdirectories inherit the nearest mapped ancestor, per provider.
- **`tagteam shell-init zsh|bash|fish`** prints one wrapper function per registered provider
  that supports sessions (for now, `claude`). The wrapper always runs
  `tagteam run --provider <id> -- "$@"`, which decides from the mappings and `exec`s plain
  `claude` where none applies (§12.1), so an unmapped directory costs one `tagteam` start. If
  `tagteam` itself is not on `PATH`, the wrapper runs `command <launch_command> "$@"`. It's
  opt-in, added by the user to their shell rc.

### 12.8 Inside a run shell

`claude` under `tagteam run`, and everything it starts (its tools, hooks, MCP servers and its
statusline command), inherit `CLAUDE_CONFIG_DIR` set to the profile. tagteam commands run there
must not take the profile for the default home: the refresh gate would then see the default
login's account as inactive, and could refresh the very token CC in `~/.claude` is using.

**Detection.** A process is in a run shell exactly when `CLAUDE_CONFIG_DIR` names a directory
that holds a profile marker (§12.2). Nothing else is consulted: not the path's location, and not
`XDG_DATA_HOME`, which a run shell may have changed.

**The outer home.** In a run shell, tagteam resolves the provider's home from the marker's
`outer` record instead of the environment, and keeps the session's account apart:
- The live login, the gate's ownership check (§7.3 step 2), the active-token refresh and usage
  collection all see the default home, exactly as outside the shell. The session's own
  account is session-owned through its reservation.
- Commands that change accounts or the live login refuse (§9.2), and so does `auto` except with
  `--dry-run` (§11.1). Read-only commands work. `status` adds the session's account, and
  `list` marks it (§13.1, §13.2).
- `run` launches as it would from the outer home (§12.1).
- `statusline` takes the account from the marker (§13.5).

**An unreadable marker** (present, but not a valid marker) leaves the outer home unknown. Every
command except `statusline` then refuses, naming the file; `statusline` prints nothing.

## 13. CLI

### 13.1 Commands

`tagteam [--json] [--debug] [--no-color] [--provider P | -p P] <command>`. A bare `tagteam` runs
`list`, until the TUI lands in sub-project 3.

**Which provider a command acts on:**
1. an explicit `--provider`
2. the provider of the account the command references, when the reference is an alias or an
   email that is unique across providers
3. `default_provider` (`claude-code`)

A bare position (`switch 2`) always means that position within the provider chosen by rule 1
or 3. `list`, `status` and `doctor` cover every provider that has accounts, grouped by
provider, unless `--provider` narrows them. With a single provider in use, the output looks
exactly as it would without providers.

| Command | Notes |
|---|---|
| `list` / `ls` | Every account with its 5h, 7d, spend and scoped usage, reset countdowns, markers (active, disabled, quarantined, in a session, ahead of pace) and data age. It fetches only when due (§8.3) |
| `status` | The live account |
| `switch [ACCOUNT] [--strategy best\|next-available [--model M]] [--force]` | §9 |
| `add`, `add-token`, `remove` / `rm`, `disable`, `enable`, `alias`, `move` | §10 |
| `auto` | §11 |
| `run`, `map`, `unmap`, `shell-init` | §12 |
| `history [ACCOUNT] [--window 5h\|7d\|spend\|<model>] [--since 14d] [--csv]` | §13.4 |
| `statusline` | §13.5 |
| `export`, `import` | §13.3 |
| `displaced [--purge ID... [--yes]]` | §6.3 |
| `config list\|get\|set\|unset\|path` | §6.4 |
| `doctor [--online]` | §13.6 |
| `purge [--yes]` | Deletes tagteam's data, including the vault Keychain items and the profiles' hashed items, after a confirmation (or `--yes`). It never deletes or replaces any provider's live login. `--provider` limits it to one provider's accounts (§10.5) |
| `completions bash\|zsh\|fish` | §13.7 |

**Exit codes:** `0` OK · `1` error · `2` usage error · `130` interrupted by SIGINT (128 + the
signal number for SIGTERM and SIGHUP, §14.1). `auto --once` uses 0–3.

**`list` text layout.** One row per account, in position order:

```
    #  ACCOUNT                  5H           7D                    SPEND         FABLE   AGE
 *  1  michael@example.com       9%  2h40m   77%  3d09h  ▲ pace    €0 of €20      0%     2m
    2  spare@example.com        31%  4h02m   12%  5d01h            —              —     14m
    3  work (w@corp.com)        relogin required
```

- A window shows its `pct` and the countdown to its reset. `▲ pace` marks a window that is
  ahead of pace (§8.7).
- A scoped window gets a column, headed by its model name, only when some account has it.
- `pct` is coloured by §13.5's severities, off under `NO_COLOR` and `--no-color`.
- A row without usage shows its `usageStatus` in words (`relogin required`, `api key`,
  `unavailable (http-429, retry 4m)`, `over budget`, …) instead of the window columns.
- `AGE` is the last good reading's age. A stale reading is still shown, with its age.
- A session-owned account (§12.5) is marked `▶` after its position, and in a run shell the
  session's own account `▶ this`.

### 13.2 JSON output (`schemaVersion: 1`)

**Compatibility.** The field names are cswap-compatible so existing scripts port unchanged. Three
fields are added: `id`, `position` (`number` equals `position`) and `provider`.

**Several providers.** Every row, `status` object, `switch` result and auto event carries
`provider`. At the top level, `activeAccountNumber` refers to `default_provider` (for cswap
compatibility), and the additive `activeByProvider: {"<provider>": <number>|null}` covers all
of them. `status --json` with more than one provider returns the default provider's object plus
an additive `others: [ … ]` array.

**Rules:**
- The output is one JSON object on stdout; warnings and notices go to stderr.
- Errors produce `{"schemaVersion":1,"error":{"type":<Kind>,"message":…}}` with a non-zero exit.
  `type` is a stable kind (§14). `keychain-locked` means the login keychain is locked and was
  not unlocked (Appendix A.3).

**`list`** returns `{schemaVersion, activeAccountNumber, accounts:[row], displacedCredentials?}`.

Each row:
- **Always:** `number, position, id, email, organizationName, organizationUuid, isOrganization,
  active, usageStatus, usage, alias?, disabled?: true, loginExpiresAt?, inSession?: true`.
  `inSession` (additive) marks a session-owned account (§12.5).
- **When `usage` is non-null:** `usageFetchedAt, usageAgeSeconds`.
- **When `usage` is null:** `lastGoodUsage, lastGoodFetchedAt, lastGoodAgeSeconds`.
- **When `usageStatus` is not `ok`** (whether `usage` is null or not): `usageError` (the
  error kind, or null) and `usageRetryAt` (when the account will next be fetched, or null
  for a status that is never retried). The human row words add `retry <countdown>` when a
  retry is scheduled.

`usageStatus` is one of `ok | token_expired | api_key | keychain_unavailable | relogin_required
| foreign_credential | no_credentials | unavailable | unsupported`. `unsupported` is for providers
without the `usage` capability; it never occurs for Claude Code.

`usage` is decision-grade only (§8.4), and is rendered by the account's provider from its generic
windows. Claude Code renders cswap's shape:
- `fiveHour {pct, resetsAt?}`
- `sevenDay {pct, resetsAt?, expectedPct?, aheadOfPace?, projectedExhaustionAt?,
  willLastToReset?, projectionMethod?}`
- `spend {used, limit, pct, currency, resetsAt?}`
- `scoped [{name, …same fields as sevenDay}]`

**`status`** returns one of:
- `{schemaVersion, provider, active: null}`
- `{schemaVersion, provider, active: {email, provider, managed: false}}`
- `{schemaVersion, provider, active: {number, position, id, email, …row fields, managed: true}, totalManagedAccounts}`

In a run shell (§12.8) each shape gains an additive `session: {number, position, id, email}`
naming the session's account, or `session: null` when the marker's account is not managed.

An account command's result is `{schemaVersion, ok, account: row, created?}`; `remove`'s reports
`active` as it was before the removal.

**`switch`** returns `{schemaVersion, provider, switched, from, to, strategy, reason, message,
credentialStore, warnings}`.
- `strategy` is `rotation | best | next-available | direct`.
- `reason` is `switched | already-active | activated | unmanaged-account | only-one-account |
  usage-unavailable | already-best | candidates-exhausted | no-valid-target | session-owned |
  interrupted-switch | profile-conflict`. The last three are additive to cswap's set.
- `credentialStore` (additive) is where this switch's write stored the credential, as the
  provider reported it: `keychain`; `file`, either the platform's only store (Linux) or a
  fallback after the keychain refused the write (Appendix A.3); or `null` when it wrote none.
  A fallback is also a warning on stderr that names the file.

`history`'s `--json` shape is documented in `--help`. The shapes of `config` (§6.4),
`displaced` (§6.3), `purge` (§10.5), `export` and `import` (§13.3) and `doctor` (§13.6) are
given with each command. All of them are snapshot tested.

### 13.3 Export and import

**An OAuth export hands a login over; it does not copy it.** A refresh token is single-use
(§7.3), so whichever machine refreshes an exported account first invalidates every other copy
of that generation: the other machine's next refresh gets `invalid_grant` and quarantines the
account there (§7.4). Export is for moving accounts to another machine, or for a short-lived
backup; to use one account on two machines, log in on each. Setup tokens and API keys do not
rotate, so their copies keep working. Whenever the file holds an OAuth account, export's human
output says so.

**Command:** `tagteam export [FILE|-] [--account A]... [--full] [--recipient R]...
[--recipient-file F]... [--plaintext]`

**Encryption is on by default.** The output is **age**, ASCII-armored.
- **Passphrase.** Prompted twice on a terminal, derived with scrypt.
- **Recipients.** `--recipient` takes `age1…` or `ssh-ed25519 …` keys; `--recipient-file`
  reads them from a file.
- **No terminal and no recipient:** error, suggesting `--recipient` or `--plaintext`.
- **`--plaintext`** is the explicit opt-out.

**Writing.**
- The output file is created 0600 with `O_EXCL` in the destination directory and then renamed
  into place.
- A directory destination is rejected.
- `-` writes to stdout. With `--json` that is a usage error (exit 2), since stdout carries the
  export.
- The passphrase is asked for, and recipients are parsed, before any lock is taken.
  Encryption and the write run after every lock has been released.

**Which generation is exported.** Each account's newest generation, read where its lineage
advances, so the file never carries a generation this machine has already consumed:
- **The live login** (the account the live identity names): the live credential, read fresh
  under the provider's credential locks, so a refresh CC has in flight completes first. The
  generation exported is the one §7.5 step 3 would settle on among the live credential, the
  vault, its `.prev` and a rescue, worked out without writing anything. When the live
  credential is the vault's generation or its `.prev`, that is the vault's generation, or the
  pending rescue that succeeds it. When the live credential is a rescue's generation, it is
  that rescue's. Any other full token pair is CC's rotation, and is exported itself. A
  stale-marked live store (§12.5) exports the vault's generation instead: the replacement
  wins.
- **A session-owned account** (§12.5): the profile's credential, read as CC reads it (§8.1),
  fresh, under the profile's own credential locks. The provenance table (§12.5) decides
  between it and the vault, again without writing: the profile's generation when it rotated
  since its seed (P ≠ V, V = S); the vault's when they agree, when the vault moved on, or when
  the profile is stale-marked. A profile whose identity drifted is ignored, as everywhere
  (§12.5), and the vault's generation is exported.
- **Any other account:** the vault's generation, after the work every holder of its account
  lock does first: reconcile a pending replacement (§12.5), settle pending rescues (§6.2), and
  capture a quiescent profile that rotated (lazy capture, §12.5).

Export takes `MutationGuard`, then one account lock at a time in ascending ID order, and
releases both between accounts. Ownership cannot change while they are held (§12.5). An
interrupted switch is recovered first, as by any holder of `MutationGuard` (§9.6). Export
sends no request: a live credential that is not its account's own is caught where it is used,
by the token endpoint's identity check (§7.4). It works inside a run shell, where the live
login is the default home's (§12.8).

**In use here.** The live login and session-owned accounts are exported, and the output names
them with a warning: this machine goes on refreshing them, so their exported copies stop
working the next time it does.

**Broken accounts.** An account is broken when its exportable generation cannot be determined,
or is known to be dead:
- it has no vault credential or no `identity_json`;
- it is quarantined (§7.4);
- a read it needs is `Unreadable` or `Degraded`, or its rescues are unreadable (§6.3);
- its profile reports a provenance conflict (§12.5);
- its live or profile copy is the one to export but was wiped by CC (§9.4 step 4's `Wiped`),
  or lacks the refresh token the vault has;
- an interrupted switch that recovery cannot decide names it (§9.6).

When exporting all accounts, a broken one is skipped with a warning that names its position and
the reason. With an explicit `--account`, a broken account is a hard error, and nothing is
written.

**Envelope** (inside the encryption):

```json
{ "format": "tagteam-export", "version": 1, "exportedAt": "…Z", "exportedFrom": "macos|linux",
  "tagteamVersion": "…", "active": { "claude-code": 2 },
  "accounts": [ { "provider": "claude-code", "position": 2, "kind": "oauth", "label": "…",
                  "alias": "dev", "disabled": false, "addedAt": "…Z",
                  "identity":   { "email": "…", "accountUuid": "…", "organizationUuid": "",
                                  "organizationName": null, "oauthAccount": { … } },
                  "credential": { "claudeAiOauth": { … } } } ] }
```

- **Provider-owned payload.** The engine owns `provider`, `position`, `kind`, `label`, `alias`,
  `disabled` and `addedAt`. `identity` and `credential` are written and validated by the
  provider (`export_login` / `import_login`), so a new provider needs no new format version.
- **Default contents are slim.** For Claude Code the credential is reduced to
  `{claudeAiOauth}`: the machine-shared keys and the device-bound `trustedDeviceToken` stay on
  the source machine. `--full` keeps everything.
- **Nothing machine-local is exported:** no account ID, login epoch, activation epoch,
  quarantine, usage state or `rejected_fp`. `active` gives the live login's position per
  provider, for information only; import ignores it.

`--json` returns `{schemaVersion, ok, file, encrypted, accounts: [{provider, number, email,
source, inUse}], skipped: [{provider, number, email, reason}]}`, where `source` is `vault`,
`live` or `profile`.

**Command:** `tagteam import <FILE|-> [--force] [--identity F]...`

The format is detected automatically: armored or binary age (prompting for a passphrase, or
using `--identity F`), tagteam plaintext, or a **cswap v1 export** (`version: 1`,
`swapVersion`, `encrypted: false`, `accounts[].{number, credentials, config.oauthAccount}`).
cswap accounts import as provider `claude-code`. A tagteam account whose `provider` is not
registered in this build is refused in pass 1, naming the provider. With `-`, the file comes
from stdin and the passphrase from the terminal.

Import replaces logins, so it is an account-changing command: it refuses inside a run shell
(§12.8), and while an interrupted switch for a provider in the file cannot be decided (§9.6).
On macOS it runs the Keychain lock check first (Appendix A.3).

1. **Pass 1 validates everything** before writing anything:
   - the provider's identity validation (CC: the email regex) and integer positions ≥ 1 (a
     path-traversal defence)
   - field types, the provider's credential kinds, and alias rules
   - no duplicate identity keys within a provider, and no duplicate aliases
   - an alias owned locally by a different identity is dropped
   - an account ID is never read from the file: every created account gets a new one (§6.1)
2. **Pass 2 writes,** one account at a time, under `MutationGuard` and the account's lock:
   - **An existing identity** is skipped unless `--force` is given, or it is quarantined (it is
     then replaced automatically, with the cleared strike reported). It is replaced as an
     explicit replacement (§12.5 steps 1–3): its login epoch moves first, and the evidence is
     recorded from the live identity as `add-token` records it, so neither a profile nor the
     live store can capture the old lineage back over it. It keeps its local position, alias
     and `disabled` flag.
   - **A new identity** is created as `add` creates one, the store row before the vault entry
     (§10.1), at its exported position if that is free, else at the provider's next position
     (§6.1). Its alias and `disabled` flag come from the file.
   - Quarantines are cleared, each with its `unquarantine` event (§7.4).
   - The store's active account is never set from the file. It records what tagteam made live
     (§12.5), and an import makes nothing live.
   - If the live login's account was replaced, tagteam tells the user to run
     `tagteam switch N --force`: CC keeps the old login until then (§12.5).
   - If a session-owned account was replaced, it says that the session keeps its login until
     it exits, and that the next `run` starts with the new one (§12.5).
   - A failure on one account is reported, and the others go on. The command exits 1 if any
     account failed.

`--json` returns `{schemaVersion, ok, accounts: [{provider, number, email, outcome, message}],
warnings}`, where `outcome` is `created`, `replaced`, `skipped` or `failed`.

### 13.4 History

`tagteam history` renders per-window sparklines, the current burn rate (points per hour), and
an ETA with its projection method. `--csv` and `--json` dump the raw samples.

- **Defaults:** the live account, its relevant windows (§8.2), and `--since 7d`.
- For each window it also says whether the window will last to its reset (§8.7).
- It reads `usage_samples` only and never fetches.

### 13.5 Statusline

`tagteam statusline` is built for CC's `statusLine` command and meant to be fast. It is a
provider capability: the command resolves the provider from `--provider`, else from the
environment it runs in (a `CLAUDE_CONFIG_DIR` or a CC-invoked process means Claude Code), else
`default_provider`. It refuses for a provider without the capability.

- It drains piped stdin (up to 64 KiB) and ignores the contents.
- It does **no network and no Keychain access**, and never constructs the `Http` adapter: the
  engine builds it lazily, on first use, which keeps the command within §1.1's 10 ms p95.
- **Which account.** In a run shell (§12.8), the session's account, named by the profile
  marker; CC runs its statusline command with the session's environment, so the command sees
  `CLAUDE_CONFIG_DIR` (Appendix A.7). The marker is one small file, and the profile's
  `.claude.json` is never parsed. A marker whose account is not managed prints nothing.
  Otherwise, the live identity from `live_identity_cache`, re-parsing `~/.claude.json` only
  when its mtime or size changed. The parse deserializes `oauthAccount` alone and skips the
  rest.
- It reads `usage_state` and prints one line using `statusline.format`.

**Placeholders:** `{account}` (the alias, else the email local part), `{position}`, `{email}`,
`{5h}`, `{7d}`, `{5h_reset}`, `{7d_reset}`, `{spend}`, `{model:<name>}`, `{stale}` (` · 12m
old` when the data is older than 15 min, else empty).

- ANSI severity colours: ≥ 90% critical, ≥ 70% warning. They are off under `NO_COLOR`.
- **Unmanaged live login:** print the email alone. **No live login:** print nothing.
- **Setup.** `tagteam statusline --print-config` prints the `settings.json` snippet for the user
  to paste. tagteam never edits CC's settings (§3).

### 13.6 Doctor

`tagteam doctor [--online] [--json]` checks tagteam's own state and its interop with each
provider.

**Read-only.** Doctor writes nothing and creates nothing.
- It takes no `MutationGuard`, so it reports an interrupted switch rather than recovering it
  (§9.6).
- It opens the store read-only and never migrates it. With no data directory, it reports that
  tagteam has no state yet.
- It tests locks without waiting, and takes none that another process could wait on.
- It asks nothing. A locked Keychain is reported (Appendix A.3), never unlocked, and the
  checks that would read it are skipped with one `warn`. (Whether the lock check itself can
  raise the system's unlock dialog is §17 O3.)
- Every check that finds a problem names the fix. Doctor applies none itself.

**Result.** Each check reports `ok`, `info`, `warn` or `fail`. A check whose input cannot be
read reports `warn` and why, never `ok`. Doctor exits 1 if any check fails, else 0. Checks are
grouped per provider, as `list` groups accounts (§13.1).

JSON: `{schemaVersion, ok, checks: [{id, provider, status, message, fix}]}`. `id` is a stable
dotted name (`store.integrity`, `accounts.quarantined`, `cc.version`, …). `provider` is null
for a check that is not a provider's, and `fix` is null when there is nothing to do.

**Engine checks:**
- **Store:**
  - `PRAGMA quick_check` on `tagteam.db` → fail on error
  - the schema version: newer than this binary → fail; older → info, since the next command
    migrates it
  - modes: the store not 0600, a tagteam directory not 0700, a Linux vault file not 0600 →
    warn, naming the `chmod`
  - a temp file the atomic writer left behind (§9.5), in tagteam's directories or beside a CC
    credential file, whose writer's pid is no longer live → warn: it may hold a secret, and is
    safe to delete
- **Accounts:**
  - each account's vault entry is readable → fail if not; an account with no vault entry,
    as a purge that stopped part-way leaves one (§10.5), names `tagteam remove` as the fix
  - no orphaned vault items: on Linux, a `vault/` file naming no account; on macOS, any item
    of service `tagteam` while the store has no account (`find-generic-password -s tagteam`,
    attributes only, Appendix A.3) → warn, with `tagteam purge` as the fix
  - quarantined accounts → warn, naming the reason, and the fix: log in, then `tagteam add`
  - `login_expires_at` within 7 days → warn
  - a pending replacement (§12.5) → warn; an unparseable `replacing_meta` → fail, with
    `tagteam remove` as the fix
  - the live store stale-marked (§12.5) → warn: CC still runs the login an explicit command
    replaced. The fix is `tagteam switch N --force`
- **Usage:**
  - an account in backoff → info, with its `last_error` and when it is retried
  - an identity at its hourly budget (§8.6) → warn
  - a reading, poll plan or backoff stamped further ahead than §8.4 allows → warn: the clock
    is, or was, skewed
- **Pending storage:**
  - pending `rescue/` entries → warn; a `rescue` path that is not a listable directory (§6.3)
    → fail; a rescue file whose account is gone → warn, with `tagteam purge` or deleting it
    as the fix
  - `displaced/` entries → info; a displaced file with no row, or a row with no file → info
- **Interrupted switch:** a `switch_journal` row whose holder is dead → warn, or fail if §9.6
  cannot decide it. A holder that reads live but may not be (an `EPERM` pid, which can be
  another user's recycled pid) → warn, naming the pid and its start time, with
  `tagteam switch --force` as the fix if no tagteam process is running.
- **Auto-switch** (§11):
  - an engine running → info, with its pid, from the engine lock's record (§11.1)
  - `unhealthy_ticks` above zero → warn
  - a `consume-first` strategy for a provider without a long window (§4.5) → warn
  - `autoswitch.models` naming a model that no account's last reading has → warn, as the
    tick's `config-warning` does
- **Settings** (§6.4): an unparseable file → fail, since every command runs on the defaults;
  an invalid value or an unknown key → warn, naming it.
- **Log** (§14.2): its path and size → info; a log that cannot be written → warn.
- **Session profiles:**
  - a real `projects/` or `history.jsonl` inside a profile (§12.2) → fail; another shared file
    split into a regular file → warn
  - a profile marker that is unreadable, or whose recorded spelling is no longer the
    profile's canonical path (§12.2) → warn; a profile directory without a store account →
    warn, naming what to delete
  - reservations whose `tagteam` parent is gone but whose lock is still held, with the
    holding processes where the OS can tell; baselines awaiting merge-back → info
  - a provenance conflict (§12.5) → fail, naming the account and the fix
  - a profile that is stale-marked, or whose seed records that it needs a bootstrap (§12.3)
    → info: its next launch bootstraps it
  - a profile credential that cannot be read → warn: the account cannot switch or launch
    until it can

**Claude Code checks** (`doctor_checks`, §4.5):
- the `claude` binary was found, and its version compared against the tested version in
  `crates/tagteam-cc/compat/tested-cc-version` (§15.4): newer → warn
- resolved paths: config home, global config, secure-storage dir, and Keychain service and
  account names
- **Keychain:**
  - reachable, locked or unknown, by the lock check (Appendix A.3)
  - whether the account-name rule falls back to `claude-code-user`
  - the managed-key item present but empty → fail: every switch refuses on it. The fix names
    the `security delete-generic-password` command that removes it
  - an item under a former fallback name of the home doctor runs in (Appendix A.2): the
    unsuffixed item when `CLAUDE_CONFIG_DIR` names the default home, or the item named from a
    symlinked config dir's target → info, naming the command that deletes it
- **Environment.** The variables doctor runs with, which a `claude` started from the same
  shell inherits:
  - `CLAUDE_CONFIG_DIR` set but empty, or set to a spelling of the default home: CC 2.1.286
    then reads a hashed Keychain item, not the one a switch writes (Appendix A.2) → warn
  - a non-production OAuth switch (Appendix A.1) → warn
  - any variable that `run` scrubs because it supplies or redirects a login (§12.5) → warn,
    naming it
- **The default home's login,** by `claude auth status` (Appendix A.7) with a 10 s timeout, in
  the outer home's environment inside a run shell (§12.8):
  - logged in by a method other than its stored `claude.ai` login (`api_key_helper`,
    `oauth_token`, …) → warn: a switch changes nothing for `claude` started here
  - logged in as an email or org other than the live identity's → warn
- **Locks:** CC lock directories present and older than their staleness window → warn.
- **Unknown entries:** entries in `~/.claude` that are on neither the allowlist nor the
  known-private list, and so are not shared (§12.2) → warn.

**Online** (`--online`): TLS reachability of the token, profile and usage hosts, with no
credentials sent.

### 13.7 Completions

`tagteam completions bash|zsh|fish` prints a completion script for that shell on stdout,
generated by `clap_complete` from the command definitions, for the user or a package manager
to install. The script is static: commands, flags, provider IDs, settings keys (§6.4) and the
other fixed values complete. Account references do not, since completing them would read the
store. The command needs no store and no Keychain.

## 14. Errors and logging

- **Library crates** use `thiserror` enums: `CcError`, `VaultError`, `StoreError`, `NetError`,
  `SwitchError`, and so on. Each variant has a stable `kind()` used as the JSON `error.type`.
- **The binary** maps them to exit codes and human messages that give the next action (for
  example, "log in with that account and run `tagteam add --position 3`").
- **Panics** are bugs. Lock guards release in `Drop`, and the switch transaction's rollback
  also runs from `Drop` if the transaction didn't commit. The signals a user sends are caught
  and unwind cleanly (§14.1), but a process killed by SIGKILL runs no `Drop`; the switch
  journal (§9.6) and launch reservations (§12.5) cover that case.
- **After a vault write advances an account, nothing may fail upward.** Any follow-up, such
  as replanning a poll, is contained: on failure it logs at ERROR. Profiles need no follow-up:
  the login epoch (bumped before the write) and the launch-time generation check (§12.5)
  cover them.
- **stdout is reserved for command output.** Refresh-persist warnings and similar notices go to
  stderr, so `--json` output stays a single object.
- **Contained errors are logged, never discarded.** An error a command deliberately contains
  (a cleanup that fails, a best-effort store write, a follow-up after a vault write) is logged
  at WARN with its cause (§14.2). A timeout and a failure to spawn a process are different
  causes, and are reported as such.

### 14.1 Signals and cancellation

SIGINT, SIGTERM and SIGHUP never stop a command at the instruction they arrive on. The CLI's
handler only records the signal in the engine's cancel token (§4.2), which tests set directly.

- **Cancellation points.** The work checks the token, and unwinds with an `interrupted`
  error, only where stopping loses nothing:
  - each iteration of a lock wait: the mutation lock, account locks, and the provider's live
    locks, CC's storage-write lock included;
  - before reserving or sending a usage request;
  - at a prompt;
  - in `auto`'s sleep, and between its ticks.

  Unwinding runs every `Drop`. Locks are released, so CC's lock directories are removed, temp
  files are deleted, and a switch that has not journaled has written nothing. An interrupted
  usage fetch is not a usage failure: it records nothing and gives its slot back (§8.3).
- **Critical spans run to completion.** No cancellation point lies inside:
  - the switch transaction, from its journal row to its commit or rollback (§9.4 steps 6–10);
  - recovery's writes (§9.6);
  - the refresh gate (§7.3) and active-token refresh (§7.5), from sending the token request
    to persisting the successor;
  - an explicit replacement's three steps (§12.5);
  - any vault, rescue or atomic write.

  These spans are bounded by their request timeouts and local I/O. A signal that arrives
  inside one, or a repeated signal, takes effect at the next cancellation point. A command
  that reaches none, because its work finished first, reports what it did, with its normal
  output and exit code, plus a stderr notice that the signal came too late to stop it. A
  switch that committed is reported as switched, never as interrupted. SIGKILL cannot be
  caught; the journal (§9.6) and launch reservations (§12.5) cover it.
- **Child processes.** Non-interactive children (`/usr/bin/security`) run in their own process
  group, so a Ctrl-C at the terminal reaches tagteam alone and never kills a Keychain write
  midway. Interactive children (`security unlock-keychain`, and `claude` under `run`) stay in
  the terminal's foreground group.
- **Prompts.** A Ctrl-C at a prompt, including a no-echo secret prompt, restores the terminal
  and counts as an interruption.
- **Exit.** An interrupted command exits 130 after SIGINT, and 128 + the signal number after
  SIGTERM or SIGHUP. With `--json`, it prints the error envelope with type `interrupted`.
  `auto`'s loop stops cleanly and exits 0 instead (§11.4).
- **`tagteam run`** handles signals as §12.5 says: before the spawn, while `claude` runs, and
  in its exit handling.

### 14.2 Logging

**The file** is `$XDG_STATE_HOME/tagteam/tagteam.log` (§5), mode 0600 in a 0700 directory. It
is opened on the first event that passes its filter, so a command that logs nothing never
opens it, and `statusline` stays within §1.1's budget. When it exceeds 1 MiB it is rotated:
`.1` becomes `.2`, the file becomes `.1`, and the oldest is dropped.

**Levels.**
- The file records INFO and above by default.
- `--debug` records DEBUG and above, in the file and on stderr.
- `TAGTEAM_LOG` replaces the file's filter with an `EnvFilter` directive (`debug`,
  `tagteam_engine=trace`, …), and `off` disables the file. An invalid directive keeps the
  default, with a warning on stderr.
- Otherwise stderr shows ERROR events only. The notices and warnings a command prints for its
  user (§14) are separate from the log.

**What INFO records:** the state changes and decisions a user may later need to reconstruct.
These are switches and their rollbacks, recovery, quarantines and their clearing, refresh
outcomes, captures, replacements, rescues and displacements, launches and their exit handling,
auto-switch decisions, imports, exports (the accounts by position, never the contents), purges
and settings writes. Routine reads, each usage fetch, and everything `statusline` does log at
DEBUG at most.

**One line per event:** a UTC timestamp to the millisecond, the pid, the level, the module, the
message, then `key=value` fields. An account is named as `account=<id> position=<n>`, the only
spelling. A path under the home directory is written `~/…`.

**Never logged,** at any level: an email, label or organization name; a token, key or
credential, or any part of one; a passphrase; an export's contents; a request's
`Authorization` header or body. A fingerprint may appear as its first 12 hex digits.

**Several processes** write the same file. Each event is one `write` on a descriptor opened
with `O_APPEND`, so lines never interleave. The process that finds the file over 1 MiB rotates
it while holding a try-only `flock` on `tagteam.log.lock`, re-checking the size under that
lock; a process that cannot take the lock does not rotate. Before each write, a process checks
that the path's inode still matches its descriptor, and reopens the path when it does not. A
line written just after another process rotated lands in `.1`, which is kept. Nothing waits on
the log, so the log is best-effort in one case: a process paused between that check and its
write, across two rotations, writes its line to a file that is already gone.

**Logging never fails a command.** A file that cannot be opened, written or rotated disables
file logging for the rest of the process, silently unless `--debug`, and `doctor` reports it
(§13.6). A panic is logged at ERROR, with its location, before the process unwinds.

## 15. Testing

### 15.1 Isolation

- **All paths derive from an injected `Env`:** HOME, `XDG_*`, USER, `CLAUDE_CONFIG_DIR` (raw
  string), and `CLAUDE_SECURESTORAGE_CONFIG_DIR` (defined vs. undefined). In a run shell the
  provider's home comes from the profile marker instead (§12.8), which tests write into the
  fixture.
- **A harness guard** panics if any resolved path falls under the real HOME. There is also a
  test asserting the guard trips.
- **The Keychain** is an in-memory fake by default. Tests marked `real_keychain` run the real
  `/usr/bin/security` against a temporary keychain made with `security create-keychain`; they
  run on macOS CI and never touch the login keychain.
- **HTTP** never leaves the machine in tests.
  - Engine logic runs against a scripted in-process `Http` fake.
  - The `ureq` adapter, and every cross-process test, run against a small `std::net` mock
    server. It scripts 200, 401, 429 with `Retry-After`, `invalid_grant`, `invalid_client`,
    timeouts, resets partway through a response, and malformed bodies, and it keeps one
    request log that every process shares.
  - Response bodies are recorded from the real endpoints and redacted before they are
    committed: tokens, emails, uuids and organization names are replaced.
- **The clock** is injected everywhere.

### 15.2 Layers

- **`tagteam-core`:** unit and property tests. Poll planning (including the post-jitter floor),
  backoff (including the clamps and `Infinity`), trust, pace and regression, classification,
  and `decide()` with the §11.5 simulations.
- **`tagteam-cc`:**
  - service and account naming vectors, including NFC normalization, trailing slashes, the
    username regex fallback, and managed-key hashing
  - `security` argv and stdin shapes, including the 4032-byte argv fallback, hex decoding, and
    rc 44/36
  - the `proper-lockfile` protocol, including staleness, touching, the legacy-contention
    release, cleanup on panic, and compromise: a holder suspended past staleness resumes, sees
    the takeover, writes nothing and removes nothing
  - the splice against a corpus of `.claude.json` shapes (large files, unicode, numbers like
    `1e400` and `0.1000`, CRLF, nested `oauthAccount`, missing key, torn file)
- **`tagteam-engine`:** fixture-home integration tests for every command.
  - **Crash injection** at each switch step verifies the rollback. A separate run **kills the
    process** (SIGKILL) at each step and verifies §9.6 recovery. It covers a CC rotation, and a
    CC logout, between the kill and the recovery, and switches between two OAuth accounts,
    two setup-token accounts, two API-key accounts, and across kinds.
  - **Active-refresh reconciliation** (§7.5) covers every row of its table, including a CC
    rotation after a rescue was published, and a 401 on a locally valid token.
  - **Login epoch:** an `import` or `add-token` over an account with a quiescent profile that
    holds a later-expiring login is never undone by any capture path. The replacement is also
    killed at each of its three steps: a rotation held by the profile is captured afterwards,
    never stranded.
  - **Provenance:** every row of the §12.5 table, including capture and relaunch with a
    rotated refresh token whose `expiresAt` does not increase. On macOS with the real
    `security` driver, the full sequence (session exit → inactive vault refresh → relaunch →
    capture) ends with the vault's generation effective in the profile, and never captures
    the consumed one.
  - **Concurrency tests** run several engines, in separate processes, against one store and
    home: double-switch prevention, lease fencing, refresh single-flight with a holder stopped
    (SIGSTOP) past any timeout, every vault writer racing the refresh gate, launch and exit
    racing `remove` / `switch` / the gate, and two overlapping sessions of one account. Two
    `auto` engines for one provider: the second skips it, or reports `engine-running`; an
    auto-switch and a manual switch racing: the auto switch reports `live-changed`.
  - **Cancellation** (§14.1): the token set during each lock wait ends the wait promptly,
    leaves no CC lock directory behind, and exits 130 with nothing written. Set at each
    switch crash point inside the critical span, it changes nothing about the switch: it
    commits (reported as switched, exit 0, with the stderr notice), or rolls back on an
    injected failure exactly as without the signal; either way no journal row is left. A real
    SIGINT to the CLI during a switch's critical span ends the same way. `security` children run outside the terminal's
    process group, and a secret prompt interrupted with Ctrl-C restores the terminal.
  - **The auto loop** under an injected wall clock that jumps forward (a suspend) ticks once,
    on waking, and re-reads changed settings before its next tick.
  - **Budget:** across any interleaving of processes and on-demand callers, no account is sent
    more than 20 usage requests in a rolling hour.
  - **Fresh home:** a `run` against a home with no `projects/` or `history.jsonl` leaves both
    shared.
  - **Run shell** (§12.8): with `CLAUDE_CONFIG_DIR` set to a profile, the gate never refreshes
    the default home's live account, `list` and `status` show the default login as active and
    the session's account as in session, account-changing commands refuse, and an unreadable
    marker refuses everything but `statusline`. Detection holds with `XDG_DATA_HOME` changed
    inside the shell, and a directory outside `sessions/` holding a marker is a run shell.
  - **Sharing** (§12.2): only allowlisted entries are linked, an unknown entry is noted once
    and stays private, a link whose entry left the allowlist is removed, a split shared file
    warns (and refuses for a must-share entry), and a joining launch only creates links.
  - **Bootstrap** (§12.3): the composed credential carries the vault's account-scoped keys and
    the profile's own machine-shared keys, none for a new profile; a changed spelling deletes
    the old item first; a bootstrap stopped between writing the file and deleting the item is
    redone with the same machine-shared keys; each validation outcome acts as its row says, and
    only `invalid` at a bootstrap deletes a profile; an `apiKeyHelper` added to the shared
    settings after a profile's first launch makes the next launch refuse as `overridden`.
  - **Replacement evidence** (§12.5): an `import` over the live account stale-marks the live
    store whatever `active_accounts` held before, including a row that named another account
    while the live identity named this one, and an `add` that dies part-way is reconciled with
    its activation epoch recorded.
  - **Exit paths** under §14.1: a signal during each launch lock wait, or during the login
    check, launches nothing and leaves no reservation; a SIGINT or SIGTERM recorded just
    before the spawn reaches `claude` once it starts; SIGTERM and SIGHUP while `claude` runs reach the child and exit
    handling still runs; a signal during exit handling defers it, and the next launch
    completes the capture and merge-back. The exit code is the child's in every case.
  - **Liveness** (§12.6): `lstart`, all-digit and absent `procStart` records, a recycled pid
    for each, and a `daemon` record keeping the account session-owned after the last `run`.
  - **Activation epoch** (§12.5): an `import` over the live account is never undone by the
    active-token refresh, the switch's outgoing capture or recovery; `switch <N> --force`
    clears the stale mark; a forward recovery after a replacement leaves the live store
    stale-marked.
  - **Storage-write lock** (§9.1): a CC-style writer that updates a machine-shared key under
    that lock while a switch, recovery, active-token refresh or bootstrap waits for it loses
    nothing, and an account-scoped change under it aborts the tagteam write.
  - **Session-owned usage** (§8.1): the profile's token is read without a lock, an expired one
    sends nothing and gives back its slot, and a 401 stamps `rejected_fp`.
  - **Export** (§13.3): the generation exported for each source and each case of its rules: a
    live login CC rotated, one whose live copy is the vault's `.prev`, a pending rescue, a
    stale-marked live store, a profile rotated since its seed, a stale-marked profile, a
    drifted profile. A refresh in flight in the live store or in a profile completes before
    the read. Each broken case is skipped from a bulk export and is an error with `--account`.
    The file holds no machine-local field.
  - **Import** (§13.3): a replacement of the live account stale-marks the live store and is
    never undone by any capture path; a replacement of a session-owned account stale-marks
    its profile; the store's active account is never set; an ID in the file is ignored; a
    file that fails pass 1 writes nothing.
  - **Purge and remove** (§10.3, §10.5): killed at each deletion and run again, each finishes.
    In between, the partly deleted account is never switched to, refreshed or launched, and
    with a pending rescue or a rotated quiescent profile, its consumed vault generation is
    never sent or activated. An `add`, `import` or launch
    started during a purge waits for it and lands in the emptied store, never in a deleted
    one. Purge refuses while an account or an orphaned profile is session-owned, inside a run
    shell, while an engine runs, and when the accounts changed after confirmation; a full
    purge over a `rescue` file that is not a directory still finishes. A full purge leaves no `tagteam`
    Keychain item (with the real `security` driver) and no deleted row readable in the store
    file or its WAL, and keeps `config.toml` and the lock files. `--provider` leaves the other
    provider's data and the usage budget alone.
  - **Doctor** (§13.6): a fixture home in each state each check reports, snapshot tested in
    human and JSON form. Against a missing data directory it creates nothing, and against any
    home it leaves every file byte-identical and asks nothing.
  - **Settings** (§6.4): `set` and `unset` change nothing outside their key, byte for byte,
    comments included, and write through a symlink; two concurrent `set`s both land; every
    registry key's bounds are refused by `set` and defaulted by reads; a running `auto` picks
    a change up.
  - **Logging** (§14.2): two processes logging past 1 MiB together leave three files whose
    lines, read oldest first, are each process's lines in order, with no gap after the first
    line kept; `statusline` with the default filter opens no log file; a log that cannot be
    written fails no command.
  - **Displaced** (§6.3): the listing joins rows and files, including a file with no row and
    a row with no file; `--purge` deletes the file before the row, and nothing when one ID is
    unknown.
- **Provider neutrality.** The test-only `FakeAgent` provider, in its own crate `tagteam-fake`
  (§4.1), is registered alongside Claude Code. It has its own home layout, a file-based
  credential store, a single live lock (its config lock is a no-op), refresh without a
  managed-key axis, its own usage windows, and some capabilities switched off. It grows with
  the trait: every trait method lands with its `FakeAgent` implementation. Its shapes differ from CC's on purpose: an identity with no email, credential
  kinds CC doesn't have, and no primary long window (§4.5; its `monthly` window is `Long`, but
  it names none for consume-first). It supports sessions, with its own config-dir
  variable, share policy and session records, so the generic `run` machinery is exercised
  against a second provider. Engine tests run against both providers, and assert that:
  - positions, auto-switch state, leases and mappings stay per provider
  - a switch, refresh or auto tick on one provider never touches the other's state
  - a missing capability degrades as §4.5 specifies
  - a `run` writes nothing outside the provider's declared surface and its own profile, for
    either provider
  - its accounts store, list, export and import through the same schema and format, with no
    CC-shaped field

  This keeps the trait from quietly taking on Claude Code's shape before a real second provider
  exists.
- **`tagteam` (CLI):** snapshot tests (`insta`) of human and JSON output, of the exit codes,
  and of each shell's completion script.

### 15.3 Pinned invariants

- **Local state.** The test is generic and runs once per registered provider, using the
  provider's fixture home and its declared identity surface (§3). For Claude Code, the fixture
  home contains:
  - `~/.claude/projects/-x/memory/MEMORY.md`, `CLAUDE.md`, `history.jsonl`, `settings.json`,
    `skills/`, `plugins/`
  - a `~/.claude.json` with `projects`, `mcpServers`, `userID` and unknown keys

  After *every* mutating command, and after `run` with merge-back disabled, the test asserts
  that every file outside the identity surface is byte-identical, and that `~/.claude.json` is
  byte-identical outside the `oauthAccount` span. The active credential entry is inside the
  surface only for its account-scoped keys: its machine-shared keys are compared by value and
  must be unchanged after every command, including API-key activation and switching back. It
  also asserts that no provider's command touches another provider's home.
- **Refresh tokens.** For every error injected after a token response is received, the
  successor ends up in the vault or in `rescue/`. When both writes are made to fail, the
  command reports `Unpersisted`.
- **Degraded reads.** A degraded read never reaches the token endpoint. This is a compile-time
  guarantee, backed by a test that attempts it through the public API.
- **Logs.** Every command, run at TRACE against the fixture home, leaves no fixture email,
  organization name, token, key or passphrase in the log (§14.2).

### 15.4 Claude Code compatibility

**Shared facts.** `crates/tagteam-cc/compat/` is the single source of the facts tagteam keeps
about CC's layout, one file each: the tested version (`tested-cc-version`), §12.2's share
allowlist (`known-shared`) and known-private list (`known-private`), and the classified
environment names (`known-env`: every `CLAUDE_CODE_*` and `ANTHROPIC_*` name tagteam knows,
with whether `run` scrubs it, §12.5). `run`, `doctor`, the weekly job and `cargo xtask compat`
all read these files, so none can drift from another.

**`cargo xtask compat`** runs locally against the real `claude` and a real test account. `xtask`
is a workspace crate (`publish = false`) reached through a cargo alias. It drives a build of
`tagteam` with the `test-support` feature, and the real `claude`, as a user would.
- **Isolation.** Every CC home it uses is a scratch directory exported as
  `CLAUDE_CONFIG_DIR`, so every CC Keychain item it reads, writes or deletes is named by the
  hash of a scratch spelling (Appendix A.2). It refuses to start if any service name it would
  touch lacks that hash suffix: an unsuffixed item is the user's own login. tagteam's vault
  uses a temporary keychain, through the same `test-support` hook as the `real_keychain` tests
  (§15.1). Everything it created is deleted when it ends; `--keep` leaves it for inspection.
- **The test account** is dedicated to compat. `cargo xtask compat login` runs `claude` in a
  scratch home for the user to log in once, and keeps the login in a compat store under
  `$XDG_STATE_HOME/tagteam-compat/`. Later runs take the account from there, and tagteam's own
  capture keeps that copy current. It is never an export of an account in daily use: compat
  refreshes it, which would consume the user's copy (§13.3).
- **Checks:**
  - `claude auth status --json` against seeded profiles: every §12.3 outcome, `configDirectory`
    equal to the exported spelling, and the `authMethod` a setup-token account yields
  - the profile's hashed Keychain item is the one named from the exported spelling, and CC
    moves a bootstrapped `.credentials.json` into it on its first credential write
  - CC writes `settings.json` through a single link, appends `history.jsonl` through one, and
    what it does to a linked `CLAUDE.md` and `keybindings.json`
  - session records: written at start, removed on SIGINT, SIGTERM and SIGHUP, with an `lstart`
    `procStart` on macOS and Linux; a `claude --bg` daemon's record in a profile
  - the storage-write lock: CC waits for tagteam's, and the reverse
  - hot reload after a switch (file mtime, and the Keychain within 30 s)
  - CC reads tagteam-written Keychain items silently from a non-GUI (SSH) session, which covers
    risk R1
  - the existence probe's result on a locked Keychain, which decides whether a file-fallback
    activation can commit (Appendix A.3)
  - CC runs on an API key while the credential entry keeps only machine-shared keys (§9.4)
  - lock interop while CC refreshes
  - CC honours `~/.claude.json.lock` around its own writes of the global config (§9.1)
  - CC accepts a `~/.claude.json` that tagteam created on a fresh machine (§9.5), and its own
    next write of the file leaves the span tagteam spliced byte-identical, which pins §9.5's
    rendering against `JSON.stringify`
  - which of the managed-key item and `primaryApiKey` CC reads first, and whether a key in
    `primaryApiKey` applies on the next message as §9.4's hint says
  - CC writes `expiresAt` as an integer
  - `claude auth status` writes nothing in the home it inspects (§13.6 relies on it)
  - opt-in, since they need the user's own session: the lock check on a locked login keychain
    in a GUI session (§17 O3), and CC reading tagteam-written items over SSH (`--ssh`)
- Checks that need CC to refresh expire the scratch profile's access token and run one minimal
  `claude -p` on the smallest model, so a run spends a few requests of the test account's
  usage.
- **Result.** A report in JSON and Markdown under `target/compat/`, one entry per check with
  its evidence. It exits 0 when every check passes, 1 when one fails, and 2 when the harness
  itself fails. `--bless`, after a full pass, writes the version of `claude` it ran against to
  `compat/tested-cc-version`. That is the only way the tested version advances.
- **A weekly CI job** installs the latest `claude` and runs the checks that need no account:
  version, the `auth status` shape when logged out, the top-level `~/.claude` entries against
  `known-shared` and `known-private`, and the `CLAUDE_CODE_*` and `ANTHROPIC_*` names the binary
  contains against `known-env`. It opens an issue on drift, or comments on the open one when
  the report changes.
  - A logged-out `claude` creates almost nothing in `~/.claude`. So before listing entries,
    the job runs one headless `claude -p` with a dummy API key, which the API rejects.

## 16. Build, release, repository

- **Repository.** `~/Code/tagteam`, published as GitHub `michaelrigart/tagteam`. MIT license,
  with a `NOTICE` crediting claude-swap (MIT, © Onur Cetinkol), whose behaviour and constants
  are derived.
- **Toolchain.** `rust-toolchain.toml` pins the stable Rust current at implementation time
  (edition 2024, MSRV ≥ 1.85). mise respects that file.
- **CI** (GitHub Actions) on `macos-latest` and `ubuntu-latest`: `cargo fmt --check`,
  `clippy -D warnings`, and `cargo test` (plus the `real_keychain` tests on macOS).
- **Release.** `cargo-dist` builds `aarch64-apple-darwin`, `x86_64-apple-darwin`,
  `x86_64-unknown-linux-musl`, and `aarch64-unknown-linux-musl`, one archive per target.
  cargo-dist 0.33 cannot merge the two macOS builds into a universal binary; the Homebrew
  formula picks the architecture. It publishes GitHub Releases and the Homebrew tap
  `michaelrigart/homebrew-tap` (formula `tagteam`). Archives carry `LICENSE` and `NOTICE`.
  - Every pull request builds all four targets without publishing.
  - A smoke job gates publishing. On each musl target the binary must be statically linked,
    run on Alpine, and round-trip the bundled SQLite store. An ignored live test, built for the
    same target, proves the TLS platform verifier accepts a real chain and rejects an untrusted
    root.
- **Package names.** On crates.io, `tagteam` for the binary crate, plus the `tagteam-*` library
  crates. Name availability was checked on 2026-09-26: crates.io and Homebrew are free.

## 17. Risks and open items

| # | Risk / item | Mitigation |
|---|---|---|
| R1 | Keychain access control. Items created through Security.framework might make CC's `security` reads prompt or fail (rc 36) over SSH or under launchd (*inferred*) | Verified on 2026-09-27, macOS 27.0, CC 2.1.283: `claude` silently reads tagteam-written items from SSH and GUI sessions alike. An SSH session's login keychain stays locked (rc 36) until `security unlock-keychain`, for Claude Code's own items as much as for tagteam's. Use only `/usr/bin/security` for every item. |
| R2 | CC drift beyond 2.1.286. A feature-flagged storage layer ("storageV5") may bypass the `~/.claude.json` lock; new per-account files, locks and auth variables keep appearing (2.1.286 added a storage-write lock, a credential-using daemon, and org policy caches in the config home) | The weekly compat job, `doctor` version warnings, the known-entries lists, the scrub list, and validation's `overridden` outcome (§12.3) |
| R3 | The usage endpoint budget is empirical (~30 requests per hour per identity) and could change | Enforce a hard budget of 20 per hour below it, and keep the 180 s floor and AIMD. Re-derive the constants from logs, not from comments |
| R4 | The merge-back can conflict with concurrent edits of `~/.claude.json` | Three-way merge against the baseline, the default file wins, under CC's config lock |
| R5 | Sharing could carry one account's state into another's profile, or leave a new user-content entry unshared | Sharing is by allowlist, so an unknown entry stays private (§12.2): the failure is a feature that starts empty in a profile, never a leak. Unknown entries are reported by the launch, `doctor` and the weekly compat job; `run.share_extra` covers a user's own files |
| R8 | CC replaces a symlinked file with a regular file when it writes it, splitting a shared file between the homes | Share directories where possible; the shared files are ones CC appends to or writes through a link, or that the user edits. A launch detects a split and refuses for a must-share entry (§12.2) |
| R6 | cswap's heuristics (§11) are complex | Named predicates, simulation tests, and a documented trace corpus |
| R7 | The `Provider` trait is designed from one real implementation, so it may not fit Codex, Gemini CLI or Grok (different auth models, API-key-only logins, no usage endpoint, no session isolation variable) | Keep the trait internal, not a public API. Use explicit capability flags. Exercise the engine with the `FakeAgent` provider (§15.2). Revise the trait in the first real second-provider spec rather than guessing now |
| O1 | Whether `.device-keys.json` should be shared | Closed: CC 2.1.286 keeps device keys machine-wide, keyed by account (Appendix A.7), so a profile's copy is never used |
| O2 | CC 2.1.286 passes `rate_limits` (five-hour, seven-day and spend percentages with resets) on the statusline command's stdin | Not used: `statusline` ignores stdin (§13.5). A later version could record them as free readings for the session's account, outside the request budget |
| R9 | An exported OAuth login is a single-use refresh lineage, so using it on two machines quarantines it on whichever refreshes second | Export reads each account's newest generation and says that it hands logins over (§13.3); one login per machine is the supported way to share an account |
| O3 | On a Mac whose login keychain is locked, the lock check may show a SecurityAgent dialog for up to its 5 s timeout, and whether killing `security` at the timeout dismisses it is unknown (found while planning M4a) | An opt-in `cargo xtask compat` check settles it (§15.4). Until then every command, `doctor` included, bounds the check by its timeout |

---

## Appendix A — Claude Code provider: interop contract (verified against CC 2.1.283, re-verified against 2.1.286)

This appendix is the contract that `tagteam-cc` implements. Nothing in it applies to other
providers. It was re-verified against CC 2.1.286 on 2026-10-01, by reading the JavaScript the
binary embeds and by probing a fresh config dir; facts marked *2.1.286* are new or changed
since 2.1.283.

### A.1 Paths

- **Config home:** `$CLAUDE_CONFIG_DIR` if non-empty, else `~/.claude`, NFC-normalized and
  never resolved through `realpath`. *2.1.286:* a `CLAUDE_CONFIG_DIR` that is set but empty
  makes some of CC's paths relative to the working directory while its Keychain naming treats
  it as unset. tagteam treats an empty value as unset, never exports one, and `doctor` warns
  when one is set.
- **Global config:** `<config_home>/.config.json` if it exists (legacy). Otherwise
  `($CLAUDE_CONFIG_DIR or $HOME)/.claude.json`. By default that is `~/.claude.json`, not inside
  `~/.claude`. *2.1.286:* the non-production OAuth switches (`CLAUDE_CODE_CUSTOM_OAUTH_URL`,
  `USE_LOCAL_OAUTH`, `USE_STAGING_OAUTH`) rename it `.claude-custom-oauth.json`,
  `.claude-local-oauth.json` or `.claude-staging-oauth.json`, and give the Keychain services a
  matching `OAUTH_FILE_SUFFIX`. tagteam supports production names only: `run` scrubs the
  switches (§12.5), and `doctor` warns when one is set.
- **Plaintext credential:** `<secure-storage dir>/.credentials.json`.
- **Secure-storage dir:** if `CLAUDE_SECURESTORAGE_CONFIG_DIR` is *defined* (even empty), use it
  NFC-normalized, with empty meaning `~/.claude`, whatever `CLAUDE_CONFIG_DIR` says. Otherwise
  use the config home.
- **Credential locks:** `<secure-storage dir>/.oauth_refresh.lock` (symlinks not resolved) and
  `<realpath(secure-storage dir)>.lock`, both `proper-lockfile` with stale 60 s and update 5 s
  (§9.1). CC takes the refresh lock first, then the legacy one, and releases the first to retry
  when the second is contended.
- **Storage-write lock** (*2.1.286*): `<secure-storage dir>/.storage-write`, `proper-lockfile`
  with stale 15 s and up to 10 retries. CC takes it around every write to its secure storage,
  re-reading the entry strictly under it (§9.1, A.3).
- **Refresh-lock owner record** (*2.1.286*): CC writes `.oauth_refresh.lock.owner` (its pid,
  `procStart` and the lock directory's birth time) when it takes the refresh lock, and, behind
  a feature flag, takes over a lock whose recorded owner is dead. It never takes over a lock
  that has no owner record, or whose record names another birth time than the directory's, so
  tagteam's owner-less locks (§9.1) stay safe.
- **File writes.** CC writes `.claude.json` and the user `settings.json` through a symlink,
  one hop only: it reads the link, resolves it against the `realpath` of the link's directory,
  and renames a temporary file over that target, so a link to a link loses its second link
  (anthropics/claude-code#78162). It writes `.credentials.json` and most other files by
  renaming a temporary file over the path itself, which replaces a symlink with a regular file.
  Project and local settings refuse symlinks altogether (*2.1.286*, not tested empirically).
- **Session records:** `<config_home>/sessions/<pid>.json` (A.7). **IDE locks:**
  `<config_home>/ide/<port>.lock`; with `CLAUDE_CONFIG_DIR` set, CC also reads the default
  `~/.claude/ide`.

### A.2 Keychain naming (macOS)

- **Service:** `"Claude Code" + OAUTH_FILE_SUFFIX + n + (useDefault ? "" : "-" +
  hex(sha256(NFC(dir)))[..8])`.
  - `n` is `"-credentials"` for OAuth and `""` for the managed API key.
  - `OAUTH_FILE_SUFFIX` is `""` for production builds.
  - `useDefault` is `!value` when `CLAUDE_SECURESTORAGE_CONFIG_DIR` is defined, else
    `!CLAUDE_CONFIG_DIR`.
  - `dir` is the **raw exported string**, NFC-normalized and never canonicalized; a trailing
    slash changes the hash.
- **One item, no fallback** (*2.1.286*). Every read, write and delete uses that one service.
  CC no longer falls back to the unsuffixed item for an explicitly set
  `CLAUDE_CONFIG_DIR=~/.claude`, nor to `hash(readlink target)` for a symlinked config dir, so
  every spelling of a directory names its own item. tagteam reads, writes and clears only that
  item too, and exports each profile under one recorded spelling (§12.2). Items left under the
  former fallback names are inert; `doctor` reports them.
- **Account:** `$USER`, else the passwd name for `geteuid()`, else `claude-code-user`. It is also
  `claude-code-user` when the name fails `^[a-zA-Z0-9._-]+$`.

### A.3 `security` driver

- **Binary:** always the absolute `/usr/bin/security`, with a 5 s timeout per spawn.
- **Read:** `find-generic-password -a <acct> -w -s <svc>`.
  - rc 0: the bytes, with exactly one trailing `\n` stripped. `security` emits `0x<HEX>` for
    any data it would otherwise have to escape, not only non-printable bytes, so that hex
    rendering and a printable secret that happens to be all lowercase hex digits of even
    length are byte-for-byte identical: that shape alone is ambiguous. Output that is not
    hex-shaped needs no further check — it is `-w`'s verbatim rendering of printable text.
    Resolve the ambiguous case with one `find-generic-password -a <acct> -g -s <svc>` call on
    the same item: its `password:` line is `0x<HEX>` for binary or a quoted string for
    verbatim text, and either decodes exactly back to the stored bytes. A failed or
    unparseable `-g` call, or one whose decoded value disagrees with `-w`'s bytes, is
    `Unreadable`.
  - rc 44: `Absent`. rc 36, any other rc, or a timeout: `Unreadable`.
- **Existence probe:** the same command without `-w` (attributes only; never prompts).
  - On a locked keychain file or a locked SSH session: rc 0 for present, rc 44 for absent; never prompts.
  - `show-keychain-info` on a locked keychain file returns rc 128. The argv/new-item versus `-i`/`-U` write difference does not change the item's ACL (both use the same binary).
- **Write:** `security -i`, with the stdin line `add-generic-password -U -a "<acct>" -s "<svc>"
  -X "<hex>"`.
  - If the line exceeds 4032 bytes (the 4096-byte `-i` line limit minus 64), use argv instead.
    An over-long `-i` line truncates silently and leaves the old entry.
  - `-U` updates the item in place and preserves its access control.
- **Delete:** `delete-generic-password -a <acct> -s <svc>`. rc 44 counts as success.
- **Delete by service** (a full `purge`, §10.5): `delete-generic-password -s tagteam`, without
  `-a`, deletes one item of that service per call and returns rc 44 once none is left
  (*inferred*; a `real_keychain` test pins it). Purge repeats it until rc 44, at most 10 000
  times, then verifies with `find-generic-password -s tagteam` (attributes only) that the
  service is gone.
- **Lock check and unlock** (§17, R1). Over SSH the login keychain stays locked: reads, writes
  and `show-keychain-info` all return rc 36.
  - A command that will read or write a Keychain item first runs `show-keychain-info` on the
    default keychain: rc 0 is unlocked, rc 36 locked. Any other rc (such as 128 for a locked
    keychain file) or a timeout is unknown, and the command proceeds; its tri-state reads
    refuse safely.
  - A command that touches no Keychain item runs no check, so `list` and `status` with no
    store never spawn `security`. Linux has no check. `doctor` reports the state instead
    (§13.6).
  - Exception: `list` and `status` run no check even when collecting usage reads Keychain
    items. A locked keychain degrades the affected rows to `keychain_unavailable` instead of
    failing the command, so a script over SSH still gets every row.
  - **On a terminal** (stdin and stderr are TTYs, no `--json`), a locked keychain prompts on
    stderr: `The login keychain is locked (common over SSH). Unlock it now? [Y/n]`. Yes runs
    `security unlock-keychain` with the terminal attached and no password argument, so macOS
    asks for the password; tagteam never sees, stores or passes it. Exit 0 continues the
    command.
  - **Otherwise** (no terminal, `--json`, declined, or a failed unlock), the command fails
    before touching anything: exit 1, `keychain-locked`, naming
    `security unlock-keychain ~/Library/Keychains/login.keychain-db`.
- **Active reads** retry the Keychain twice, 300 ms apart. CC caches Keychain reads for 30 s and
  serves its stale cache on a read failure.
- **CC's read precedence.** CC reads the Keychain first and `.credentials.json` only as a
  fallback. Its default read treats *any* Keychain failure as absent and falls through to the
  file. *2.1.286:* its refresh and every secure-storage write read the Keychain strictly, so an
  item that exists but cannot be read fails the operation instead of being overwritten.
- **Writes** (*2.1.286*) run under the storage-write lock (A.1): CC re-reads strictly, applies
  its change, and writes. Its refresh saves compare-and-swap on the refresh token, adopting a
  sibling's newer write. On `invalid_grant` it writes both tokens empty and `expiresAt` 0,
  the `Wiped` shape (§9.4 step 4).
- **Plaintext migration** (*2.1.286*). CC never migrates on a read. On its next secure-storage
  write (a login, a refresh, any update) the Keychain write lands, and if the Keychain held no
  item before, CC deletes `.credentials.json`. If the Keychain write fails for a lasting
  reason, CC writes the file and deletes the Keychain item instead.
- **Hot reload.** CC invalidates its memoized token when the mtime of `.credentials.json`
  changes; *2.1.286* checks it before every pre-request refresh check.
  - After a Keychain write, rewrite `.credentials.json` with the same bytes if it *already
    exists*, to bump its mtime. Never create it.
  - **File fallback.** If the Keychain write fails and the write falls back to the file, the
    old Keychain item must then be deleted and verified `Absent` with the existence probe,
    because CC would keep reading it the moment the Keychain is readable. Only then does the
    activation commit. If the item cannot be verified absent, the switch rolls back. File mode
    stays pinned until the operation that fell back (one switch or one recovery) ends, so its
    later writes go to the file too, and a rollback clears the pin. The next operation tries
    the Keychain again, which matters for a long-lived process such as `auto` or the daemon.
- **Vault reads on macOS.** The Keychain is primary. A Linux-style file vault is never used on
  macOS; the fallback for a failed vault write is `rescue/` (§6.3).

### A.4 Credential shape

```
{ "claudeAiOauth": { "accessToken", "refreshToken", "expiresAt" (epoch ms), "scopes": [..],
                     "refreshTokenExpiresAt" (epoch ms), "subscriptionType", "rateLimitTier", … },
  "mcpOAuth", "mcpOAuthClientConfig", "mcpXaaIdp", "mcpXaaIdpConfig", "pluginSecrets",
  "trustedDeviceToken" }
```

- **Machine-shared keys** (taken from the live credential on activation, and from the
  profile's own credential at a profile bootstrap, §12.3):
  `mcpOAuth, mcpOAuthClientConfig, mcpXaaIdp, mcpXaaIdpConfig, pluginSecrets`.
- **Account-scoped keys:** `claudeAiOauth, trustedDeviceToken`. Unknown sibling keys are treated
  as account-scoped. *2.1.286* also stores `designOauth`, `gatewayTrust` and
  `enterpriseGateway` in the entry; tagteam treats them as unknown keys, so account-scoped
  (*inferred*).
- **Managed API key:** stored raw, not as JSON.

### A.5 Endpoints

| Purpose | Request | Notes |
|---|---|---|
| Token refresh | `POST https://platform.claude.com/v1/oauth/token`, JSON `{"grant_type":"refresh_token","refresh_token":…,"client_id":"9d1c250a-e61b-44d9-88ed-5944d1962f5e","scope":"<scopes joined by space>"}` | See the response handling below |
| Profile | `GET https://api.anthropic.com/api/oauth/profile`, `Authorization: Bearer`, 5 s timeout | Returns `{uuid: account.uuid, email: account.email, organizationUuid: organization.uuid}` |
| Usage | `GET https://api.anthropic.com/api/oauth/usage`, `Authorization: Bearer`, `anthropic-beta: oauth-2025-04-20`, 5 s timeout | Fields read: `five_hour`, `seven_day`, `limits[]`, `spend`, `extra_usage` (§8.2) |

**Token refresh response handling:**
- `access_token` is used as returned. `expires_in` gives `expiresAt = now_ms + expires_in·1000`.
- `refresh_token` replaces the old one only if present.
- `scope` is split on spaces into `scopes`.
- `refresh_token_expires_in` gives `refreshTokenExpiresAt`.
- The optional `account.uuid`, `account.email_address` and `organization.uuid` are used for the
  identity-conflict check.

All requests use `User-Agent: tagteam/<version>`.

Response shapes verified against the live endpoints on 2026-09-30 with Claude Code 2.1.285.
The redacted recordings are in `crates/tagteam-cc/tests/fixtures/endpoints/`. `token-200.json`
is synthetic, since a successful refresh can't be recorded without spending a real token.
- `invalid_grant` arrives as a 400 with `{"error": "invalid_grant", "error_description": …}`.
- An unknown client id arrives as a 400 with
  `{"type": "error", "error": {"type": "invalid_request_error", "message": "Client with id … not found"}, "request_id": …}`,
  not RFC 6749's `invalid_client`. §7.3 step 7 classifies both shapes as systemic.
- The usage response carries more than tagteam reads.
  - `five_hour` and `seven_day` are objects of `utilization` (a float percentage),
    `resets_at` (ISO 8601 with an offset), and dollar fields that are null on a subscription.
  - `limits[]` items carry `kind` (`session`, `weekly_all`, `weekly_scoped`), `group`, an
    integer `percent`, `resets_at`, and `scope.model.display_name` on scoped items.
  - `spend` gives `used` and `limit` as `{amount_minor, currency, exponent}`, plus `enabled`.
    The older `extra_usage` gives `used_credits` and `monthly_limit` in minor units, with
    `decimal_places`.
  - There are also per-model `seven_day_*` objects, code-named windows (mostly null),
    and `seven_day_breakdown`. §8.2 says which fields tagteam reads.

### A.6 `~/.claude.json` fields tagteam reads

| Field | Use |
|---|---|
| `oauthAccount.emailAddress` | identity; absence means no live login |
| `oauthAccount.organizationUuid` | identity; null counts as `''` |
| `oauthAccount.organizationName` | display |
| `oauthAccount.accountUuid` | uuid corroboration |
| `primaryApiKey`, `customApiKeyResponses.approved` | the managed-key axis |
| `projects`, `mcpServers` | profile seeding and merge-back. *2.1.286:* top-level `mcpServers` is the user scope and `projects.<path>.mcpServers` the local scope; each project also carries `allowedTools`, `hasTrustDialogAccepted`, the `.mcp.json` approvals and the external-include approvals |
| `theme`, `hasCompletedOnboarding` | profile seeding. *2.1.286:* `hasCompletedOnboarding` is the only onboarding gate; `theme` is not one |

Other flat account-dependent fields (`hasAvailableSubscription`, …) are left alone; CC
refreshes them itself (*inferred*). *2.1.286* creates per-home identifiers on a config dir's
first start (`userID`, `machineID`, `summonSidKey`, `firstStartTime`, `migrationVersion`), and
caches account- and org-scoped data (`groveConfigCache`, `modelAccessCache`,
`cachedGrowthBookFeatures`, …). Profile seeding copies none of them (§12.4).

### A.7 Sessions, background daemon and auth status (*2.1.286*)

- **Session records.** `<config_home>/sessions/<pid>.json` (directory 0700), written when a
  session starts and before its first prompt, and removed by a graceful exit (SIGINT, SIGTERM
  and SIGHUP all shut down gracefully); SIGKILL leaves it behind. Its fields include `pid`,
  `sessionId`, `cwd`, `startedAt` (epoch ms), `procStart`, `version`, `kind` (`interactive`,
  `bg`, `daemon` or `daemon-worker`) and `entrypoint`, and later `status` and `updatedAt`.
  `procStart` is `ps -o lstart=` output with `LC_ALL=C` and `TZ=UTC`, on macOS and Linux
  alike, and is omitted when `ps` fails. A session that CC itself spawned
  (`CLAUDE_CODE_CHILD_SESSION`) writes no record, unless `CLAUDE_CODE_FORCE_SESSION_PERSISTENCE`
  is set; a `run` started from inside a session relies on its reservation (§12.5).
- **Background daemon.** `claude --bg`, background agents and `claude daemon install` run a
  supervisor per config home. Its state is `<config_home>/daemon/` with sibling `daemon.*`
  files, and its sockets are under `/tmp/cc-daemon-<uid>/<sha256(config home)[..8]>/`. It reads
  that home's credential, refreshes it proactively under the credential locks, and passes
  access tokens to its workers. It registers a session record of kind `daemon`. The installed
  launchd or systemd service is for the default config dir only: `claude daemon install`
  refuses when `CLAUDE_CONFIG_DIR` is set.
- **`claude auth status`** prints JSON by default; `--json` is accepted and changes nothing,
  and `--text` prints text. Fields: `loggedIn`, `authMethod`, `apiProvider`,
  `analyticsDisabled`, `projectsDirectory`, `configDirectory` always; `email`, `orgId`,
  `orgName` and `subscriptionType` for a `claude.ai` login; `apiKeySource` and others when
  they apply. `authMethod` is `claude.ai`, `api_key`, `api_key_helper`, `oauth_token`,
  `third_party`, or `none` when logged out. It exits 0 only when logged in. Its source order is
  `ANTHROPIC_AUTH_TOKEN`, `CLAUDE_CODE_OAUTH_TOKEN`, the token file descriptor, an
  `apiKeyHelper`, a workload-identity profile, and only then the stored `claude.ai` login.
- **Workload-identity profiles** live outside the config dir (`$ANTHROPIC_CONFIG_DIR`, else
  `$XDG_CONFIG_HOME/anthropic` or `~/.config/anthropic`) and are selected by
  `ANTHROPIC_PROFILE` or an `active_config` file. They are not keyed by `CLAUDE_CONFIG_DIR`.
- **The statusline command** runs with CC's own environment (`CLAUDE_CONFIG_DIR` included) plus
  `CLAUDE_PROJECT_DIR`, `CLAUDECODE=1`, `CLAUDE_CODE_SESSION_ID` and others, in the session's
  working directory. Its stdin JSON carries the session, model, workspace, cost and context
  fields, and `rate_limits` (§17 O2).
- **Machine-wide state outside any config dir.** Device keys live in the Keychain item
  `Claude Code-device-keys` and the fallback file `~/.claude/.device-keys.json`, keyed by
  account uuid, whatever `CLAUDE_CONFIG_DIR` says. `~/.claude/bridge-spawn/` is fixed to the
  default home too. A profile shares these with the default home by CC's own design.

## Appendix B — Invariants carried over from cswap

Each is a one-liner, and each gets at least one test.

1. Reads are tri-state; "unreadable" never becomes "absent" or empty.
2. A degraded read is never consumed or captured into the vault, and an automatic capture never
   replaces a refresh token with a credential that lacks one.
3. A check-then-use uses the bytes that were checked; it never re-reads.
4. An empty or unreadable live read never overwrites a vault generation.
5. Bytes that are not ours are displaced before being overwritten, and a failed displacement
   aborts.
6. Wiped credentials never overwrite the vault's refresh token.
7. The identity oracle is advisory, never called under a lock, and never blocks a switch.
8. Attribution across accounts requires a positive uuid match.
9. Machine-shared credential keys come from the live credential, absence included;
   `trustedDeviceToken` stays with its account.
10. Writing OAuth clears the managed key, and the reverse; the `approved` list is append-only.
11. `~/.claude.json` is never overwritten without being read. A file that is unreadable, torn,
    or not a JSON object aborts the write; it is never replaced (changed from cswap, which
    salvaged and replaced it).
12. Atomic writes go through symlinks, with the temp file beside the resolved target.
13. Lock order is fixed and matches CC's. CC's credential locks go stale only after 60 s.
14. No network while holding a contended lock, except the bounded refresh requests (§4.3).
15. A received refresh successor is never discarded; if both persistence paths fail, the loss
    is reported.
16. A permanent (Dead) verdict requires a top-level `invalid_grant`, or a structurally complete
    credential with no refresh token, and a re-read showing the lineage unchanged.
17. A quarantine is bound to the fingerprint that was sent; any fingerprint change clears it.
18. A 429 doesn't invalidate `last_good`; it stays trusted until the earliest reset, capped at
    2 h.
19. Retry-After is honoured with a margin, capped, and never stored as non-finite.
20. The backoff exponent is clamped.
21. Usage results are fenced by lease and identity; late writers are dropped.
22. A failure never erases `last_good`.
23. Auto-switch never acts on an unmanaged live login. No switch, manual or automatic, activates
    a session-owned account, even with `--force`.
24. `at-limit` and `failover` bypass the anti-flap gates; `proactive` requires a landing below
    the threshold and the hysteresis margin.
25. The cooldown is re-checked under the mutation lock before the switch's first write, and the
    switch's commit records the auto-switch state in the same transaction as the switch
    (§11.2 step 11).
26. A healthy below-threshold tick is NO_ACTION, never BLOCKED.
27. A sleep may be shortened by the poll plan, never lengthened.
28. A running profile is never re-seeded or invalidated underneath its `claude`; it is left
    stale-marked instead (§12.5).
29. Only a definite `invalid` auth status deletes a profile.
30. tagteam only removes links it created; real history directories are never deleted.
31. Destructive operations refuse while an affected account is session-owned (a live launch
    reservation, or a session record that is live or unreadable).
32. Commands that change accounts or the live login refuse inside a `tagteam run` shell.
33. Secret-bearing files are created 0600 at creation, never chmod'ed afterwards.
34. Import validates everything before writing anything; the email and position checks defend
    against path traversal.
35. Log lines identify accounts by position or ID, never by email.
36. `--json` stdout is exactly one object (or one JSONL stream for `auto`); everything else goes
    to stderr.
37. A process that spawns `claude` holds no lock that anything waits on while it runs; its
    launch reservation is only ever tested non-blocking.

**Added by this design** (not in cswap), each with at least one test:

38. Every vault write holds the account lock and compares and replaces within one hold; the
    refresh gate is single-flight per account, and a suspended holder is never preempted.
39. The refresh gate never refreshes the live login's token, a session-owned account's, or an
    account named in an unresolved switch journal.
40. A lock tagteam has lost to a staleness takeover is neither written under nor removed;
    ownership is re-checked before every protected write, not only by the heartbeat.
41. No identity is sent more than its provider's hourly budget of usage requests, retries
    included, across all processes, and removing an account does not reset its count.
42. A switch that dies mid-way is recovered from its journal into a state the live credential
    decides, and the journal is cleared only when every surface agrees. After CC's locks are
    released, an old credential is never written back.
43. Memory and history are never split: must-share entries are created before linking, and a
    private copy refuses the launch.
44. A profile is seeded only when quiescent; a pending merge-back runs before any re-seed, and
    its failure aborts the launch.
45. The store, export and import hold no Claude Code–shaped field outside provider-owned JSON.
46. A launch reservation stays live while the parent or `claude` lives, and a session starts
    only from the vault's current generation (or hands a newer one back first). This is
    verified on the credential CC will actually read, after any obsolete Keychain item has
    been verified gone.
47. Switch and launch re-derive their decisions (outgoing account, target ownership, the
    default-login fast path) after taking their locks, and restart if the lock set no longer
    fits.
48. Refresh reconciliation (rescue adoption, self-heal) runs before any token request, on both
    refresh paths. Generation order comes from the live store and fingerprints, never from
    access-token expiry, and a rescue is never published over a live generation that has
    advanced past it.
49. A credential entry that cannot be read fresh is never overwritten, even with `--force`.
50. An explicit login replacement is never undone by a capture from any profile, live or
    quiescent (the login epoch), nor from the default home's live store (the activation
    epoch). One that dies part-way is reconciled by the next holder of the account lock, and
    never strands a profile's rotation.
51. Every credential kind has a fingerprint (§2), so interrupted switches between accounts of
    any kind recover. Recovery never re-activates the vault on the absence of a live credential
    alone.
52. A profile and the vault are compared only through the profile's seed, never by expiry. When
    both have moved in an unknown order, nothing is captured, refreshed or overwritten until an
    explicit replacement resolves it.
53. A signal never stops a critical span: a journaled switch, recovery's writes, a token
    request through the persistence of its successor, an explicit replacement, or a vault,
    rescue or atomic write. Lock waits are cancellable, and unwinding releases every lock
    (§14.1).
54. Non-interactive child processes run outside the terminal's process group, so a terminal
    Ctrl-C never kills one midway.
55. At most one auto-switch engine drives a provider on a machine, so `autoswitch_state` has a
    single writer (§11.1).
56. An auto-switch never replaces a live account other than the one its tick decided on; a
    manual switch made meanwhile wins (`live-changed`).
57. A run shell is recognized from its profile marker alone, and inside one tagteam resolves
    the provider's home from the marker: the live login it reads, refreshes or protects is
    always the default home's (§12.8).
58. A profile is exported under one recorded spelling, and every operation on its credential
    uses that spelling (§12.2).
59. A profile shares only allowlisted entries of the source home. Unknown entries stay private,
    and known-private entries (credentials, config, daemon state, account caches, org policy,
    tokens and locks) are never linked (§12.2).
60. A profile's credential carries the vault's account-scoped keys and only the profile's own
    machine-shared keys, so no rotating token, an account's or an MCP server's, has a copy in
    two homes (§12.3).
61. Every write of a CC credential entry holds CC's storage-write lock, re-reads the entry under
    it, and keeps the machine-shared keys CC wrote meanwhile (§9.1). It aborts when the
    account-scoped keys changed since the writer last read or wrote them, except by CC's
    dead-token marking (§9.1), which it writes over. A rollback's restore writes over nothing:
    it puts an entry back only while nothing has written it since tagteam did (§9.4 step 10).
62. Every launch checks that the session will authenticate as its account. Validation deletes
    a profile only at a bootstrap, and only when CC reports it logged out or logged in as
    another account; a login that something outside the profile overrides, or that the check
    cannot confirm, refuses the launch and keeps the profile (§12.3).
63. `run` exits with the child's status. Signals sent while `claude` runs never cancel the exit
    handling its exit starts, and exit handling that is cancelled later loses nothing: lazy
    capture and the next launch complete it (§12.5).
64. An export never carries a degraded generation, nor one this machine has consumed: each
    account's generation is its newest, read where its lineage advances (§13.3).
65. Import never takes an account ID from its file and never sets the store's active account,
    and it replaces an existing login only as an explicit replacement (§12.5).
66. Purge never deletes or replaces a provider's live login, and refuses while an affected
    account is session-owned. It holds `MutationGuard` throughout, so nothing is created
    behind it, and a purge that stops part-way is finished by running it again (§10.5).
67. `doctor` writes nothing, creates nothing and asks nothing (§13.6).
68. Every settings write is validated against the one key registry, and changes only the key
    it names (§6.4).
69. No log line, at any level, holds an email, organization name, token, key, credential or
    passphrase (§14.2).
70. `cargo xtask compat` never touches a CC Keychain item without a scratch hash suffix
    (§15.4).
71. Every quarantine that is cleared records an `unquarantine` event (§7.4).

## Appendix C — cswap → tagteam command map

| cswap | tagteam | Change |
|---|---|---|
| `add [--slot N] [--alias A]` | `add [--position N] [--alias A]` | "slot" renamed "position" |
| `add-token` | `add-token` | — |
| `switch [X] [--strategy …] [--force]` | same | — |
| `list [--json] [--token-status]` | `list [--json]` | Token diagnostics moved to `doctor` |
| `status` | `status` | — |
| `remove`, `disable`, `enable`, `alias` | same | — |
| `move A B`, `swap A B` | `move A POS` | Swaps when the position is taken |
| `auto …` | `auto …` | Same flags, events and exit codes |
| `run … [--share-history] [--no-share]` | `run …` | Sharing is always on; the flags are gone |
| `map`, `unmap` | same | Plus `shell-init` |
| `config …` | `config …` | TOML instead of JSON; snake_case keys |
| `export` / `import` | same | age-encrypted by default; imports cswap v1 exports |
| `unclaimed [--purge ID]` | `displaced [--purge ID...]` | Rescued successors are adopted automatically |
| `tui`, `watch`, `menubar` | — | Sub-projects 3 and 4 |
| `upgrade`, update check | — | Dropped; package managers handle it |
| `purge` | `purge [--provider P]` | Keeps `config.toml` (§10.5) |
| — | `history`, `statusline`, `doctor`, `completions`, `shell-init` | New |
