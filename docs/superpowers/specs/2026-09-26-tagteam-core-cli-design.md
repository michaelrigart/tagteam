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
**2.1.283**, except where marked *inferred*.

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
| Launch reservation | tagteam's own record of a `run` in progress, written before `claude` starts and removed after exit handling (§12.5) |
| Quiescent | A profile with no live launch reservation, and no live or unreadable session record |
| Session-owned | An account whose profile is not quiescent |
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
`Clock`, `Http` and `ProcessProbe`. Every account-scoped operation resolves the account's
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
    fn auto_tick(&self, cfg: &AutoConfig, sink: &dyn EventSink) -> Result<TickOutcome>;
    fn run_session(&self, req: RunRequest) -> Result<ExitStatus>;
    fn export / import / history / doctor(...);
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
  are the refresh and legacy locks, and the config lock is `~/.claude.json.lock` (§9.1).
  Order: tagteam mutation lock → account locks (ascending account ID) → provider live locks
  (credential locks → config lock). Any prefix may be skipped, but a lock is never taken while
  holding one that comes later: in particular, `MutationGuard` is never taken while holding an
  account lock. The one exception is a standalone config lock for profile seeding and
  merge-back (§12.4), which take no
  credential lock while holding it; taking a lone lock cannot invert the order.
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
  is never used (§17, R1).
- **Encryption.** The `age` crate: passphrase via scrypt, or X25519 and SSH recipients.
- **Logging.** `tracing` to a rolling file, controlled by `--debug` or `TAGTEAM_LOG`. Log lines
  identify accounts by position and ID, never by email, because users paste logs into public
  issues.

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
    fn primary_long_window(&self) -> Option<WindowKey>;     // CC: "7d"; ranks consume-first (§11.2)
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
    fn session_env(&self, profile: &Path) -> SessionEnv;   // vars to set and vars to scrub
    fn share_policy(&self, env: &Env) -> SharePolicy;      // source home + denylist (§12.2)
    fn seed_profile / capture_profile / merge_back(...);   // §12.3–12.5
    fn session_records(&self, profile: &Path) -> Read<Vec<SessionRecord>>;
    fn validate_profile(&self, profile: &Path) -> Validity;

    // Diagnostics
    fn doctor_checks(&self, env: &Env) -> Vec<Check>;
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
- A provider need not have a `Long` window. Features that depend on one (consume-first, pace)
  are unavailable for such a provider, and say so.
- Pace and projection (§8.7) apply to `Long` and `Scoped` windows that have a known period.
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
| Session profiles | `$XDG_DATA_HOME/tagteam/sessions/<id>/`, with its provenance (login epoch and seed generation) in `<profile>/.tagteam-seed.json` |
| Launch reservations | `<profile>/.tagteam-launch/<pid>.lock` |
| Mutation lock | `$XDG_DATA_HOME/tagteam/.mutation.lock` |
| Account locks | `$XDG_DATA_HOME/tagteam/locks/<id>.lock` |
| Log | `$XDG_STATE_HOME/tagteam/tagteam.log` (1 MiB × 3) |

Every file that contains secrets is created with mode 0600 at creation time (`O_EXCL`, then
write, then rename). It is never chmod'ed afterwards.

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
  account_id TEXT REFERENCES accounts(id) ON DELETE SET NULL
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
  name       TEXT PRIMARY KEY,  -- 'usage:<id>' | 'autoswitch:<provider>'
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
  started_at   INTEGER NOT NULL,
  prior        TEXT              -- the row a forced switch superseded; restored if this one never lands (§9.6)
);

CREATE TABLE autoswitch_state (       -- one row per provider; auto-switch never crosses providers
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

Leases only bound work whose overlap is harmless (a duplicate usage fetch, a skipped auto
tick). They never protect a refresh token: a lease can expire under a suspended holder, which
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
  other lineage must never undo that.
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
- **`displaced/`** holds live credentials that were not ours, stashed before a switch
  overwrote them. The files are forensic and write-only. `tagteam displaced` lists them;
  `tagteam displaced --purge ID` deletes one.

### 6.4 Settings (`config.toml`)

`tagteam config list|get|set|unset|path [--json]` edits the file with `toml_edit`, which
preserves comments and formatting. `set` writes only the key it is given, so defaults are never
frozen into the file.

- **Reads are forgiving.** A corrupt file or an out-of-range value falls back to the default,
  with a warning.
- **`set` and `unset` are strict.** They refuse to write to a corrupt file.
- **Booleans** parse only `true/false/1/0/yes/no`.

**Provider overrides.** The `[autoswitch]` table holds defaults for every provider. A
`[provider.<id>.autoswitch]` table overrides individual keys for one provider, for example
`tagteam config set provider.claude-code.autoswitch.models Fable`. Keys that only make sense for
one provider (`models`, `statusline.*`) are read from that provider's table first, then from
the global table.

| Key | Default | Valid |
|---|---|---|
| `default_provider` | `claude-code` | a registered `ProviderId` |
| `autoswitch.threshold` | 90.0 | 50–99.9 |
| `autoswitch.interval_seconds` | 60 | 15–3600 |
| `autoswitch.cooldown_seconds` | 300 | 0–86400 |
| `autoswitch.hysteresis_pct` | 10.0 | 0–50 |
| `autoswitch.strategy` | `best` | `best`, `consume-first` |
| `autoswitch.include_api_key_accounts` | false | bool |
| `autoswitch.unhealthy_ticks` | 3 | 1–100 |
| `autoswitch.models` | `[]` | list of model display names, or `["all"]` |
| `usage.history_retention_days` | 180 | 1–3650 |
| `statusline.format` | `"{account} · 5h {5h}% · 7d {7d}%{stale}"` | placeholders listed in §13.5 |
| `ui.color` | `auto` | `auto`, `always`, `never` (`NO_COLOR` and `FORCE_COLOR` also honoured) |

CLI flags override settings for a single invocation and are clamped to the same ranges.

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
  | `Owned` or `Conflict` | The matching §9.2 refusal: `interrupted-switch`, `session-owned` or `profile-conflict` |

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
   sent.
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
     only.
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
   | `invalid_client` | Systemic. Never counts as a strike |
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
- **The active account's quarantine** holds while *either* the live credential or the vault
  matches `quarantine_fp`.

### 7.5 Active-token refresh

The active token normally belongs to CC, and tagteam leaves it alone. tagteam refreshes it only
when the token has expired, or when the server returned 401 on a token that is still valid
locally (a sibling machine revoked it). The procedure:

1. Take a `MutationGuard`, then the account lock, then CC's credential locks (§4.3 order).
2. Re-read the live credential. It must be `Fresh`, and the live identity must still match the
   account.
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
   live write. If CC's locks turn out to be compromised when the response arrives, the
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
- **asked at most once per process for a given credential**, keyed by its fingerprint
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
- **Active account.** Usage fetches never refresh it; only §7.5 does that. A 401 on an access
  token that is still valid locally stamps `rejected_fp` with that token's fingerprint and
  hands the account to §7.5.
- **Session-owned account** (§12.5). The fetch is read-only and uses the profile's token.
  - A 401 stamps `rejected_fp` with the access-token fingerprint and reports `token_expired`.
  - The same bytes are not sent again until they change.
- **Retry-After** is parsed in its seconds form only.

### 8.2 Normalization

A fetch is normalized into generic windows (§4.5), stored as `last_good`. The rendering layer
turns them back into each provider's output shape (for CC, cswap's, §13.2). Claude Code's
windows:

- `5h` (`Short`) and `7d` (`Long`) from `five_hour` and `seven_day`: `pct`, `resets_at?`.
- `spend` (`Spend`) from `extra_usage`, with `detail {used, limit, currency}`. Present only
  when `extra_usage.is_enabled` is true and all three numbers are non-null. `used =
  used_credits / 100`; `limit = monthly_limit / 100`.
- `scoped:<name>` (`Scoped`), one per `limits[]` item that has a model `display_name` and a
  numeric `percent`.
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
2. **Fetch**, with no lock held.
3. **Record.** In a transaction fenced by lease holder and account identity; a late or
   superseded result is dropped.
   - **Success** writes `last_good` and `fetched_at`, resets the failure fields, stores the
     next plan, **and inserts `usage_samples` rows**.
   - **Failure** never touches `last_good` or `fetched_at`.

### 8.4 Trust (can a reading drive a decision?)

- A reading is decision-grade when its age is ≤ **300 s**.
- **Extended trust.** Trust extends to ≤ **3600 s** while failures are being retried, while a
  scheduled plan is in force, or while a live lease exists.
- **After a 429.** `last_good` is trusted until the earliest relevant window reset, capped at
  `fetched_at + 7200 s`. Usage only rises within a window, so the old reading is a valid lower
  bound.

### 8.5 Failure backoff

- **Base:** `min(30 · 2^(n−1), 600)` seconds, with the exponent clamped at 32.
- **429 with `Retry-After: 0`:** at least 300 s.
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
- **Movement ≥ 1 point:** `max(180, base/2)`. **No movement:** `min(ceiling, max(180, base ·
  1.5))`.
- **Urgent:** 60 s.
- **Recent 429:** `min(1800, max(interval, max(base · 1.5, 360)))`. A 429 counts as recent from
  when its backoff lifts, not from when the 429 arrived.
- **Exhausted:** at least 600 s.
- **Then:** apply jitter, then clamp: never below the 180 s floor (60 s when urgent), and never
  later than the next relevant reset + 60 s.

**Auto-switch scheduling** is O(1) per tick: the active account if it's due, plus the single
stalest due candidate. All candidates are fetched only when the active account is within 15
points of the threshold, or its usage is unknown for a reason other than an expired token.

### 8.7 History and projection

`usage_samples` feeds `tagteam history` and the projections.

- **Rate (regression).** Least-squares slope over the samples of the current window instance
  (same `resets_at` ± 60 s) from the last 48 h. It requires ≥ 3 samples spanning ≥ 2 h and a
  positive slope.
- **Rate (fallback).** cswap's average pace, for `Long` windows with a known period (CC's 7d:
  `period` = 604800 s):
  - `elapsed = period − ((reset − fetched_at) mod period)`
  - suppressed when `elapsed < 86400 s`
  - `expected = min(100, elapsed / period · 100)`
  - `rate = pct / elapsed`
- **Projections:**
  - `projectedExhaustionAt = now + (100 − pct) / rate`
  - `willLastToReset = pct + rate · (reset − now) ≤ 100`
  - `aheadOfPace = pct − expected ≥ 15` (`Long` windows only)
- **Where they appear.** `list` shows `(ahead of pace)` on `Long` windows. `history` shows the
  ETA. JSON carries all fields plus the additive `projectionMethod: "regression" | "average"`.

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

Both credential locks are anchored at the secure-storage dir, not the config home; the two
differ when `CLAUDE_SECURESTORAGE_CONFIG_DIR` is set (Appendix A.1).

When a switch or a recovery takes the three CC locks together, one 9 s budget covers all
three. The active-token refresh takes the credential locks with that budget and, after its
request, the config lock with a fresh 9 s budget (§7.5).

The CC locks follow the `proper-lockfile` protocol:

- `mkdir` acquires the lock.
- On `EEXIST`, if `now − mtime > staleness`, `rmdir` it and retry. Otherwise sleep a jittered
  250–500 ms.
- While the lock is held, a thread touches the directory's mtime every 3 s. CC touches every
  5 s; both are well inside the staleness windows.
- **Compromise detection.** An ownership check confirms that the directory still exists and
  still carries the mtime tagteam last set. If it doesn't, the lock has been taken over, and
  the guard is marked compromised. The check runs:
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

- **Inside a `tagteam run` shell** (`CLAUDE_CONFIG_DIR` under `sessions/`): every command that
  changes accounts or the live login is refused.
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
| `switch --strategy next-available` | next-available | the live account | Like rotation, but skips candidates with headroom ≤ 0 (the message names the binding window). If every candidate is exhausted: `candidates-exhausted` |
| `switch --strategy best` | best | the live account | Switch only if some switchable account has strictly more headroom. Ties stay put |
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
   planning then follows §7.2's quarantined-target rule, and a bare rotation plans again
   without it.
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
   without a refresh token never replaces a vault credential that has one. It is displaced
   instead.

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
   credentials, and the outgoing `oauthAccount` object. It holds no secret.
7. **Write the active credential** (Appendix A.3). The target's credential is always written
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
9. **Commit** in one store transaction: set the active account, insert an `events` row
   (`source` = `cli` or `auto`), and delete the journal row.
10. **Rollback.** Any failure in steps 7–9 restores, in reverse order, the original
    `~/.claude.json` bytes and the original live credential, then restores the journal row's
    `prior` if it carried one — a forced switch's superseded row (§9.6) — or deletes the row
    otherwise. Writing the original credential back is safe here, and only here: CC's credential
    locks have been held throughout, so CC cannot have rotated it. The operation fails with
    "rolled back", or with "rollback also failed" listing what could not be restored; the
    journal row then stays for recovery (§9.6). This covers errors and panics, through `Drop`. A
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
  fsynced and renamed. The mode is preserved, or 0600 for new files.

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
| The target's is present (`to_fp`), or the oracle resolves it to `to_id` | The switch landed; CC may have rotated the credential since | Finish forward: clear the other auth axis (§9.4 step 7), splice the target's `oauthAccount`, and commit (step 9) |
| The outgoing one is present (`from_fp`), or the oracle resolves it to `from_id` | The switch never landed, or its credential rollback succeeded | Finish backward, without touching the credential: splice `from_identity` back into `oauthAccount` if the live object names a different identity (by identity key; a CC-updated object for the same identity is kept), and keep the store's active account |
| Anything else, including no credential on either axis | Undecidable. For example, CC rotated the credential while the oracle is unavailable; or it rotated it and then logged out, so absence does not prove the switch never published | Keep the row. Account-changing commands for the provider refuse with `interrupted-switch` until recovery can decide; `switch --force` resolves it by displacing any live credential and activating the chosen account. The vault is never re-activated on absence alone |

**Account-changing commands** here are the ones that read or write credentials or the live
login: `switch` (without `--force`), `add`, `add-token` and `remove`. `alias`, `disable`,
`enable` and `move` change only store metadata and proceed regardless. They still attempt
recovery, but from fingerprints alone: they never ask the oracle (§7.6).

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

- **`remove <ACCOUNT>`** deletes the vault entries (strict), the store row (which cascades), the
  mappings, and the session profile. For the profile it deletes the profile's hashed Keychain
  item first, then the directory.
- **`disable` / `enable <ACCOUNT>`** hold an account out of automatic selection. It stays a
  valid explicit `switch` target.
- **`alias <ACCOUNT> <NAME>` / `alias <ACCOUNT> --unset` / `alias`** (list).
  - Aliases are lowercase and match `^[a-z0-9_.-]+$`.
  - They cannot be all digits or start with `-`, and they are unique case-insensitively.
- **`move <ACCOUNT> <POSITION>`.** If the target position is taken, the two accounts swap
  positions.
- **Guard.** Destructive commands (`remove`, `move`, `purge`, `add` over an occupied position,
  and profile bootstrap) refuse while an affected account is session-owned: a live launch
  reservation, or a session record that is live **or unreadable**.

### 10.4 Account references

An `ACCOUNT` argument is resolved in this order:

1. All digits: a position within the chosen provider (§13.1).
2. An alias. Aliases are unique across providers, so an alias alone identifies the provider.
3. An exact email, within `--provider` if given, otherwise across all providers.

If an email matches several orgs, or several providers, it is ambiguous:
- On a terminal, prompt with the candidates.
- With `--json`, or with no terminal, fail with an error listing the candidates.

An empty alias never matches.

## 11. Auto-switch

### 11.1 Structure

- **`tagteam-core`:** `decide(snapshot, state, config, now) -> Decision`. It is pure: no
  clock, no I/O. It is built from named predicates (`below_threshold`, `landing_ok`,
  `beats_by_hysteresis`, `recovered_since_departure`, `recovery_axis_useful`, …), each unit
  tested.
- **`tagteam-engine`:** fetch, freshen, switch, record and emit.
- **Per provider.** `tagteam auto` runs one independent engine per provider that has at least
  two switchable accounts, or only the provider given with `--provider`. Each has its own
  `autoswitch_state` row, `autoswitch:<provider>` lease, settings (§6.4) and poll budget.
  Nothing on one provider ever triggers a switch on another. Every JSONL event carries an
  additive `provider` field.
- **`--once` with several providers** exits with the most severe outcome: `1` if any provider
  errored, else `0` if any switched, else `3` if any was blocked, else `2`.

### 11.2 Tick

1. **Load state.** Release any quarantine whose fingerprint or identity has since changed (not
   in dry-run).
2. **No managed live account:** `no-switch` with reason `unmanaged-active-account` or
   `no-active-account`; outcome NO_ACTION. tagteam never acts on a login it doesn't manage.
3. **Collect usage** as scheduled (§8.6) and emit `poll`. Run a one-time check that the
   configured model names exist, emitting `config-warning` if not.
4. **Active account is an API key** and `include_api_key_accounts` is false: `active-api-key`.
5. **Decide the trigger:**
   - **Known headroom** resets the unhealthy-tick counter.
     - With `best`: usage below the threshold → `below-threshold` (NO_ACTION).
     - With `consume-first`: usage below the threshold → trigger `consume-first`.
     - Headroom ≤ 0 → `at-limit`. Otherwise → `proactive`.
   - **Unknown headroom with an expired but owned token:** idle-hold for up to **1800 s**
     (`active-idle`, slow cadence). CC refreshes the token on your next message.
   - **Otherwise unknown:** increment `unhealthy_ticks`. At `autoswitch.unhealthy_ticks`,
     trigger `failover`; before that, `active-usage-unknown n/N`.
6. **Cooldown.** `proactive` and `consume-first` within `cooldown_seconds` of the last switch →
   `cooldown`. `at-limit` and `failover` bypass it.
7. **Candidates** must be switchable, not the current account, not quarantined, and not
   session-owned. API-key accounts qualify only when enabled, and only as a last
   resort (never for `consume-first`). No candidates → `no-candidates` (BLOCKED).
8. **Rank:**
   - **Skip** unknown headroom, headroom ≤ 0, and the barred account (§11.3).
   - **Landing rule.** For `proactive` and `consume-first`, the landing account must be below
     the threshold, unless *every* account is above it.
   - **`best`:** candidate headroom − active headroom ≥ `hysteresis_pct`. Order by most
     headroom; ties go to the lower position.
   - **`consume-first`:** ranked on the provider's primary `Long` window
     (`primary_long_window()`, CC: 7d); a provider without one does not offer the strategy.
     The target's reset of that window must be strictly sooner than the active account's
     (unknown → skip). Order by soonest reset, then most headroom. Before
     switching, re-fetch the current account and all candidates and re-rank; the target's
     reading must be ≤ 180 s old, else `stale-usage`.
   - **Every account above the threshold:** for each pair, pick the recovery axis when both the
     active account and the candidate are within 3 points of headroom, or either one resets
     within 4 h. Otherwise use the headroom axis.
     - The recovery axis requires the candidate's binding-window recovery to be ≥ 300 s sooner
       than the active account's.
     - The headroom axis requires ≥ 2 × the active account's headroom.
     - Select the binding window first, then its reset. A past or unknown reset sorts last.
   - **`at-limit` and `failover`** skip every anti-flap gate.
9. **Nothing ranked:**

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

11. **Perform.** Take the `autoswitch` lease, re-check the cooldown inside the transaction, then
    call `switch` (direct). Record `last_switch_*`, `left_headroom`, `left_recovery_at` and
    `left_trigger`, then emit `switch`.
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
| BLOCKED otherwise, or idle-hold | `max(interval, 300)` |
| Anything else | `interval × U(0.9, 1.1)`, shortened (never lengthened) to the active account's `next_poll_at`, floored at 60 s |

A `sleep` event is emitted when the delay is more than 1.5 × `interval`. SIGINT and SIGTERM stop
the loop cleanly; the loop exits 0.

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

The flags `--once`, `--dry-run`, `--json`, `--threshold`, `--interval`, `--cooldown`,
`--strategy`, `--model` and `--include-api-key-accounts` override the settings.

### 11.5 Simulation tests

A proptest harness drives `decide()` through multi-day synthetic traces: 2–6 accounts, burn
rates, 5h and 7d resets, 429s, dead tokens, and unknown readings. It asserts:

- no A→B→A return within a cooldown unless A recovered
- never landing on an account with headroom ≤ 0
- never idling at the limit while a viable candidate exists
- `--once` exit codes consistent with the outcome
- deterministic results for a given seed

## 12. Parallel sessions: `tagteam run`

### 12.1 Invocation

`tagteam run [ACCOUNT] [--provider P] [--require-session] [-- <agent args>]`

The command launched is the provider's `launch_command` (`claude` for Claude Code). The rest of
this section describes the Claude Code provider; the sharing, liveness and capture rules are
engine-generic, with the provider supplying paths, the denylist and the env vars.

- **With no `ACCOUNT`,** use the nearest mapped ancestor of the current directory's canonical
  path, for the provider given with `--provider` (default: `default_provider`).
  - A mapping to a removed account: warn and run plain `claude`.
  - No mapping: run plain `claude` with an untouched environment.
- **API-key accounts** are refused.
- **The target is already the live default login:** run plain `claude`, so there are never two
  copies of one rotating token. `--require-session` refuses instead.

### 12.2 Profile layout

The profile is `$XDG_DATA_HOME/tagteam/sessions/<id>/`, and `CLAUDE_CONFIG_DIR` is set to that
exact absolute string with no trailing slash. CC hashes that string for the profile's Keychain
item (Appendix A.2).

**Shared by default.** Every top-level entry of the resolved default config home (`~/.claude`)
is symlinked into the profile, pointing at the *fully resolved* source (CC's settings writer
breaks on link-to-link, anthropics/claude-code#78162). That includes `projects/` (transcripts
and auto-memory), `history.jsonl`, `CLAUDE.md`, `settings.json`, `keybindings.json`, `skills/`,
`agents/`, `commands/`, `plugins/`, `todos/`, and any entry a future CC version adds.

**Must-share entries.** `projects/` (transcripts and auto-memory) and `history.jsonl` hold the
memory and history that §3 protects, and CC creates both on demand. If either is absent from
the default home, tagteam first creates it empty there (the create-only row of §3), so the
profile always links to it and CC never starts a private copy. If a profile holds a real
`projects/` or `history.jsonl` in place of tagteam's link, `run` refuses and names both
paths, for the user to merge by hand. Splitting memory or history silently is never an
option.

**Private to the profile** (the denylist):

| Entry | Why it's private |
|---|---|
| `.credentials.json` | the profile's own credential |
| `.claude.json` | the profile's own config; see §12.4 |
| `.config.json` | CC's legacy global config, which CC prefers over `.claude.json` when it exists (Appendix A.1). Shared, it would make the profile read and write the default identity and config |
| `sessions/`, `ide/` | per-profile process records |
| `backups/` | CC's config backups; restoring from a shared one could cross profiles |
| `.device-keys.json` | device key store (conservative) |
| `*.lock`, `*.lock.owner` | lock directories and owner files |

Unknown entries are shared, and `doctor` reports any entry that is not on the known-shared or
known-private lists.

**Sync runs on every launch.**
- Create any missing links.
- Remove only links that tagteam created (tracked in `<profile>/.tagteam-links.json`) whose
  source has disappeared.
- A real file or directory where a link belongs is never replaced; tagteam reports it (and
  refuses, for a must-share entry).
- Real history directories are never deleted.
- `.tagteam-*` entries are tagteam's own and are never linked.

### 12.3 Bootstrap and validation

This runs within a launch (§12.5), under `MutationGuard` and the account lock, and only when
the profile is quiescent and is missing, invalid, stale-marked, or holding a credential other
than the vault's current generation.

1. Refresh the vault credential through the gate first, before the launch takes its locks.
   - `Transient` with `rescued`, and `Unpersisted`, abort with advice.
   - A plain `Transient` continues with the stored credential.
2. If the profile is stale-marked, displace its current credential (§6.3): it may be a live
   generation of the login that was replaced.
3. **Always** delete the profile's hashed Keychain item (macOS), whatever the reason for the
   bootstrap, and verify it `Absent` with the existence probe (Appendix A.3). CC reads the
   Keychain first, so an item left behind, such as the consumed generation from a refresh-driven
   re-bootstrap, would stay authoritative over the file. If the item cannot be verified
   absent, the launch aborts.
4. Write the vault credential to `<profile>/.credentials.json` (0600). CC migrates it into its
   own hashed item on first write; tagteam never writes that item.
5. **Verify the effective credential.** Re-read the profile's credential the way CC would
   (Keychain first, then the file), and check that it is the vault's current generation.
   Only then record the profile's provenance in `<profile>/.tagteam-seed.json`: the account's
   current `login_epoch`, and that generation's fingerprint as the seed. A mismatch aborts
   the launch.
6. Seed `<profile>/.claude.json` (§12.4).
7. Validate with `claude auth status --json`, in exactly the session environment (§12.5), with
   a 10 s timeout. The profile is
   valid when `rc == 0`, `loggedIn === true`, `authMethod == "claude.ai"`, the `email` matches,
   and the `orgId` matches when both are present.
   - A timeout or unparseable output is `unknown`.
   - A spawn failure is `unreachable`.
   - **Only `invalid` deletes a profile.**

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

The write takes the profile's own config lock.

**Merge back, when the last session exits** (§12.5). This runs inside the exit handling,
under `MutationGuard`, and takes the default profile's config lock (`~/.claude.json.lock`) on
its own. If `~/.claude.json` cannot be spliced (§9.5) or written, the merge-back fails: it
warns, and keeps the profile's changes and the baseline, so it is retried before any re-seed.

1. Diff the profile's `projects.<path>.<key>` and `mcpServers.<name>` against the baseline.
2. Apply each changed or removed key to `~/.claude.json` with the §9.5 splice (`projects` and
   `mcpServers` subtrees only).
3. If the default file also changed a key since the baseline, the default wins, and tagteam
   warns on stderr naming the key.

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

   If the profile is not quiescent, join the running session without seeding.
4. Create this process's reservation.
5. Release `MutationGuard` and the account lock, and spawn `claude` with the reservation fd.

- The parent ignores SIGINT and SIGQUIT (the child owns the terminal) and forwards SIGTERM and
  SIGHUP to the child.
- The exit code mirrors the child's (`128 + signal` if the child was killed by a signal).

**Environment.** These variables are scrubbed from the session environment, with a warning:
`ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, `CLAUDE_CODE_OAUTH_TOKEN`,
`CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR`, `CLAUDE_CODE_API_KEY_FILE_DESCRIPTOR`,
`CLAUDE_SECURESTORAGE_CONFIG_DIR`. They are not scrubbed on the plain-`claude` fast path. A
pre-set `CLAUDE_CONFIG_DIR` is overridden, with a warning.

Removing `CLAUDE_SECURESTORAGE_CONFIG_DIR` makes the secure-storage dir resolve to the profile
(Appendix A.1). Set to an empty string, it would send CC to the default `~/.claude` credentials;
set to anything else, it would redirect them. Every profile credential operation (bootstrap,
validation, capture, and the hashed-item deletion in `remove`) resolves paths with this same
environment.

**When the child exits**, under `MutationGuard`, then the account lock:
- If the profile is quiescent apart from this process's own reservation, this is the last
  session out:
  1. **Capture.** Adopt the profile's credential into the vault if its provenance (below) says
     it rotated, and it is the same identity. The comparison and write happen under the
     account lock (§6.2), with no network.
  2. **Merge back** `.claude.json` (§12.4).
- In either case, unlink this process's reservation last.

**Lazy capture.** If tagteam itself was killed, the same adoption runs once the profile is
quiescent: at the next launch, usage collection, switch pre-check, or refresh gate, under the
account lock. The conditions are the same: quiescent, rotated according to its provenance,
and the same identity. The unmerged baseline is merged back at the next launch (step 2
above).

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
   kind, expiry);
2. write the vault;
3. in one store transaction, clear `replacing_fp` and `replacing_meta`.

The epoch moves first, so no profile, running or not, is ever captured over a replacement that
landed. The crash gap this ordering opens is closed by recovery. Any holder of the account lock
that finds `replacing_fp` set knows the replacer died, since the replacer held that lock
throughout, and reconciles before doing anything else:
- if the vault holds `replacing_fp`, the replacement landed: the recorded `replacing_meta` is
  installed onto the account (identity, kind, expiry; any quarantine is cleared with it), and
  the marker cleared;
- otherwise it never landed: `login_epoch` is decremented and the marker cleared. That restores
  the profile's eligibility, so a rotation it holds is captured rather than stranded.

A running profile is never touched: it is simply stale-marked, and is re-bootstrapped at its
next quiescent launch, which writes the new epoch and seed.

**Identity drift.** If the profile's `oauthAccount` email, or its org when both are set,
differs from the account's, the profile is ignored for that account.

### 12.6 Process liveness

Session records are read from `<profile>/sessions/*.json`, with fields `pid`, `procStart`,
`startedAt`, and so on.

A pid is live if `kill(pid, 0)` succeeds or returns `EPERM`, **and** it still belongs to the
record's writer:

- **Linux:** `/proc/<pid>/stat` field 22 (counted after the last `)`) must equal an all-digit
  `procStart`.
- **Otherwise (macOS, or a non-digit `procStart`):** get the start time from
  `proc_pidinfo(PROC_PIDTBSDINFO)` and the arguments from `sysctl(KERN_PROCARGS2)`. The pid is
  treated as recycled **only if** the process started more than 120 s after the record's
  `startedAt` **and** neither its executable name nor its arguments contain `claude`. This
  mirrors cswap, whose rule is based on `ps lstart`.

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
  that supports sessions (for now, `claude`). In a directory mapped for that provider, the
  wrapper runs `tagteam run --provider <id> -- "$@"`; elsewhere it runs
  `command <launch_command> "$@"`. It's opt-in, added by the user to their shell rc.

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
| `list` / `ls` | Every account with its 5h, 7d, spend and scoped usage, reset countdowns, markers (active, disabled, quarantined, ahead of pace) and data age. It fetches only when due (§8.3) |
| `status` | The live account |
| `switch [ACCOUNT] [--strategy best\|next-available] [--force]` | §9 |
| `add`, `add-token`, `remove` / `rm`, `disable`, `enable`, `alias`, `move` | §10 |
| `auto` | §11 |
| `run`, `map`, `unmap`, `shell-init` | §12 |
| `history [ACCOUNT] [--window 5h\|7d\|spend\|<model>] [--since 14d] [--csv]` | §13.4 |
| `statusline` | §13.5 |
| `export`, `import` | §13.3 |
| `displaced [--purge ID]` | §6.3 |
| `config` | §6.4 |
| `doctor [--online]` | §13.6 |
| `completions <shell>`, `purge` | `purge` deletes all tagteam data, including the vault Keychain items and the profiles' hashed items, after a confirmation (or `--yes`). It never touches any provider's live login. `--provider` limits it to one provider's accounts |

**Exit codes:** `0` OK · `1` error · `2` usage error · `130` interrupted. `auto --once` uses
0–3.

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
  active, usageStatus, usage, alias?, disabled?: true, loginExpiresAt?`.
- **When `usage` is non-null:** `usageFetchedAt, usageAgeSeconds`.
- **When `usage` is null:** `lastGoodUsage, lastGoodFetchedAt, lastGoodAgeSeconds`. If
  `usageStatus` is `unavailable`, also `usageError` and `usageRetryAt`.

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

`doctor` and `history` have their own `--json` shapes, documented in `--help` and snapshot
tested.

### 13.3 Export and import

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
- `-` writes to stdout.

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
- **The active account** is exported from the live store.
- **Broken accounts.** When exporting all accounts, a broken one is skipped with a warning.
  With an explicit `--account`, a broken account is a hard error.

**Command:** `tagteam import <FILE|-> [--force] [--identity F]...`

The format is detected automatically: armored or binary age (prompting for a passphrase, or
using `--identity F`), tagteam plaintext, or a **cswap v1 export** (`version: 1`,
`swapVersion`, `encrypted: false`, `accounts[].{number, credentials, config.oauthAccount}`).
cswap accounts import as provider `claude-code`. A tagteam account whose `provider` is not
registered in this build is refused in pass 1, naming the provider.

1. **Pass 1 validates everything** before writing anything:
   - the provider's identity validation (CC: the email regex) and integer positions ≥ 1 (a
     path-traversal defence)
   - field types, the provider's credential kinds, and alias rules
   - no duplicate identity keys within a provider, and no duplicate aliases
   - an alias owned locally by a different identity is dropped
2. **Pass 2 writes:**
   - **An existing identity** is skipped unless `--force` is given, or it is quarantined (it is
     then replaced automatically, with the cleared strike reported).
   - **A new identity** goes to its exported position if free, else to the next one.
   - Quarantines are cleared.
   - The active account is seeded only if none is set locally.
   - If the live login's account was rewritten, tagteam tells the user to run
     `tagteam switch N --force`.

### 13.4 History

`tagteam history` renders per-window sparklines, the current burn rate (points per hour), and
an ETA with its projection method. `--csv` and `--json` dump the raw samples.

### 13.5 Statusline

`tagteam statusline` is built for CC's `statusLine` command and meant to be fast. It is a
provider capability: the command resolves the provider from `--provider`, else from the
environment it runs in (a `CLAUDE_CONFIG_DIR` or a CC-invoked process means Claude Code), else
`default_provider`. It refuses for a provider without the capability.

- It drains piped stdin (up to 64 KiB) and ignores the contents.
- It does **no network and no Keychain access**.
- **Which account.** Inside a profile (from `CLAUDE_CONFIG_DIR`), the profile's account.
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

`tagteam doctor [--online] [--json]` reports each check as `ok | warn | fail`. It exits 1 if
anything fails.

**Checks:**
- **Claude Code:**
  - the `claude` binary was found, and its version compared against `TESTED_CC_VERSION`
    (`2.1.283`) — newer → warn
  - resolved paths: config home, global config, secure-storage dir, and Keychain service and
    account names
- **Keychain:**
  - reachable, or locked (`security show-keychain-info`, rc 36)
  - whether the account-name rule falls back to `claude-code-user`
- **Locks:** CC lock directories present and older than their staleness window.
- **Store:** `PRAGMA quick_check` on `tagteam.db`, and store/vault consistency (every account
  has a vault entry; no orphaned vault items).
- **Accounts:**
  - per-account vault readability
  - quarantined accounts
  - `login_expires_at` within 7 days → warn
- **Pending storage:** pending `rescue/` entries → warn. `displaced/` entries → info.
- **Interrupted switch:** a `switch_journal` row whose holder is dead → warn, or fail if §9.6
  cannot decide it.
- **Session profiles:**
  - entries in `~/.claude` that are on neither the known-shared nor the known-private list
    (§12.2) → warn
  - a real `projects/` or `history.jsonl` inside a profile (§12.2) → fail
  - reservations whose `tagteam` parent is gone but whose lock is still held, with the
    holding processes where the OS can tell; baselines awaiting merge-back → info
  - a provenance conflict (§12.5) → fail, naming the account and the fix; a pending
    replacement → warn
- **Online** (`--online`): TLS reachability of the token, profile and usage hosts, with no
  credentials sent.

## 14. Errors and logging

- **Library crates** use `thiserror` enums: `CcError`, `VaultError`, `StoreError`, `NetError`,
  `SwitchError`, and so on. Each variant has a stable `kind()` used as the JSON `error.type`.
- **The binary** maps them to exit codes and human messages that give the next action (for
  example, "log in with that account and run `tagteam add --position 3`").
- **Panics** are bugs. Lock guards release in `Drop`, and the switch transaction's rollback
  also runs from `Drop` if the transaction didn't commit. A killed process runs no `Drop`;
  the switch journal (§9.6) and launch reservations (§12.5) cover that case.
- **After a vault write advances an account, nothing may fail upward.** Any follow-up, such
  as replanning a poll, is contained: on failure it logs at ERROR. Profiles need no follow-up:
  the login epoch (bumped before the write) and the launch-time generation check (§12.5)
  cover them.
- **stdout is reserved for command output.** Refresh-persist warnings and similar notices go to
  stderr, so `--json` output stays a single object.

## 15. Testing

### 15.1 Isolation

- **All paths derive from an injected `Env`:** HOME, `XDG_*`, USER, `CLAUDE_CONFIG_DIR` (raw
  string), and `CLAUDE_SECURESTORAGE_CONFIG_DIR` (defined vs. undefined).
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
    racing `remove` / `switch` / the gate, and two overlapping sessions of one account.
  - **Budget:** across any interleaving of processes and on-demand callers, no account is sent
    more than 20 usage requests in a rolling hour.
  - **Fresh home:** a `run` against a home with no `projects/` or `history.jsonl` leaves both
    shared.
- **Provider neutrality.** The test-only `FakeAgent` provider, in its own crate `tagteam-fake`
  (§4.1), is registered alongside Claude Code. It has its own home layout, a file-based
  credential store, a single live lock (its config lock is a no-op), refresh without a
  managed-key axis, its own usage windows, and some capabilities switched off. It grows with
  the trait: every trait method lands with its `FakeAgent` implementation. Its shapes differ from CC's on purpose: an identity with no email, credential
  kinds CC doesn't have, and no `Long` window. Engine tests run against both providers, and
  assert that:
  - positions, auto-switch state, leases and mappings stay per provider
  - a switch, refresh or auto tick on one provider never touches the other's state
  - a missing capability degrades as §4.5 specifies
  - its accounts store, list, export and import through the same schema and format, with no
    CC-shaped field

  This keeps the trait from quietly taking on Claude Code's shape before a real second provider
  exists.
- **`tagteam` (CLI):** snapshot tests (`insta`) of human and JSON output, and of the exit codes.

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

### 15.4 Claude Code compatibility

- **`cargo xtask compat`** runs locally against the real `claude` and a real test account:
  - `claude auth status --json` against seeded profiles
  - hot reload after a switch (file mtime, and the Keychain within 30 s)
  - CC reads tagteam-written Keychain items silently from a non-GUI (SSH) session, which covers
    risk R1
  - the existence probe's result on a locked Keychain, which decides whether a file-fallback
    activation can commit (Appendix A.3)
  - CC runs on an API key while the credential entry keeps only machine-shared keys (§9.4)
  - lock interop while CC refreshes
- **A weekly CI job** installs the latest `claude` and runs the checks that need no account:
  version, the `auth status` shape when logged out, and top-level `~/.claude` entries against
  the known lists. It opens an issue on drift.

## 16. Build, release, repository

- **Repository.** `~/Code/tagteam`, published as GitHub `michaelrigart/tagteam`. MIT license,
  with a `NOTICE` crediting claude-swap (MIT, © Onur Cetinkol), whose behaviour and constants
  are derived.
- **Toolchain.** `rust-toolchain.toml` pins the stable Rust current at implementation time
  (edition 2024, MSRV ≥ 1.85). mise respects that file.
- **CI** (GitHub Actions) on `macos-latest` and `ubuntu-latest`: `cargo fmt --check`,
  `clippy -D warnings`, and `cargo test` (plus the `real_keychain` tests on macOS).
- **Release.** `cargo-dist` builds `aarch64-apple-darwin` and `x86_64-apple-darwin` (merged into
  a universal binary), `x86_64-unknown-linux-musl`, and `aarch64-unknown-linux-musl`. It
  publishes GitHub Releases and the Homebrew tap `michaelrigart/homebrew-tap` (formula
  `tagteam`).
- **Package names.** On crates.io, `tagteam` for the binary crate, plus the `tagteam-*` library
  crates. Name availability was checked on 2026-09-26: crates.io and Homebrew are free.

## 17. Risks and open items

| # | Risk / item | Mitigation |
|---|---|---|
| R1 | Keychain access control. Items created through Security.framework might make CC's `security` reads prompt or fail (rc 36) over SSH or under launchd (*inferred*) | Verified on 2026-09-27, macOS 27.0, CC 2.1.283: `claude` silently reads tagteam-written items from SSH and GUI sessions alike. An SSH session's login keychain stays locked (rc 36) until `security unlock-keychain`, for Claude Code's own items as much as for tagteam's. Use only `/usr/bin/security` for every item. |
| R2 | CC drift beyond 2.1.283. A feature-flagged storage layer ("storageV5") may bypass the `~/.claude.json` lock; new per-account files may appear in `~/.claude` | The weekly compat job, `doctor` version warnings, and the known-entries lists |
| R3 | The usage endpoint budget is empirical (~30 requests per hour per identity) and could change | Enforce a hard budget of 20 per hour below it, and keep the 180 s floor and AIMD. Re-derive the constants from logs, not from comments |
| R4 | The merge-back can conflict with concurrent edits of `~/.claude.json` | Three-way merge against the baseline, the default file wins, under CC's config lock |
| R5 | Sharing by denylist could share a future per-account file | Unknown entries are reported by `doctor` and by the weekly compat job |
| R6 | cswap's heuristics (§11) are complex | Named predicates, simulation tests, and a documented trace corpus |
| R7 | The `Provider` trait is designed from one real implementation, so it may not fit Codex, Gemini CLI or Grok (different auth models, API-key-only logins, no usage endpoint, no session isolation variable) | Keep the trait internal, not a public API. Use explicit capability flags. Exercise the engine with the `FakeAgent` provider (§15.2). Revise the trait in the first real second-provider spec rather than guessing now |
| O1 | Whether `.device-keys.json` should be shared | Private until verified; revisit in the compat suite |

---

## Appendix A — Claude Code provider: interop contract (verified against CC 2.1.283)

This appendix is the contract that `tagteam-cc` implements. Nothing in it applies to other
providers.

### A.1 Paths

- **Config home:** `$CLAUDE_CONFIG_DIR` if non-empty, else `~/.claude`.
- **Global config:** `<config_home>/.config.json` if it exists (legacy). Otherwise
  `($CLAUDE_CONFIG_DIR or $HOME)/.claude.json`. By default that is `~/.claude.json`, not inside
  `~/.claude`.
- **Plaintext credential:** `<secure-storage dir>/.credentials.json`.
- **Secure-storage dir:** if `CLAUDE_SECURESTORAGE_CONFIG_DIR` is *defined* (even empty), use it
  NFC-normalized, with empty meaning `~/.claude`, whatever `CLAUDE_CONFIG_DIR` says. Otherwise
  use the config home.
- **Credential locks:** `<secure-storage dir>/.oauth_refresh.lock` (symlinks not resolved) and
  `<realpath(secure-storage dir)>.lock`, both `proper-lockfile` with stale 60 s and update 5 s
  (§9.1).
- **Session records:** `<config_home>/sessions/<pid>.json`. **IDE locks:**
  `<config_home>/ide/<port>.lock`.

### A.2 Keychain naming (macOS)

- **Service:** `"Claude Code" + OAUTH_FILE_SUFFIX + n + (useDefault ? "" : "-" +
  hex(sha256(NFC(dir)))[..8])`.
  - `n` is `"-credentials"` for OAuth and `""` for the managed API key.
  - `OAUTH_FILE_SUFFIX` is `""` for production builds.
  - `useDefault` is `!value` when `CLAUDE_SECURESTORAGE_CONFIG_DIR` is defined, else
    `!CLAUDE_CONFIG_DIR`.
  - `dir` is the **raw exported string**, never canonicalized; a trailing slash changes the
    hash.
- **An explicitly set `CLAUDE_CONFIG_DIR=~/.claude` produces a suffixed item.** Readers try the
  suffixed item first, then the unsuffixed one.
- **A symlinked profile** is read as `hash(link)` first, then `hash(readlink target)`. A
  relative target is joined to the link's parent.
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
- **Lock check and unlock** (§17, R1). Over SSH the login keychain stays locked: reads, writes
  and `show-keychain-info` all return rc 36.
  - A command that will read or write a Keychain item first runs `show-keychain-info` on the
    default keychain: rc 0 is unlocked, rc 36 locked. Any other rc (such as 128 for a locked
    keychain file) or a timeout is unknown, and the command proceeds; its tri-state reads
    refuse safely.
  - A command that touches no Keychain item runs no check, so `list` and `status` with no
    store never spawn `security`. Linux has no check. `doctor` reports the state instead
    (§13.6).
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
  file; only its refresh path reads the Keychain strictly.
- **Hot reload.** CC invalidates its memoized token when the mtime of `.credentials.json`
  changes.
  - After a Keychain write, rewrite `.credentials.json` with the same bytes if it *already
    exists*, to bump its mtime. Never create it.
  - **File fallback.** If the Keychain write fails and the write falls back to the file, the
    old Keychain item must then be deleted and verified `Absent` with the existence probe,
    because CC would keep reading it the moment the Keychain is readable. Only then does the
    activation commit, and file mode is pinned for the rest of the process. If the item cannot
    be verified absent, the switch rolls back.
- **Vault reads on macOS.** The Keychain is primary. A Linux-style file vault is never used on
  macOS; the fallback for a failed vault write is `rescue/` (§6.3).

### A.4 Credential shape

```
{ "claudeAiOauth": { "accessToken", "refreshToken", "expiresAt" (epoch ms), "scopes": [..],
                     "refreshTokenExpiresAt" (epoch ms), "subscriptionType", "rateLimitTier", … },
  "mcpOAuth", "mcpOAuthClientConfig", "mcpXaaIdp", "mcpXaaIdpConfig", "pluginSecrets",
  "trustedDeviceToken" }
```

- **Machine-shared keys** (taken from the live credential on activation):
  `mcpOAuth, mcpOAuthClientConfig, mcpXaaIdp, mcpXaaIdpConfig, pluginSecrets`.
- **Account-scoped keys:** `claudeAiOauth, trustedDeviceToken`. Unknown sibling keys are treated
  as account-scoped.
- **Managed API key:** stored raw, not as JSON.

### A.5 Endpoints

| Purpose | Request | Notes |
|---|---|---|
| Token refresh | `POST https://platform.claude.com/v1/oauth/token`, JSON `{"grant_type":"refresh_token","refresh_token":…,"client_id":"9d1c250a-e61b-44d9-88ed-5944d1962f5e","scope":"<scopes joined by space>"}` | See the response handling below |
| Profile | `GET https://api.anthropic.com/api/oauth/profile`, `Authorization: Bearer`, 5 s timeout | Returns `{uuid: account.uuid, email: account.email, organizationUuid: organization.uuid}` |
| Usage | `GET https://api.anthropic.com/api/oauth/usage`, `Authorization: Bearer`, `anthropic-beta: oauth-2025-04-20`, 5 s timeout | Fields: `five_hour`, `seven_day`, `extra_usage`, `limits[]` |

**Token refresh response handling:**
- `access_token` is used as returned. `expires_in` gives `expiresAt = now_ms + expires_in·1000`.
- `refresh_token` replaces the old one only if present.
- `scope` is split on spaces into `scopes`.
- `refresh_token_expires_in` gives `refreshTokenExpiresAt`.
- The optional `account.uuid`, `account.email_address` and `organization.uuid` are used for the
  identity-conflict check.

All requests use `User-Agent: tagteam/<version>`.

### A.6 `~/.claude.json` fields tagteam reads

| Field | Use |
|---|---|
| `oauthAccount.emailAddress` | identity; absence means no live login |
| `oauthAccount.organizationUuid` | identity; null counts as `''` |
| `oauthAccount.organizationName` | display |
| `oauthAccount.accountUuid` | uuid corroboration |
| `primaryApiKey`, `customApiKeyResponses.approved` | the managed-key axis |
| `projects`, `mcpServers` | profile seeding and merge-back |
| `theme`, `hasCompletedOnboarding` | profile seeding |

Other flat account-dependent fields (`hasAvailableSubscription`, …) are left alone; CC
refreshes them itself (*inferred*).

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
25. The cooldown is re-checked in the same transaction that records the switch.
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
    quiescent (the login epoch). One that dies part-way is reconciled by the next holder of
    the account lock, and never strands a profile's rotation.
51. Every credential kind has a fingerprint (§2), so interrupted switches between accounts of
    any kind recover. Recovery never re-activates the vault on the absence of a live credential
    alone.
52. A profile and the vault are compared only through the profile's seed, never by expiry. When
    both have moved in an unknown order, nothing is captured, refreshed or overwritten until an
    explicit replacement resolves it.

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
| `unclaimed [--purge ID]` | `displaced [--purge ID]` | Rescued successors are adopted automatically |
| `tui`, `watch`, `menubar` | — | Sub-projects 3 and 4 |
| `upgrade`, update check | — | Dropped; package managers handle it |
| `purge` | `purge` | — |
| — | `history`, `statusline`, `doctor`, `completions`, `shell-init` | New |
