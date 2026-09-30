# tagteam M2a — Network Layer and Credential Lifecycle Implementation Plan

**Status:** In progress

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** tagteam talks to the network safely. Its profile oracle resolves who owns a live
token. The refresh gate refreshes stored tokens without ever losing a successor, and
quarantines dead ones. A manual switch freshens an expiring target first. Recovery captures a
rotated outgoing token it can attribute. Active-token refresh exists as a tested engine
operation. A second, test-only provider (`FakeAgent`) keeps the trait provider-neutral while
it grows.

**Architecture:**
- **HTTP.** A blocking `Http` port in `tagteam-provider`, with a `ureq` 3 adapter in
  `tagteam-engine`. The provider builds and parses every request.
- **The engine owns the safety machinery:** locks, the vault compare-and-swap, `rescue/` and
  quarantine (spec §4.5).
- **`FakeAgent`** is its own crate, `tagteam-fake`. Every trait method this plan adds lands
  with its `FakeAgent` implementation.
- **Lock split.** Claude Code's live locks split into credential locks and a config lock, so
  active-token refresh can hold only the credential locks across its request.

**Tech Stack:** Rust (edition 2024), `ureq` 3 (rustls, `platform-verifier`), rusqlite
(bundled), serde_json (`preserve_order`, `arbitrary_precision`), clap 4, sha2, libc,
thiserror, tracing; tests with tempfile and assert_cmd, a scripted in-process `Http` fake, and
a `std::net` mock server.

**Spec:** `docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md`. Read §2, §3, §4, §5,
§6.1–§6.3, §7, §9, §15 and Appendix A.5 before starting any task. Section numbers below refer
to that spec. The amendments this plan implements landed in commits `33b97d5`, `d307d39`, `d224220`,
`6f8bbe6` and `ac25b2d`.

## Execution notes

- When execution starts, set this plan's `**Status:**` to `In progress` in one commit. The spec
  is already `In progress` and stays so until M5.
- Tasks 1 and 19 are human steps.
  - Task 1 is a decision gate: nothing else starts until it passes.
  - Task 19's live acceptance run is the last step before the merge request.
- Feature flags used by tests:
  - `tagteam-provider/file-keychain` and `tagteam-provider/mock-server` (new: the `std::net`
    mock server).
  - `tagteam-engine/test-hooks` and `tagteam-cc/test-hooks`.
  - `tagteam/test-support`, which enables all of the above.
- The M1 run notes (`.superpowers/research/m1-run/final-review.md`) triage the carried-over
  items this plan picks up. The finding IDs used below (M-5, M-6, M-8, M-9, L300, …) are that
  file's.
- Tests never reach the real network. Engine tests use `ScriptedHttp`. The binary's test
  harness points every endpoint at `http://127.0.0.1:9` (connection refused, so `PreSend`)
  unless a test starts a `MockServer` and points them there.

## Milestones

The M1 plan's table split M2 in two during its brainstorming (2026-09-29):

| Milestone | Scope |
|---|---|
| M1 | Implemented (`6c80ff7`) |
| **M2a (this plan)** | `Http` port and `ureq` adapter, `FakeAgent`, HTTP oracle, refresh gate, rescue, quarantine, freshen-before-activation, rotation changes, pending rescues before activation, recovery capture, live-lock split, active-token refresh (engine operation), carried-over fixes |
| M2b | Usage fetch and normalization, collector, budget, poll policy, backoff and trust, `list` with usage and pace, `history`, `statusline`, post-switch poll replan, the §7.5 triggers. M2a's `build_engine` constructs a `UreqHttp` on every run, so M2b makes it lazy, keeping `statusline` within its 10 ms budget |
| M3 | Auto-switch (`decide()`, simulations, `auto`), `best` / `next-available`, the SIGINT handler (release blocker) |
| M4 | `tagteam run` |
| M5 | Export/import, `doctor`, `config`, `displaced`, `purge`, `completions`, logging to file, `cargo xtask compat`, CI and release |

**Deliberately absent from M2a, and why that is safe:**
- **No usage fetches, so no caller of active-token refresh (§7.5).** Both of §7.5's triggers
  are usage events (M2b). Until then CC stays the only process that refreshes the live
  token. Task 16 builds and tests the operation, so M2b only wires it in.
- **No profiles, so nothing is session-owned.** The gate's session-ownership check is a stub
  that returns "not session-owned" until M4.
- **Recovery events keep `source: "cli"`.** No other source exists until auto-switch (M3), so
  the value is already correct (L475's source item moves to M3).

## Global Constraints

Every task's requirements include these. Values are copied from the spec.

- Platforms: macOS and Linux only (§1.2).
- Rust edition 2024, MSRV ≥ 1.85; `rust-toolchain.toml` pins the toolchain (§16).
- Keychain access only through `/usr/bin/security` (§4.4, Appendix A.3).
- HTTP: blocking `ureq` with rustls and `rustls-platform-verifier`; no async runtime (§4.4).
- Every request carries `User-Agent: tagteam/<version>`. Response bodies are capped at 1 MiB.
  A request's `Debug` never shows its `Authorization` header or body (§4.4).
- Transport errors: `PreSend` only when the request provably never left (a DNS or connect
  failure); everything else is `Ambiguous`, TLS errors included (§4.4).
- Timeouts:
  - token request: 10 s from the gate (§7.3), 6 s from active-token refresh (§7.5)
  - profile request: 5 s (§7.6, Appendix A.5)
- Token endpoint: `POST https://platform.claude.com/v1/oauth/token`, JSON body
  `{"grant_type":"refresh_token","refresh_token":…,"client_id":"9d1c250a-e61b-44d9-88ed-5944d1962f5e","scope":"<scopes joined by space>"}`
  (Appendix A.5).
- Profile endpoint: `GET https://api.anthropic.com/api/oauth/profile` with
  `Authorization: Bearer <accessToken>` (Appendix A.5).
- No network while holding a contended lock, except the gate's token request under the
  account lock (10 s) and active-token refresh's under CC's credential locks (6 s) (§4.3). The
  oracle is never called under any lock (§7.6).
- Lock order: tagteam mutation lock → account locks (ascending ID) → provider credential locks
  → provider config lock (§4.3). `MutationGuard` is never taken while holding an account lock.
- Only the refresh gate sends a stored refresh token (§7.3); only active-token refresh sends
  the live one (§7.5). A degraded credential can never reach either (a compile-time guarantee
  via `FreshCredential`).
- A refresh-token successor tagteam receives is persisted to the vault or `rescue/`, or to
  `displaced/` when the token endpoint says it belongs to another account; it never enters
  that account's vault or `rescue/` (§7.3 step 6, §7.4). If every write fails, the command
  reports `Unpersisted`, never a different error, and the gate quarantines the account
  `successor_lost` (§1.1 criterion 2, §7.3, §7.4).
- Every file that holds a secret is created 0600 at creation (`O_EXCL`, write, rename) and never
  chmod'ed afterwards. `rescue/` and `displaced/` files are always plain files (§5, §6.3).
- Expired means `now_ms + 5 min ≥ expiresAt`; a non-numeric `expiresAt` counts as not expired.
  Freshen threshold: 10 min (§7.2).
- Log lines identify accounts by position and ID, never by email (§4.4). Secrets never reach
  `Debug`, logs or error messages.
- `--json` stdout is exactly one JSON object; warnings and notices go to stderr (§13.2).
- Tests never touch the real HOME, the login keychain, or the network (§15.1).
- Commits: small, imperative mood, no license headers, no agent attribution of any kind.

## Review Focus

Inputs and conditions the spec implies but no feature test would naturally hit. Each has a
pinning test in the task named.

1. **Switching offline to an account whose token is about to expire.** Expected: the switch
   proceeds within the request timeout, with a warning that Claude Code will refresh it when
   online. The vault and quarantine are untouched. → Task 14.
2. **A 200 token response that names neither token, or has a non-JSON body.** Expected:
   Transient `bad-response`. The vault is unchanged, nothing is quarantined, and nothing is
   rescued, since no successor was received. A 200 that carries a `refresh_token` but no
   `access_token` has delivered a successor, so it is `Refreshed` and persisted (§7.3's
   never-discard rule). → Task 8.
3. **A token response that omits `refresh_token`.** Expected: the old refresh token is kept, so
   the lineage fingerprint is unchanged. `.prev` does not rotate, and no quarantine is set or
   cleared by it. → Task 8 (shape) and Task 11 (persistence).
4. **A `rescue/` file that was hand-edited or truncated.** Expected: a switch to that account
   refuses with `rescue-pending`, naming the file, and never activates the vault's consumed
   generation. The gate returns `rescue-unreadable` without sending a request. → Task 13.
5. **A profile endpoint that hangs (captive portal).** Expected: `switch` finishes once the 5 s
   oracle timeout passes. The outgoing credential classifies `Unresolved` and is captured with
   `.prev` kept, and no lock is held while it waits. → Task 7.

---

## File Structure

```
.gitignore                                  + !*.sql
Cargo.toml                                  + ureq workspace dependency, tagteam-fake member (crates/*)
scripts/spikes/m2a-endpoints.sh             Task 1 probe (human-run)
crates/tagteam-provider/
  Cargo.toml                                + feature mock-server
  src/lib.rs                     MOD        http and mock_server modules, re-exports
  src/http.rs                    NEW        Http port, HttpRequest/Response/Error, ScriptedHttp, NoHttp
  src/mock_server.rs             NEW        std::net MockServer (feature mock-server)
  src/read.rs                    MOD        ReadError implements std::error::Error (Task 12)
  src/provider.rs                MOD        Capabilities, KindTraits, RefreshResult, DeadReason,
                                            TransientKind, CredLocks, LiveLocks split, trait growth
  src/atomic.rs                  MOD        temp-file cleanup guard (L319)
  src/provider.rs Identity       MOD        redacting Debug (L370)
crates/tagteam-cc/
  src/endpoints.rs               NEW        Endpoints (production, with_base), CLIENT_ID
  src/oauth.rs                   NEW        token request/response, profile request/response
  src/lib.rs                     MOD        endpoints and oauth modules
  src/shape.rs                   MOD        access_token, access_expires_at, scopes, apply_refresh
  src/locks.rs                   MOD        credential locks and config lock acquired separately
  src/provider.rs                MOD        new trait methods; ConfigUnsplicable remedy (M-6)
  src/config.rs, src/live.rs     MOD        the unsplicable-config wording moves to CONFIG_REMEDY (M-6)
  src/naming.rs                  MOD        getpwuid_r (L380)
  tests/fixtures/endpoints/*.json NEW       redacted recorded bodies from Task 1
  tests/oauth.rs                 NEW
crates/tagteam-fake/             NEW CRATE  FakeAgent (publish = false)
  Cargo.toml, src/lib.rs, src/paths.rs, src/shape.rs, src/provider.rs, tests/provider.rs,
  tests/network.rs (Tasks 7, 8)
crates/tagteam-engine/
  Cargo.toml                                + ureq; dev: tagteam-fake, provider mock-server
  src/net.rs                     NEW        UreqHttp
  src/oracle.rs                  MOD        HttpOracle, CachingOracle
  src/rescue.rs                  NEW        rescue envelope files
  src/refresh.rs                 NEW        the refresh gate (§7.3), persist_generation
  src/active.rs                  NEW        active-token refresh (§7.5)
  src/quarantine.rs              NEW        quarantine / unquarantine with events (§7.4)
  src/switch.rs                  MOD        lazy rotation walk, freshen, pending rescues
  src/recover.rs                 MOD        forward capture (§9.6), identity-key compare
  src/engine.rs                  MOD        http port, metadata_guard, drop providers() (M-9)
  src/lifecycle.rs               MOD        kind_traits instead of KIND_API_KEY (M-5); L444
  src/store/mod.rs               MOD        quarantine ops, set_login_expires_at, db 0600 (L421)
  src/error.rs                   MOD        NeedsRelogin, RescuePending, UnreadableAccount
  tests/common/mod.rs            MOD        Fx.http, script helpers, FakeFx
  src/testutil.rs                NEW        in-crate test harness `T` for the unit tests (test-only)
  tests/common/mod.rs            MOD        also the shared free helpers: token_requests, due,
                                            quarantine_of, block_rescue, unblock_rescue, credential,
                                            two_accounts, rescue_files, prev_refresh_token
  tests/{net,oracle,rescue_switch,gate,gate_persist,rotation,freshen,active,fake_agent}.rs NEW
crates/tagteam-core/src/rotation.rs MOD     rotation_order replaces next_in_rotation
crates/tagteam-core/src/classify.rs MOD     the Superseded outgoing class (Task 16); L300 test (Task 7)
crates/tagteam/
  Cargo.toml                                + mock-server in test-support and the dev-dependency
  src/app.rs                     MOD        UreqHttp + CachingOracle(HttpOracle); test API base
  src/render.rs                  MOD        kind display and usageStatus via kind_traits; markers
  tests/common/mod.rs            MOD        TAGTEAM_TEST_API_BASE default; shared two_accounts,
                                            expire_vault, live_email
  tests/app.rs, tests/cli.rs     MOD        api_base in the in-process Context; new binary tests
  tests/gate_race.rs             NEW        SIGSTOP single-flight through the binary
```

---

## Interface Contract

Every task implements exactly these names and signatures. A task that finds one unworkable
stops and reports it rather than inventing a variant, because other tasks' code is written
against this list. Doc comments are abbreviated here; the tasks carry the full ones.

### `tagteam-provider`

**`src/http.rs`** (Task 2):

```rust
pub const USER_AGENT: &str = concat!("tagteam/", env!("CARGO_PKG_VERSION"));
pub const MAX_BODY: usize = 1 << 20;

// `pub mod http` and (feature `mock-server`) `pub mod mock_server`; the types below are also
// re-exported at the crate root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Method { Get, Post }
impl Method { pub fn as_str(self) -> &'static str; }       // "GET" | "POST"

#[derive(Clone)]
pub struct HttpRequest {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(&'static str, String)>,
    pub body: Option<Vec<u8>>,
    pub timeout: std::time::Duration,
}
// impl Debug for HttpRequest: method, url, header names; `authorization` value shown as
// "<redacted>", body as "<N bytes>".
impl HttpRequest {
    pub fn get(url: impl Into<String>, timeout: Duration) -> Self;
    pub fn post_json(url: impl Into<String>, body: &serde_json::Value, timeout: Duration) -> Self;
    pub fn bearer(self, token: &str) -> Self;              // adds ("authorization", "Bearer …")
    pub fn header(self, name: &'static str, value: impl Into<String>) -> Self;
}

#[derive(Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,                    // names lowercased
    pub body: Vec<u8>,
}
// impl Debug for HttpResponse: status and "<N bytes>" (bodies can carry tokens).
impl HttpResponse {
    pub fn json_body(status: u16, body: &serde_json::Value) -> Self;   // test and fake convenience
    pub fn header(&self, name: &str) -> Option<&str>;      // case-insensitive
    pub fn json(&self) -> Option<serde_json::Value>;       // None if not valid JSON
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HttpError {
    #[error("the request was never sent: {0}")]
    PreSend(String),
    #[error("the request may have been sent, but no response was read: {0}")]
    Ambiguous(String),
}

pub trait Http: Send + Sync {
    fn send(&self, req: &HttpRequest) -> Result<HttpResponse, HttpError>;
}

/// Never sends anything: every request is `PreSend("network disabled")`.
pub struct NoHttp;

/// A scripted fake for tests: replies are queued per `(method, url)` and served FIFO; the last
/// queued reply for a route repeats once the queue is down to one. An unscripted route is
/// `PreSend("no scripted reply for <METHOD> <url>")`. Every request is recorded.
pub struct ScriptedHttp { /* Mutex-guarded routes and log */ }
#[derive(Clone)]
pub struct RecordedRequest { pub method: Method, pub url: String,
                             pub headers: Vec<(String, String)>, pub body: Option<Vec<u8>> }
// impl Debug for RecordedRequest: redacts like HttpRequest's (recorded headers carry bearer tokens).
impl ScriptedHttp {
    pub fn new() -> Self;
    pub fn push(&self, method: Method, url: &str, reply: Result<HttpResponse, HttpError>);
    pub fn push_json(&self, method: Method, url: &str, status: u16, body: serde_json::Value);
    pub fn requests(&self) -> Vec<RecordedRequest>;
    pub fn count(&self, method: Method, url: &str) -> usize;
    pub fn clear(&self);                                   // drops routes and log
}
impl Default for ScriptedHttp;
```

**`src/mock_server.rs`** (Task 2, feature `mock-server`):

```rust
pub enum MockReply {
    Json { status: u16, body: serde_json::Value },
    Raw { status: u16, headers: Vec<(String, String)>, body: Vec<u8> },
    /// Waits, then replies.
    Delay(std::time::Duration, Box<MockReply>),
    /// Accepts, reads the request, then closes without writing a byte: the client sees EOF or
    /// a reset, which is `Ambiguous` either way.
    Close,
    /// Accepts, reads the request, then never answers (the client times out).
    Hang,
}
#[derive(Debug, Clone)]
pub struct MockRequest { pub method: String, pub path: String,
                         pub headers: Vec<(String, String)>, pub body: Vec<u8> }
pub struct MockServer { /* listener thread, routes, log */ }
impl MockServer {
    pub fn start() -> Self;                                // binds 127.0.0.1:0
    pub fn base_url(&self) -> String;                      // "http://127.0.0.1:<port>"
    pub fn on(&self, method: &str, path: &str, reply: MockReply);   // FIFO; last repeats
    pub fn hits(&self, method: &str, path: &str) -> usize;
    pub fn requests(&self) -> Vec<MockRequest>;
}
// Unrouted requests get 404 {"error":"not_found"}. Drop stops the listener thread.
```

**`src/provider.rs`** additions (Tasks 4, 5, 8):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Capabilities {
    pub usage: bool, pub refresh: bool, pub api_keys: bool, pub sessions: bool, pub statusline: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KindTraits {
    /// The gate may refresh a credential of this kind (§7.3).
    pub refreshable: bool,
    /// Lives on the separate managed-key axis, not in the credential entry (§9.4 step 7).
    pub managed_key_axis: bool,
    /// The prefix of a defaulted `add-token` email (§10.2): `<prefix>-<N>@token.local`.
    pub default_email_prefix: Option<&'static str>,
    /// What `list` prints after the account ("api key"); `None` prints nothing.
    pub display: Option<&'static str>,
}

// DeadReason, TransientKind and RefreshResult are defined by Task 8 (not Task 4), and
// re-exported at the crate root with the other provider types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeadReason { InvalidGrant, NoRefreshToken }
impl DeadReason { pub fn as_str(self) -> &'static str; }   // "invalid_grant" | "no_refresh_token"

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransientKind { PreSend, Ambiguous, Http(u16), BadResponse }
impl TransientKind { pub fn token(&self) -> String; }     // "pre-send" | "ambiguous" | "http-<code>" | "bad-response"

pub enum RefreshResult {
    /// `successor` is the whole new credential in the provider's stored shape; `owner` is the
    /// identity the token response named, if it named one (§7.4 identity_conflict).
    Refreshed { successor: Vec<u8>, owner: Option<Identity> },
    Dead(DeadReason),
    /// The token endpoint refused the request itself: a top-level `invalid_client`, or a 400
    /// `invalid_request_error` (an unknown client id, Appendix A.5). Never a strike (§7.3).
    /// The string is the server's own message, else the error code.
    Systemic(String),
    Transient(TransientKind),
}
// impl Debug for RefreshResult: never prints `successor` bytes.

/// A provider's credential locks (§4.3). Only constructible from a held `MutationGuard`.
pub struct CredLocks<'g> { /* set, PhantomData<&'g MutationGuard> */ }
impl<'g> CredLocks<'g> {
    pub fn new(guard: &'g MutationGuard, set: Box<dyn LiveLockSet + 'g>) -> Self;
    pub fn check_owned(&self) -> Result<(), LockError>;
    /// The config lock can only be added to held credential locks.
    pub fn with_config(self, config: Box<dyn LiveLockSet + 'g>) -> LiveLocks<'g>;
}
/// Credential locks plus the config lock. Fields are declared config-first, so the config lock
/// is released first.
pub struct LiveLocks<'g> { /* config: Option<Box<..>>, cred: CredLocks<'g> */ }
impl<'g> LiveLocks<'g> {
    /// One lock set covering both stages (single-lock providers and tests).
    pub fn new(guard: &'g MutationGuard, set: Box<dyn LiveLockSet + 'g>) -> Self;
    pub fn check_owned(&self) -> Result<(), LockError>;   // credential locks, then config
}

// ProviderError::ConfigUnsplicable becomes a struct variant (M-6):
//   ConfigUnsplicable { path: PathBuf, remedy: &'static str }
//   #[error("{} is torn or not a JSON object; {remedy}", path.display())]
```

`Identity` keeps its fields but gets a hand-written `Debug` (Task 17, L370):
`Identity { label: <redacted>, email: Some(<redacted>) / None, org_uuid, org_name: <redacted>,
account_uuid, raw: <redacted> }`.

**`Provider` trait, final shape after M2a.** Methods marked new are added by the task named.
`write_credential` loses its unused `live` parameter (M-9, Task 4).

```rust
pub trait Provider: Send + Sync {
    fn id(&self) -> ProviderId;
    fn display_name(&self) -> &'static str;
    fn capabilities(&self) -> Capabilities;                                  // new, Task 4
    fn identity_surface(&self, env: &Env) -> IdentitySurface;
    fn identity_key(&self, id: &Identity) -> IdentityKey;
    fn credential_kinds(&self) -> &'static [&'static str];
    fn kind_traits(&self, kind: &str) -> KindTraits;                         // new, Task 4
    fn parse_identity(&self, raw: &Value) -> Result<Identity, ProviderError>;
    fn token_identity(&self, email: &str) -> Identity;
    fn token_secret(&self, token: &str) -> (String, Vec<u8>);

    fn classify(&self, secret: &[u8]) -> String;
    fn fingerprint(&self, secret: &[u8]) -> Option<Fingerprint>;
    fn has_refresh_token(&self, secret: &[u8]) -> bool;
    fn is_wiped(&self, secret: &[u8]) -> bool;
    fn login_expires_at(&self, secret: &[u8]) -> Option<i64>;
    /// Epoch ms of the access token's expiry; `None` when absent or non-numeric (§7.2).
    fn access_expires_at(&self, secret: &[u8]) -> Option<i64>;              // new, Task 4
    /// The access token's own fingerprint (gate step 4; M2b's `rejected_fp`).
    fn access_fingerprint(&self, secret: &[u8]) -> Option<Fingerprint>;     // new, Task 4

    fn live_identity(&self, env: &Env) -> Read<Identity>;
    fn read_live_auth(&self, env: &Env) -> LiveAuth;
    /// How long the live locks may take, all stages together (§9.1: CC 9 s).
    fn live_lock_budget(&self) -> Duration;                                  // new, Task 5
    fn lock_credentials<'g>(&self, env: &Env, g: &'g MutationGuard, budget: Duration)
        -> Result<CredLocks<'g>, ProviderError>;                             // new, Task 5
    fn lock_config<'g>(&self, env: &Env, cred: CredLocks<'g>, budget: Duration)
        -> Result<LiveLocks<'g>, ProviderError>;                             // new, Task 5
    /// Both stages under one budget (§9.1). Provided; providers do not override it.
    fn lock_live<'g>(&self, env: &Env, g: &'g MutationGuard)
        -> Result<LiveLocks<'g>, ProviderError> { /* default, Task 5 */ }
    fn doomed(&self, env: &Env, locks: &LiveLocks<'_>, change: LiveChange<'_>) -> Vec<DoomedEntry>;
    fn write_credential<'l>(&self, env: &Env, locks: &'l LiveLocks<'_>, target: &StoredLogin,
        before_fallback: BeforeFallback<'_>) -> Result<Written<'l>, ProviderError>;   // Task 4 drops `live`
    fn clear_other_axis<'l>(&self, env: &Env, locks: &'l LiveLocks<'_>, kept_kind: &str)
        -> Result<Box<dyn Undo + 'l>, ProviderError>;
    fn write_identity<'l>(&self, env: &Env, locks: &'l LiveLocks<'_>, identity: Option<&Identity>)
        -> Result<Box<dyn Undo + 'l>, ProviderError>;

    /// §7.6. `None`, with no request, when the credential has no token it can show:
    /// expired (`now_ms + 5 min ≥ expiresAt`), or a kind that cannot be resolved.
    fn resolve_owner(&self, http: &dyn Http, cred: &Credential, now_ms: i64)
        -> Option<Identity>;                                                  // new, Task 7
    /// §7.3 steps 5 and 7 (the provider's half). `now_ms` stamps the successor's expiry.
    /// A credential without a refresh token is `Dead(NoRefreshToken)` with no request.
    fn refresh(&self, http: &dyn Http, cred: &FreshCredential, now_ms: i64,
        timeout: Duration) -> RefreshResult;                                  // new, Task 8
}
```

### `tagteam-cc`

```rust
// src/endpoints.rs (Task 7)
pub const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints { pub token: String, pub profile: String, pub usage: String }
impl Endpoints {
    pub fn production() -> Self;   // https://platform.claude.com/v1/oauth/token, https://api.anthropic.com/api/oauth/{profile,usage}
    pub fn with_base(base: &str) -> Self;   // "<base>/v1/oauth/token", "<base>/api/oauth/profile", "<base>/api/oauth/usage"
}
// ClaudeCode::with_endpoints(self, Endpoints) -> Self    (always compiled; production code
// never calls it outside the CLI's test-support override)

// src/shape.rs additions (Task 4)
pub fn access_token(bytes: &[u8]) -> Option<String>;
pub fn access_expires_at(bytes: &[u8]) -> Option<i64>;      // claudeAiOauth.expiresAt as i64
pub fn scopes(bytes: &[u8]) -> Vec<String>;
pub fn is_expired(bytes: &[u8], now_ms: i64) -> bool;         // §7.2: now + 300_000 >= expiresAt
/// The token fields of a refresh response applied to the stored credential; every other key kept.
pub struct TokenFields { pub access_token: String, pub refresh_token: Option<String>,
    pub expires_at: i64, pub scopes: Option<Vec<String>>, pub refresh_token_expires_at: Option<i64> }
pub fn apply_refresh(old: &[u8], f: &TokenFields) -> Result<Vec<u8>, ProviderError>;   // Task 8

// src/oauth.rs (Tasks 7, 8)
pub const TOKEN_TIMEOUT: Duration = Duration::from_secs(10);      // gate default
pub const PROFILE_TIMEOUT: Duration = Duration::from_secs(5);
pub fn profile_request(e: &Endpoints, access_token: &str) -> HttpRequest;
pub fn parse_profile(resp: &HttpResponse) -> Option<Identity>;   // needs a non-empty account.uuid
pub fn refresh_request(e: &Endpoints, refresh_token: &str, scopes: &[String], timeout: Duration)
    -> HttpRequest;
pub fn parse_refresh(old: &[u8], reply: Result<HttpResponse, HttpError>, now_ms: i64) -> RefreshResult;

// src/locks.rs (Task 5): CcCredSet and CcConfigSet replace CcLockSet
pub fn acquire_credentials(paths: &CcPaths, timeout: Duration) -> Result<CcCredSet, LockError>;
pub fn acquire_config(paths: &CcPaths, timeout: Duration) -> Result<CcConfigSet, LockError>;

// src/provider.rs (Task 4): the remedy text moves here (M-6)
pub const CONFIG_REMEDY: &str =
    "restore it from Claude Code's backups (~/.claude/backups/) or repair it, then retry";
```

Claude Code's kind traits (Task 4):

| kind | refreshable | managed_key_axis | default_email_prefix | display |
|---|---|---|---|---|
| `oauth` | true | false | None | None |
| `setup_token` | false | false | `Some("setup-token")` | `Some("setup token")` |
| `api_key` | false | true | `Some("api-key")` | `Some("api key")` |

Claude Code's capabilities: `usage`, `refresh`, `api_keys`, `sessions` and `statusline` are
all `true`.

**Recorded fixtures** (Task 1): `crates/tagteam-cc/tests/fixtures/endpoints/<name>.json`, each
`{"status": <u16>, "headers": {"content-type": …, "retry-after": …?}, "body": <redacted JSON>,
"synthetic": <bool>}`.
- Recorded by the probe, then scrubbed (fixed fakes for ids, names, creation times and request
  ids; the other timestamps shifted, keeping their exact format): `profile-200`, `usage-200`,
  `token-invalid-grant`, `token-invalid-client`. `token-invalid-client` is the endpoint's real
  answer to an unknown client id: a 400 with a nested `error.type` of `invalid_request_error`,
  not RFC 6749's `invalid_client`.
- Hand-built from Appendix A.5, since a successful refresh can't be recorded without spending
  a real token: `token-200`, with `"synthetic": true`.

Tests load them with `include_str!`. When a recorded shape disagrees with Appendix A.5,
Task 1's decision gate stops the plan.

**`AccountView`** (Task 4) gains `pub kind: KindTraits`, filled in by the engine's views, so the
CLI renders kinds without naming any provider's kind strings.

### `tagteam-fake` (Task 6)

```rust
pub const FAKE_AGENT: &str = "fake-agent";
pub const KIND_TOKEN: &str = "fa_token";     // refreshable; carries a renew token
pub const KIND_STATIC: &str = "fa_static";   // not refreshable, no managed-key axis
pub struct FakeAgent { /* endpoint base, lock budget */ }
impl FakeAgent {
    pub fn new() -> Self;                                  // base "https://fake-agent.invalid"
    pub fn with_endpoint_base(self, base: &str) -> Self;
    pub fn with_lock_budget(self, budget: Duration) -> Self;
    pub fn renew_url(&self) -> String;                     // "<base>/fa/renew"
    pub fn whoami_url(&self) -> String;                    // "<base>/fa/whoami"
}
pub struct FakePaths { pub dir: PathBuf, pub identity: PathBuf, pub credential: PathBuf, pub lock: PathBuf }
impl FakePaths { pub fn resolve(env: &Env) -> Self; }    // <home>/.fakeagent/{identity.json,credential.json,.live.lock}
// Test helpers (pub, used by engine tests):
pub fn credential_json(token: &str, renew: Option<&str>, expires: Option<i64>) -> Value;
     // {"fa":{"token":…,"renew":…,"expires":…},"device":{"id":"machine-shared"}}
pub fn identity_json(handle: &str, workspace: &str, uid: &str) -> Value;
     // {"handle":…,"workspace":…,"uid":…}
pub fn login(env: &Env, handle: &str, workspace: &str, token: &str, renew: &str);
     // writes identity.json as {"identity": <identity_json>, "prefs": {"theme": "x"}} and credential.json
```

`FakeAgent` identities have no email: `label` is `handle@workspace`, `email` is `None`,
`org_uuid` is the workspace, `account_uuid` is `uid`, and the identity key is
`handle\nworkspace`. Its only machine-shared key is `device`. Capabilities: only `refresh` is
`true`. Kind traits: `fa_token` refreshable; `fa_static`
`default_email_prefix: Some("fa-static")`, `display: Some("static")`. Further exports: `KINDS`,
`DEVICE`, `LOGIN_EXPIRES`. The default lock budget is 5 s, and `FakeFx` uses 2 s.

### `tagteam-engine`

```rust
// src/net.rs (Task 3)
/// The host lookup that decides whether a DNS failure is `PreSend`; injectable so tests never
/// ask a real resolver (§15.1).
pub type Resolver = fn(&str, u16) -> io::Result<Vec<SocketAddr>>;
pub struct UreqHttp { /* ureq::Agent, Resolver */ }
impl UreqHttp { pub fn new() -> Self; pub fn with_resolver(resolver: Resolver) -> Self; }
impl Http for UreqHttp;
impl Default for UreqHttp;
// UreqHttp resolves the host itself (through its Resolver; `new()` uses the system one) before
// calling ureq, so a DNS failure is a certain `PreSend` (ureq reports it as a plain I/O error).
// It never follows redirects (`max_redirects(0)`), so a token goes only where the provider
// sent it.

// src/engine.rs (Task 3): EngineConfig and Engine gain the port
pub struct EngineConfig { /* existing fields */ pub http: Arc<dyn Http> }
// Engine: pub(crate) http: Arc<dyn Http>, and `pub fn http(&self) -> &dyn Http`;
// `providers()` is removed (M-9, Task 4)
// Task 4: `pub fn account_view(&self, row: AccountRow, active: bool) -> AccountView` (views.rs),
//         so the CLI builds rows without naming kind strings; an unregistered provider's row
//         gets all-false traits.
// Task 7: fn guard_recovering(&self, ask_oracle: bool); pub(crate) fn metadata_guard(&self)
//         -> Result<MutationGuard, EngineError>  (recovery without the oracle)

// src/oracle.rs (Task 7)
pub struct HttpOracle { /* http: Arc<dyn Http>, clock: Arc<dyn Clock> */ }
impl HttpOracle { pub fn new(http: Arc<dyn Http>, clock: Arc<dyn Clock>) -> Self; }
impl Oracle for HttpOracle;
/// Asks `inner` at most once per process for a given (provider, fingerprint) (§7.6).
pub struct CachingOracle<O: Oracle> { /* inner, Mutex<HashMap<(String, String), Option<Identity>>> */ }
impl<O: Oracle> CachingOracle<O> { pub fn new(inner: O) -> Self; }
impl<O: Oracle> Oracle for CachingOracle<O>;

// src/quarantine.rs (Task 9, which declares `pub mod quarantine`)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuarantineReason { InvalidGrant, NoRefreshToken, IdentityConflict, SuccessorLost }
impl QuarantineReason { pub fn as_str(self) -> &'static str; pub fn parse(s: &str) -> Option<Self>; }
impl From<DeadReason> for QuarantineReason;
impl Engine {
    /// Sets the quarantine bound to `fp` and records a `quarantine` event (§7.4).
    pub(crate) fn quarantine(&self, row: &AccountRow, reason: QuarantineReason, fp: &str) -> Result<(), EngineError>;
    /// Clears it and records `unquarantine`; `false` when there was none.
    pub(crate) fn unquarantine(&self, row: &AccountRow) -> Result<bool, EngineError>;
}

// src/rescue.rs (Task 9)
pub(crate) struct RescueEntry { pub path: PathBuf, pub predecessor_fp: String,
    pub credential: Vec<u8> }   // no Debug; the parser still validates accountId and loginEpoch
pub(crate) enum RescueFile { Entry(RescueEntry), Unreadable { path: PathBuf, detail: String } }
impl Engine {
    pub(crate) fn write_rescue(&self, id: &AccountId, login_epoch: i64, predecessor_fp: &str,
        successor: &[u8], successor_fp: &Fingerprint) -> Result<PathBuf, EngineError>;
    pub(crate) fn rescues_for(&self, id: &AccountId) -> Vec<RescueFile>;   // never creates rescue/
    pub(crate) fn delete_rescue(&self, path: &Path) -> Result<(), EngineError>;
    /// §6.2 "Pending rescues before activation", and §7.3 step 3's adoption: adopts a rescue
    /// whose predecessor is the vault's current generation (vault write, verify, delete);
    /// `RescuePending` when a rescue for the account is unreadable or cannot be adopted. A
    /// rescue whose predecessor is any other generation is superseded and left alone. The
    /// caller holds `lock`. Task 9 defines it; the gate (Task 10) and the switch (Task 13) call it.
    pub(crate) fn settle_rescues(&self, p: &dyn Provider, row: &AccountRow, lock: &AccountLock)
        -> Result<(), EngineError>;
}

// src/refresh.rs (Tasks 9–11; Task 9 declares `pub mod refresh`)
pub const GATE_TIMEOUT: Duration = Duration::from_secs(10);   // the engine never names CC's constants
/// §7.2's expiry buffer and test, shared by the gate and active-token refresh (Tasks 10, 16).
pub(crate) const EXPIRY_BUFFER_MS: i64 = 5 * 60 * 1000;
pub(crate) fn expired(p: &dyn Provider, bytes: &[u8], now_ms: i64) -> bool;
impl Engine {
    /// Writes a new generation under `lock` (§6.2): `.prev` rotates only on a lineage change;
    /// records `login_expires_at`; clears a quarantine when the fingerprint changed (§7.4).
    pub(crate) fn persist_generation(&self, p: &dyn Provider, row: &AccountRow,
        lock: &AccountLock, bytes: &[u8]) -> Result<(), EngineError>;          // Task 9
    /// §7.3. `snapshot` is the vault bytes the caller decided on. `pub` so integration tests
    /// reach it.
    pub fn refresh_stored(&self, p: &dyn Provider, id: &AccountId, snapshot: &[u8])
        -> Result<GateOutcome, EngineError>;                                   // Tasks 10–11
    /// The vault, else `rescue/`, else `Unpersisted`, which is not yet a loss: the caller
    /// decides (§7.3 step 6, §7.5 step 5). Shared by both refresh paths.        // Task 11
    pub(crate) fn persist_received(&self, p: &dyn Provider, row: &AccountRow,
        lock: &AccountLock, received: &mut Received<'_>) -> Persisted;
    /// An error after receipt: keep the successor (`Kept(cause)`), else record the loss
    /// (`Lost`; the caller reports `Unpersisted`, never the error).             // Task 11
    pub(crate) fn abandon(&self, row: &AccountRow, sent_fp: &str,
        received: &mut Received<'_>, cause: EngineError) -> Abandoned;
    /// §7.4 `successor_lost`: log at ERROR and quarantine bound to `sent_fp`, best effort.
    /// Never panics (called from `Received::drop` while unwinding).             // Task 11
    pub(crate) fn record_loss(&self, row: &AccountRow, sent_fp: &str,
        cause: &dyn std::fmt::Display);
    pub(crate) fn quarantine_best_effort(&self, row: &AccountRow, reason: QuarantineReason,
        fp: &str);                                                              // Task 11
}
pub(crate) enum Abandoned { Kept(EngineError), Lost }                           // Task 11
/// A successor that belongs to another account: displaced, the account quarantined (§7.4).
pub(crate) enum Displacement { Kept, Lost }                                     // Task 11
impl Engine {
    pub(crate) fn displace_received(&self, row: &AccountRow, sent_fp: &str,
        received: &mut Received<'_>) -> Result<Displacement, EngineError>;      // Task 11
}
pub(crate) fn log_lost(row: &AccountRow, cause: &dyn std::fmt::Display);        // Task 11
/// §7.4, Task 10: the token response named another account (a non-empty uuid that differs
/// from a known one) or another organization (both non-empty and different); either alone.
pub(crate) fn names_another_account(owner: &Identity, row: &AccountRow) -> bool;
impl Engine {
    /// A successor that belongs to another account, to `displaced/` (reason
    /// `identity-conflict`, naming the response's owner).                   // Task 10
    pub(crate) fn keep_foreign(&self, row: &AccountRow, successor: &[u8],
        fp: Option<&Fingerprint>, owner: &Identity) -> Result<(), EngineError>;
}
/// A received successor not yet persisted; dropped while armed (a panic) it keeps itself,
/// or records the loss (`record_loss`; `identity_conflict` for a foreign one). Its `Drop`
/// never panics, and `Engine::store`'s slot lock tolerates poisoning so it cannot. No `Debug`.
///                                                                              // Task 11
pub(crate) struct Received<'e> { /* … */ }
impl<'e> Received<'e> {
    /// `foreign`: the response's owner when it names another account (§7.4); fixed at receipt.
    pub(crate) fn new(engine: &'e Engine, p: &dyn Provider, row: &AccountRow,
        predecessor_fp: &str, bytes: Vec<u8>, foreign: Option<Identity>) -> Self;
    pub(crate) fn is_foreign(&self) -> bool;
    /// `rescue/`, or `displaced/` for a foreign successor; disarms whatever the result.
    pub(crate) fn keep(&mut self) -> Result<(), EngineError>;
    pub(crate) fn bytes(&self) -> &[u8];                                     // Task 16
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Persisted { Vault, Rescued, Unpersisted }                      // Task 11
// An identity-conflicting successor is displaced, never written to the vault or rescue/, and
// the identity_conflict quarantine is bound to the SENT fingerprint (the vault still holds it)
// (§7.3 step 6, §7.4). Checked before any persistence, on both refresh paths.
// Unpersisted also quarantines the account `successor_lost`, bound to the sent fingerprint,
// best effort (§7.4); not when the vault had moved to a newer generation meanwhile.
// Any error after receipt keeps the successor explicitly and returns the error, or returns
// Unpersisted when keeping it fails (never the error).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnedBy { Live, Journal, Session }
pub enum GateOutcome {
    Refreshed(Vec<u8>),          // the vault now holds it
    AlreadyFresh(Vec<u8>),       // step 4
    Busy,
    Owned(OwnedBy),
    Conflict,                    // provenance conflict (M4; never produced in M2a)
    Dead(QuarantineReason),      // quarantined, now or before
    Systemic(String),
    Transient { kind: String, rescued: bool },
    Unpersisted,                 // every write failed; the account is quarantined successor_lost
}
// impl Debug for GateOutcome: never prints credential bytes.

// src/active.rs (Task 16)
pub const ACTIVE_REFRESH_TIMEOUT: Duration = Duration::from_secs(6);
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActiveTrigger { Expired, Rejected { access_fp: String } }
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActiveOutcome {
    /// Reconciliation left a live token that is neither expired nor the rejected one.
    NotNeeded { reconciled: bool },
    /// The successor is in tagteam's storage and in the live store.
    Refreshed,
    /// Persisted to the vault or `rescue/`, but not published to the live store: CC's locks
    /// were compromised when the response arrived, or a self-heal (or the live write) could not
    /// take the config lock. Nothing further is sent; the next pass reconciles (§7.5 step 5).
    PersistedNotPublished,
    /// The vault and `rescue/` both failed, but the live store holds the successor: nothing
    /// is lost, nothing is quarantined, and the next pass adopts it into the vault.
    PublishedOnly,
    Dead(QuarantineReason),      // incl. a quarantine that still holds (§7.4): no request sent
    Systemic(String),
    Transient { kind: String },
    /// Held nowhere (vault, `rescue/` and live store all failed or were skipped): logged at
    /// ERROR and quarantined `successor_lost` (§7.5 step 5). Takes precedence over a conflict
    /// or a compromised lock.
    Unpersisted,
}
impl Engine {
    /// Unreadable vault, `.prev` or rescue reads refuse before any write or request. A pending
    /// self-heal is published before the lineage advances (a second pass then refreshes). A
    /// successor that belongs to another account is displaced, never stored or published.
    pub fn refresh_active(&self, provider: &ProviderId, trigger: ActiveTrigger)
        -> Result<ActiveOutcome, EngineError>;
}

// src/error.rs additions
EngineError::NeedsRelogin { position: u32, label: String }            // kind "relogin-required"   (Task 14)
//   "{label} (position {position}) needs a new login: its stored refresh token can no longer be used; log in with `claude`, then run `tagteam add`"
//   Also the refusal for a gate `Unpersisted`: the successor is lost and the vault's generation consumed.
EngineError::RescuePending { position: u32, label: String, detail: String }   // kind "rescue-pending" (Task 9)
//   "{label} (position {position}) has a refreshed token that is not in the vault yet: {detail}; retry once the vault can be written"
//   `detail` names the rescue file for an unreadable or unparseable one.
EngineError::UnreadableAccount { position: u32, label: String, source: ReadError }   // kind "unreadable" (Task 12)
//   "the stored credential for {label} (position {position}) is unreadable: {source}"
//   Task 12 adds `impl std::error::Error for ReadError` (thiserror treats `source` as the cause).
EngineError::ForeignLiveCredential { position: u32 }                   // kind "foreign-credential" (Task 16)

// src/store/mod.rs additions (Task 9)
pub fn set_quarantine(&self, id: &AccountId, reason: &str, fp: &str, at: i64) -> Result<(), StoreError>;
pub fn clear_quarantine(&self, id: &AccountId) -> Result<bool, StoreError>;   // true if one was set
pub fn set_login_expires_at(&self, id: &AccountId, at: Option<i64>) -> Result<(), StoreError>;
// AccountRow gains `quarantine_at: Option<i64>` (ACCOUNT_COLUMNS includes it).
```

**`tagteam-core`** (Task 12): `next_in_rotation` is replaced by

```rust
/// §9.3: the positions a rotation tries, in order. With an anchor, every position after it,
/// wrapping, and never the anchor itself; with none, every position from the first.
pub fn rotation_order(positions: &[u32], anchor: Option<u32>) -> Vec<u32>;
```

and (Task 16, §9.4 step 4 as amended) the outgoing classification gains a class, decided
right after `Ours`:

```rust
pub enum OutgoingClass { Ours, Superseded, Wiped, OursRotated, Foreign, Unresolved }
pub struct OutgoingFacts { /* M1 fields */ pub equals_vault_prev: bool }
// equals_vault_prev → (Superseded, Nothing): the live credential is the vault's `.prev`, a
// generation an active-token refresh superseded without publishing. `settle_outgoing` reads
// `.prev` (tri-state; Unreadable aborts) only when the live credential is not the vault's own.
```

### Engine test fixture (`crates/tagteam-engine/tests/common/mod.rs`)

- `Fx.http: Arc<ScriptedHttp>`, passed as `EngineConfig.http` (Task 3). Every `engine_over`
  engine shares it.
- `Fx::endpoints() -> tagteam_cc::endpoints::Endpoints`: `Endpoints::production()` (Task 7).
- `Fx::script_profile(&self, email: &str)`: queues a 200 profile reply for `email`. The body is
  the recorded fixture `profile-200.json` with its account fields replaced to match
  `Fx::oauth_account(email)`: `uuid-{email}`, `email`, and `organizationUuid` `""` (Task 7).
- `Fx::script_refresh(&self, new_rt: Option<&str>)`: queues a 200 token reply with
  `access_token: "at-<new_rt or 'same'>"`, `expires_in: 28800`, and `refresh_token: new_rt` when
  `Some` (Task 8).
- `Fx::script_token_error(&self, status: u16, error: &str)`: queues `{"error": error}` (Task 8).
- `Fx::expire_access(&self, id: &AccountId)`: rewrites the vault credential's
  `claudeAiOauth.expiresAt` to `fx.clock.now_ms() + 60_000`, which is inside the 10-minute
  freshen window (Task 10).
- `Fx::quarantine(&self, id, reason, fp)`: now calls `store.set_quarantine` (Task 9).
- `Fx::put_vault(&self, id, bytes)` (Task 10); `Fx::engine_with_vault(&self, vault) -> Engine`
  (Task 11); `Fx::plant_rescue(&self, id, predecessor_fp: &str, successor: &[u8]) -> PathBuf`
  writes a §6.3 envelope (Task 13; Task 16 reuses it).
- `Fx::assert_only_surface_changed_for(&IdentitySurface, …)` (Task 6); the existing CC method
  delegates to it.
- `FakeFx` (Task 6): one engine with both `ClaudeCode` and `FakeAgent` registered over one
  Env, `ScriptedHttp` and store. Its helpers are `fake_login(handle, token, renew)`,
  `fake_add(handle, token, renew) -> AccountId`, `fake_provider`, `fake_add_options`,
  `switch_fake` and `fake_live_label`.

**CLI test harness** (Task 7). `Context` gains `api_base: Option<String>`, which the
test-support override `TAGTEAM_TEST_API_BASE` sets. `common::std_cmd` defaults it to
`http://127.0.0.1:9`, and the in-process `app.rs` tests build their `Context` the same way, so
no CLI test reaches the network.

---

## Tasks

| # | Task | Human? |
|---|---|---|
| 1 | Live endpoint probe and recorded fixtures | yes: decision gate |
| 2 | The `Http` port, `ScriptedHttp`, the mock server, and `!*.sql` | |
| 3 | The `ureq` adapter and the engine's `http` port | |
| 4 | Trait cleanup: capabilities, kind traits, access-token facts (M-5, M-6, M-9) | |
| 5 | The live-lock split | |
| 6 | The `FakeAgent` crate and provider-neutrality tests | |
| 7 | The HTTP oracle, its limits, and offline metadata commands | |
| 8 | Token refresh request and response (Claude Code and `FakeAgent`) | |
| 9 | Quarantine, rescue files, and `persist_generation` | |
| 10 | The refresh gate: lock, ownership, adoption, request, verdict | |
| 11 | Gate persistence: compare-and-swap, rescue, `Unpersisted`, the pinned invariant | |
| 12 | Rotation: lazy walk, quarantined accounts skipped, unreadable account named | |
| 13 | Pending rescues before activation | |
| 14 | Freshen before activation | |
| 15 | Recovery: forward capture and identity-key comparison | |
| 16 | Active-token refresh | |
| 17 | Carried-over fixes (L319, L370, L380, L421, L444, L360) | |
| 18 | The CLI: wiring, errors, markers, and the single-flight race through the binary | |
| 19 | Final verification and live acceptance | yes: acceptance run |


### Task 1: Live endpoint probe and recorded fixtures

**Human step.** The implementer writes the script and the synthetic fixture; Michael runs the
probe on his Mac against his real Claude Code login, reviews every recorded file, and commits.
**Decision gate:** if a recorded shape disagrees with Appendix A.5 (the profile lacks
`account.uuid` or `account.email`, its `organization` is present but not `null` and carries no
string `uuid`, the `invalid_grant` reply lacks a top-level `error == "invalid_grant"`, or the
unknown-client reply is neither a top-level `error == "invalid_client"` nor a 400 whose nested
`error.type` is `invalid_request_error`), stop and bring the redacted recordings to Michael
before any further task — Tasks 7 and 8 parse exactly those shapes.

**Where this stands.** Steps 1–5 ran on 2026-09-30 with Claude Code 2.1.285. The probe was
committed at `d1dfd1d` and refined in `450c102`. The gate passed on the second shape: the
endpoint answers an unknown client id with a 400 carrying a nested `invalid_request_error`, not
RFC 6749's `invalid_client`, so the spec was amended (`7e96d7c`: §7.3 step 7 and Appendix A.5)
to classify both shapes as systemic. The run's recordings sit uncommitted in the working tree,
and their review found real values the redaction did not cover: creation times, request ids and
the usage timestamps. Michael ruled to scrub them in place rather than re-run the probe. The
script in Step 1 is the committed probe plus that scrubbing, the two-shape gate and a `rescrub`
subcommand. Step 7 applies it, Step 8 rescrubs the recordings, and Step 9 commits them.

What the probe spends, and why it is safe:
- **Profile and usage:** two read-only GETs with the live access token, as Claude Code itself
  makes them. The usage call spends one request of that identity's hourly budget (§8.6).
- **Token endpoint:** two POSTs that carry a made-up refresh token, so no real refresh token is
  ever sent. The first uses the real client id (expected: `invalid_grant`), the second a
  made-up client id (expected: a client refusal, in either of the two shapes above; the
  2026-09-30 run recorded the nested one).
- **No token is ever printed, logged or put in argv.** The access token lives only in a shell
  variable and reaches curl through `--config -` on stdin, never on the command line (`ps`
  would show it). Bodies are redacted before they are written to disk or shown.
- **Redaction keeps every key, type and format.** Emails, uuids, tokens, request ids and
  account/organization names become numbered fakes. `created_at` and
  `subscription_created_at` become `2020-01-01T00:00:00` plus zeros in the same fractional
  digits and the same offset. Every other ISO timestamp moves by one whole-second delta that
  puts the earliest at `2030-01-01T00:00:00`, keeping order, spacing, fractional digits and the
  `+00:00` style, so M2b's parser still sees the real formats. Plan tier and amounts stay.
- A successful refresh cannot be recorded without spending a real refresh token, so
  `token-200.json` is hand-built from Appendix A.5 and marked `"synthetic": true`.

**Files:**
- Create: `scripts/spikes/m2a-endpoints.sh`
- Create: `crates/tagteam-cc/tests/fixtures/endpoints/token-200.json` (synthetic)
- Create (by the probe run): `crates/tagteam-cc/tests/fixtures/endpoints/profile-200.json`,
  `usage-200.json`, `token-invalid-grant.json`, `token-invalid-client.json`
- Modify: `scripts/spikes/m2a-endpoints.sh` (Step 7: scrubbing, the two-shape gate, `rescrub`)
- Modify: `docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md` (§7.3 step 7 and
  Appendix A.5's verification line; already committed in `7e96d7c`)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: the fixture envelope from the Interface Contract —
  `{"status": <u16>, "headers": {"content-type": …, "retry-after": …?}, "body": <redacted JSON>, "synthetic": <bool>}`
  — at `crates/tagteam-cc/tests/fixtures/endpoints/<name>.json` for `profile-200`,
  `usage-200`, `token-invalid-grant`, `token-invalid-client` and `token-200`. Tasks 7 and 8 load
  them with `include_str!`. `token-invalid-client` is the nested shape
  (`{"type": "error", "error": {"type": "invalid_request_error", "message": …}, "request_id": …}`).

- [ ] **Step 1: Write the probe script**

Create `scripts/spikes/m2a-endpoints.sh`:

```bash
#!/usr/bin/env bash
# M2a Task 1 probe (spec Appendix A.5): records what Claude Code's profile, usage and token
# endpoints actually return, so tagteam's mock fixtures match reality.
#
# Safe against the real account: the profile and usage calls are read-only GETs with the live
# access token, as Claude Code makes them (the usage call spends one request of the hourly
# budget); the two token calls send a made-up refresh token, so no real refresh token is ever
# spent. No token is ever printed, logged or put on a command line: the access token reaches
# curl through `--config -` on stdin, and every body is redacted before it is written or shown.
set -euo pipefail

OUT=${TAGTEAM_PROBE_OUT:-crates/tagteam-cc/tests/fixtures/endpoints}
SECURITY=/usr/bin/security
PY=/usr/bin/python3
UA="tagteam/0.1.0"
CLIENT_ID=9d1c250a-e61b-44d9-88ed-5944d1962f5e
FAKE_CLIENT_ID=00000000-0000-4000-8000-0000000000ff
FAKE_RT=sk-ant-ort01-tagteam-probe-not-a-real-refresh-token
TOKEN_URL=https://platform.claude.com/v1/oauth/token
PROFILE_URL=https://api.anthropic.com/api/oauth/profile
USAGE_URL=https://api.anthropic.com/api/oauth/usage

WORK=$(mktemp -d "${TMPDIR:-/tmp}/tagteam-probe.XXXXXX")
chmod 700 "$WORK"
trap 'rm -rf "$WORK"' EXIT

# The redaction, shared by `record` (fresh responses) and `rescrub` (recordings on disk).
# Deterministic, and it keeps every key, type, array length and string format.
export PYTHONPATH="$WORK" PYTHONDONTWRITEBYTECODE=1
cat > "$WORK/redactor.py" <<'PYEOF'
import re
from datetime import datetime

EMAIL = re.compile(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}")
UUID = re.compile(r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}")
ISO = re.compile(r"(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(\.\d+)?(Z|[+-]\d{2}:\d{2})")
NAME_KEYS = {"name", "full_name", "display_name", "organization_name"}
SHIFT_BASE = datetime(2030, 1, 1)
fakes = {"email": {}, "uuid": {}, "token": {}, "name": {}, "request": {}}

def fake(kind, value):
    table = fakes[kind]
    if value not in table:
        n = len(table) + 1
        table[value] = {
            "email": f"probe{n}@example.com",
            "uuid": f"00000000-0000-4000-8000-{n:012d}",
            "token": f"redacted-token-{n}",
            "name": f"Probe Name {n}",
            "request": "req_" + str(n).zfill(max(len(value) - 4, 1)),
        }[kind]
    return table[value]

def is_creation(key):
    return key.lower().endswith("created_at")

def wall(m):
    return datetime(*(int(g) for g in m.groups()[:6]))

def redact(v, key="", path=()):
    if isinstance(v, dict):
        return {k: redact(x, k, path + (k,)) for k, x in v.items()}
    if isinstance(v, list):
        return [redact(x, key, path) for x in v]
    if not isinstance(v, str):
        return v
    k = key.lower()
    m = ISO.fullmatch(v)
    if m and is_creation(k):
        # A creation time identifies the account: a fixed fake, same fractional digits and offset.
        return "2020-01-01T00:00:00" + re.sub(r"\d", "0", m.group(7) or "") + m.group(8)
    if k == "request_id":
        return fake("request", v)
    if EMAIL.fullmatch(v) or "email" in k:
        return fake("email", v)
    owned = any(p in ("account", "organization", "user") for p in path)
    if UUID.fullmatch(v) or k.endswith("uuid") or (k == "id" and owned):
        return fake("uuid", v)
    if v.startswith("sk-ant-") or "token" in k or "secret" in k:
        return fake("token", v)
    if k in NAME_KEYS and owned:
        return fake("name", v)
    v = EMAIL.sub(lambda m: fake("email", m.group(0)), v)
    return UUID.sub(lambda m: fake("uuid", m.group(0)), v)

def shift_times(body):
    # Every other ISO timestamp moves by one whole-second delta that puts the earliest at
    # 2030-01-01T00:00:00: order and spacing survive, and so do the fractional digits and the
    # offset, so a parser sees the real formats. Scrubbing a scrubbed body moves nothing.
    seen = []
    def collect(v, key=""):
        if isinstance(v, dict):
            for k, x in v.items():
                collect(x, k)
        elif isinstance(v, list):
            for x in v:
                collect(x, key)
        elif isinstance(v, str) and not is_creation(key):
            m = ISO.fullmatch(v)
            if m:
                seen.append(wall(m))
    collect(body)
    if not seen:
        return body
    delta = SHIFT_BASE - min(seen)
    def move(v, key=""):
        if isinstance(v, dict):
            return {k: move(x, k) for k, x in v.items()}
        if isinstance(v, list):
            return [move(x, key) for x in v]
        m = ISO.fullmatch(v) if isinstance(v, str) else None
        if m and not is_creation(key):
            return (wall(m) + delta).strftime("%Y-%m-%dT%H:%M:%S") + (m.group(7) or "") + m.group(8)
        return v
    return move(body)

def scrub(body):
    return shift_times(redact(body))
PYEOF

acct() {
  local u="${USER:-}"
  [[ -n "$u" ]] || u=$(id -un 2>/dev/null || true)
  if [[ "$u" =~ ^[a-zA-Z0-9._-]+$ ]]; then printf '%s' "$u"; else printf 'claude-code-user'; fi
}
# Appendix A.2: CLAUDE_SECURESTORAGE_CONFIG_DIR (defined and non-empty), else CLAUDE_CONFIG_DIR,
# suffixes the item with the first 8 hex digits of its sha256; unset means the default item.
# TAGTEAM_PROBE_SERVICE overrides the whole name.
svc() {
  if [[ -n "${TAGTEAM_PROBE_SERVICE:-}" ]]; then printf '%s' "$TAGTEAM_PROBE_SERVICE"; return; fi
  local dir=""
  if [[ -n "${CLAUDE_SECURESTORAGE_CONFIG_DIR+x}" ]]; then
    dir="${CLAUDE_SECURESTORAGE_CONFIG_DIR}"
  else
    dir="${CLAUDE_CONFIG_DIR:-}"
  fi
  if [[ -z "$dir" ]]; then
    printf 'Claude Code-credentials'
  else
    printf 'Claude Code-credentials-%s' "$(printf '%s' "$dir" | shasum -a 256 | cut -c1-8)"
  fi
}

# The live credential JSON on stdout, captured by the caller into a variable, never shown.
live_credential() {
  if [[ "$(uname -s)" == Darwin ]]; then
    "$SECURITY" find-generic-password -a "$(acct)" -s "$(svc)" -w
  else
    cat "${CLAUDE_CONFIG_DIR:-$HOME/.claude}/.credentials.json"
  fi
}

# The access token, from the credential JSON on stdin. `security -w` renders a secret with
# non-printable bytes as hex; Claude Code's JSON is printable, but decode hex just in case.
access_token() {
  "$PY" -c '
import json, sys
raw = sys.stdin.read().strip()
try:
    doc = json.loads(raw)
except ValueError:
    doc = json.loads(bytes.fromhex(raw).decode())
tok = (doc.get("claudeAiOauth") or {}).get("accessToken") or ""
if not tok:
    sys.exit("the live credential has no access token; log in with `claude` first")
sys.stdout.write(tok)
'
}

# Turns $WORK/{status,headers,body} into the redacted fixture envelope at $OUT/$1.json, and
# shows it. Redaction (redactor.py above) is deterministic and keeps every key, type and array
# length: emails, uuids, tokens, request ids and account/organization names become fixed fakes,
# numbered in order of first appearance, so two equal values stay equal; creation times become
# a fixed fake and every other timestamp is shifted, both keeping their exact format.
record() {
  local name=$1
  mkdir -p "$OUT"
  "$PY" - "$WORK" "$OUT/$name.json" <<'PYEOF'
import json, sys
from redactor import scrub
work, dest = sys.argv[1], sys.argv[2]
status = int(open(f"{work}/status").read().strip())
headers = {}
block = open(f"{work}/headers", encoding="latin-1").read().replace("\r\n", "\n").strip().split("\n\n")[-1]
for line in block.split("\n")[1:]:
    if ":" in line:
        k, v = line.split(":", 1)
        k = k.strip().lower()
        if k in ("content-type", "retry-after"):
            headers[k] = v.strip()
raw = open(f"{work}/body", "rb").read()
try:
    body = json.loads(raw)
except ValueError:
    body = raw.decode("utf-8", "replace")

env = {"status": status, "headers": headers, "body": scrub(body), "synthetic": False}
with open(dest, "w") as f:
    json.dump(env, f, indent=2)
    f.write("\n")
print(f"--- {dest}")
print(json.dumps(env, indent=2))
PYEOF
}

# curl with its secret-bearing options on stdin (`--config -`), never in argv.
curl_with() {
  local config=$1; shift
  printf '%s' "$config" | curl -sS --max-time 10 -o "$WORK/body" -D "$WORK/headers" \
    -w '%{http_code}' --config - "$@" > "$WORK/status"
}

profile() {
  local cred at
  cred=$(live_credential)
  at=$(printf '%s' "$cred" | access_token)
  curl_with "header = \"Authorization: Bearer $at\"
header = \"User-Agent: $UA\"
url = \"$PROFILE_URL\""
  record profile-200
}

usage() {
  local cred at
  cred=$(live_credential)
  at=$(printf '%s' "$cred" | access_token)
  curl_with "header = \"Authorization: Bearer $at\"
header = \"anthropic-beta: oauth-2025-04-20\"
header = \"User-Agent: $UA\"
url = \"$USAGE_URL\""
  record usage-200
}

token_post() {
  local client=$1 name=$2
  local body
  body=$(printf '{"grant_type":"refresh_token","refresh_token":"%s","client_id":"%s","scope":"user:inference user:profile"}' "$FAKE_RT" "$client")
  curl_with "header = \"Content-Type: application/json\"
header = \"User-Agent: $UA\"
url = \"$TOKEN_URL\"" --data-binary "$body"
  record "$name"
}

invalid_grant() { token_post "$CLIENT_ID" token-invalid-grant; }
invalid_client() { token_post "$FAKE_CLIENT_ID" token-invalid-client; }

# The decision gate (Appendix A.5): exits non-zero, naming what differs, when a recording's
# shape is not the one Tasks 7 and 8 parse.
check() {
  "$PY" - "$OUT" <<'PYEOF'
import json, sys
out = sys.argv[1]
def load(n):
    return json.load(open(f"{out}/{n}.json"))
problems = []
p = load("profile-200")
b = p["body"] if isinstance(p["body"], dict) else {}
if p["status"] != 200: problems.append(f"profile: status {p['status']}")
for path in (("account", "uuid"), ("account", "email")):
    cur = b
    for part in path:
        cur = cur.get(part) if isinstance(cur, dict) else None
    if not isinstance(cur, str) or not cur:
        problems.append(f"profile: no string at {'.'.join(path)}")
# Appendix A.6 treats a null or empty organization as '': a personal account has none.
org = b.get("organization")
if org is not None and not (isinstance(org, dict) and isinstance(org.get("uuid"), str)):
    problems.append("profile: organization is neither null nor an object with a string uuid")
u = load("usage-200")
if u["status"] != 200: problems.append(f"usage: status {u['status']}")
g = load("token-invalid-grant")
gerr = g["body"].get("error") if isinstance(g["body"], dict) else None
if g["status"] not in (400, 401, 403): problems.append(f"token-invalid-grant: status {g['status']}")
if gerr != "invalid_grant": problems.append(f"token-invalid-grant: top-level error is {gerr!r}, expected 'invalid_grant'")
# An unknown client is refused in one of two shapes, and Task 8 classifies both as systemic:
# RFC 6749's top-level "invalid_client", or a 400 whose nested error.type is
# "invalid_request_error" (what the endpoint returned on 2026-09-30).
c = load("token-invalid-client")
cerr = c["body"].get("error") if isinstance(c["body"], dict) else None
rfc = cerr == "invalid_client"
nested = isinstance(cerr, dict) and cerr.get("type") == "invalid_request_error"
if c["status"] not in (400, 401, 403): problems.append(f"token-invalid-client: status {c['status']}")
if not (rfc or nested):
    problems.append(f"token-invalid-client: error is {cerr!r}, expected 'invalid_client' or an object of type 'invalid_request_error'")
elif nested and c["status"] != 400:
    problems.append(f"token-invalid-client: the nested shape is only classified on a 400, got {c['status']}")
if problems:
    print("GATE FAILS:"); [print(" -", x) for x in problems]; sys.exit(1)
print("gate passes: every recorded shape matches Appendix A.5 (unknown client: %s)" % ("top-level invalid_client" if rfc else "nested invalid_request_error"))
PYEOF
}

# Re-applies the redaction, offline and in place, to recordings already on disk: no network,
# no credential. Never touches token-200.json (synthetic). Idempotent.
rescrub() {
  "$PY" - "$OUT" <<'PYEOF'
import json, sys
from redactor import scrub
out = sys.argv[1]
for name in ("profile-200", "usage-200", "token-invalid-grant", "token-invalid-client"):
    path = f"{out}/{name}.json"
    with open(path) as f:
        env = json.load(f)
    if env.get("synthetic"):
        continue
    env["body"] = scrub(env["body"])
    with open(path, "w") as f:
        json.dump(env, f, indent=2)
        f.write("\n")
    print(f"rescrubbed {path}")
PYEOF
}

case "${1:-}" in
  profile|usage|invalid_grant|invalid_client|check|rescrub) "$1" ;;
  all) profile; usage; invalid_grant; invalid_client; check ;;
  *) echo "usage: $0 all|profile|usage|invalid_grant|invalid_client|check|rescrub" >&2; exit 2 ;;
esac
```

- [ ] **Step 2: Make it executable and lint it**

Run: `chmod +x scripts/spikes/m2a-endpoints.sh && bash -n scripts/spikes/m2a-endpoints.sh`
Expected: no output, exit 0.

- [ ] **Step 3: Write the synthetic success fixture**

A successful refresh cannot be recorded without spending a real refresh token. Create
`crates/tagteam-cc/tests/fixtures/endpoints/token-200.json`, built from Appendix A.5's response
handling (every field it names, with fake values):

```json
{
  "status": 200,
  "headers": {
    "content-type": "application/json"
  },
  "body": {
    "token_type": "Bearer",
    "access_token": "sk-ant-oat01-synthetic-access-token",
    "expires_in": 28800,
    "refresh_token": "sk-ant-ort01-synthetic-refresh-token",
    "refresh_token_expires_in": 7776000,
    "scope": "user:inference user:profile",
    "account": {
      "uuid": "00000000-0000-4000-8000-000000000001",
      "email_address": "probe1@example.com"
    },
    "organization": {
      "uuid": "00000000-0000-4000-8000-000000000002"
    }
  },
  "synthetic": true
}
```

- [ ] **Step 4: Commit the script and the synthetic fixture**

```bash
git add scripts/spikes/m2a-endpoints.sh crates/tagteam-cc/tests/fixtures/endpoints/token-200.json
git commit -m "Add the M2a endpoint probe and the synthetic token fixture"
```

- [ ] **Step 5: Michael runs the probe**

From the repository root, in an ordinary terminal (not a sandboxed agent shell — it needs the
network and the login keychain), with a working Claude Code login:

Run: `scripts/spikes/m2a-endpoints.sh all`
Expected: four `--- crates/tagteam-cc/tests/fixtures/endpoints/<name>.json` blocks, then
`gate passes: every recorded shape matches Appendix A.5`, followed by which unknown-client
shape it saw. Little Snitch may ask about `curl` reaching `api.anthropic.com` and
`platform.claude.com`; allow it for this run.

If `profile-200` or `usage-200` recorded a 401, the live access token had expired: send
`claude` one message (so Claude Code refreshes its own token), then re-run
`scripts/spikes/m2a-endpoints.sh profile`, `usage` and `check`. A 429 on `usage` means the
hourly budget is spent; wait an hour and re-run `usage` and `check`. Never refresh the live
token any other way — that is Claude Code's.

If `check` printed `GATE FAILS`, stop. Bring the printed problems and the redacted recordings
to Michael; do not start Task 2. The fix is a spec amendment to Appendix A.5 and the matching
change to Tasks 7 and 8 of this plan, made before either runs.

Record in the task report (not in git): the `claude --version` output, the date, and the
four `status` values. (2026-09-30, Claude Code 2.1.285: `200`, `200`, `400`, `400`.)

- [ ] **Step 6: Confirm the spec records the verification**

The spec amendment landed in `7e96d7c`. No spec edit is left for this task.

Run: `rg -n 'Response shapes verified against the live endpoints|invalid_request_error' docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md`
Expected: three matches: §7.3 step 7's verdict row, and Appendix A.5's verification line and
its unknown-client bullet (the `invalid_request_error` text). If Appendix A.5 lacks the
verification line, stop and bring it to Michael: the spec is the canonical record, and this
task does not amend it.

- [ ] **Step 7: Bring the committed probe up to date**

Overwrite `scripts/spikes/m2a-endpoints.sh` with the script in Step 1. Against the committed
copy it adds the shared `redactor.py` (written into the probe's work directory, used by
`record` and `rescrub`), the timestamp and request-id scrubbing, the two-shape `check`, and
the `rescrub` subcommand.

Run: `bash -n scripts/spikes/m2a-endpoints.sh`
Expected: no output, exit 0.

Prove `rescrub` and `check` offline on a scratch copy, so the working files are untouched:

```bash
S=$(mktemp -d "${TMPDIR:-/tmp}/rescrub.XXXXXX")
cp crates/tagteam-cc/tests/fixtures/endpoints/*.json "$S"/
TAGTEAM_PROBE_OUT="$S" scripts/spikes/m2a-endpoints.sh rescrub
TAGTEAM_PROBE_OUT="$S" scripts/spikes/m2a-endpoints.sh check
mkdir "$S/once" && cp "$S"/*.json "$S/once"/
TAGTEAM_PROBE_OUT="$S" scripts/spikes/m2a-endpoints.sh rescrub > /dev/null
diff -r "$S/once" "$S" --exclude=once && echo idempotent
rg -n '202[1-9]-' "$S"/*.json
rm -rf "$S"
```
Expected: four `rescrubbed …/<name>.json` lines; `gate passes: every recorded shape matches
Appendix A.5 (unknown client: nested invalid_request_error)`; `idempotent` (the second rescrub
changed nothing); and no output from the `rg`, so no real date survives (the fakes are 2020,
the shifted times are 2030 or later). `token-200.json` is untouched, since a synthetic
envelope is skipped.

```bash
git add scripts/spikes/m2a-endpoints.sh
git commit -m "Scrub timestamps and request ids in the endpoint probe and add rescrub"
```

- [ ] **Step 8: Rescrub the recordings and Michael re-reviews**

Run: `scripts/spikes/m2a-endpoints.sh rescrub`
Expected: four `rescrubbed crates/tagteam-cc/tests/fixtures/endpoints/<name>.json` lines. No
network, no keychain: it rewrites the four recordings in place.

Run: `scripts/spikes/m2a-endpoints.sh check`
Expected: `gate passes: every recorded shape matches Appendix A.5 (unknown client: nested invalid_request_error)`

Then Michael reviews every file by eye:

Run: `cat crates/tagteam-cc/tests/fixtures/endpoints/{profile-200,usage-200,token-invalid-grant,token-invalid-client}.json`
Expected: nothing real anywhere.
- Only `probeN@example.com`, `00000000-0000-4000-8000-00000000000N`, `Probe Name N` and
  `redacted-token-N`.
- `created_at` and `subscription_created_at` are `2020-01-01T00:00:00.000000Z`, and
  `request_id` is `req_` plus zeros ending in `1`.
- Every usage timestamp is 2030 or later, and still looks like `2030-01-01T00:00:00.751075+00:00`
  (six fractional digits where the live one had six, none where it had none, always `+00:00`).
  `window_started_at` is the earliest, at `2030-01-01T00:00:00…`.
- Kept as recorded: `rate_limit_tier`, the `utilization` and `percent` values, the dollar and
  `amount_minor` figures, and every key and `null`.

If anything real survives, extend `redact()` in the script, redo Steps 7–8, and review again.
Never commit a file with a real value in it.

- [ ] **Step 9: Commit the recordings**

```bash
git add crates/tagteam-cc/tests/fixtures/endpoints/profile-200.json \
  crates/tagteam-cc/tests/fixtures/endpoints/usage-200.json \
  crates/tagteam-cc/tests/fixtures/endpoints/token-invalid-grant.json \
  crates/tagteam-cc/tests/fixtures/endpoints/token-invalid-client.json
git commit -m "Record redacted responses from the live Claude Code endpoints"
```

---

### Task 2: The `Http` port, `ScriptedHttp`, the mock server, and `!*.sql`

The port every network call goes through (§4.4), the scripted fake engine tests use, and the
`std::net` mock server that the adapter tests and the cross-process tests use (§15.1). Nothing
here sends a real request.

**Files:**
- Create: `crates/tagteam-provider/src/http.rs`
- Create: `crates/tagteam-provider/src/mock_server.rs`
- Modify: `crates/tagteam-provider/src/lib.rs`
- Modify: `crates/tagteam-provider/Cargo.toml` (feature `mock-server`)
- Modify: `.gitignore`
- Test: unit tests inside `http.rs` and `mock_server.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces (exactly the Interface Contract's `src/http.rs` and `src/mock_server.rs`, plus
  two additive helpers):
  - `pub const USER_AGENT: &str`, `pub const MAX_BODY: usize = 1 << 20`
  - `pub enum Method { Get, Post }` (also `Hash`, and `Method::as_str(self) -> &'static str`)
  - `pub struct HttpRequest { method, url, headers: Vec<(&'static str, String)>, body: Option<Vec<u8>>, timeout: Duration }`
    with `get`, `post_json`, `bearer`, `header` and a redacting `Debug`
  - `pub struct HttpResponse { status: u16, headers: Vec<(String, String)>, body: Vec<u8> }`
    with `header`, `json`, a length-only `Debug`, and the additive
    `HttpResponse::json_body(status: u16, body: &serde_json::Value) -> Self`
  - `pub enum HttpError { PreSend(String), Ambiguous(String) }`
  - `pub trait Http: Send + Sync { fn send(&self, req: &HttpRequest) -> Result<HttpResponse, HttpError>; }`
  - `pub struct NoHttp;`
  - `pub struct ScriptedHttp` with `new`, `push`, `push_json`, `requests`, `count`, `clear`,
    `Default`; `pub struct RecordedRequest { method, url, headers: Vec<(String, String)>, body: Option<Vec<u8>> }`
    (its `Debug` redacts `authorization` and the body, like `HttpRequest`'s)
  - behind feature `mock-server`: `MockServer::{start, base_url, on, hits, requests}`,
    `MockReply::{Json, Raw, Delay, Close, Hang}`, `MockRequest`
  - re-exported from the crate root: `Http, HttpError, HttpRequest, HttpResponse, Method,
    NoHttp, RecordedRequest, ScriptedHttp`

- [ ] **Step 1: Write the failing `http` tests**

Create `crates/tagteam-provider/src/http.rs` with only the tests for now:

```rust
#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use super::*;

    const T: Duration = Duration::from_secs(5);

    #[test]
    fn debug_never_shows_the_bearer_token_or_the_body() {
        let req = HttpRequest::post_json(
            "https://example.test/token",
            &json!({"refresh_token": "sk-ant-ort01-SENTINEL"}),
            T,
        )
        .bearer("sk-ant-oat01-SENTINEL");
        let shown = format!("{req:?}");
        assert!(!shown.contains("SENTINEL"), "{shown}");
        assert!(shown.contains("authorization: <redacted>"), "{shown}");
        assert!(shown.contains("content-type: application/json"), "{shown}");
        assert!(shown.contains("https://example.test/token"), "{shown}");

        let resp = HttpResponse::json_body(200, &json!({"access_token": "sk-ant-SENTINEL"}));
        let shown = format!("{resp:?}");
        assert!(!shown.contains("SENTINEL"), "{shown}");
        assert!(shown.contains("200"), "{shown}");
    }

    #[test]
    fn builders_set_method_headers_and_body() {
        let get = HttpRequest::get("https://example.test/p", T).header("anthropic-beta", "b1");
        assert_eq!(get.method, Method::Get);
        assert_eq!(get.body, None);
        assert_eq!(get.headers, vec![("anthropic-beta", "b1".to_owned())]);
        assert_eq!(get.timeout, T);

        let post = HttpRequest::post_json("https://example.test/t", &json!({"a": 1}), T).bearer("tok");
        assert_eq!(post.method, Method::Post);
        assert_eq!(post.body.as_deref(), Some(br#"{"a":1}"#.as_slice()));
        assert_eq!(
            post.headers,
            vec![
                ("content-type", "application/json".to_owned()),
                ("authorization", "Bearer tok".to_owned()),
            ]
        );
    }

    #[test]
    fn response_headers_are_case_insensitive_and_json_is_optional() {
        let resp = HttpResponse {
            status: 429,
            headers: vec![("retry-after".into(), "30".into())],
            body: b"not json".to_vec(),
        };
        assert_eq!(resp.header("Retry-After"), Some("30"));
        assert_eq!(resp.header("x-missing"), None);
        assert_eq!(resp.json(), None);
        assert_eq!(
            HttpResponse::json_body(200, &json!({"k": [1]})).json(),
            Some(json!({"k": [1]}))
        );
    }

    #[test]
    fn scripted_replies_are_fifo_and_the_last_one_repeats() {
        let http = ScriptedHttp::new();
        let url = "https://example.test/x";
        http.push_json(Method::Get, url, 200, json!({"n": 1}));
        http.push(Method::Get, url, Err(HttpError::Ambiguous("reset".into())));
        http.push_json(Method::Get, url, 500, json!({"n": 3}));
        let req = HttpRequest::get(url, T);
        assert_eq!(http.send(&req).unwrap().json(), Some(json!({"n": 1})));
        assert_eq!(http.send(&req).unwrap_err(), HttpError::Ambiguous("reset".into()));
        assert_eq!(http.send(&req).unwrap().status, 500);
        assert_eq!(http.send(&req).unwrap().status, 500, "the last reply repeats");
        assert_eq!(http.count(Method::Get, url), 4);
        assert_eq!(http.count(Method::Post, url), 0);
    }

    #[test]
    fn an_unscripted_route_is_pre_send_and_still_recorded() {
        let http = ScriptedHttp::new();
        http.push_json(Method::Get, "https://example.test/a", 200, json!({}));
        let err = http
            .send(&HttpRequest::post_json("https://example.test/a", &json!({}), T))
            .unwrap_err();
        assert_eq!(
            err,
            HttpError::PreSend("no scripted reply for POST https://example.test/a".into())
        );
        assert_eq!(http.count(Method::Post, "https://example.test/a"), 1);
    }

    #[test]
    fn requests_are_recorded_with_headers_and_body_and_clear_drops_everything() {
        let http = ScriptedHttp::default();
        let url = "https://example.test/t";
        http.push_json(Method::Post, url, 200, json!({}));
        http.send(&HttpRequest::post_json(url, &json!({"r": "x"}), T).bearer("tok"))
            .unwrap();
        let log = http.requests();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].method, Method::Post);
        assert_eq!(log[0].url, url);
        assert!(log[0]
            .headers
            .contains(&("authorization".to_owned(), "Bearer tok".to_owned())));
        assert_eq!(log[0].body.as_deref(), Some(br#"{"r":"x"}"#.as_slice()));
        let shown = format!("{:?}", log[0]);
        assert!(!shown.contains("Bearer tok"), "recorded Debug redacts: {shown}");
        assert!(shown.contains("authorization: <redacted>"), "{shown}");

        http.clear();
        assert!(http.requests().is_empty());
        assert!(matches!(
            http.send(&HttpRequest::get(url, T)),
            Err(HttpError::PreSend(_))
        ));
    }

    #[test]
    fn no_http_never_sends() {
        assert_eq!(
            NoHttp
                .send(&HttpRequest::get("https://example.test/", T))
                .unwrap_err(),
            HttpError::PreSend("network disabled".into())
        );
    }

    #[test]
    fn the_response_body_cap_is_one_mebibyte() {
        // The User-Agent is pinned on the wire by Task 3's adapter test, not against its own definition.
        assert_eq!(MAX_BODY, 1_048_576);
    }
}
```

And register the module in `crates/tagteam-provider/src/lib.rs`, after `pub mod flock;`:

```rust
pub mod http;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tagteam-provider --lib http::`
Expected: FAIL to compile — undeclared type `HttpRequest` (and the other port types).

- [ ] **Step 3: Implement the port**

Put this above the `#[cfg(test)]` block in `crates/tagteam-provider/src/http.rs`:

```rust
//! The `Http` port (§4.4): one blocking request, with its own timeout. The engine's `ureq`
//! adapter is the production implementation; `ScriptedHttp` is the tests'. A provider builds
//! every request and parses every response; the engine owns everything around them.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use serde_json::Value;

/// Sent on every request by the production adapter, whatever the provider asked for (§4.4).
pub const USER_AGENT: &str = concat!("tagteam/", env!("CARGO_PKG_VERSION"));

/// The largest response body the adapter reads (§4.4).
pub const MAX_BODY: usize = 1 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Method {
    Get,
    Post,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
        }
    }
}

/// Renders headers for `Debug`, with the `authorization` value redacted: it carries a token.
fn shown_headers<'a>(headers: impl Iterator<Item = (&'a str, &'a str)>) -> Vec<String> {
    headers
        .map(|(k, v)| {
            if k.eq_ignore_ascii_case("authorization") {
                format!("{k}: <redacted>")
            } else {
                format!("{k}: {v}")
            }
        })
        .collect()
}

fn shown_body(body: Option<&[u8]>) -> String {
    match body {
        Some(b) => format!("<{} bytes>", b.len()),
        None => "none".into(),
    }
}

/// One request. Its body can carry a refresh token and its `authorization` header an access
/// token, so `Debug` shows neither (§4.4).
#[derive(Clone)]
pub struct HttpRequest {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(&'static str, String)>,
    pub body: Option<Vec<u8>>,
    pub timeout: Duration,
}

impl HttpRequest {
    pub fn get(url: impl Into<String>, timeout: Duration) -> Self {
        Self {
            method: Method::Get,
            url: url.into(),
            headers: Vec::new(),
            body: None,
            timeout,
        }
    }

    pub fn post_json(url: impl Into<String>, body: &Value, timeout: Duration) -> Self {
        Self {
            method: Method::Post,
            url: url.into(),
            headers: vec![("content-type", "application/json".into())],
            body: Some(serde_json::to_vec(body).expect("a Value always serializes")),
            timeout,
        }
    }

    pub fn bearer(self, token: &str) -> Self {
        self.header("authorization", format!("Bearer {token}"))
    }

    pub fn header(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.headers.push((name, value.into()));
        self
    }
}

impl fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpRequest")
            .field("method", &self.method)
            .field("url", &self.url)
            .field(
                "headers",
                &shown_headers(self.headers.iter().map(|(k, v)| (*k, v.as_str()))),
            )
            .field("body", &format_args!("{}", shown_body(self.body.as_deref())))
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// A response, whatever its status: a 4xx or 5xx is a response, not an error. Its body can
/// carry tokens, so `Debug` shows only its length.
#[derive(Clone)]
pub struct HttpResponse {
    pub status: u16,
    /// Names lowercased.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// A response with a JSON body, as fakes and tests build one.
    pub fn json_body(status: u16, body: &Value) -> Self {
        Self {
            status,
            headers: vec![("content-type".into(), "application/json".into())],
            body: serde_json::to_vec(body).expect("a Value always serializes"),
        }
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The body as JSON; `None` when it is not valid JSON.
    pub fn json(&self) -> Option<Value> {
        serde_json::from_slice(&self.body).ok()
    }
}

impl fmt::Debug for HttpResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpResponse")
            .field("status", &self.status)
            .field("headers", &self.headers)
            .field("body", &format_args!("<{} bytes>", self.body.len()))
            .finish()
    }
}

/// A transport failure (§4.4). `PreSend` only when the request provably never left the
/// machine; anything else is `Ambiguous`, since the server may have acted on it (§7.3).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HttpError {
    #[error("the request was never sent: {0}")]
    PreSend(String),
    #[error("the request may have been sent, but no response was read: {0}")]
    Ambiguous(String),
}

pub trait Http: Send + Sync {
    fn send(&self, req: &HttpRequest) -> Result<HttpResponse, HttpError>;
}

/// Never sends anything: for engines that must make no request at all.
pub struct NoHttp;

impl Http for NoHttp {
    fn send(&self, _req: &HttpRequest) -> Result<HttpResponse, HttpError> {
        Err(HttpError::PreSend("network disabled".into()))
    }
}

/// A request as `ScriptedHttp` saw it. `Debug` redacts like `HttpRequest`'s.
#[derive(Clone)]
pub struct RecordedRequest {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

impl fmt::Debug for RecordedRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecordedRequest")
            .field("method", &self.method)
            .field("url", &self.url)
            .field(
                "headers",
                &shown_headers(self.headers.iter().map(|(k, v)| (k.as_str(), v.as_str()))),
            )
            .field("body", &format_args!("{}", shown_body(self.body.as_deref())))
            .finish()
    }
}

type Reply = Result<HttpResponse, HttpError>;

/// The tests' `Http`: replies queued per `(method, url)` and served first in, first out; the
/// last reply queued for a route repeats once it is the only one left. An unscripted route is
/// `PreSend`, as an unreachable host would be. Every request is recorded, scripted or not.
#[derive(Default)]
pub struct ScriptedHttp {
    routes: Mutex<HashMap<(Method, String), VecDeque<Reply>>>,
    log: Mutex<Vec<RecordedRequest>>,
}

impl ScriptedHttp {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, method: Method, url: &str, reply: Reply) {
        self.routes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry((method, url.to_owned()))
            .or_default()
            .push_back(reply);
    }

    pub fn push_json(&self, method: Method, url: &str, status: u16, body: Value) {
        self.push(method, url, Ok(HttpResponse::json_body(status, &body)));
    }

    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn count(&self, method: Method, url: &str) -> usize {
        self.requests()
            .iter()
            .filter(|r| r.method == method && r.url == url)
            .count()
    }

    /// Drops every queued reply and the request log.
    pub fn clear(&self) {
        self.routes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        self.log.lock().unwrap_or_else(PoisonError::into_inner).clear();
    }
}

impl Http for ScriptedHttp {
    fn send(&self, req: &HttpRequest) -> Result<HttpResponse, HttpError> {
        self.log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(RecordedRequest {
                method: req.method,
                url: req.url.clone(),
                headers: req
                    .headers
                    .iter()
                    .map(|(k, v)| ((*k).to_owned(), v.clone()))
                    .collect(),
                body: req.body.clone(),
            });
        let mut routes = self.routes.lock().unwrap_or_else(PoisonError::into_inner);
        match routes.get_mut(&(req.method, req.url.clone())) {
            Some(queue) if queue.len() > 1 => queue.pop_front().expect("the queue is not empty"),
            Some(queue) => queue.front().cloned().expect("a queued route is never empty"),
            None => Err(HttpError::PreSend(format!(
                "no scripted reply for {} {}",
                req.method.as_str(),
                req.url
            ))),
        }
    }
}
```

Then re-export the port from `crates/tagteam-provider/src/lib.rs`, after the `pub use flock::…;`
line:

```rust
pub use http::{
    Http, HttpError, HttpRequest, HttpResponse, Method, NoHttp, RecordedRequest, ScriptedHttp,
};
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p tagteam-provider --lib http::`
Expected: PASS, 8 tests.

- [ ] **Step 5: Write the failing mock-server tests**

Add the feature to `crates/tagteam-provider/Cargo.toml`, below `real_keychain = []`:

```toml
# A local std::net HTTP server for adapter and cross-process tests. Never in release builds.
mock-server = []
```

Create `crates/tagteam-provider/src/mock_server.rs` with only the tests:

```rust
#[cfg(test)]
mod tests {
    use std::io::{ErrorKind, Read, Write};
    use std::net::TcpStream;
    use std::time::{Duration, Instant};

    use serde_json::json;

    use super::*;

    /// Sends `request` raw and reads until the server closes, or until `wait` passes.
    fn raw(server: &MockServer, request: &str, wait: Duration) -> std::io::Result<String> {
        let addr = server.base_url().trim_start_matches("http://").to_owned();
        let mut s = TcpStream::connect(addr)?;
        s.set_read_timeout(Some(wait))?;
        s.write_all(request.as_bytes())?;
        let mut out = Vec::new();
        s.read_to_end(&mut out)?;
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    fn get(path: &str) -> String {
        format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
    }

    const WAIT: Duration = Duration::from_secs(5);

    #[test]
    fn routes_reply_fifo_then_repeat_the_last_and_every_request_is_logged() {
        let server = MockServer::start();
        server.on("GET", "/a", MockReply::Json { status: 200, body: json!({"n": 1}) });
        server.on("GET", "/a", MockReply::Json { status: 503, body: json!({"n": 2}) });
        let first = raw(&server, &get("/a"), WAIT).unwrap();
        assert!(first.starts_with("HTTP/1.1 200 "), "{first}");
        assert!(first.ends_with(r#"{"n":1}"#), "{first}");
        assert!(first.to_ascii_lowercase().contains("connection: close"), "{first}");
        for _ in 0..2 {
            let again = raw(&server, &get("/a?x=1"), WAIT).unwrap();
            assert!(again.starts_with("HTTP/1.1 503 "), "{again}");
        }
        assert_eq!(server.hits("GET", "/a"), 3, "a query string routes by its path");
        assert_eq!(server.hits("POST", "/a"), 0);

        let body = r#"{"k":"v"}"#;
        raw(
            &server,
            &format!(
                "POST /t HTTP/1.1\r\nHost: x\r\nX-Probe: Yes\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            ),
            WAIT,
        )
        .unwrap();
        let log = server.requests();
        let post = log.iter().find(|r| r.method == "POST").unwrap();
        assert_eq!(post.path, "/t");
        assert_eq!(post.body, body.as_bytes());
        assert!(post.headers.contains(&("x-probe".to_owned(), "Yes".to_owned())));
    }

    #[test]
    fn an_unrouted_path_is_404() {
        let server = MockServer::start();
        let out = raw(&server, &get("/nowhere"), WAIT).unwrap();
        assert!(out.starts_with("HTTP/1.1 404 "), "{out}");
        assert!(out.ends_with(r#"{"error":"not_found"}"#), "{out}");
    }

    #[test]
    fn raw_replies_carry_their_own_headers_and_bytes() {
        let server = MockServer::start();
        server.on(
            "GET",
            "/r",
            MockReply::Raw {
                status: 429,
                headers: vec![("Retry-After".into(), "30".into())],
                body: b"slow down".to_vec(),
            },
        );
        let out = raw(&server, &get("/r"), WAIT).unwrap();
        assert!(out.starts_with("HTTP/1.1 429 "), "{out}");
        assert!(out.contains("Retry-After: 30\r\n"), "{out}");
        assert!(out.ends_with("slow down"), "{out}");
    }

    #[test]
    fn delay_waits_before_replying() {
        let server = MockServer::start();
        server.on(
            "GET",
            "/d",
            MockReply::Delay(
                Duration::from_millis(300),
                Box::new(MockReply::Json { status: 200, body: json!({}) }),
            ),
        );
        let t = Instant::now();
        let out = raw(&server, &get("/d"), WAIT).unwrap();
        assert!(t.elapsed() >= Duration::from_millis(300));
        assert!(out.starts_with("HTTP/1.1 200 "), "{out}");
    }

    #[test]
    fn close_answers_with_no_bytes_at_all() {
        let server = MockServer::start();
        server.on("POST", "/c", MockReply::Close);
        let out = raw(
            &server,
            "POST /c HTTP/1.1\r\nHost: x\r\nContent-Length: 2\r\n\r\n{}",
            WAIT,
        );
        match out {
            Ok(s) => assert!(s.is_empty(), "{s}"),
            Err(e) => assert_eq!(e.kind(), ErrorKind::ConnectionReset),
        }
        assert_eq!(server.hits("POST", "/c"), 1, "the request was read before closing");
    }

    #[test]
    fn hang_never_answers() {
        let server = MockServer::start();
        server.on("GET", "/h", MockReply::Hang);
        let err = raw(&server, &get("/h"), Duration::from_millis(300)).unwrap_err();
        assert!(
            matches!(err.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut),
            "{err:?}"
        );
        assert_eq!(server.hits("GET", "/h"), 1);
    }

    #[test]
    fn dropping_the_server_stops_it() {
        let server = MockServer::start();
        server.on("GET", "/h", MockReply::Hang);
        let addr = server.base_url().trim_start_matches("http://").to_owned();
        let _ = raw(&server, &get("/h"), Duration::from_millis(100));
        drop(server);
        assert!(TcpStream::connect(addr).is_err(), "the listener is gone");
    }
}
```

Register the module in `crates/tagteam-provider/src/lib.rs`, after `pub mod mkdir_lock;`:

```rust
#[cfg(feature = "mock-server")]
pub mod mock_server;
```

- [ ] **Step 6: Run the tests to verify they fail**

Run: `cargo test -p tagteam-provider --features mock-server --lib mock_server::`
Expected: FAIL to compile — undeclared type `MockServer`.

- [ ] **Step 7: Implement the mock server**

Put this above the `#[cfg(test)]` block in `crates/tagteam-provider/src/mock_server.rs`:

```rust
//! A local HTTP/1.1 server for tests (§15.1): scripted replies per route, one request log
//! that every client process shares, and the failure shapes a real network produces (a
//! connection closed with no reply, a server that never answers). Plain `std::net`, one thread
//! per connection, and `Connection: close` on every reply, so each request is its own
//! connection.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// The longest `Hang` holds a connection, so a forgotten test cannot pin a thread forever.
const HANG_CAP: Duration = Duration::from_secs(120);

#[derive(Debug, Clone)]
pub enum MockReply {
    Json {
        status: u16,
        body: Value,
    },
    Raw {
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    },
    /// Waits, then replies.
    Delay(Duration, Box<MockReply>),
    /// Reads the request, then closes the connection without writing a byte: the request may
    /// have been acted on, and no response ever arrives (an `Ambiguous` failure).
    Close,
    /// Reads the request, then never answers; the client times out.
    Hang,
}

#[derive(Debug, Clone)]
pub struct MockRequest {
    pub method: String,
    /// The request target as sent, query string included.
    pub path: String,
    /// Names lowercased.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[derive(Default)]
struct State {
    routes: HashMap<(String, String), VecDeque<MockReply>>,
    log: Vec<MockRequest>,
}

pub struct MockServer {
    addr: SocketAddr,
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    accept: Option<JoinHandle<()>>,
}

/// The route a target belongs to: its path, without the query string.
fn route_path(target: &str) -> &str {
    target.split('?').next().unwrap_or(target)
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        302 => "Found",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Status",
    }
}

fn read_request(stream: &TcpStream) -> Option<MockRequest> {
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_owned();
    let path = parts.next()?.to_owned();
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).ok()? == 0 {
            return None;
        }
        let h = h.trim_end_matches(['\r', '\n']);
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_owned()));
        }
    }
    let len = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0; len];
    reader.read_exact(&mut body).ok()?;
    Some(MockRequest {
        method,
        path,
        headers,
        body,
    })
}

fn write_response(stream: &mut TcpStream, status: u16, headers: &[(String, String)], body: &[u8]) {
    let mut head = format!("HTTP/1.1 {status} {}\r\n", reason(status));
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    ));
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
    let _ = stream.shutdown(Shutdown::Write);
}

fn sleep_unless_stopped(total: Duration, stop: &AtomicBool) {
    let until = Instant::now() + total;
    while !stop.load(Ordering::SeqCst) && Instant::now() < until {
        thread::sleep(Duration::from_millis(20));
    }
}

fn respond(mut stream: TcpStream, reply: MockReply, stop: &AtomicBool) {
    match reply {
        MockReply::Json { status, body } => {
            let bytes = serde_json::to_vec(&body).expect("a Value always serializes");
            let headers = [("Content-Type".to_owned(), "application/json".to_owned())];
            write_response(&mut stream, status, &headers, &bytes);
        }
        MockReply::Raw {
            status,
            headers,
            body,
        } => write_response(&mut stream, status, &headers, &body),
        MockReply::Delay(wait, then) => {
            sleep_unless_stopped(wait, stop);
            respond(stream, *then, stop);
        }
        MockReply::Close => {
            let _ = stream.shutdown(Shutdown::Both);
        }
        MockReply::Hang => sleep_unless_stopped(HANG_CAP, stop),
    }
}

fn serve(stream: TcpStream, state: &Mutex<State>, stop: &AtomicBool) {
    let Some(req) = read_request(&stream) else {
        return;
    };
    let reply = {
        let mut s = state.lock().unwrap_or_else(PoisonError::into_inner);
        s.log.push(req.clone());
        let key = (req.method.clone(), route_path(&req.path).to_owned());
        match s.routes.get_mut(&key) {
            Some(queue) if queue.len() > 1 => queue.pop_front(),
            Some(queue) => queue.front().cloned(),
            None => None,
        }
    };
    let reply = reply.unwrap_or(MockReply::Json {
        status: 404,
        body: json!({"error": "not_found"}),
    });
    respond(stream, reply, stop);
}

impl MockServer {
    /// Binds `127.0.0.1:0` and starts serving at once.
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a local port");
        let addr = listener.local_addr().expect("a bound address");
        let state = Arc::new(Mutex::new(State::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let accept = {
            let (state, stop) = (state.clone(), stop.clone());
            thread::spawn(move || {
                for conn in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(stream) = conn else { continue };
                    let (state, stop) = (state.clone(), stop.clone());
                    thread::spawn(move || serve(stream, &state, &stop));
                }
            })
        };
        Self {
            addr,
            state,
            stop,
            accept: Some(accept),
        }
    }

    /// `http://127.0.0.1:<port>`.
    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Queues `reply` for `method` and `path` (without a query string). Replies are served
    /// first in, first out; the last one repeats once it is the only one left.
    pub fn on(&self, method: &str, path: &str, reply: MockReply) {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .routes
            .entry((method.to_owned(), path.to_owned()))
            .or_default()
            .push_back(reply);
    }

    /// How many requests reached `method` and `path`, from any process.
    pub fn hits(&self, method: &str, path: &str) -> usize {
        self.requests()
            .iter()
            .filter(|r| r.method == method && route_path(&r.path) == path)
            .count()
    }

    pub fn requests(&self) -> Vec<MockRequest> {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .log
            .clone()
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wakes the accept loop, which then sees the flag and returns, dropping the listener.
        let _ = TcpStream::connect(self.addr);
        if let Some(accept) = self.accept.take() {
            let _ = accept.join();
        }
    }
}
```

And add the re-export to `crates/tagteam-provider/src/lib.rs`, after the
`pub use mkdir_lock::…;` line:

```rust
#[cfg(feature = "mock-server")]
pub use mock_server::{MockReply, MockRequest, MockServer};
```

- [ ] **Step 8: Run the tests to verify they pass**

Run: `cargo test -p tagteam-provider --features mock-server --lib mock_server::`
Expected: PASS, 7 tests.

Run: `cargo clippy -p tagteam-provider --all-targets --features mock-server -- -D warnings`
Expected: no warnings.

- [ ] **Step 9: Stop new `.sql` files being silently ignored**

The global gitignore (`~/.config/git/gitignore_global:26`) ignores `*.sql`. `schema.sql` is
tracked, but any new `.sql` file (the next migration) would be skipped without a warning.

Run: `git check-ignore -v crates/tagteam-engine/src/store/new.sql; echo "rc=$?"`
Expected (before): a line naming `gitignore_global:26:*.sql`, then `rc=0`.

Append to `.gitignore`:

```gitignore
# The global gitignore ignores *.sql; migrations must never be skipped silently.
!*.sql
```

Run: `git check-ignore -v crates/tagteam-engine/src/store/new.sql; echo "rc=$?"`
Expected (after): `rc=1` and no other output (the path is not ignored).

- [ ] **Step 10: Commit**

```bash
git add crates/tagteam-provider/src/http.rs crates/tagteam-provider/src/mock_server.rs \
  crates/tagteam-provider/src/lib.rs crates/tagteam-provider/Cargo.toml .gitignore
git commit -m "Add the Http port, a scripted fake, and a local mock server"
```

---

### Task 3: The `ureq` adapter and the engine's `http` port

The production `Http` (§4.4): blocking `ureq` 3 over rustls with the OS trust store, so
corporate TLS proxies work. It is the one place a transport failure is classified. `PreSend`
means the request provably never left the machine; anything else is `Ambiguous`, because the
server may already have acted on it (§7.3's retry rule depends on this). The engine gains the
port it will hand to providers. Nothing calls it yet: the oracle (Task 7) and the gate
(Task 10) are its first users.

**Why the adapter resolves the host itself first.** `ureq` reports a failed DNS lookup as a
plain `Error::Io`, indistinguishable from an I/O error after the request was sent. Resolving
first, within the request's own timeout, makes a DNS failure a certain `PreSend`. The second
lookup inside `ureq` is served by the OS cache. If it fails anyway (a race), the result is
`Ambiguous`, which is the harmless direction. The lookup goes through an injectable
`Resolver`, so the test of a failed lookup never asks the machine's real resolver (§15.1:
tests never reach the network).

**Files:**
- Create: `crates/tagteam-engine/src/net.rs`
- Modify: `Cargo.toml` (workspace dependency `ureq`)
- Modify: `crates/tagteam-engine/Cargo.toml` (`ureq`; dev-dependency feature `mock-server`)
- Modify: `crates/tagteam-engine/src/lib.rs`
- Modify: `crates/tagteam-engine/src/engine.rs` (`EngineConfig.http`, `Engine.http`, `Engine::http()`)
- Modify: `crates/tagteam-engine/tests/common/mod.rs` (`Fx.http`)
- Modify: `crates/tagteam/src/app.rs` (`build_engine`)
- Test: `crates/tagteam-engine/tests/net.rs`; unit tests in `net.rs`

**Interfaces:**
- Consumes (Task 2): `tagteam_provider::http::{Http, HttpError, HttpRequest, HttpResponse,
  Method, MAX_BODY, USER_AGENT}`, `tagteam_provider::{NoHttp, ScriptedHttp}`,
  `tagteam_provider::mock_server::{MockReply, MockServer}` (feature `mock-server`).
- Produces:
  - `pub struct UreqHttp`, `UreqHttp::new() -> Self`, `impl Default`, `impl Http for UreqHttp`
    (in `tagteam_engine::net`)
  - `pub type Resolver = fn(&str, u16) -> io::Result<Vec<SocketAddr>>` and
    `UreqHttp::with_resolver(resolver: Resolver) -> Self` (`new()` uses the system resolver)
  - `EngineConfig { …, pub http: Arc<dyn Http> }`; `Engine` keeps it as
    `pub(crate) http: Arc<dyn Http>` and exposes `pub fn http(&self) -> &dyn Http`
  - the engine fixture's `Fx.http: Arc<ScriptedHttp>`, passed as `EngineConfig.http` and
    shared by every `engine_over` engine (Interface Contract, engine test fixture)

- [ ] **Step 1: Add the dependency**

In the workspace `Cargo.toml`, add under `[workspace.dependencies]`, after `tracing-subscriber`:

```toml
# Blocking HTTP over rustls with the OS trust store (§4.4); no async runtime.
ureq = { version = "3", default-features = false, features = ["rustls", "platform-verifier"] }
```

In `crates/tagteam-engine/Cargo.toml`, add `ureq.workspace = true` to `[dependencies]` (after
`tracing.workspace = true`), and enable the mock server for its tests by changing the
dev-dependency line

```toml
tagteam-provider = { workspace = true, features = ["file-keychain"] }
```

to

```toml
tagteam-provider = { workspace = true, features = ["file-keychain", "mock-server"] }
```

Run: `cargo check -p tagteam-engine`
Expected: compiles (it downloads `ureq` 3 and its rustls dependencies on first use).

- [ ] **Step 2: Write the failing adapter tests**

Create `crates/tagteam-engine/tests/net.rs`:

```rust
//! The production `Http` adapter against a local server (§4.4, §15.1): the User-Agent, the body
//! cap, and which transport failures are `PreSend` and which `Ambiguous`.

use std::io;
use std::net::{SocketAddr, TcpListener};
use std::time::{Duration, Instant};

use serde_json::json;
use tagteam_engine::net::{Resolver, UreqHttp};
use tagteam_provider::http::{MAX_BODY, USER_AGENT};
use tagteam_provider::mock_server::{MockReply, MockServer};
use tagteam_provider::{Http, HttpError, HttpRequest};

const T: Duration = Duration::from_secs(5);

fn is_pre_send(r: &Result<tagteam_provider::HttpResponse, HttpError>) -> bool {
    matches!(r, Err(HttpError::PreSend(_)))
}

fn is_ambiguous(r: &Result<tagteam_provider::HttpResponse, HttpError>) -> bool {
    matches!(r, Err(HttpError::Ambiguous(_)))
}

#[test]
fn every_request_carries_tagteams_user_agent_and_only_it() {
    let server = MockServer::start();
    server.on(
        "GET",
        "/p",
        MockReply::Raw {
            status: 200,
            headers: vec![
                ("X-Thing".into(), "v".into()),
                ("Content-Type".into(), "application/json".into()),
            ],
            body: b"{}".to_vec(),
        },
    );
    let req = HttpRequest::get(format!("{}/p", server.base_url()), T).header("user-agent", "evil/1");
    let resp = UreqHttp::new().send(&req).unwrap();
    assert_eq!(resp.status, 200);
    assert_eq!(resp.header("x-thing"), Some("v"));
    assert!(
        resp.headers.iter().all(|(k, _)| *k == k.to_ascii_lowercase()),
        "names are lowercased: {:?}",
        resp.headers
    );
    let seen = &server.requests()[0];
    let agents: Vec<&str> = seen
        .headers
        .iter()
        .filter(|(k, _)| k == "user-agent")
        .map(|(_, v)| v.as_str())
        .collect();
    assert_eq!(agents, vec![USER_AGENT]);
}

#[test]
fn a_json_post_round_trips_with_its_headers() {
    let server = MockServer::start();
    server.on("POST", "/t", MockReply::Json { status: 200, body: json!({"ok": true}) });
    let req = HttpRequest::post_json(format!("{}/t", server.base_url()), &json!({"a": 1}), T)
        .bearer("tok");
    let resp = UreqHttp::default().send(&req).unwrap();
    assert_eq!(resp.json(), Some(json!({"ok": true})));
    let seen = &server.requests()[0];
    assert_eq!(seen.body, br#"{"a":1}"#);
    assert!(seen.headers.contains(&("authorization".to_owned(), "Bearer tok".to_owned())));
    assert!(seen.headers.contains(&("content-type".to_owned(), "application/json".to_owned())));
}

#[test]
fn a_4xx_is_a_response_not_an_error() {
    let server = MockServer::start();
    server.on(
        "POST",
        "/t",
        MockReply::Json { status: 400, body: json!({"error": "invalid_grant"}) },
    );
    let req = HttpRequest::post_json(format!("{}/t", server.base_url()), &json!({}), T);
    let resp = UreqHttp::new().send(&req).unwrap();
    assert_eq!(resp.status, 400);
    assert_eq!(resp.json(), Some(json!({"error": "invalid_grant"})));
}

#[test]
fn redirects_are_returned_not_followed() {
    let server = MockServer::start();
    server.on(
        "GET",
        "/moved",
        MockReply::Raw {
            status: 302,
            headers: vec![("Location".into(), format!("{}/elsewhere", server.base_url()))],
            body: vec![],
        },
    );
    let req = HttpRequest::get(format!("{}/moved", server.base_url()), T).bearer("tok");
    let resp = UreqHttp::new().send(&req).unwrap();
    assert_eq!(resp.status, 302);
    assert_eq!(server.hits("GET", "/elsewhere"), 0, "a token never follows a redirect");
}

#[test]
fn a_body_over_one_mebibyte_is_ambiguous() {
    let server = MockServer::start();
    server.on(
        "GET",
        "/big",
        MockReply::Raw { status: 200, headers: vec![], body: vec![b'x'; MAX_BODY + 1] },
    );
    let r = UreqHttp::new().send(&HttpRequest::get(format!("{}/big", server.base_url()), T));
    assert!(is_ambiguous(&r), "{r:?}");
}

#[test]
fn a_connection_closed_without_a_reply_is_ambiguous() {
    let server = MockServer::start();
    server.on("POST", "/t", MockReply::Close);
    let r = UreqHttp::new().send(&HttpRequest::post_json(
        format!("{}/t", server.base_url()),
        &json!({"refresh_token": "x"}),
        T,
    ));
    assert!(is_ambiguous(&r), "{r:?}");
    assert_eq!(server.hits("POST", "/t"), 1, "the request did reach the server");
}

#[test]
fn a_server_that_never_answers_times_out_as_ambiguous() {
    let server = MockServer::start();
    server.on("GET", "/h", MockReply::Hang);
    let t = Instant::now();
    let r = UreqHttp::new().send(&HttpRequest::get(
        format!("{}/h", server.base_url()),
        Duration::from_millis(300),
    ));
    assert!(is_ambiguous(&r), "{r:?}");
    assert!(t.elapsed() < Duration::from_secs(3), "the request's own timeout bounds it");
}

#[test]
fn a_refused_connection_is_pre_send() {
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let r = UreqHttp::new().send(&HttpRequest::get(format!("http://127.0.0.1:{port}/"), T));
    assert!(is_pre_send(&r), "{r:?}");
}

/// A lookup that fails, injected so the machine's real resolver is never asked (§15.1).
fn no_such_host(_: &str, _: u16) -> io::Result<Vec<SocketAddr>> {
    Err(io::Error::new(io::ErrorKind::NotFound, "no such host"))
}

/// A lookup that answers with no address at all.
fn no_addresses(_: &str, _: u16) -> io::Result<Vec<SocketAddr>> {
    Ok(vec![])
}

#[test]
fn a_host_that_does_not_resolve_is_pre_send() {
    let server = MockServer::start();
    server.on("GET", "/p", MockReply::Json { status: 200, body: json!({}) });
    for resolver in [no_such_host as Resolver, no_addresses] {
        let r = UreqHttp::with_resolver(resolver)
            .send(&HttpRequest::get(format!("{}/p", server.base_url()), T));
        assert!(is_pre_send(&r), "{r:?}");
    }
    assert_eq!(server.hits("GET", "/p"), 0, "nothing was sent");
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p tagteam-engine --test net`
Expected: FAIL to compile — `unresolved import tagteam_engine::net`.

- [ ] **Step 4: Implement the adapter**

Create `crates/tagteam-engine/src/net.rs`:

```rust
//! The production `Http` port (§4.4): blocking `ureq` over rustls with the platform verifier,
//! so the OS trust store (and a corporate TLS proxy's root) is honoured.

use std::io;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use tagteam_provider::http::{
    Http, HttpError, HttpRequest, HttpResponse, MAX_BODY, Method, USER_AGENT,
};
use ureq::Agent;
use ureq::tls::{RootCerts, TlsConfig};

/// Resolves a host and port to addresses. Injectable, so a test can fail a lookup without
/// asking the machine's real resolver (§15.1).
pub type Resolver = fn(&str, u16) -> io::Result<Vec<SocketAddr>>;

fn system_resolver(host: &str, port: u16) -> io::Result<Vec<SocketAddr>> {
    Ok((host, port).to_socket_addrs()?.collect())
}

pub struct UreqHttp {
    agent: Agent,
    resolver: Resolver,
}

impl UreqHttp {
    /// 4xx and 5xx are responses, not errors; redirects are never followed, so a request that
    /// carries a token is only ever sent where the provider addressed it.
    pub fn new() -> Self {
        Self::with_resolver(system_resolver)
    }

    /// `new()`, with the lookup that decides whether a DNS failure is `PreSend` replaced.
    pub fn with_resolver(resolver: Resolver) -> Self {
        let config = Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .user_agent(USER_AGENT)
            .tls_config(
                TlsConfig::builder()
                    .root_certs(RootCerts::PlatformVerifier)
                    .build(),
            )
            .build();
        Self {
            agent: config.into(),
            resolver,
        }
    }
}

impl Default for UreqHttp {
    fn default() -> Self {
        Self::new()
    }
}

/// An I/O error that can only happen while connecting, before a byte of the request is sent.
fn connect_failure(kind: io::ErrorKind) -> bool {
    matches!(
        kind,
        io::ErrorKind::ConnectionRefused
            | io::ErrorKind::NetworkUnreachable
            | io::ErrorKind::HostUnreachable
            | io::ErrorKind::AddrNotAvailable
            | io::ErrorKind::NetworkDown
    )
}

/// §4.4: `PreSend` only for failures that provably precede sending (resolving, connecting, a
/// request that could not even be built); everything else is `Ambiguous`. TLS errors are
/// `Ambiguous` too: rustls can also surface one while the response is being read, after the
/// request left, so a handshake failure is misfiled as `ambiguous`, the harmless direction.
fn classify(e: ureq::Error) -> HttpError {
    use ureq::{Error, Timeout};
    let detail = e.to_string();
    match e {
        Error::HostNotFound
        | Error::ConnectionFailed
        | Error::BadUri(_)
        | Error::Http(_)
        | Error::RequireHttpsOnly(_)
        | Error::InvalidProxyUrl
        | Error::ConnectProxyFailed(_)
        | Error::TlsRequired
        | Error::Timeout(Timeout::Resolve | Timeout::Connect) => HttpError::PreSend(detail),
        Error::Io(ref io) if connect_failure(io.kind()) => HttpError::PreSend(detail),
        _ => HttpError::Ambiguous(detail),
    }
}

/// Resolves the URL's host within `timeout`, so a DNS failure is a certain `PreSend`: `ureq`
/// reports a failed lookup as a plain I/O error, which on its own could also have come after
/// the request left.
fn resolve(url: &str, timeout: Duration, resolver: Resolver) -> Result<(), HttpError> {
    let uri: ureq::http::Uri = url
        .parse()
        .map_err(|e| HttpError::PreSend(format!("invalid URL: {e}")))?;
    let host = uri
        .host()
        .ok_or_else(|| HttpError::PreSend("the URL has no host".into()))?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_owned();
    let port = uri
        .port_u16()
        .unwrap_or(if uri.scheme_str() == Some("http") { 80 } else { 443 });
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(resolver(&host, port).map(|addrs| addrs.len()));
    });
    match rx.recv_timeout(timeout) {
        Ok(Ok(n)) if n > 0 => Ok(()),
        Ok(Ok(_)) => Err(HttpError::PreSend("the host did not resolve".into())),
        Ok(Err(e)) => Err(HttpError::PreSend(format!("could not resolve the host: {e}"))),
        Err(_) => Err(HttpError::PreSend("resolving the host timed out".into())),
    }
}

impl Http for UreqHttp {
    fn send(&self, req: &HttpRequest) -> Result<HttpResponse, HttpError> {
        let started = Instant::now();
        resolve(&req.url, req.timeout, self.resolver)?;
        let remaining = req.timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(HttpError::PreSend(
                "the request timed out before it was sent".into(),
            ));
        }
        // The agent sets tagteam's User-Agent; a provider's own is dropped (§4.4).
        let headers = req
            .headers
            .iter()
            .filter(|(k, _)| !k.eq_ignore_ascii_case("user-agent"));
        let sent = match req.method {
            Method::Get => {
                let mut b = self.agent.get(&req.url);
                for (k, v) in headers {
                    b = b.header(*k, v.as_str());
                }
                b.config().timeout_global(Some(remaining)).build().call()
            }
            Method::Post => {
                let mut b = self.agent.post(&req.url);
                for (k, v) in headers {
                    b = b.header(*k, v.as_str());
                }
                let b = b.config().timeout_global(Some(remaining)).build();
                match &req.body {
                    Some(body) => b.send(body.as_slice()),
                    None => b.send_empty(),
                }
            }
        };
        let mut resp = sent.map_err(classify)?;
        let status = resp.status().as_u16();
        let headers = resp
            .headers()
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().to_ascii_lowercase(),
                    String::from_utf8_lossy(v.as_bytes()).into_owned(),
                )
            })
            .collect();
        // A response was already arriving, so the request was acted on: a body that cannot be
        // read in full is `Ambiguous`, never `PreSend`.
        let body = resp
            .body_mut()
            .with_config()
            .limit(MAX_BODY as u64)
            .read_to_vec()
            .map_err(|e| HttpError::Ambiguous(format!("reading the response body: {e}")))?;
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_failures_before_sending_are_pre_send() {
        use ureq::{Error, Timeout};
        let pre = [
            Error::HostNotFound,
            Error::ConnectionFailed,
            Error::Timeout(Timeout::Resolve),
            Error::Timeout(Timeout::Connect),
            Error::Io(io::Error::from(io::ErrorKind::ConnectionRefused)),
            Error::Io(io::Error::from(io::ErrorKind::NetworkUnreachable)),
        ];
        for e in pre {
            let shown = e.to_string();
            assert!(matches!(classify(e), HttpError::PreSend(_)), "{shown}");
        }
        let ambiguous = [
            Error::Timeout(Timeout::Global),
            Error::Timeout(Timeout::SendBody),
            Error::Timeout(Timeout::RecvResponse),
            Error::Timeout(Timeout::RecvBody),
            Error::Io(io::Error::from(io::ErrorKind::ConnectionReset)),
            Error::Io(io::Error::from(io::ErrorKind::UnexpectedEof)),
            // A TLS error can surface after the request left, so it is never `PreSend`.
            Error::Tls("handshake failed"),
        ];
        for e in ambiguous {
            let shown = e.to_string();
            assert!(matches!(classify(e), HttpError::Ambiguous(_)), "{shown}");
        }
    }
}
```

Register it in `crates/tagteam-engine/src/lib.rs`, after `pub mod lifecycle;`:

```rust
pub mod net;
```

If a variant named in `classify` does not exist in the `ureq` 3 release Cargo resolved, check
`https://docs.rs/ureq/<resolved version>/ureq/enum.Error.html`, keep every variant that
exists, and leave the rest to the `_` arm. That is the safe direction: anything not listed is
`Ambiguous`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p tagteam-engine --test net && cargo test -p tagteam-engine --lib net::`
Expected: PASS, 9 integration tests and 1 unit test. `a_host_that_does_not_resolve_is_pre_send`
injects the failed lookup, so no test asks a real DNS resolver.

- [ ] **Step 6: Give the engine its port**

In `crates/tagteam-engine/src/engine.rs`:

Change the provider import line

```rust
use tagteam_provider::{Clock, Env, MutationGuard, Provider, Read};
```

to

```rust
use tagteam_provider::{Clock, Env, Http, MutationGuard, Provider, Read};
```

Add the field to `EngineConfig`, after `pub clock: Arc<dyn Clock>,`:

```rust
    /// Every network request goes through this port (§4.4).
    pub http: Arc<dyn Http>,
```

Add the field to `Engine`, after `pub(crate) clock: Arc<dyn Clock>,`:

```rust
    pub(crate) http: Arc<dyn Http>,
```

In `Engine::new`, after `clock: cfg.clock,`:

```rust
            http: cfg.http,
```

And add this method after `pub fn now_ms(&self) -> i64 { … }`:

```rust
    /// The port providers send their requests through (§4.4).
    pub fn http(&self) -> &dyn Http {
        self.http.as_ref()
    }
```

In the same file's `#[cfg(test)] mod tests`, give `test_engine` a port that never sends:
after `clock: Arc::new(tagteam_provider::SystemClock),` add

```rust
            http: Arc::new(tagteam_provider::NoHttp),
```

- [ ] **Step 7: Give the engine fixture its scripted port**

In `crates/tagteam-engine/tests/common/mod.rs`:

Add `ScriptedHttp` to the `tagteam_provider` import:

```rust
use tagteam_provider::{
    Credential, Env, FakeClock, FakeKeychain, Identity, MutationGuard, ProcessStamp, Provider, Read,
    ScriptedHttp,
};
```

Add the field to `Fx`, after `pub clock: Arc<FakeClock>,`:

```rust
    /// Every engine this fixture builds sends through this one scripted port.
    pub http: Arc<ScriptedHttp>,
```

In `Fx::build`, after `let clock = Arc::new(FakeClock::new(1_790_000_000_000));`:

```rust
        let http = Arc::new(ScriptedHttp::new());
```

In the same function's `EngineConfig { … }`, after `clock: clock.clone(),`:

```rust
            http: http.clone(),
```

and in the `Fx { … }` literal it returns, after `clock,`:

```rust
            http,
```

In `Fx::engine_over`, after `clock: self.clock.clone(),`:

```rust
            http: self.http.clone(),
```

- [ ] **Step 8: Give the CLI the real adapter**

In `crates/tagteam/src/app.rs`, add the import after `use tagteam_engine::lifecycle::{AddOptions, AddTokenOptions};`:

```rust
use tagteam_engine::net::UreqHttp;
```

and in `build_engine`'s `EngineConfig { … }`, after `clock: Arc::new(SystemClock),`:

```rust
        http: Arc::new(UreqHttp::new()),
```

Nothing in M1's commands sends a request, so no CLI behaviour changes: the oracle stays
`NoOracle` until Task 7.

- [ ] **Step 9: Run everything that builds an engine**

Run: `rg -n 'EngineConfig \{' crates`
Expected: the struct definition (`engine.rs`) plus exactly four construction sites, each of
which now sets `http`: the unit-test engine in `crates/tagteam-engine/src/engine.rs`, `Fx::build`
and `Fx::engine_over` in `crates/tagteam-engine/tests/common/mod.rs`, and `build_engine` in
`crates/tagteam/src/app.rs`.

Run: `cargo test --workspace --features tagteam/test-support`
Expected: PASS — every M1 test, plus Task 2's and this task's.

Run: `cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings`
Expected: no warnings.

- [ ] **Step 10: Commit**

```bash
git add Cargo.toml Cargo.lock crates/tagteam-engine/Cargo.toml crates/tagteam-engine/src/net.rs \
  crates/tagteam-engine/src/lib.rs crates/tagteam-engine/src/engine.rs \
  crates/tagteam-engine/tests/net.rs crates/tagteam-engine/tests/common/mod.rs \
  crates/tagteam/src/app.rs
git commit -m "Add the ureq HTTP adapter and give the engine its network port"
```

---

### Task 4: Trait cleanup: capabilities, kind traits, access-token facts (M-5, M-6, M-9)

**Files:**
- Modify: `crates/tagteam-provider/src/provider.rs`, `crates/tagteam-provider/src/lib.rs`
- Modify: `crates/tagteam-cc/src/shape.rs`, `crates/tagteam-cc/src/provider.rs`,
  `crates/tagteam-cc/src/config.rs`, `crates/tagteam-cc/src/live.rs`
- Modify: `crates/tagteam-engine/src/{engine.rs,error.rs,lifecycle.rs,recover.rs,switch.rs,views.rs}`
- Modify: `crates/tagteam/src/render.rs`, `crates/tagteam/src/app.rs`
- Test: `crates/tagteam-cc/src/shape.rs` (unit), `crates/tagteam-cc/tests/provider.rs`,
  `crates/tagteam-provider/src/provider.rs` (unit), `crates/tagteam-engine/tests/engine_basics.rs`,
  `crates/tagteam/tests/app.rs`

**Interfaces:**
- Consumes: the M1 trait and engine as they stand (no earlier M2a task's code).
- Produces:
```rust
// tagteam-provider (re-exported from lib.rs)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Capabilities { pub usage: bool, pub refresh: bool, pub api_keys: bool,
                          pub sessions: bool, pub statusline: bool }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KindTraits { pub refreshable: bool, pub managed_key_axis: bool,
                        pub default_email_prefix: Option<&'static str>, pub display: Option<&'static str> }
// ProviderError::ConfigUnsplicable { path: PathBuf, remedy: &'static str }
// Provider trait gains:
fn capabilities(&self) -> Capabilities;
fn kind_traits(&self, kind: &str) -> KindTraits;
fn access_expires_at(&self, secret: &[u8]) -> Option<i64>;
fn access_fingerprint(&self, secret: &[u8]) -> Option<Fingerprint>;
// and `write_credential` loses its `live: &LiveAuth` parameter:
fn write_credential<'l>(&self, env: &Env, locks: &'l LiveLocks<'_>, target: &StoredLogin,
    before_fallback: BeforeFallback<'_>) -> Result<Written<'l>, ProviderError>;

// tagteam-cc
pub const CONFIG_REMEDY: &str;                                      // provider.rs
pub fn shape::access_token(bytes: &[u8]) -> Option<String>;
pub fn shape::access_expires_at(bytes: &[u8]) -> Option<i64>;
pub fn shape::scopes(bytes: &[u8]) -> Vec<String>;
pub fn shape::is_expired(bytes: &[u8], now_ms: i64) -> bool;
pub fn shape::kind_traits(kind: &str) -> KindTraits;

// tagteam-engine
// `lifecycle::KIND_API_KEY` is gone; `Engine::providers()` is gone.
pub(crate) fn Axis::of(p: &dyn Provider, kind: &str) -> Axis;    // switch.rs
pub struct AccountView { pub row: AccountRow, pub active: bool, pub kind: KindTraits }
impl Engine { pub fn account_view(&self, row: AccountRow, active: bool) -> AccountView; }  // views.rs
```

`Engine::account_view` exists because the CLI builds
`AccountView`s in two places (the result of `remove` and of the metadata commands), and it must
not look up kind traits itself. An unregistered provider's row gets all-false traits.

- [ ] **Step 1: Write the failing `shape` tests**

Append to the `mod tests` block of `crates/tagteam-cc/src/shape.rs`:
```rust
    #[test]
    fn access_token_facts_read_the_access_token() {
        let c = json!({"claudeAiOauth": {
            "accessToken": "at-1", "refreshToken": "rt", "expiresAt": 1_000_000i64,
            "scopes": ["user:inference", "user:profile"]
        }})
        .to_string()
        .into_bytes();
        assert_eq!(access_token(&c).as_deref(), Some("at-1"));
        assert_eq!(access_expires_at(&c), Some(1_000_000));
        assert_eq!(scopes(&c), vec!["user:inference", "user:profile"]);
        // §7.2: expired once `now + 5 min >= expiresAt`.
        assert!(!is_expired(&c, 1_000_000 - 300_001));
        assert!(is_expired(&c, 1_000_000 - 300_000));
    }

    #[test]
    fn a_missing_or_non_numeric_expiry_is_unknown_and_never_expired() {
        for exp in [json!(null), json!("1790000000000"), json!({"at": 1})] {
            let c = json!({"claudeAiOauth": {"accessToken": "at", "expiresAt": exp}})
                .to_string()
                .into_bytes();
            assert_eq!(access_expires_at(&c), None, "{exp}");
            assert!(!is_expired(&c, i64::MAX - 1), "{exp}");
        }
        let bare = json!({"claudeAiOauth": {"accessToken": "at"}})
            .to_string()
            .into_bytes();
        assert_eq!(access_expires_at(&bare), None);
        assert!(scopes(&bare).is_empty());
        assert_eq!(access_token(b"sk-ant-api03-k"), None);
        assert_eq!(access_expires_at(b"sk-ant-api03-k"), None);
        let empty = json!({"claudeAiOauth": {"accessToken": ""}}).to_string().into_bytes();
        assert_eq!(access_token(&empty), None, "an empty token is no token");
    }

    #[test]
    fn kind_traits_follow_the_plan_table() {
        let plain = KindTraits {
            refreshable: false,
            managed_key_axis: false,
            default_email_prefix: None,
            display: None,
        };
        assert_eq!(
            kind_traits(KIND_OAUTH),
            KindTraits {
                refreshable: true,
                ..plain
            }
        );
        assert_eq!(
            kind_traits(KIND_SETUP_TOKEN),
            KindTraits {
                default_email_prefix: Some("setup-token"),
                display: Some("setup token"),
                ..plain
            }
        );
        assert_eq!(
            kind_traits(KIND_API_KEY),
            KindTraits {
                managed_key_axis: true,
                default_email_prefix: Some("api-key"),
                display: Some("api key"),
                ..plain
            }
        );
        assert_eq!(kind_traits("not-a-kind"), plain);
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p tagteam-cc --lib shape`
Expected: FAIL to compile: `cannot find function 'access_token' in this scope` (and likewise
`access_expires_at`, `scopes`, `is_expired`, `kind_traits`, and the type `KindTraits`).

- [ ] **Step 3: Add `Capabilities` and `KindTraits` to `tagteam-provider`**

In `crates/tagteam-provider/src/provider.rs`, insert after the `IdentitySurface` struct (after its
closing `}` at the end of `pub machine_shared_keys: Vec<&'static str>,\n}`):
```rust

/// What a provider can do at all (§4.5). A missing capability degrades the engine rather than
/// failing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Capabilities {
    pub usage: bool,
    pub refresh: bool,
    pub api_keys: bool,
    pub sessions: bool,
    pub statusline: bool,
}

/// What the engine and the CLI need to know about one credential kind, so neither ever names a
/// provider's kind strings (§4.5 "Kind traits").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KindTraits {
    /// The gate may refresh a credential of this kind (§7.3).
    pub refreshable: bool,
    /// Lives on the separate managed-key axis, not in the credential entry (§9.4 step 7).
    pub managed_key_axis: bool,
    /// The prefix of a defaulted `add-token` email (§10.2): `<prefix>-<N>@token.local`.
    pub default_email_prefix: Option<&'static str>,
    /// What `list` prints after the account ("api key"); `None` prints nothing.
    pub display: Option<&'static str>,
}
```

In `crates/tagteam-provider/src/lib.rs`, replace the `pub use provider::{…}` block with:
```rust
pub use provider::{
    BeforeFallback, Capabilities, CapturedLogin, DoomedEntry, Identity, IdentitySurface,
    KindTraits, LiveAuth, LiveChange, LiveLockSet, LiveLocks, Provider, ProviderError,
    SecretStore, StoredLogin, Undo, Written,
};
```

- [ ] **Step 4: Add the `shape` functions**

In `crates/tagteam-cc/src/shape.rs`, change the import line
`use tagteam_provider::{Identity, ProviderError};` to:
```rust
use tagteam_provider::{Identity, KindTraits, ProviderError};
```
and insert after `pub fn login_expires_at(…) { … }`:
```rust

/// The access token, when the credential has a non-empty one.
pub fn access_token(bytes: &[u8]) -> Option<String> {
    oauth_obj(bytes).and_then(|o| token(&o, "accessToken").map(str::to_owned))
}

/// `claudeAiOauth.expiresAt` in epoch ms; `None` when it is missing or not an integer (§7.2: a
/// non-numeric `expiresAt` counts as not expired).
pub fn access_expires_at(bytes: &[u8]) -> Option<i64> {
    oauth_obj(bytes)?.get("expiresAt")?.as_i64()
}

/// The credential's recorded scopes; empty when it records none.
pub fn scopes(bytes: &[u8]) -> Vec<String> {
    oauth_obj(bytes)
        .and_then(|o| o.get("scopes").and_then(Value::as_array).cloned())
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// §7.2: expired means `now_ms + 5 min >= expiresAt`; an unknown expiry never is.
pub fn is_expired(bytes: &[u8], now_ms: i64) -> bool {
    access_expires_at(bytes).is_some_and(|at| now_ms.saturating_add(300_000) >= at)
}

/// Claude Code's kind traits (§4.5). A kind this provider never stores has none.
pub fn kind_traits(kind: &str) -> KindTraits {
    let plain = KindTraits {
        refreshable: false,
        managed_key_axis: false,
        default_email_prefix: None,
        display: None,
    };
    match kind {
        KIND_OAUTH => KindTraits {
            refreshable: true,
            ..plain
        },
        KIND_SETUP_TOKEN => KindTraits {
            default_email_prefix: Some("setup-token"),
            display: Some("setup token"),
            ..plain
        },
        KIND_API_KEY => KindTraits {
            managed_key_axis: true,
            default_email_prefix: Some("api-key"),
            display: Some("api key"),
            ..plain
        },
        _ => plain,
    }
}
```

- [ ] **Step 5: Run the `shape` tests to verify they pass**

Run: `cargo test -p tagteam-cc --lib shape`
Expected: PASS, including the three new tests.

- [ ] **Step 6: Write the failing provider tests**

In `crates/tagteam-cc/tests/provider.rs`, change the `tagteam_provider` import to:
```rust
use tagteam_core::Fingerprint;
use tagteam_provider::{
    Capabilities, Env, FakeKeychain, KindTraits, MutationGuard, Provider, ProviderError, Read,
    SecretStore, StoredLogin,
};
```
and append:
```rust
#[test]
fn claude_code_has_every_capability_and_the_kind_table() {
    let f = fx();
    assert_eq!(
        f.cc.capabilities(),
        Capabilities {
            usage: true,
            refresh: true,
            api_keys: true,
            sessions: true,
            statusline: true,
        }
    );
    for kind in f.cc.credential_kinds() {
        assert_eq!(f.cc.kind_traits(kind), tagteam_cc::shape::kind_traits(kind));
    }
    assert_eq!(
        f.cc.kind_traits("api_key"),
        KindTraits {
            refreshable: false,
            managed_key_axis: true,
            default_email_prefix: Some("api-key"),
            display: Some("api key"),
        }
    );
    assert!(f.cc.kind_traits("oauth").refreshable);
}

#[test]
fn access_token_facts_come_from_the_access_token_not_the_lineage() {
    let f = fx();
    let bytes = json!({"claudeAiOauth": {
        "accessToken": "at-1", "refreshToken": "rt-1", "expiresAt": 1_790_003_600_000i64
    }})
    .to_string()
    .into_bytes();
    assert_eq!(f.cc.access_expires_at(&bytes), Some(1_790_003_600_000));
    assert_eq!(
        f.cc.access_fingerprint(&bytes),
        Some(Fingerprint::of_secret(b"at-1"))
    );
    assert_ne!(f.cc.access_fingerprint(&bytes), f.cc.fingerprint(&bytes));
    // A setup token's lineage is its access token; an API key has no access token.
    let setup = tagteam_cc::shape::setup_token_credential("tok");
    assert_eq!(f.cc.access_fingerprint(&setup), f.cc.fingerprint(&setup));
    assert_eq!(f.cc.access_expires_at(&setup), None);
    assert_eq!(f.cc.access_fingerprint(b"sk-ant-api03-k"), None);
}
```

- [ ] **Step 7: Run them to verify they fail**

Run: `cargo test -p tagteam-cc --test provider`
Expected: FAIL to compile: `no method named 'capabilities' found for struct 'ClaudeCode'` (and
likewise `kind_traits`, `access_expires_at`, `access_fingerprint`).

- [ ] **Step 8: Add the four methods to the trait and to `ClaudeCode`**

In `crates/tagteam-provider/src/provider.rs`, inside `pub trait Provider`, replace:
```rust
    fn id(&self) -> ProviderId;
    fn display_name(&self) -> &'static str;
    fn identity_surface(&self, env: &Env) -> IdentitySurface;
    fn identity_key(&self, id: &Identity) -> IdentityKey;
    fn credential_kinds(&self) -> &'static [&'static str];
```
with:
```rust
    fn id(&self) -> ProviderId;
    fn display_name(&self) -> &'static str;
    fn capabilities(&self) -> Capabilities;
    fn identity_surface(&self, env: &Env) -> IdentitySurface;
    fn identity_key(&self, id: &Identity) -> IdentityKey;
    fn credential_kinds(&self) -> &'static [&'static str];
    fn kind_traits(&self, kind: &str) -> KindTraits;
```
and replace:
```rust
    fn login_expires_at(&self, secret: &[u8]) -> Option<i64>;
```
with:
```rust
    fn login_expires_at(&self, secret: &[u8]) -> Option<i64>;
    /// Epoch ms of the access token's expiry; `None` when absent or not an integer (§7.2).
    fn access_expires_at(&self, secret: &[u8]) -> Option<i64>;
    /// The access token's own fingerprint. The gate's "someone already refreshed" check
    /// (§7.3 step 4) and M2b's `rejected_fp` compare access tokens, not lineages.
    fn access_fingerprint(&self, secret: &[u8]) -> Option<Fingerprint>;
```

In `crates/tagteam-cc/src/provider.rs`, change the `tagteam_provider` import to add
`Capabilities` and `KindTraits`:
```rust
use tagteam_provider::{
    BeforeFallback, Capabilities, Credential, DoomedEntry, Env, Identity, IdentitySurface,
    Keychain, KindTraits, LiveAuth, LiveChange, LiveLocks, MutationGuard, Provider,
    ProviderError, Read, StoredLogin, Undo, Written,
};
```
In `impl Provider for ClaudeCode`, after `fn display_name(…) { "Claude Code" }` insert:
```rust

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            usage: true,
            refresh: true,
            api_keys: true,
            sessions: true,
            statusline: true,
        }
    }
```
after `fn credential_kinds(…) { &KINDS }` insert:
```rust

    fn kind_traits(&self, kind: &str) -> KindTraits {
        shape::kind_traits(kind)
    }
```
and after `fn login_expires_at(…) { shape::login_expires_at(secret) }` insert:
```rust

    fn access_expires_at(&self, secret: &[u8]) -> Option<i64> {
        shape::access_expires_at(secret)
    }

    fn access_fingerprint(&self, secret: &[u8]) -> Option<Fingerprint> {
        shape::access_token(secret).map(|t| Fingerprint::of_secret(t.as_bytes()))
    }
```

- [ ] **Step 9: Run the provider tests to verify they pass**

Run: `cargo test -p tagteam-cc`
Expected: PASS.

- [ ] **Step 10: Commit**

```bash
git add crates/tagteam-provider/src/provider.rs crates/tagteam-provider/src/lib.rs \
  crates/tagteam-cc/src/shape.rs crates/tagteam-cc/src/provider.rs crates/tagteam-cc/tests/provider.rs
git commit -m "Give providers capabilities, kind traits and access-token facts"
```

- [ ] **Step 11: Write the failing test for a provider-neutral unsplicable-config error (M-6)**

Append to the `mod tests` block of `crates/tagteam-provider/src/provider.rs`:
```rust
    #[test]
    fn an_unsplicable_config_names_the_file_and_the_providers_remedy() {
        let e = ProviderError::ConfigUnsplicable {
            path: PathBuf::from("/h/.agent.json"),
            remedy: "repair it, then retry",
        };
        assert_eq!(
            e.to_string(),
            "/h/.agent.json is torn or not a JSON object; repair it, then retry"
        );
    }
```

- [ ] **Step 12: Run it to verify it fails**

Run: `cargo test -p tagteam-provider --lib provider`
Expected: FAIL to compile: `variant 'ProviderError::ConfigUnsplicable' has no field named 'path'`.

- [ ] **Step 13: Make the variant a struct and move the wording into `tagteam-cc`**

In `crates/tagteam-provider/src/provider.rs`, replace:
```rust
    #[error(
        "{0} is torn or not a JSON object; restore it from Claude Code's backups (~/.claude/backups/) or repair it, then retry"
    )]
    ConfigUnsplicable(PathBuf),
```
with:
```rust
    /// A config file that is torn or not a JSON object is never replaced (§9.5). `remedy` is
    /// the provider's advice on repairing it.
    #[error("{} is torn or not a JSON object; {remedy}", path.display())]
    ConfigUnsplicable {
        path: PathBuf,
        remedy: &'static str,
    },
```

In `crates/tagteam-cc/src/provider.rs`, after the `LIVE_NOT_FRESH` constant, add:
```rust

/// What to do about a `~/.claude.json` that cannot be spliced (§9.5).
pub const CONFIG_REMEDY: &str =
    "restore it from Claude Code's backups (~/.claude/backups/) or repair it, then retry";
```

In `crates/tagteam-cc/src/config.rs`, add `use crate::provider::CONFIG_REMEDY;` after
`use crate::paths::CcPaths;`, and replace the start of `splice_key`'s body:
```rust
    let before = read_bytes(path);
    let unsplicable = |_| ProviderError::ConfigUnsplicable(path.to_path_buf());
    let new = match (&before, value) {
        (Read::Unreadable(_), _) => {
            return Err(ProviderError::ConfigUnsplicable(path.to_path_buf()));
        }
```
with:
```rust
    let before = read_bytes(path);
    let unsplicable = || ProviderError::ConfigUnsplicable {
        path: path.to_path_buf(),
        remedy: CONFIG_REMEDY,
    };
    let new = match (&before, value) {
        (Read::Unreadable(_), _) => return Err(unsplicable()),
```
and the two splice arms:
```rust
        (Read::Present(b), Some(v)) => splice::replace_top_level(b, key, v).map_err(unsplicable)?,
        (Read::Present(b), None) => splice::remove_top_level(b, key).map_err(unsplicable)?,
```
with:
```rust
        (Read::Present(b), Some(v)) => {
            splice::replace_top_level(b, key, v).map_err(|_| unsplicable())?
        }
        (Read::Present(b), None) => splice::remove_top_level(b, key).map_err(|_| unsplicable())?,
```

In `crates/tagteam-cc/src/live.rs`, add `use crate::provider::CONFIG_REMEDY;` after
`use crate::paths::CcPaths;`, and in `write_managed_key` replace:
```rust
            Read::Unreadable(_) => {
                return Err(ProviderError::ConfigUnsplicable(
                    paths.global_config.clone(),
                ));
            }
```
with:
```rust
            Read::Unreadable(_) => {
                return Err(ProviderError::ConfigUnsplicable {
                    path: paths.global_config.clone(),
                    remedy: CONFIG_REMEDY,
                });
            }
```

In `crates/tagteam-engine/src/error.rs`, replace the `kind()` arm
`EngineError::Provider(ProviderError::ConfigUnsplicable(_)) => "config-unsplicable",` with:
```rust
            EngineError::Provider(ProviderError::ConfigUnsplicable { .. }) => "config-unsplicable",
```
and in `kind_is_pinned_for_every_variant` replace:
```rust
                EngineError::Provider(ProviderError::ConfigUnsplicable(PathBuf::from("x"))),
```
with:
```rust
                EngineError::Provider(ProviderError::ConfigUnsplicable {
                    path: PathBuf::from("x"),
                    remedy: "r",
                }),
```

- [ ] **Step 14: Remove the dead trait surface (M-9)**

`Engine::providers()` has no caller, and `write_credential`'s `live` parameter is unused.

1. In `crates/tagteam-engine/src/engine.rs`, delete:
```rust
    pub fn providers(&self) -> Vec<Arc<dyn Provider>> {
        self.registry.all().to_vec()
    }

```
2. In `crates/tagteam-provider/src/provider.rs`, replace the `write_credential` declaration:
```rust
    fn write_credential<'l>(
        &self,
        env: &Env,
        locks: &'l LiveLocks<'_>,
        target: &StoredLogin,
        live: &LiveAuth,
        before_fallback: BeforeFallback<'_>,
    ) -> Result<Written<'l>, ProviderError>;
```
with:
```rust
    fn write_credential<'l>(
        &self,
        env: &Env,
        locks: &'l LiveLocks<'_>,
        target: &StoredLogin,
        before_fallback: BeforeFallback<'_>,
    ) -> Result<Written<'l>, ProviderError>;
```
3. In `crates/tagteam-cc/src/provider.rs`, delete these four lines from `fn write_credential`'s
   parameter list:
```rust
        // Never composed from: a caller's read may have gone stale by the time the write
        // actually happens under the locks, so `write_credential` re-reads fresh instead
        // (see `fresh_live_object`).
        _live: &LiveAuth,
```
   `LiveAuth` stays imported: `read_live_auth` still returns it.
4. In `crates/tagteam-engine/src/switch.rs`, in `fn apply`, delete the parameter line
   `        live: &LiveAuth,` and change the write to:
```rust
            p.write_credential(&self.env, locks, target_login, before_fallback)
```
   In `fn transact`, change the call `match self.apply(` so that its argument list no longer
   passes `&live` (the fourth argument):
```rust
        let stored_in = match self.apply(
            p,
            &target,
            &target_login,
            outgoing.as_ref(),
            req,
            &mut tx,
            &mut before_fallback,
        ) {
```
5. In `crates/tagteam-cc/tests/provider.rs`, remove the `&live` argument from every
   `write_credential(` call (12 calls). Delete each `let live = f.cc.read_live_auth(…);`
   binding that is then unused: the ones after `lock_live` at the current lines 68, 116,
   134, 215, 268, 333, 363, 396, 425, 484 and 517. Keep the binding at line 456
   (`let live = f.cc.read_live_auth(&f.env); // the caller's read: unreadable`), whose next
   line asserts on `live.credential`. Then run
   `rg -n 'write_credential\(' crates/` and confirm that no call passes five arguments.

- [ ] **Step 15: Run the affected crates to verify they pass**

Run: `cargo test -p tagteam-provider -p tagteam-cc -p tagteam-engine --features tagteam-engine/test-hooks`
Expected: PASS, including `an_unsplicable_config_names_the_file_and_the_providers_remedy` and the
existing `live_store.rs` test that expects "backups" in the message.

- [ ] **Step 16: Commit**

```bash
git add crates/tagteam-provider/src/provider.rs crates/tagteam-cc/src/provider.rs \
  crates/tagteam-cc/src/config.rs crates/tagteam-cc/src/live.rs crates/tagteam-cc/tests/provider.rs \
  crates/tagteam-engine/src/engine.rs crates/tagteam-engine/src/error.rs crates/tagteam-engine/src/switch.rs
git commit -m "Move Claude Code's config remedy into its provider and drop dead trait surface"
```

- [ ] **Step 17: Write the failing tests for kind traits in views and the CLI (M-5)**

Append to `crates/tagteam-engine/tests/engine_basics.rs`:
```rust
#[test]
fn views_carry_each_rows_kind_traits_from_its_provider() {
    let fx = Fx::new();
    fx.add("a@b.co", "rt-a");
    fx.add_api_key(common::API_KEY);
    let lists = fx.engine.accounts(None).unwrap();
    let kinds: Vec<_> = lists[0]
        .accounts
        .iter()
        .map(|v| (v.row.kind.as_str(), v.kind))
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("oauth", fx.cc.kind_traits("oauth")),
            ("api_key", fx.cc.kind_traits("api_key")),
        ]
    );
    let key_row = lists[0].accounts[1].row.clone();
    let view = fx.engine.account_view(key_row, false);
    assert!(view.kind.managed_key_axis);
    assert_eq!(view.kind.display, Some("api key"));
}
```

Append to `crates/tagteam/tests/app.rs`:
```rust
#[test]
fn token_accounts_list_their_kind_from_the_provider() {
    // M-5: the CLI names no kind strings. The provider's kind traits decide the label, the
    // JSON `usageStatus`, and the default email.
    let h = H::new();
    h.ok(&["add-token", "sk-ant-oat01-setup"]);
    h.ok(&["add-token", "sk-ant-api03-key"]);
    assert_eq!(
        h.ok(&["list"]),
        "  1  setup-token-1@token.local  setup token\n  2  api-key-2@token.local  api key\n"
    );
    let v = h.json(&["list", "--json"]);
    assert_eq!(v["accounts"][0]["usageStatus"], "unavailable");
    assert_eq!(v["accounts"][1]["usageStatus"], "api_key");
}
```

- [ ] **Step 18: Run them to verify they fail**

Run: `cargo test -p tagteam-engine --test engine_basics views_carry && cargo test -p tagteam --test app token_accounts`
Expected: the engine test fails to compile (`no field 'kind' on type '&AccountView'`, `no
method named 'account_view'`). The CLI test passes already, since today's hard-coded strings
render the same. It stays as the pin that the rewrite below must keep green.

- [ ] **Step 19: Route kinds through the provider (engine)**

1. `crates/tagteam-engine/src/lifecycle.rs`: delete
```rust
/// The credential kind whose secret lives on the managed-key axis rather than in the
/// credential entry (§9.4 step 7).
pub(crate) const KIND_API_KEY: &str = "api_key";

```
   and add, after `fn alias_taken`:
```rust

/// §10.2: a token kind whose provider names no default email must be given `--email`.
fn no_default_email(kind: &str) -> EngineError {
    EngineError::InvalidInput(format!(
        "a {kind} token has no default email address; pass --email"
    ))
}
```
   In `add_token`, replace:
```rust
        let (kind, secret) = p.token_secret(&opts.token);
        let prefix = if kind == KIND_API_KEY {
            "api-key"
        } else {
            "setup-token"
        };
```
   with:
```rust
        let (kind, secret) = p.token_secret(&opts.token);
        let prefix = p.kind_traits(&kind).default_email_prefix;
        // Refused before anything exists (§5): the lock-held branch below cannot need it.
        if opts.email.is_none() && prefix.is_none() {
            return Err(no_default_email(&kind));
        }
```
   and replace the identity match:
```rust
        let identity = match &opts.email {
            Some(email) => p.token_identity(email),
            // Under the lock the position is authoritative, so a default email follows it.
            None => {
                let position = match opts.position {
                    Some(pos) => pos,
                    None => store.next_position(&opts.provider)?,
                };
                unused_token_identity(&store, p.as_ref(), &opts.provider, prefix, position)?
            }
        };
```
   with:
```rust
        let identity = match (&opts.email, prefix) {
            (Some(email), _) => p.token_identity(email),
            // Under the lock the position is authoritative, so a default email follows it.
            (None, Some(prefix)) => {
                let position = match opts.position {
                    Some(pos) => pos,
                    None => store.next_position(&opts.provider)?,
                };
                unused_token_identity(&store, p.as_ref(), &opts.provider, prefix, position)?
            }
            (None, None) => return Err(no_default_email(&kind)),
        };
```
2. `crates/tagteam-engine/src/switch.rs`: delete `use crate::lifecycle::KIND_API_KEY;` and replace
   `Axis::of`:
```rust
    pub(crate) fn of(kind: &str) -> Self {
        if kind == KIND_API_KEY {
            Axis::ManagedKey
        } else {
            Axis::Entry
        }
    }
```
   with:
```rust
    /// The axis `p` keeps a credential of `kind` on (§9.4 step 7).
    pub(crate) fn of(p: &dyn Provider, kind: &str) -> Self {
        if p.kind_traits(kind).managed_key_axis {
            Axis::ManagedKey
        } else {
            Axis::Entry
        }
    }
```
   Then change each call site to pass the provider:
   - `oracle_hint`: `Axis::of(p, &out.kind).live_secret(&p.read_live_auth(&self.env))?`
   - `plan` (self-switch): `let reconcile = Axis::of(p, &target.kind)`
   - `transact`, after `settle_outgoing`: `if let Some(bytes) = Axis::of(p, &out.kind).live_secret(&live) {`
   - `transact`, step 6: `.and_then(|o| Axis::of(p, &o.kind).live_secret(&live))`
   - `settle_outgoing`: `let Some(bytes) = Axis::of(p, &out.kind).live_secret(live) else {`
3. `crates/tagteam-engine/src/recover.rs`: in `finish_forward`, change `let own = Axis::of(&to.kind);`
   to `let own = Axis::of(p, &to.kind);`; in `finish_backward`, change
   `let own = from.as_ref().map_or(Axis::Entry, |r| Axis::of(&r.kind));` to
   `let own = from.as_ref().map_or(Axis::Entry, |r| Axis::of(p, &r.kind));`.
4. `crates/tagteam-engine/src/views.rs`: change the imports to
```rust
use tagteam_core::ProviderId;
use tagteam_provider::{KindTraits, Read};
```
   replace the `AccountView` struct with:
```rust
#[derive(Debug, Clone)]
pub struct AccountView {
    pub row: AccountRow,
    /// The live identity wins over the store's active account.
    pub active: bool,
    /// The row's credential kind, as its provider describes it (§4.5).
    pub kind: KindTraits,
}

/// The kind traits of a row whose provider this build does not register: nothing special.
const UNREGISTERED: KindTraits = KindTraits {
    refreshable: false,
    managed_key_axis: false,
    default_email_prefix: None,
    display: None,
};
```
   in `fn view`, replace:
```rust
                AccountView { row, active }
```
   with:
```rust
                AccountView {
                    kind: p.kind_traits(&row.kind),
                    row,
                    active,
                }
```
   and add to `impl Engine` (after `fn view`):
```rust

    /// A row as the views show it, for a caller that already knows whether it is active.
    pub fn account_view(&self, row: AccountRow, active: bool) -> AccountView {
        let kind = self
            .registry
            .get(&row.provider)
            .map_or(UNREGISTERED, |p| p.kind_traits(&row.kind));
        AccountView { row, active, kind }
    }
```
5. Verify: `rg -n 'KIND_API_KEY|"api_key"|"setup_token"' crates/tagteam-engine/src`
   prints nothing. (The CLI's `render.rs` still names the kinds until Step 20.) (`engine.rs`'s own unit test keeps its literal `"oauth"` fixture kind, since
   no provider is registered there.)

- [ ] **Step 20: Route kinds through the provider (CLI)**

In `crates/tagteam/src/render.rs`, replace `usage_status`:
```rust
fn usage_status(r: &AccountRow) -> &'static str {
    if r.kind == "api_key" {
        "api_key"
    } else if r.quarantine_reason.is_some() {
        "relogin_required"
    } else {
        "unavailable"
    }
}
```
with:
```rust
/// §13.2: a managed-key account has no usage to fetch, which cswap reports as `api_key`.
fn usage_status(v: &AccountView) -> &'static str {
    if v.kind.managed_key_axis {
        "api_key"
    } else if v.row.quarantine_reason.is_some() {
        "relogin_required"
    } else {
        "unavailable"
    }
}
```
In `row_json`, change `let status = usage_status(r);` to `let status = usage_status(v);`. In
`list_human`, replace:
```rust
            match r.kind.as_str() {
                "api_key" => line.push_str("  api key"),
                "setup_token" => line.push_str("  setup token"),
                _ => {}
            }
```
with:
```rust
            if let Some(kind) = v.kind.display {
                line.push_str(&format!("  {kind}"));
            }
```

In `crates/tagteam/src/app.rs`, replace (in `Command::Remove`):
```rust
                self.print_view(&human, AccountView { row, active }, None);
```
with:
```rust
                let view = self.engine.account_view(row, active);
                self.print_view(&human, view, None);
```
and in `print_account`, replace:
```rust
        self.print_view(human, AccountView { row, active }, created);
```
with:
```rust
        let view = self.engine.account_view(row, active);
        self.print_view(human, view, created);
```
Then verify with `rg -n '"setup_token"|"oauth"|kind == "' crates/tagteam/src`: nothing prints.
`rg -n '"api_key"' crates/tagteam/src` prints exactly one line, the `usageStatus` value that
`usage_status` returns (`"api_key"` is cswap's output string there, not a kind check).

- [ ] **Step 21: Run the whole workspace to verify it passes**

Run:
```bash
cargo fmt --all
cargo test --workspace --features tagteam/test-support
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
```
Expected: every test passes, including `views_carry_each_rows_kind_traits_from_its_provider`,
`token_accounts_list_their_kind_from_the_provider`, and the existing `key_row`/`api key` list
pins in `tests/app.rs` and `tests/cli.rs`. No warnings.

- [ ] **Step 22: Commit**

```bash
git add crates/tagteam-engine/src/lifecycle.rs crates/tagteam-engine/src/switch.rs \
  crates/tagteam-engine/src/recover.rs crates/tagteam-engine/src/views.rs \
  crates/tagteam-engine/tests/engine_basics.rs crates/tagteam/src/render.rs crates/tagteam/src/app.rs \
  crates/tagteam/tests/app.rs
git commit -m "Route credential kinds through the provider's kind traits"
```

---

### Task 5: The live-lock split

**Files:**
- Modify: `crates/tagteam-provider/src/provider.rs`, `crates/tagteam-provider/src/lib.rs`
- Modify: `crates/tagteam-cc/src/locks.rs`, `crates/tagteam-cc/src/provider.rs`
- Test: `crates/tagteam-provider/src/provider.rs` (unit), `crates/tagteam-cc/src/locks.rs` (unit),
  `crates/tagteam-cc/tests/provider.rs`

**Interfaces:**
- Consumes: the trait as Task 4 leaves it.
- Produces:
```rust
// tagteam-provider (CredLocks re-exported from lib.rs)
pub struct CredLocks<'g> { /* set, PhantomData<&'g MutationGuard> */ }
impl<'g> CredLocks<'g> {
    pub fn new(guard: &'g MutationGuard, set: Box<dyn LiveLockSet + 'g>) -> Self;
    pub fn check_owned(&self) -> Result<(), LockError>;
    pub fn with_config(self, config: Box<dyn LiveLockSet + 'g>) -> LiveLocks<'g>;
}
pub struct LiveLocks<'g> { /* config: Option<Box<dyn LiveLockSet + 'g>>, cred: CredLocks<'g> */ }
impl<'g> LiveLocks<'g> {
    pub fn new(guard: &'g MutationGuard, set: Box<dyn LiveLockSet + 'g>) -> Self;   // unchanged signature
    pub fn check_owned(&self) -> Result<(), LockError>;   // credential locks, then config
}
// Provider trait: `lock_live` becomes a provided method; three required methods are added
fn live_lock_budget(&self) -> Duration;
fn lock_credentials<'g>(&self, env: &Env, g: &'g MutationGuard, budget: Duration)
    -> Result<CredLocks<'g>, ProviderError>;
fn lock_config<'g>(&self, env: &Env, cred: CredLocks<'g>, budget: Duration)
    -> Result<LiveLocks<'g>, ProviderError>;
fn lock_live<'g>(&self, env: &Env, g: &'g MutationGuard) -> Result<LiveLocks<'g>, ProviderError>; // provided

// tagteam-cc locks.rs: CcCredSet and CcConfigSet replace CcLockSet; acquire/acquire_with are gone
pub struct CcCredSet;   // impl LiveLockSet
pub struct CcConfigSet; // impl LiveLockSet
pub fn acquire_credentials(paths: &CcPaths, timeout: Duration) -> Result<CcCredSet, LockError>;
pub fn acquire_config(paths: &CcPaths, timeout: Duration) -> Result<CcConfigSet, LockError>;
// ClaudeCode::with_lock_timeout (test-hooks) keeps its name and now sets the whole budget.
```

Every caller of `lock_live` (switch, recovery, `add_live`) is unchanged: the provided method takes
both stages under the provider's one budget, as the combined lock set did. Only active-token
refresh (Task 16) takes the stages apart. The `compile_fail` doctest on `Undo` builds
`LiveLocks::new(&guard, …)`, whose signature is unchanged, so it stays valid as written.

- [ ] **Step 1: Write the failing `tagteam-provider` tests**

In the `mod tests` block of `crates/tagteam-provider/src/provider.rs`, add after the `Held` type and
its `live_locks_delegate_ownership_checks` test:
```rust
    /// A lock set that records when it is released.
    struct Recorded(&'static str, std::sync::Arc<std::sync::Mutex<Vec<&'static str>>>);
    impl LiveLockSet for Recorded {
        fn check_owned(&self) -> Result<(), LockError> {
            Ok(())
        }
    }
    impl Drop for Recorded {
        fn drop(&mut self) {
            self.1.lock().unwrap().push(self.0);
        }
    }

    fn guard(d: &tempfile::TempDir) -> (Env, MutationGuard) {
        let env = Env::for_test(d.path());
        let g = MutationGuard::acquire(&env, std::time::Duration::from_millis(100)).unwrap();
        (env, g)
    }

    fn owned(on: bool) -> Box<Held> {
        Box::new(Held(std::cell::Cell::new(on)))
    }

    #[test]
    fn the_config_lock_is_released_before_the_credential_locks() {
        let d = tempfile::tempdir().unwrap();
        let (_env, g) = guard(&d);
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let locks = CredLocks::new(&g, Box::new(Recorded("credentials", log.clone())))
            .with_config(Box::new(Recorded("config", log.clone())));
        drop(locks);
        assert_eq!(*log.lock().unwrap(), ["config", "credentials"]);
    }

    #[test]
    fn a_compromised_config_lock_or_credential_lock_is_reported() {
        let d = tempfile::tempdir().unwrap();
        let (_env, g) = guard(&d);
        let config_lost = CredLocks::new(&g, owned(true)).with_config(owned(false));
        assert!(matches!(
            config_lost.check_owned(),
            Err(LockError::Compromised(_))
        ));
        let cred_lost = CredLocks::new(&g, owned(false)).with_config(owned(true));
        assert!(matches!(
            cred_lost.check_owned(),
            Err(LockError::Compromised(_))
        ));
        assert!(CredLocks::new(&g, owned(true)).check_owned().is_ok());
        assert!(
            CredLocks::new(&g, owned(true))
                .with_config(owned(true))
                .check_owned()
                .is_ok()
        );
    }

    #[test]
    fn both_stages_share_one_budget() {
        // §9.1: what the credential stage spends is gone from the config stage's share.
        let d = tempfile::tempdir().unwrap();
        let (_env, g) = guard(&d);
        let budget = std::time::Duration::from_millis(500);
        let given = std::cell::Cell::new(None);
        let locks = staged(
            budget,
            |b| {
                assert_eq!(b, budget, "the credential stage starts with the whole budget");
                std::thread::sleep(std::time::Duration::from_millis(200));
                Ok::<_, ProviderError>(CredLocks::new(&g, owned(true)))
            },
            |cred, b| {
                given.set(Some(b));
                Ok(cred.with_config(owned(true)))
            },
        );
        assert!(locks.is_ok());
        let left = given.get().unwrap();
        assert!(left <= std::time::Duration::from_millis(300), "{left:?}");
    }

    #[test]
    fn a_spent_budget_leaves_the_config_stage_nothing() {
        let d = tempfile::tempdir().unwrap();
        let (_env, g) = guard(&d);
        let given = std::cell::Cell::new(None);
        let _ = staged(
            std::time::Duration::from_millis(100),
            |_| {
                std::thread::sleep(std::time::Duration::from_millis(150));
                Ok::<_, ProviderError>(CredLocks::new(&g, owned(true)))
            },
            |cred, b| {
                given.set(Some(b));
                Ok(cred.with_config(owned(true)))
            },
        );
        assert_eq!(given.get(), Some(std::time::Duration::ZERO));
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p tagteam-provider --lib provider`
Expected: FAIL to compile: undeclared type `CredLocks`, and `staged` not found in this scope.

- [ ] **Step 3: Split `LiveLocks` and add the stages to the trait**

In `crates/tagteam-provider/src/provider.rs`, add `use std::time::{Duration, Instant};` after
`use std::path::PathBuf;`, then replace:
```rust
/// A provider's live locks. Only constructible from a held `MutationGuard` (§4.3).
pub struct LiveLocks<'g> {
    set: Box<dyn LiveLockSet + 'g>,
    _guard: PhantomData<&'g MutationGuard>,
}

impl<'g> LiveLocks<'g> {
    pub fn new(_guard: &'g MutationGuard, set: Box<dyn LiveLockSet + 'g>) -> Self {
        Self {
            set,
            _guard: PhantomData,
        }
    }

    pub fn check_owned(&self) -> Result<(), LockError> {
        self.set.check_owned()
    }
}
```
with:
```rust
/// A provider's credential locks, the first stage of its live locks (§4.3). Only
/// constructible from a held `MutationGuard`.
pub struct CredLocks<'g> {
    set: Box<dyn LiveLockSet + 'g>,
    _guard: PhantomData<&'g MutationGuard>,
}

impl<'g> CredLocks<'g> {
    pub fn new(_guard: &'g MutationGuard, set: Box<dyn LiveLockSet + 'g>) -> Self {
        Self {
            set,
            _guard: PhantomData,
        }
    }

    pub fn check_owned(&self) -> Result<(), LockError> {
        self.set.check_owned()
    }

    /// The config lock can only be added to held credential locks (§4.3).
    pub fn with_config(self, config: Box<dyn LiveLockSet + 'g>) -> LiveLocks<'g> {
        LiveLocks {
            config: Some(config),
            cred: self,
        }
    }
}

/// A provider's live locks: its credential locks plus its config lock (§4.3). Fields drop in
/// declaration order, so the config lock is released before the credential locks.
pub struct LiveLocks<'g> {
    config: Option<Box<dyn LiveLockSet + 'g>>,
    cred: CredLocks<'g>,
}

impl<'g> LiveLocks<'g> {
    /// One lock set covering both stages: a provider with a single live lock, and tests.
    pub fn new(guard: &'g MutationGuard, set: Box<dyn LiveLockSet + 'g>) -> Self {
        Self {
            config: None,
            cred: CredLocks::new(guard, set),
        }
    }

    /// The credential locks, then the config lock.
    pub fn check_owned(&self) -> Result<(), LockError> {
        self.cred.check_owned()?;
        match &self.config {
            Some(config) => config.check_owned(),
            None => Ok(()),
        }
    }
}

/// `lock_live`'s two stages under one budget (§9.1): whatever the credential stage spends is
/// gone from the config stage's share.
fn staged<'g, E>(
    budget: Duration,
    credentials: impl FnOnce(Duration) -> Result<CredLocks<'g>, E>,
    config: impl FnOnce(CredLocks<'g>, Duration) -> Result<LiveLocks<'g>, E>,
) -> Result<LiveLocks<'g>, E> {
    let deadline = Instant::now() + budget;
    let held = credentials(budget)?;
    config(held, deadline.saturating_duration_since(Instant::now()))
}
```

In `pub trait Provider`, replace:
```rust
    fn lock_live<'g>(
        &self,
        env: &Env,
        g: &'g MutationGuard,
    ) -> Result<LiveLocks<'g>, ProviderError>;
```
with:
```rust
    /// How long the live locks may take, both stages together (§9.1: CC 9 s).
    fn live_lock_budget(&self) -> Duration;
    /// The first stage of the live locks (for CC: the refresh lock, then the legacy lock).
    fn lock_credentials<'g>(
        &self,
        env: &Env,
        g: &'g MutationGuard,
        budget: Duration,
    ) -> Result<CredLocks<'g>, ProviderError>;
    /// The second stage, taken only from held credential locks. On failure `cred` is dropped,
    /// which releases it.
    fn lock_config<'g>(
        &self,
        env: &Env,
        cred: CredLocks<'g>,
        budget: Duration,
    ) -> Result<LiveLocks<'g>, ProviderError>;
    /// Both stages under one budget (§9.1). Provided; providers do not override it.
    fn lock_live<'g>(
        &self,
        env: &Env,
        g: &'g MutationGuard,
    ) -> Result<LiveLocks<'g>, ProviderError> {
        staged(
            self.live_lock_budget(),
            |budget| self.lock_credentials(env, g, budget),
            |cred, budget| self.lock_config(env, cred, budget),
        )
    }
```

In `crates/tagteam-provider/src/lib.rs`, add `CredLocks` to the `pub use provider::{…}` list,
keeping it alphabetical:
```rust
pub use provider::{
    BeforeFallback, Capabilities, CapturedLogin, CredLocks, DoomedEntry, Identity,
    IdentitySurface, KindTraits, LiveAuth, LiveChange, LiveLockSet, LiveLocks, Provider,
    ProviderError, SecretStore, StoredLogin, Undo, Written,
};
```

- [ ] **Step 4: Run the `tagteam-provider` tests to verify they pass**

Run: `cargo test -p tagteam-provider`
Expected: PASS, including the four new tests, the existing `live_locks_delegate_ownership_checks`,
and the `compile_fail` doctest on `Undo`. (`tagteam-cc` does not compile yet: it still
implements `lock_live` and not the new stages. Step 7 fixes that.)

- [ ] **Step 5: Write the failing `tagteam-cc` tests**

Replace the whole `#[cfg(test)] mod tests` block of `crates/tagteam-cc/src/locks.rs` with:
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
    fn the_credential_locks_never_touch_the_config_lock() {
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        let set = acquire_credentials(&p, ACQUIRE_TIMEOUT).unwrap();
        assert!(p.refresh_lock.is_dir() && p.legacy_lock().is_dir());
        assert!(!p.config_lock.exists(), "only the config stage takes it");
        assert!(set.check_owned().is_ok());
        assert!(!p.config_home.join(".oauth_refresh.lock.owner").exists());
        drop(set);
        assert!(!p.refresh_lock.exists() && !p.legacy_lock().exists());
    }

    #[test]
    fn the_config_lock_is_taken_and_released_on_its_own() {
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        let set = acquire_config(&p, ACQUIRE_TIMEOUT).unwrap();
        assert!(p.config_lock.is_dir());
        assert!(!p.refresh_lock.exists() && !p.legacy_lock().exists());
        assert!(set.check_owned().is_ok());
        drop(set);
        assert!(!p.config_lock.exists());
    }

    #[test]
    fn a_contended_legacy_lock_releases_the_refresh_lock_while_waiting() {
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        fs::create_dir(p.legacy_lock()).unwrap(); // CC holds it, freshly
        assert!(matches!(
            acquire_credentials(&p, Duration::from_millis(700)),
            Err(LockError::Timeout(_))
        ));
        assert!(
            !p.refresh_lock.exists(),
            "the refresh lock must not be held while waiting"
        );
        assert!(p.legacy_lock().is_dir(), "CC's lock is left alone");
    }

    /// Proves the release happens *during* the wait, not just after `acquire_credentials` gives
    /// up: while a background acquirer is genuinely still blocked on the (still-held) legacy
    /// lock, this thread must itself be able to actually acquire the refresh lock. That's true
    /// no matter how long the lock sits free between the background thread's retries — a
    /// microsecond or a second — unlike polling `exists()`, which can miss a window far
    /// narrower than its poll interval.
    ///
    /// The refresh lock being free proves nothing until the acquirer has actually tried it, so
    /// the test first waits for evidence that it has: taking the refresh lock and releasing it on
    /// the legacy contention (`mkdir`, then `rmdir`) changes the mtime of the directory holding
    /// it. Without that, this thread could take the free lock before the acquirer ever ran.
    #[test]
    fn the_refresh_lock_is_actually_free_during_a_legacy_wait_not_only_after_it() {
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        fs::create_dir(p.legacy_lock()).unwrap(); // CC holds it, freshly
        let lock_dir = p.refresh_lock.parent().unwrap().to_path_buf();
        let untouched = fs::metadata(&lock_dir).unwrap().modified().unwrap();
        let p2 = p.clone();
        // Generous on purpose: only this test's own deadline below is meant to be tight; this
        // just must outlast it plus the eventual release.
        let handle = thread::spawn(move || acquire_credentials(&p2, Duration::from_secs(30)));

        let tried = Instant::now() + Duration::from_secs(10);
        while fs::metadata(&lock_dir).unwrap().modified().unwrap() == untouched {
            assert!(
                Instant::now() < tried,
                "the acquirer never reached the refresh lock"
            );
            thread::sleep(Duration::from_millis(5));
        }
        assert!(p.legacy_lock().is_dir(), "the legacy lock is still held: it is contended");

        let refresh_spec = MkdirLockSpec::new(p.refresh_lock.clone(), CRED_STALE, Duration::ZERO);
        let deadline = Instant::now() + Duration::from_secs(10);
        let observed = loop {
            if let Some(lock) = MkdirLock::try_acquire(&refresh_spec).unwrap() {
                break lock;
            }
            assert!(
                Instant::now() < deadline,
                "the refresh lock was never free while the acquirer waited on the legacy lock"
            );
        };
        assert!(
            !handle.is_finished(),
            "it should still be waiting on the legacy lock, not have given up or succeeded"
        );
        drop(observed);

        fs::remove_dir(p.legacy_lock()).unwrap(); // CC releases its lock
        let set = handle.join().unwrap().unwrap();
        assert!(p.legacy_lock().is_dir());
        drop(set);
        assert!(!p.refresh_lock.exists() && !p.legacy_lock().exists() && !p.config_lock.exists());
    }

    #[test]
    fn a_held_refresh_lock_times_out_without_touching_it() {
        // Review Focus 1 (M1): CC is mid-refresh.
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        fs::create_dir(&p.refresh_lock).unwrap();
        let start = std::time::Instant::now();
        assert!(matches!(
            acquire_credentials(&p, Duration::from_millis(500)),
            Err(LockError::Timeout(_))
        ));
        assert!(start.elapsed() < Duration::from_secs(2));
        assert!(p.refresh_lock.is_dir());
    }

    #[test]
    fn a_held_config_lock_times_out_without_touching_it() {
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        fs::create_dir(&p.config_lock).unwrap(); // something else holds it, freshly
        assert!(matches!(
            acquire_config(&p, Duration::from_millis(500)),
            Err(LockError::Timeout(_))
        ));
        assert!(p.config_lock.is_dir(), "the other holder's lock is left alone");
    }
}
```

In `crates/tagteam-cc/tests/provider.rs`, add `LockError` to the `tagteam_provider` import and
`use std::time::Instant;` after `use std::time::Duration;`, then append:
```rust
#[test]
fn claude_code_s_live_locks_share_the_nine_second_budget_of_section_9_1() {
    assert_eq!(fx().cc.live_lock_budget(), Duration::from_secs(9));
}

#[test]
fn the_stages_are_taken_separately_and_released_config_first() {
    let f = fx();
    let paths = CcPaths::resolve(&f.env);
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let cred = f
        .cc
        .lock_credentials(&f.env, &g, Duration::from_secs(1))
        .unwrap();
    assert!(paths.refresh_lock.is_dir() && paths.legacy_lock().is_dir());
    assert!(!paths.config_lock.exists(), "credential locks alone never take it");
    let live = f
        .cc
        .lock_config(&f.env, cred, Duration::from_secs(1))
        .unwrap();
    assert!(paths.config_lock.is_dir());
    assert!(live.check_owned().is_ok());
    drop(live);
    assert!(
        !paths.refresh_lock.exists()
            && !paths.legacy_lock().exists()
            && !paths.config_lock.exists()
    );
}

#[test]
fn a_held_config_lock_releases_the_credential_locks_it_was_given() {
    let f = fx();
    let paths = CcPaths::resolve(&f.env);
    fs::create_dir(&paths.config_lock).unwrap(); // someone else holds it, freshly
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let cred = f
        .cc
        .lock_credentials(&f.env, &g, Duration::from_millis(500))
        .unwrap();
    let start = Instant::now();
    assert!(matches!(
        f.cc.lock_config(&f.env, cred, Duration::from_millis(500)),
        Err(ProviderError::Lock(LockError::Timeout(_)))
    ));
    assert!(start.elapsed() < Duration::from_millis(1500));
    assert!(!paths.refresh_lock.exists() && !paths.legacy_lock().exists());
    assert!(paths.config_lock.is_dir(), "the other holder's lock is left alone");
}

/// §9.1: one budget covers both stages. The legacy lock is held for most of a 2 s budget and a
/// config lock for good, so the credential stage spends about 1.5 s. With one shared budget the
/// config stage gets what remains, and `lock_live` gives up by ~2.5 s. A fresh budget per
/// stage would take at least 3.5 s.
#[cfg(feature = "test-hooks")]
#[test]
fn lock_live_spends_one_budget_across_both_stages() {
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    fs::create_dir_all(env.home.join(".claude")).unwrap();
    let kc = Arc::new(FakeKeychain::new());
    let cc = ClaudeCode::with_store(
        LiveStore::new(kc, Platform::MacOs).with_retry_delay(Duration::ZERO),
    )
    .with_lock_timeout(Duration::from_secs(2));
    let paths = CcPaths::resolve(&env);
    fs::create_dir(&paths.config_lock).unwrap();
    fs::create_dir(paths.legacy_lock()).unwrap();
    let legacy = paths.legacy_lock();
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(1500));
        fs::remove_dir(legacy).unwrap();
    });
    let g = MutationGuard::acquire(&env, Duration::from_secs(1)).unwrap();
    let start = Instant::now();
    assert!(matches!(
        cc.lock_live(&env, &g),
        Err(ProviderError::Lock(LockError::Timeout(_)))
    ));
    let spent = start.elapsed();
    release.join().unwrap();
    assert!(spent < Duration::from_millis(3000), "{spent:?}");
    assert!(!paths.refresh_lock.exists() && !paths.legacy_lock().exists());
}
```

- [ ] **Step 6: Run them to verify they fail**

Run: `cargo test -p tagteam-cc --lib locks && cargo test -p tagteam-cc --features test-hooks --test provider`
Expected: FAIL to compile: `cannot find function 'acquire_credentials'`, then
`not all trait items implemented, missing: 'live_lock_budget', 'lock_credentials', 'lock_config'`.

- [ ] **Step 7: Split CC's locks and implement the stages**

In `crates/tagteam-cc/src/locks.rs`, replace everything from `/// CC's credential locks and config
lock (§9.1).` down to the end of `pub fn acquire_with` (the `CcLockSet` struct, its
`LiveLockSet` impl, `acquire` and `acquire_with`) with:
```rust
/// CC's credential locks (§9.1): the refresh lock, then the legacy lock. Fields drop in
/// declaration order, so the legacy lock is released first and the refresh lock last.
pub struct CcCredSet {
    legacy: MkdirLock,
    refresh: MkdirLock,
}

impl LiveLockSet for CcCredSet {
    fn check_owned(&self) -> Result<(), LockError> {
        self.refresh.check_owned()?;
        self.legacy.check_owned()
    }
}

/// CC's config lock (§9.1), the second stage of its live locks.
pub struct CcConfigSet {
    config: MkdirLock,
}

impl LiveLockSet for CcConfigSet {
    fn check_owned(&self) -> Result<(), LockError> {
        self.config.check_owned()
    }
}

/// The refresh lock, then the legacy lock. If the legacy lock is contended the refresh lock is
/// released and the pair retried, as CC does. tagteam never writes `.oauth_refresh.lock.owner`.
pub fn acquire_credentials(paths: &CcPaths, timeout: Duration) -> Result<CcCredSet, LockError> {
    let deadline = Instant::now() + timeout;
    let remaining = || deadline.saturating_duration_since(Instant::now());
    loop {
        let refresh = MkdirLock::acquire(&MkdirLockSpec::new(
            paths.refresh_lock.clone(),
            CRED_STALE,
            remaining(),
        ))?;
        let legacy_spec = MkdirLockSpec::new(paths.legacy_lock(), CRED_STALE, Duration::ZERO);
        match MkdirLock::try_acquire(&legacy_spec)? {
            Some(legacy) => return Ok(CcCredSet { legacy, refresh }),
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

/// The config lock alone. A caller takes it only while holding the credential locks
/// (`CredLocks::with_config`, §4.3).
pub fn acquire_config(paths: &CcPaths, timeout: Duration) -> Result<CcConfigSet, LockError> {
    Ok(CcConfigSet {
        config: MkdirLock::acquire(&MkdirLockSpec::new(
            paths.config_lock.clone(),
            CONFIG_STALE,
            timeout,
        ))?,
    })
}
```

In `crates/tagteam-cc/src/provider.rs`:
1. Add `CredLocks` to the `tagteam_provider` import list.
2. Replace the struct and its constructors:
```rust
pub struct ClaudeCode {
    live: Arc<LiveStore>,
    /// How long `lock_live` waits for CC's locks: `locks::ACQUIRE_TIMEOUT`, except in tests.
    lock_timeout: Duration,
}
```
with:
```rust
pub struct ClaudeCode {
    live: Arc<LiveStore>,
    /// How long CC's live locks may take, both stages together (§9.1):
    /// `locks::ACQUIRE_TIMEOUT`, except in tests.
    lock_budget: Duration,
}
```
   In `with_store`, change `lock_timeout: locks::ACQUIRE_TIMEOUT,` to
   `lock_budget: locks::ACQUIRE_TIMEOUT,`. Replace `with_lock_timeout`'s doc comment and body:
```rust
    /// A shorter budget for CC's locks, so a test of a held lock need not wait the full 9 s.
    #[cfg(feature = "test-hooks")]
    pub fn with_lock_timeout(mut self, timeout: Duration) -> Self {
        self.lock_budget = timeout;
        self
    }
```
3. In `impl Provider for ClaudeCode`, replace the whole `fn lock_live<'g>(…) { … }` with:
```rust
    fn live_lock_budget(&self) -> Duration {
        self.lock_budget
    }

    fn lock_credentials<'g>(
        &self,
        env: &Env,
        g: &'g MutationGuard,
        budget: Duration,
    ) -> Result<CredLocks<'g>, ProviderError> {
        let set = locks::acquire_credentials(&CcPaths::resolve(env), budget)?;
        Ok(CredLocks::new(g, Box::new(set)))
    }

    fn lock_config<'g>(
        &self,
        env: &Env,
        cred: CredLocks<'g>,
        budget: Duration,
    ) -> Result<LiveLocks<'g>, ProviderError> {
        // On a timeout `cred` is dropped as this returns, releasing the credential locks.
        let set = locks::acquire_config(&CcPaths::resolve(env), budget)?;
        Ok(cred.with_config(Box::new(set)))
    }
```
4. Confirm that nothing else names the old functions:
   `rg -n 'CcLockSet|acquire_with|locks::acquire\b|lock_timeout' crates/` should print only
   `with_lock_timeout` (the method, its callers, and the new test that uses it) and
   `Fx::with_lock_timeout`; no `CcLockSet`, `acquire_with`, `locks::acquire` or `lock_timeout` field.

- [ ] **Step 8: Run the CC tests to verify they pass**

Run: `cargo test -p tagteam-cc && cargo test -p tagteam-cc --features test-hooks --test provider`
Expected: PASS, including every rewritten `locks.rs` test and the four new provider tests.
`lock_live_spends_one_budget_across_both_stages` takes about 2.5 s.

- [ ] **Step 9: Run the engine suites unchanged**

Run: `cargo fmt --all && cargo test -p tagteam-engine --features test-hooks && cargo test -p tagteam --features test-support && cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings`
Expected: PASS, with no change to any engine or CLI test. Switch, recovery and `add_live` still
call `lock_live`. `cc_holding_its_refresh_lock_blocks_the_switch_for_the_injected_timeout` and
`a_recovery_that_cannot_take_cc_s_lock_names_it_instead_of_advising_force` still see a
`lock-timeout` naming `.oauth_refresh.lock` within the 300 ms budget they inject.

- [ ] **Step 10: Commit**

```bash
git add crates/tagteam-provider/src/provider.rs crates/tagteam-provider/src/lib.rs \
  crates/tagteam-cc/src/locks.rs crates/tagteam-cc/src/provider.rs crates/tagteam-cc/tests/provider.rs
git commit -m "Split the live locks into credential locks and a config lock under one budget"
```

---

### Task 6: The `FakeAgent` crate and provider-neutrality tests

**Files:**
- Modify: `Cargo.toml` (workspace dependency `tagteam-fake`)
- Create: `crates/tagteam-fake/Cargo.toml`, `crates/tagteam-fake/src/{lib.rs,paths.rs,shape.rs,provider.rs}`
- Test: `crates/tagteam-fake/tests/provider.rs`
- Modify: `crates/tagteam-engine/Cargo.toml` (dev-dependency), `crates/tagteam-engine/tests/common/mod.rs`
- Test: `crates/tagteam-engine/tests/fake_agent.rs`

**Interfaces:**
- Consumes: the trait as Tasks 4 and 5 leave it (`Capabilities`, `KindTraits`, `CredLocks`,
  `lock_credentials`/`lock_config`/`live_lock_budget`, `write_credential` without `live`,
  `ConfigUnsplicable { path, remedy }`). From Task 3: `Fx.http: Arc<ScriptedHttp>` and
  `EngineConfig.http`. From Task 4: `AccountView.kind`.
- Produces (crate `tagteam-fake`, `publish = false`):
```rust
pub const FAKE_AGENT: &str = "fake-agent";
pub const KIND_TOKEN: &str = "fa_token";
pub const KIND_STATIC: &str = "fa_static";
pub const KINDS: [&str; 2];
pub const DEVICE: &str = "device";                    // its one machine-shared credential key
pub const LOGIN_EXPIRES: i64 = 4_102_444_800_000;     // the access expiry `login` writes
pub struct FakeAgent;                                  // impl Provider, Default
impl FakeAgent {
    pub fn new() -> Self;                              // base "https://fake-agent.invalid", lock budget 5 s
    pub fn with_endpoint_base(self, base: &str) -> Self;
    pub fn with_lock_budget(self, budget: Duration) -> Self;
    pub fn renew_url(&self) -> String;                 // "<base>/fa/renew"
    pub fn whoami_url(&self) -> String;                // "<base>/fa/whoami"
}
pub struct FakePaths { pub dir: PathBuf, pub identity: PathBuf, pub credential: PathBuf, pub lock: PathBuf }
impl FakePaths { pub fn resolve(env: &Env) -> Self; }
pub fn credential_json(token: &str, renew: Option<&str>, expires: Option<i64>) -> Value;
pub fn identity_json(handle: &str, workspace: &str, uid: &str) -> Value;
pub fn login(env: &Env, handle: &str, workspace: &str, token: &str, renew: &str);   // uid "uid-<handle>"
```
- Produces (engine test fixture `tests/common/mod.rs`):
```rust
impl Fx {
    pub fn assert_only_surface_changed_for(&self, surface: &IdentitySurface,
        before: &HomeSnapshot, after: &HomeSnapshot, step: &str);   // the CC method delegates to it
}
pub struct FakeFx { pub fx: Fx, pub fake: Arc<FakeAgent>, pub engine: Engine }
impl FakeFx {
    pub fn new() -> Self;
    pub fn fake_provider(&self) -> ProviderId;
    pub fn fake_login(&self, handle: &str, token: &str, renew: &str);   // workspace "ws"
    pub fn fake_add_options(&self) -> AddOptions;
    pub fn fake_add(&self, handle: &str, token: &str, renew: &str) -> AccountId;
    pub fn switch_fake(&self, id: &AccountId) -> SwitchOutcome;
    pub fn fake_live_label(&self) -> Option<String>;
}
```

The Interface Contract names `FakeAgent`, `FakePaths`, the kind constants and the three helpers.
`KINDS`, `DEVICE` and `LOGIN_EXPIRES` are additions, and so are `FakeFx`'s `fake_provider`,
`fake_add_options`, `switch_fake` and `fake_live_label`. Tasks 7 and 8 use them in their
`FakeAgent` tests.

`FakeAgent` is deliberately unlike Claude Code (§15.2):
- **Identity:** no email. `label` is `handle@workspace` (`handle` alone when the workspace is
  empty), `org_uuid` is the workspace, and `account_uuid` is `uid`.
- **Kinds:** its own two, `fa_token` (refreshable) and `fa_static`. There is no managed-key
  axis.
- **Storage:** one credential file, whose only machine-shared key is `device`.
- **Locks:** one live lock. Its config stage adds no lock at all.

The identity lives under the `identity` key of `~/.fakeagent/identity.json`, beside an unrelated
`prefs` key that no switch may touch.

This task gives `FakeAgent` the trait as it stands after Task 5. Tasks 7 and 8 add
`resolve_owner` and `refresh` with their `FakeAgent` implementations. The engine tests below
prove the switch, store, views and the §3 invariant are already provider-neutral. If any of
them fails, the fault is a Claude Code assumption in the engine: fix it there, never by
reshaping `FakeAgent`.

- [ ] **Step 1: Write the failing crate tests**

`crates/tagteam-fake/tests/provider.rs`:
```rust
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tagteam_core::Fingerprint;
use tagteam_fake::{
    FAKE_AGENT, FakeAgent, FakePaths, KIND_STATIC, KIND_TOKEN, LOGIN_EXPIRES, credential_json,
    identity_json, login,
};
use tagteam_provider::{
    Capabilities, Env, LiveChange, LockError, MutationGuard, Provider, ProviderError, Read,
    SecretStore, StoredLogin,
};

struct Fx {
    _d: tempfile::TempDir,
    env: Env,
    fake: FakeAgent,
}

fn fx() -> Fx {
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    Fx {
        _d: d,
        env,
        fake: FakeAgent::new().with_lock_budget(Duration::from_secs(1)),
    }
}

fn file_json(path: &std::path::Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

/// A fallback hook that saves nothing: FakeAgent never falls back, so it is never called.
fn save_nothing(_: &[u8]) -> Result<(), ProviderError> {
    Ok(())
}

#[test]
fn its_identity_has_no_email_and_its_shapes_are_its_own() {
    let f = fx();
    assert_eq!(f.fake.id().as_str(), FAKE_AGENT);
    assert!(matches!(f.fake.live_identity(&f.env), Read::Absent), "no login yet");
    login(&f.env, "alice", "ws", "tok-a", "renew-a");
    let id = f.fake.live_identity(&f.env).present().unwrap();
    assert_eq!(
        (
            id.label.as_str(),
            id.email.as_deref(),
            id.org_uuid.as_str(),
            id.account_uuid.as_deref()
        ),
        ("alice@ws", None, "ws", Some("uid-alice"))
    );
    assert_eq!(id.raw, identity_json("alice", "ws", "uid-alice"));
    assert_eq!(f.fake.identity_key(&id).as_str(), "alice\nws");

    let auth = f.fake.read_live_auth(&f.env);
    assert!(matches!(auth.managed_key, Read::Absent), "no managed-key axis");
    let bytes = auth.credential.present().unwrap().bytes().to_vec();
    assert_eq!(f.fake.classify(&bytes), KIND_TOKEN);
    assert_eq!(
        f.fake.fingerprint(&bytes),
        Some(Fingerprint::of_secret(b"renew-a"))
    );
    assert_eq!(
        f.fake.access_fingerprint(&bytes),
        Some(Fingerprint::of_secret(b"tok-a"))
    );
    assert_eq!(f.fake.access_expires_at(&bytes), Some(LOGIN_EXPIRES));
    assert_eq!(f.fake.login_expires_at(&bytes), None);
    assert!(f.fake.has_refresh_token(&bytes));

    let (kind, secret) = f.fake.token_secret("  static-1 ");
    assert_eq!(kind, KIND_STATIC);
    assert_eq!(f.fake.classify(&secret), KIND_STATIC);
    assert_eq!(
        f.fake.fingerprint(&secret),
        Some(Fingerprint::of_secret(b"static-1"))
    );
    assert!(!f.fake.has_refresh_token(&secret));
    assert!(f.fake.is_wiped(br#"{"fa":{"token":"","renew":""}}"#));
    assert!(!f.fake.is_wiped(&secret));
    let t = f.fake.token_identity("fa-static-3@token.local");
    assert_eq!(
        (t.label.as_str(), t.email.as_deref(), t.account_uuid.as_deref()),
        ("fa-static-3@token.local", None, None)
    );
}

#[test]
fn kinds_capabilities_endpoints_and_surface() {
    let f = fx();
    assert_eq!(f.fake.credential_kinds(), &[KIND_TOKEN, KIND_STATIC]);
    assert_eq!(
        f.fake.capabilities(),
        Capabilities {
            refresh: true,
            ..Capabilities::default()
        }
    );
    assert!(f.fake.kind_traits(KIND_TOKEN).refreshable);
    let st = f.fake.kind_traits(KIND_STATIC);
    assert_eq!(
        (
            st.refreshable,
            st.managed_key_axis,
            st.default_email_prefix,
            st.display
        ),
        (false, false, Some("fa-static"), Some("static"))
    );
    let s = f.fake.identity_surface(&f.env);
    let p = FakePaths::resolve(&f.env);
    assert_eq!(s.json_keys, vec![(p.identity.clone(), vec!["identity".to_string()])]);
    assert_eq!(s.credential_files, vec![p.credential.clone()]);
    assert!(s.credential_items.is_empty() && s.owned_items.is_empty());
    assert_eq!(s.machine_shared_keys, vec!["device"]);
    assert_eq!(f.fake.renew_url(), "https://fake-agent.invalid/fa/renew");
    assert_eq!(
        FakeAgent::new()
            .with_endpoint_base("http://127.0.0.1:9/")
            .whoami_url(),
        "http://127.0.0.1:9/fa/whoami"
    );
}

#[test]
fn a_write_keeps_the_machines_device_key_and_undoes_exactly() {
    let f = fx();
    login(&f.env, "alice", "ws", "tok-a", "renew-a");
    let p = FakePaths::resolve(&f.env);
    // The machine's device key moved on since any stored credential was captured.
    let mut live = file_json(&p.credential);
    live["device"] = json!({"id": "machine-2"});
    fs::write(&p.credential, serde_json::to_vec(&live).unwrap()).unwrap();
    let before_cred = fs::read(&p.credential).unwrap();
    let before_identity = fs::read(&p.identity).unwrap();

    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let locks = f.fake.lock_live(&f.env, &g).unwrap();
    assert!(p.lock.is_dir(), "its one live lock");
    let bob = StoredLogin {
        kind: KIND_TOKEN.into(),
        secret: serde_json::to_vec(&credential_json("tok-b", Some("renew-b"), Some(1))).unwrap(),
        identity: f
            .fake
            .parse_identity(&identity_json("bob", "ws", "uid-bob"))
            .unwrap(),
    };
    let doomed = f.fake.doomed(&f.env, &locks, LiveChange::Write(KIND_TOKEN));
    assert_eq!(doomed.len(), 1, "the credential file, and nothing on another axis");
    assert_eq!(doomed[0].bytes.clone().present(), Some(before_cred.clone()));
    assert!(!doomed[0].on_fallback);
    assert!(
        f.fake
            .doomed(&f.env, &locks, LiveChange::ClearOther(KIND_TOKEN))
            .is_empty()
    );

    let written = f
        .fake
        .write_credential(&f.env, &locks, &bob, &mut save_nothing)
        .unwrap();
    assert_eq!(written.stored_in, SecretStore::File(p.credential.clone()));
    let identity_undo = f
        .fake
        .write_identity(&f.env, &locks, Some(&bob.identity))
        .unwrap();
    assert_eq!(
        file_json(&p.credential),
        json!({"fa": {"token": "tok-b", "renew": "renew-b", "expires": 1}, "device": {"id": "machine-2"}})
    );
    assert_eq!(
        fs::metadata(&p.credential).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let doc = file_json(&p.identity);
    assert_eq!(doc["identity"], identity_json("bob", "ws", "uid-bob"));
    assert_eq!(doc["prefs"], json!({"theme": "x"}), "the rest of identity.json stays");
    assert_eq!(
        f.fake.live_identity(&f.env).present().unwrap().label,
        "bob@ws"
    );

    f.fake
        .clear_other_axis(&f.env, &locks, KIND_TOKEN)
        .unwrap()
        .undo(&locks)
        .unwrap();
    identity_undo.undo(&locks).unwrap();
    written.undo.undo(&locks).unwrap();
    assert_eq!(fs::read(&p.credential).unwrap(), before_cred);
    assert_eq!(fs::read(&p.identity).unwrap(), before_identity);
    drop(locks);
    assert!(!p.lock.exists(), "released on drop");
}

#[test]
fn an_unreadable_live_credential_is_never_overwritten() {
    let f = fx();
    login(&f.env, "alice", "ws", "tok-a", "renew-a");
    let p = FakePaths::resolve(&f.env);
    fs::remove_file(&p.credential).unwrap();
    fs::create_dir(&p.credential).unwrap(); // reading it fails: neither present nor absent
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let locks = f.fake.lock_live(&f.env, &g).unwrap();
    let bob = StoredLogin {
        kind: KIND_TOKEN.into(),
        secret: serde_json::to_vec(&credential_json("tok-b", Some("renew-b"), None)).unwrap(),
        identity: f
            .fake
            .parse_identity(&identity_json("bob", "ws", "uid-bob"))
            .unwrap(),
    };
    assert!(matches!(
        f.fake.write_credential(&f.env, &locks, &bob, &mut save_nothing),
        Err(ProviderError::Unreadable(_))
    ));
    assert!(p.credential.is_dir(), "left exactly as found");
}

#[test]
fn its_one_live_lock_times_out_within_its_budget_and_is_left_alone() {
    let f = fx();
    login(&f.env, "alice", "ws", "tok-a", "renew-a");
    let p = FakePaths::resolve(&f.env);
    fs::create_dir(&p.lock).unwrap(); // another FakeAgent process holds it, freshly
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let start = Instant::now();
    assert!(matches!(
        f.fake.lock_live(&f.env, &g),
        Err(ProviderError::Lock(LockError::Timeout(_)))
    ));
    assert!(start.elapsed() < Duration::from_secs(3));
    assert!(p.lock.is_dir());
}

#[test]
fn a_torn_identity_file_is_unreadable_and_never_replaced() {
    let f = fx();
    login(&f.env, "alice", "ws", "tok-a", "renew-a");
    let p = FakePaths::resolve(&f.env);
    fs::write(&p.identity, b"{\"identity\": {\"handle\": ").unwrap();
    assert!(matches!(f.fake.live_identity(&f.env), Read::Unreadable(_)));
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let locks = f.fake.lock_live(&f.env, &g).unwrap();
    let id = f
        .fake
        .parse_identity(&identity_json("bob", "ws", "uid-bob"))
        .unwrap();
    match f.fake.write_identity(&f.env, &locks, Some(&id)) {
        Err(e @ ProviderError::ConfigUnsplicable { .. }) => {
            assert!(e.to_string().contains("repair or remove it"), "{e}")
        }
        other => panic!("expected ConfigUnsplicable, got {:?}", other.err()),
    }
    assert_eq!(
        fs::read(&p.identity).unwrap(),
        b"{\"identity\": {\"handle\": ".to_vec()
    );
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p tagteam-fake`
Expected: FAIL: `failed to load manifest for workspace member` `.../crates/tagteam-fake`, caused by
`failed to read .../crates/tagteam-fake/Cargo.toml`. The root workspace's `members = ["crates/*"]`
glob now matches a directory that has `tests/` but no `Cargo.toml`, so every cargo command in the
workspace fails until Step 3 creates the manifest.

- [ ] **Step 3: Create the crate**

In the root `Cargo.toml`, under `[workspace.dependencies]`, add after the `tagteam-engine` line:
```toml
tagteam-fake = { path = "crates/tagteam-fake" }
```

`crates/tagteam-fake/Cargo.toml`:
```toml
[package]
name = "tagteam-fake"
description = "A test-only second provider that keeps tagteam's Provider trait provider-neutral"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
publish = false

[dependencies]
tagteam-core.workspace = true
tagteam-provider.workspace = true
serde_json.workspace = true

[dev-dependencies]
tempfile.workspace = true

[lints]
workspace = true
```

`crates/tagteam-fake/src/lib.rs`:
```rust
//! `FakeAgent`: a test-only provider whose shapes differ from Claude Code's on purpose (§15.2).
//! It has an identity with no email, credential kinds Claude Code does not have, one live lock,
//! and no managed-key axis. Engine tests run against it beside Claude Code, so the `Provider`
//! trait cannot quietly take on Claude Code's shape before a real second provider exists.
#![forbid(unsafe_code)]

mod paths;
mod provider;
mod shape;

use std::fs;
use std::os::unix::fs::PermissionsExt;

use serde_json::{Value, json};
use tagteam_provider::Env;

pub use paths::FakePaths;
pub use provider::FakeAgent;
pub use shape::{DEVICE, KIND_STATIC, KIND_TOKEN, KINDS};

pub const FAKE_AGENT: &str = "fake-agent";

/// The access-token expiry `login` writes: far enough out that nothing freshens it unless a
/// test says so.
pub const LOGIN_EXPIRES: i64 = 4_102_444_800_000;

/// A stored or live credential. `renew` makes it a refreshable `fa_token`, and without it the
/// credential is an `fa_static`. `device` is the machine-shared key.
pub fn credential_json(token: &str, renew: Option<&str>, expires: Option<i64>) -> Value {
    let mut fa = serde_json::Map::new();
    fa.insert("token".into(), json!(token));
    if let Some(renew) = renew {
        fa.insert("renew".into(), json!(renew));
    }
    if let Some(expires) = expires {
        fa.insert("expires".into(), json!(expires));
    }
    json!({"fa": Value::Object(fa), "device": {"id": "machine-shared"}})
}

/// A FakeAgent identity object. It has no email anywhere.
pub fn identity_json(handle: &str, workspace: &str, uid: &str) -> Value {
    json!({"handle": handle, "workspace": workspace, "uid": uid})
}

/// What logging in to FakeAgent leaves behind: `identity.json` with the login and an
/// unrelated `prefs` key, and the credential file at mode 0600. A test helper that panics on
/// I/O failure.
pub fn login(env: &Env, handle: &str, workspace: &str, token: &str, renew: &str) {
    let p = FakePaths::resolve(env);
    fs::create_dir_all(&p.dir).unwrap();
    let doc = json!({
        "identity": identity_json(handle, workspace, &format!("uid-{handle}")),
        "prefs": {"theme": "x"}
    });
    fs::write(
        &p.identity,
        format!("{}\n", serde_json::to_string_pretty(&doc).unwrap()),
    )
    .unwrap();
    let cred = credential_json(token, Some(renew), Some(LOGIN_EXPIRES));
    fs::write(&p.credential, serde_json::to_vec(&cred).unwrap()).unwrap();
    fs::set_permissions(&p.credential, fs::Permissions::from_mode(0o600)).unwrap();
}
```

`crates/tagteam-fake/src/paths.rs`:
```rust
use std::path::PathBuf;

use tagteam_provider::Env;

/// Where FakeAgent keeps its state: `<home>/.fakeagent/`.
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
        let dir = env.guard(env.home.join(".fakeagent"));
        Self {
            identity: dir.join("identity.json"),
            credential: dir.join("credential.json"),
            lock: dir.join(".live.lock"),
            dir,
        }
    }
}
```

`crates/tagteam-fake/src/shape.rs`:
```rust
use serde_json::{Map, Value};
use tagteam_core::Fingerprint;
use tagteam_provider::{Identity, KindTraits, ProviderError};

/// Refreshable: carries a `renew` token.
pub const KIND_TOKEN: &str = "fa_token";
/// Not refreshable, and on the same (only) axis as `fa_token`.
pub const KIND_STATIC: &str = "fa_static";
pub const KINDS: [&str; 2] = [KIND_TOKEN, KIND_STATIC];
/// FakeAgent's only machine-shared credential key.
pub const DEVICE: &str = "device";

fn fa(bytes: &[u8]) -> Option<Map<String, Value>> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    v.get("fa")?.as_object().cloned()
}

fn field<'a>(o: &'a Map<String, Value>, k: &str) -> Option<&'a str> {
    o.get(k).and_then(Value::as_str).filter(|s| !s.is_empty())
}

pub(crate) fn token(bytes: &[u8]) -> Option<String> {
    fa(bytes).and_then(|o| field(&o, "token").map(str::to_owned))
}

pub(crate) fn renew(bytes: &[u8]) -> Option<String> {
    fa(bytes).and_then(|o| field(&o, "renew").map(str::to_owned))
}

pub(crate) fn expires(bytes: &[u8]) -> Option<i64> {
    fa(bytes)?.get("expires")?.as_i64()
}

pub(crate) fn classify(bytes: &[u8]) -> &'static str {
    if renew(bytes).is_some() {
        KIND_TOKEN
    } else {
        KIND_STATIC
    }
}

/// §2 "Generation": the renew token, else the access token.
pub(crate) fn fingerprint(bytes: &[u8]) -> Option<Fingerprint> {
    renew(bytes)
        .or_else(|| token(bytes))
        .map(|s| Fingerprint::of_secret(s.as_bytes()))
}

/// Both tokens empty: FakeAgent's own logged-out state.
pub(crate) fn is_wiped(bytes: &[u8]) -> bool {
    fa(bytes).is_some_and(|o| field(&o, "token").is_none() && field(&o, "renew").is_none())
}

pub(crate) fn kind_traits(kind: &str) -> KindTraits {
    let plain = KindTraits {
        refreshable: false,
        managed_key_axis: false,
        default_email_prefix: None,
        display: None,
    };
    match kind {
        KIND_TOKEN => KindTraits {
            refreshable: true,
            ..plain
        },
        KIND_STATIC => KindTraits {
            default_email_prefix: Some("fa-static"),
            display: Some("static"),
            ..plain
        },
        _ => plain,
    }
}

/// `None` when `raw` names no handle: no login.
pub(crate) fn identity_from(raw: &Value) -> Option<Identity> {
    let handle = raw
        .get("handle")?
        .as_str()
        .filter(|s| !s.is_empty())?
        .to_owned();
    let workspace = raw
        .get("workspace")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let uid = raw
        .get("uid")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let label = if workspace.is_empty() {
        handle
    } else {
        format!("{handle}@{workspace}")
    };
    Some(Identity {
        label,
        email: None,
        org_uuid: workspace,
        org_name: None,
        account_uuid: uid,
        raw: raw.clone(),
    })
}

/// Account-scoped keys from the target, the machine-shared `device` key from the live
/// credential, absence included.
pub(crate) fn compose(
    target: &[u8],
    live: Option<&Map<String, Value>>,
) -> Result<Vec<u8>, ProviderError> {
    let mut out = match serde_json::from_slice::<Value>(target) {
        Ok(Value::Object(o)) => o,
        _ => {
            return Err(ProviderError::Invalid(
                "the stored FakeAgent credential is not a JSON object".into(),
            ));
        }
    };
    out.shift_remove(DEVICE);
    if let Some(device) = live.and_then(|l| l.get(DEVICE)) {
        out.insert(DEVICE.to_owned(), device.clone());
    }
    Ok(serde_json::to_vec(&Value::Object(out)).expect("a Value always serializes"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn compose_takes_the_device_key_from_live_absence_included() {
        let target = json!({"fa": {"token": "t"}, "device": {"id": "stale"}});
        let live = json!({"fa": {"token": "old"}, "device": {"id": "now"}});
        let out: Value = serde_json::from_slice(
            &compose(target.to_string().as_bytes(), live.as_object()).unwrap(),
        )
        .unwrap();
        assert_eq!(out, json!({"fa": {"token": "t"}, "device": {"id": "now"}}));
        let out: Value =
            serde_json::from_slice(&compose(target.to_string().as_bytes(), None).unwrap())
                .unwrap();
        assert_eq!(out, json!({"fa": {"token": "t"}}));
    }

    #[test]
    fn an_identity_needs_a_handle() {
        assert!(identity_from(&json!({"workspace": "ws"})).is_none());
        assert!(identity_from(&json!({"handle": ""})).is_none());
        assert_eq!(identity_from(&json!({"handle": "h"})).unwrap().label, "h");
    }
}
```

`crates/tagteam-fake/src/provider.rs`:
```rust
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use tagteam_core::{Fingerprint, IdentityKey, ProviderId};
use tagteam_provider::atomic::{
    ensure_private_dir, remove_target, write_atomic_private_with, write_atomic_with,
};
use tagteam_provider::splice::{self, render_nested};
use tagteam_provider::{
    BeforeFallback, Capabilities, CredLocks, Credential, DoomedEntry, Env, Identity,
    IdentitySurface, KindTraits, LiveAuth, LiveChange, LiveLockSet, LiveLocks, LockError,
    MkdirLock, MkdirLockSpec, MutationGuard, Provider, ProviderError, Read, ReadError,
    SecretStore, StoredLogin, Undo, Written,
};

use crate::FAKE_AGENT;
use crate::paths::FakePaths;
use crate::shape::{self, DEVICE, KIND_STATIC, KINDS};

/// FakeAgent's lock goes stale like Claude Code's credential locks.
const LOCK_STALE: Duration = Duration::from_secs(60);
const DEFAULT_BASE: &str = "https://fake-agent.invalid";
const DEFAULT_LOCK_BUDGET: Duration = Duration::from_secs(5);
/// What to do about an `identity.json` that cannot be spliced.
const REMEDY: &str = "repair or remove it, then retry";

pub struct FakeAgent {
    base: String,
    lock_budget: Duration,
}

impl Default for FakeAgent {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeAgent {
    pub fn new() -> Self {
        Self {
            base: DEFAULT_BASE.to_owned(),
            lock_budget: DEFAULT_LOCK_BUDGET,
        }
    }

    pub fn with_endpoint_base(mut self, base: &str) -> Self {
        self.base = base.trim_end_matches('/').to_owned();
        self
    }

    pub fn with_lock_budget(mut self, budget: Duration) -> Self {
        self.lock_budget = budget;
        self
    }

    pub fn renew_url(&self) -> String {
        format!("{}/fa/renew", self.base)
    }

    pub fn whoami_url(&self) -> String {
        format!("{}/fa/whoami", self.base)
    }
}

fn read_file(path: &Path) -> Read<Vec<u8>> {
    match fs::read(path) {
        Ok(b) => Read::Present(b),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Read::Absent,
        Err(e) => Read::Unreadable(ReadError::new(path.display().to_string(), e.to_string())),
    }
}

fn present_or_err(r: Read<Vec<u8>>) -> Result<Option<Vec<u8>>, ProviderError> {
    match r {
        Read::Present(b) => Ok(Some(b)),
        Read::Absent => Ok(None),
        Read::Unreadable(e) => Err(ProviderError::Unreadable(e)),
    }
}

struct FakeLock(MkdirLock);

impl LiveLockSet for FakeLock {
    fn check_owned(&self) -> Result<(), LockError> {
        self.0.check_owned()
    }
}

/// FakeAgent has one live lock, so its config stage adds none.
struct NoConfigLock;

impl LiveLockSet for NoConfigLock {
    fn check_owned(&self) -> Result<(), LockError> {
        Ok(())
    }
}

/// Restores the exact bytes one write replaced, or removes a file the write created. The bytes
/// may be a secret, so there is no `Debug`.
struct FileUndo {
    path: PathBuf,
    before: Option<Vec<u8>>,
    /// A secret file: restored at 0600 whatever its mode, like every credential write.
    private: bool,
}

impl Undo for FileUndo {
    fn undo(self: Box<Self>, locks: &LiveLocks<'_>) -> Result<(), ProviderError> {
        let fence = || locks.check_owned().map_err(ProviderError::from);
        match &self.before {
            Some(b) if self.private => write_atomic_private_with(&self.path, b, 0o600, fence),
            Some(b) => write_atomic_with(&self.path, b, 0o600, fence),
            None => {
                fence()?;
                Ok(remove_target(&self.path)?)
            }
        }
    }

    fn what(&self) -> String {
        format!("restore {}", self.path.display())
    }
}

/// For a change that wrote nothing.
struct NothingToUndo;

impl Undo for NothingToUndo {
    fn undo(self: Box<Self>, _locks: &LiveLocks<'_>) -> Result<(), ProviderError> {
        Ok(())
    }

    fn what(&self) -> String {
        "nothing".into()
    }
}

impl Provider for FakeAgent {
    fn id(&self) -> ProviderId {
        ProviderId::new(FAKE_AGENT)
    }

    fn display_name(&self) -> &'static str {
        "FakeAgent"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            refresh: true,
            ..Capabilities::default()
        }
    }

    fn identity_surface(&self, env: &Env) -> IdentitySurface {
        let p = FakePaths::resolve(env);
        IdentitySurface {
            json_keys: vec![(p.identity, vec!["identity".into()])],
            credential_files: vec![p.credential],
            credential_items: vec![],
            owned_items: vec![],
            machine_shared_keys: vec![DEVICE],
        }
    }

    fn identity_key(&self, id: &Identity) -> IdentityKey {
        let handle = id
            .raw
            .get("handle")
            .and_then(Value::as_str)
            .unwrap_or(id.label.as_str());
        IdentityKey::new(format!("{handle}\n{}", id.org_uuid))
    }

    fn credential_kinds(&self) -> &'static [&'static str] {
        &KINDS
    }

    fn kind_traits(&self, kind: &str) -> KindTraits {
        shape::kind_traits(kind)
    }

    fn parse_identity(&self, raw: &Value) -> Result<Identity, ProviderError> {
        shape::identity_from(raw).ok_or_else(|| {
            ProviderError::Invalid("the stored FakeAgent identity has no handle".into())
        })
    }

    /// Precondition: `email` is non-empty (the engine defaults or validates it).
    fn token_identity(&self, email: &str) -> Identity {
        shape::identity_from(&json!({"handle": email, "workspace": "", "uid": null}))
            .expect("a non-empty handle always parses")
    }

    fn token_secret(&self, token: &str) -> (String, Vec<u8>) {
        let bytes = serde_json::to_vec(&json!({"fa": {"token": token.trim()}}))
            .expect("a Value always serializes");
        (KIND_STATIC.into(), bytes)
    }

    fn classify(&self, secret: &[u8]) -> String {
        shape::classify(secret).into()
    }

    fn fingerprint(&self, secret: &[u8]) -> Option<Fingerprint> {
        shape::fingerprint(secret)
    }

    fn has_refresh_token(&self, secret: &[u8]) -> bool {
        shape::renew(secret).is_some()
    }

    fn is_wiped(&self, secret: &[u8]) -> bool {
        shape::is_wiped(secret)
    }

    /// FakeAgent logins never expire as a whole.
    fn login_expires_at(&self, _secret: &[u8]) -> Option<i64> {
        None
    }

    fn access_expires_at(&self, secret: &[u8]) -> Option<i64> {
        shape::expires(secret)
    }

    fn access_fingerprint(&self, secret: &[u8]) -> Option<Fingerprint> {
        shape::token(secret).map(|t| Fingerprint::of_secret(t.as_bytes()))
    }

    fn live_identity(&self, env: &Env) -> Read<Identity> {
        let p = FakePaths::resolve(env);
        match read_file(&p.identity) {
            Read::Present(b) => match splice::get_top_level(&b, "identity") {
                Ok(Some(v)) => shape::identity_from(&v).map_or(Read::Absent, Read::Present),
                Ok(None) => Read::Absent,
                Err(e) => Read::Unreadable(ReadError::new(
                    p.identity.display().to_string(),
                    e.to_string(),
                )),
            },
            Read::Absent => Read::Absent,
            Read::Unreadable(e) => Read::Unreadable(e),
        }
    }

    fn read_live_auth(&self, env: &Env) -> LiveAuth {
        LiveAuth {
            credential: read_file(&FakePaths::resolve(env).credential).map(Credential::fresh),
            managed_key: Read::Absent,
        }
    }

    fn live_lock_budget(&self) -> Duration {
        self.lock_budget
    }

    fn lock_credentials<'g>(
        &self,
        env: &Env,
        g: &'g MutationGuard,
        budget: Duration,
    ) -> Result<CredLocks<'g>, ProviderError> {
        let spec = MkdirLockSpec::new(FakePaths::resolve(env).lock, LOCK_STALE, budget);
        Ok(CredLocks::new(g, Box::new(FakeLock(MkdirLock::acquire(&spec)?))))
    }

    fn lock_config<'g>(
        &self,
        _env: &Env,
        cred: CredLocks<'g>,
        _budget: Duration,
    ) -> Result<LiveLocks<'g>, ProviderError> {
        Ok(cred.with_config(Box::new(NoConfigLock)))
    }

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
                on_fallback: false,
            }],
            // There is no other axis to clear.
            LiveChange::ClearOther(_) => vec![],
        }
    }

    /// Composes the target with the machine's live `device` key, read fresh under the locks,
    /// and writes the credential file at 0600. A live credential that cannot be read is never
    /// overwritten. FakeAgent never falls back, so `before_fallback` is never called.
    fn write_credential<'l>(
        &self,
        env: &Env,
        locks: &'l LiveLocks<'_>,
        target: &StoredLogin,
        _before_fallback: BeforeFallback<'_>,
    ) -> Result<Written<'l>, ProviderError> {
        locks.check_owned()?;
        let p = FakePaths::resolve(env);
        let before = present_or_err(read_file(&p.credential))?;
        let live = match &before {
            None => None,
            Some(b) => match serde_json::from_slice::<Value>(b) {
                Ok(Value::Object(o)) => Some(o),
                _ => {
                    return Err(ProviderError::Invalid(
                        "FakeAgent's live credential is not a JSON object".into(),
                    ));
                }
            },
        };
        let composed = shape::compose(&target.secret, live.as_ref())?;
        ensure_private_dir(&p.dir)?;
        write_atomic_private_with(&p.credential, &composed, 0o600, || {
            locks.check_owned().map_err(ProviderError::from)
        })?;
        Ok(Written {
            undo: Box::new(FileUndo {
                path: p.credential.clone(),
                before,
                private: true,
            }),
            stored_in: SecretStore::File(p.credential),
        })
    }

    fn clear_other_axis<'l>(
        &self,
        _env: &Env,
        locks: &'l LiveLocks<'_>,
        _kept_kind: &str,
    ) -> Result<Box<dyn Undo + 'l>, ProviderError> {
        locks.check_owned()?;
        Ok(Box::new(NothingToUndo))
    }

    /// Splices the `identity` key of `identity.json`, changing no other byte (§9.5). A torn or
    /// non-object file is never replaced.
    fn write_identity<'l>(
        &self,
        env: &Env,
        locks: &'l LiveLocks<'_>,
        identity: Option<&Identity>,
    ) -> Result<Box<dyn Undo + 'l>, ProviderError> {
        locks.check_owned()?;
        let p = FakePaths::resolve(env);
        let unsplicable = || ProviderError::ConfigUnsplicable {
            path: p.identity.clone(),
            remedy: REMEDY,
        };
        let before = present_or_err(read_file(&p.identity)).map_err(|_| unsplicable())?;
        let new = match (&before, identity) {
            (None, None) => return Ok(Box::new(NothingToUndo)),
            (None, Some(i)) => {
                format!("{{\n  \"identity\": {}\n}}\n", render_nested(&i.raw, 1)).into_bytes()
            }
            (Some(b), Some(i)) => {
                splice::replace_top_level(b, "identity", &i.raw).map_err(|_| unsplicable())?
            }
            (Some(b), None) => {
                splice::remove_top_level(b, "identity").map_err(|_| unsplicable())?
            }
        };
        if before.as_deref() != Some(new.as_slice()) {
            ensure_private_dir(&p.dir)?;
            write_atomic_with(&p.identity, &new, 0o600, || {
                locks.check_owned().map_err(ProviderError::from)
            })?;
        }
        Ok(Box::new(FileUndo {
            path: p.identity,
            before,
            private: false,
        }))
    }
}
```

- [ ] **Step 4: Run the crate tests to verify they pass**

Run: `cargo test -p tagteam-fake && cargo clippy -p tagteam-fake --all-targets -- -D warnings`
Expected: PASS (six integration tests and two unit tests), no warnings.

- [ ] **Step 5: Write the failing engine tests**

`crates/tagteam-engine/tests/fake_agent.rs`:
```rust
//! §15.2 provider neutrality: the engine runs the test-only `FakeAgent` beside Claude Code,
//! through the same store, vault, switch transaction and local-state invariant.
mod common;

use std::fs;

use common::FakeFx;
use serde_json::{Value, json};
use tagteam_core::AccountId;
use tagteam_engine::lifecycle::AddTokenOptions;
use tagteam_engine::switch::{SwitchRequest, SwitchTarget};
use tagteam_fake::{
    FAKE_AGENT, FakePaths, KIND_TOKEN, LOGIN_EXPIRES, credential_json, identity_json,
};
use tagteam_provider::{IdentitySurface, Provider};

fn check(ffx: &FakeFx, surface: &IdentitySurface, step: &str, op: impl FnOnce()) {
    let before = ffx.fx.snapshot();
    op();
    let after = ffx.fx.snapshot();
    ffx.fx
        .assert_only_surface_changed_for(surface, &before, &after, step);
}

fn read_json(path: &std::path::Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn fake_agent_accounts_add_and_switch_through_the_engine() {
    let ffx = FakeFx::new();
    let alice = ffx.fake_add("alice", "tok-a", "renew-a");
    let bob = ffx.fake_add("bob", "tok-b", "renew-b");
    assert_eq!(ffx.fake_live_label().as_deref(), Some("bob@ws"));
    // The machine's device key moved on since alice's credential was stored.
    let p = FakePaths::resolve(&ffx.fx.env);
    let mut live = read_json(&p.credential);
    live["device"] = json!({"id": "machine-2"});
    fs::write(&p.credential, serde_json::to_vec(&live).unwrap()).unwrap();

    let out = ffx.switch_fake(&alice);
    assert!(out.switched, "{}", out.message);
    assert_eq!(out.from.map(|r| r.id), Some(bob.clone()));
    assert_eq!(ffx.fake_live_label().as_deref(), Some("alice@ws"));
    assert_eq!(
        read_json(&p.credential),
        json!({"fa": {"token": "tok-a", "renew": "renew-a", "expires": LOGIN_EXPIRES},
               "device": {"id": "machine-2"}})
    );
    let store = ffx.engine.store().unwrap();
    assert_eq!(store.active(&ffx.fake_provider()).unwrap(), Some(alice));
    assert!(ffx.switch_fake(&bob).switched);
    assert_eq!(ffx.fake_live_label().as_deref(), Some("bob@ws"));
}

#[test]
fn positions_are_numbered_per_provider() {
    let ffx = FakeFx::new();
    let cc = ffx.fx.add("a@b.co", "rt-a");
    let alice = ffx.fake_add("alice", "tok-a", "renew-a");
    let bob = ffx.fake_add("bob", "tok-b", "renew-b");
    let store = ffx.engine.store().unwrap();
    let pos = |id: &AccountId| store.account(id).unwrap().unwrap().position;
    assert_eq!((pos(&cc), pos(&alice), pos(&bob)), (1, 1, 2));
}

#[test]
fn accounts_lists_every_provider_with_accounts() {
    let ffx = FakeFx::new();
    ffx.fx.add("a@b.co", "rt-a");
    ffx.fake_add("alice", "tok-a", "renew-a");
    ffx.fake_add("bob", "tok-b", "renew-b");
    let lists = ffx.engine.accounts(None).unwrap();
    let summary: Vec<_> = lists
        .iter()
        .map(|l| (l.provider.as_str(), l.accounts.len(), l.active_position))
        .collect();
    assert_eq!(
        summary,
        vec![("claude-code", 1, Some(1)), (FAKE_AGENT, 2, Some(2))]
    );
    let fake_only = ffx.engine.accounts(Some(&ffx.fake_provider())).unwrap();
    assert_eq!(fake_only.len(), 1);
    assert_eq!(
        fake_only[0].accounts[0].kind,
        ffx.fake.kind_traits(KIND_TOKEN)
    );
}

#[test]
fn a_fake_agent_account_has_no_claude_code_shaped_field() {
    let ffx = FakeFx::new();
    let alice = ffx.fake_add("alice", "tok-a", "renew-a");
    let row = ffx.engine.store().unwrap().account(&alice).unwrap().unwrap();
    assert_eq!(row.provider.as_str(), FAKE_AGENT);
    assert_eq!(
        (row.label.as_str(), row.email.as_deref(), row.org_uuid.as_str()),
        ("alice@ws", None, "ws")
    );
    assert_eq!(row.account_uuid.as_deref(), Some("uid-alice"));
    assert_eq!(row.kind, KIND_TOKEN);
    assert!(ffx.fake.credential_kinds().contains(&row.kind.as_str()));
    assert_eq!(row.identity_json, identity_json("alice", "ws", "uid-alice"));
    assert_eq!(row.login_expires_at, None);
    let stored: Value = serde_json::from_slice(&ffx.fx.vault_bytes(&alice).unwrap()).unwrap();
    assert_eq!(
        stored,
        credential_json("tok-a", Some("renew-a"), Some(LOGIN_EXPIRES))
    );
    assert!(stored.get("claudeAiOauth").is_none());
    assert!(row.identity_json.get("emailAddress").is_none());
}

#[test]
fn a_fake_agent_switch_leaves_claude_code_untouched() {
    let ffx = FakeFx::new();
    let _cc = ffx.fx.add("a@b.co", "rt-a");
    let alice = ffx.fake_add("alice", "tok-a", "renew-a");
    let _bob = ffx.fake_add("bob", "tok-b", "renew-b");
    let store = ffx.engine.store().unwrap();
    let cc_rows = store.accounts(&ffx.fx.provider()).unwrap();
    let cc_active = store.active(&ffx.fx.provider()).unwrap();
    let before = ffx.fx.snapshot();
    assert!(ffx.switch_fake(&alice).switched);
    let after = ffx.fx.snapshot();
    // With FakeAgent's surface, every Claude Code file and Keychain item must be unchanged.
    ffx.fx.assert_only_surface_changed_for(
        &ffx.fake.identity_surface(&ffx.fx.env),
        &before,
        &after,
        "a FakeAgent switch",
    );
    assert_eq!(store.accounts(&ffx.fx.provider()).unwrap(), cc_rows);
    assert_eq!(store.active(&ffx.fx.provider()).unwrap(), cc_active);
    assert_eq!(ffx.fx.live_email().as_deref(), Some("a@b.co"));
}

#[test]
fn a_claude_code_switch_leaves_fake_agent_untouched() {
    let ffx = FakeFx::new();
    let a = ffx.fx.add("a@b.co", "rt-a");
    let _b = ffx.fx.add("b@b.co", "rt-b");
    let _alice = ffx.fake_add("alice", "tok-a", "renew-a");
    let store = ffx.engine.store().unwrap();
    let fake_rows = store.accounts(&ffx.fake_provider()).unwrap();
    let fake_active = store.active(&ffx.fake_provider()).unwrap();
    let before = ffx.fx.snapshot();
    // Through the two-provider engine, so routing by provider is exercised too.
    assert!(ffx.engine.switch(ffx.fx.switch_request(&a, false)).unwrap().switched);
    let after = ffx.fx.snapshot();
    // With Claude Code's surface, every FakeAgent file must be unchanged.
    ffx.fx
        .assert_only_surface_changed(&before, &after, "a Claude Code switch");
    assert_eq!(store.accounts(&ffx.fake_provider()).unwrap(), fake_rows);
    assert_eq!(store.active(&ffx.fake_provider()).unwrap(), fake_active);
    assert_eq!(ffx.fake_live_label().as_deref(), Some("alice@ws"));
    assert_eq!(ffx.fx.live_email().as_deref(), Some("a@b.co"));
}

#[test]
fn every_fake_agent_command_writes_only_its_identity_surface() {
    // §15.3, once per registered provider: FakeAgent's commands move only its declared
    // surface, and a Claude Code login beside it stays byte-identical throughout.
    let ffx = FakeFx::new();
    ffx.fx.add("cc@b.co", "rt-cc");
    let surface = ffx.fake.identity_surface(&ffx.fx.env);
    let e = &ffx.engine;
    let fake = ffx.fake_provider();

    ffx.fake_login("alice", "tok-a", "renew-a");
    let mut alice = None;
    check(&ffx, &surface, "add alice", || {
        alice = Some(e.add_live(ffx.fake_add_options()).unwrap().account.id)
    });
    let alice = alice.unwrap();
    ffx.fake_login("bob", "tok-b", "renew-b");
    let mut bob = None;
    check(&ffx, &surface, "add bob", || {
        bob = Some(e.add_live(ffx.fake_add_options()).unwrap().account.id)
    });
    let bob = bob.unwrap();
    check(&ffx, &surface, "switch to alice", || {
        assert!(ffx.switch_fake(&alice).switched)
    });
    check(&ffx, &surface, "alias", || {
        e.set_alias(&alice, Some("al")).unwrap();
    });
    check(&ffx, &surface, "disable", || {
        e.set_disabled(&bob, true).unwrap();
    });
    check(&ffx, &surface, "enable", || {
        e.set_disabled(&bob, false).unwrap();
    });
    check(&ffx, &surface, "move", || {
        e.move_to(&bob, 5).unwrap();
    });
    let mut token = None;
    check(&ffx, &surface, "add-token", || {
        token = Some(
            e.add_token(AddTokenOptions {
                provider: fake.clone(),
                token: "static-secret".into(),
                position: None,
                email: None,
                alias: None,
                yes: false,
            })
            .unwrap()
            .account,
        )
    });
    let token = token.unwrap();
    assert_eq!(token.label, "fa-static-6@token.local", "the provider's own prefix");
    check(&ffx, &surface, "switch to the static token", || {
        assert!(ffx.switch_fake(&token.id).switched)
    });
    check(&ffx, &surface, "forced switch", || {
        let out = e
            .switch(SwitchRequest {
                provider: fake.clone(),
                target: SwitchTarget::Account(bob.clone()),
                force: true,
                source: "cli",
            })
            .unwrap();
        assert!(out.switched);
    });
    check(&ffx, &surface, "remove", || {
        e.remove(&alice).unwrap();
    });
    assert_eq!(ffx.fake_live_label().as_deref(), Some("bob@ws"));
    assert_eq!(ffx.fx.live_email().as_deref(), Some("cc@b.co"));
}
```

- [ ] **Step 6: Run them to verify they fail**

Run: `cargo test -p tagteam-engine --features test-hooks --test fake_agent`
Expected: FAIL to compile: `unresolved import 'common::FakeFx'` and `unresolved import 'tagteam_fake'`.

- [ ] **Step 7: Add `FakeFx` and generalize the surface comparison**

In `crates/tagteam-engine/Cargo.toml`, under `[dev-dependencies]`, add after the `tagteam-cc` line:
```toml
tagteam-fake.workspace = true
```

In `crates/tagteam-engine/tests/common/mod.rs`:
1. Add `use tagteam_fake::{FAKE_AGENT, FakeAgent};` after the `tagteam_engine` imports, and
   add `IdentitySurface` to the `use tagteam_provider::{…}` list.
2. Replace the head of `assert_only_surface_changed`:
```rust
    pub fn assert_only_surface_changed(
        &self,
        before: &HomeSnapshot,
        after: &HomeSnapshot,
        step: &str,
    ) {
        let surface = self.cc.identity_surface(&self.env);
```
   with:
```rust
    pub fn assert_only_surface_changed(
        &self,
        before: &HomeSnapshot,
        after: &HomeSnapshot,
        step: &str,
    ) {
        let surface = self.cc.identity_surface(&self.env);
        self.assert_only_surface_changed_for(&surface, before, after, step);
    }

    /// `assert_only_surface_changed` for any provider's declared surface. Since the walk covers
    /// all of HOME and every Keychain item, one provider's surface also proves that its
    /// commands left every other provider's state untouched (§15.3).
    pub fn assert_only_surface_changed_for(
        &self,
        surface: &IdentitySurface,
        before: &HomeSnapshot,
        after: &HomeSnapshot,
        step: &str,
    ) {
```
   The rest of the old body is unchanged and now reads the `surface` parameter. Every use
   (`surface.json_keys.iter()`, `surface.credential_files.iter()`,
   `&surface.machine_shared_keys`, `surface.owned_items.iter()`,
   `surface.credential_items.iter()`) compiles as-is against a `&IdentitySurface`.
3. Append:
```rust

/// One engine with Claude Code and the test-only `FakeAgent` registered over one Env, Keychain,
/// store and `ScriptedHttp` (§15.2). `fx` is the Claude Code fixture it is built on: `fx.engine`
/// sees Claude Code alone but shares this engine's store and vault, so `fx.add` and
/// `fx.login` still set up Claude Code logins.
pub struct FakeFx {
    pub fx: Fx,
    pub fake: Arc<FakeAgent>,
    pub engine: Engine,
}

impl FakeFx {
    pub fn new() -> Self {
        let fx = Fx::new();
        let fake = Arc::new(FakeAgent::new().with_lock_budget(Duration::from_secs(2)));
        let engine = Engine::new(EngineConfig {
            env: fx.env.clone(),
            registry: ProviderRegistry::new()
                .with(fx.cc.clone())
                .with(fake.clone()),
            vault: Vault::new(Box::new(KeychainVault::new(fx.kc.clone()))),
            oracle: fx.oracle.clone(),
            clock: fx.clock.clone(),
            default_provider: ProviderId::new(CLAUDE_CODE),
            http: fx.http.clone(),
        });
        FakeFx { fx, fake, engine }
    }

    pub fn fake_provider(&self) -> ProviderId {
        ProviderId::new(FAKE_AGENT)
    }

    /// What logging in to FakeAgent as `handle`, in workspace `ws`, leaves behind.
    pub fn fake_login(&self, handle: &str, token: &str, renew: &str) {
        tagteam_fake::login(&self.fx.env, handle, "ws", token, renew);
    }

    pub fn fake_add_options(&self) -> AddOptions {
        AddOptions {
            provider: self.fake_provider(),
            position: None,
            alias: None,
            yes: false,
        }
    }

    /// Logs in to FakeAgent as `handle` and captures the login (§10.1).
    pub fn fake_add(&self, handle: &str, token: &str, renew: &str) -> AccountId {
        self.fake_login(handle, token, renew);
        self.engine
            .add_live(self.fake_add_options())
            .unwrap()
            .account
            .id
    }

    /// A manual `switch` to a FakeAgent account.
    pub fn switch_fake(&self, id: &AccountId) -> SwitchOutcome {
        self.engine
            .switch(SwitchRequest {
                provider: self.fake_provider(),
                target: SwitchTarget::Account(id.clone()),
                force: false,
                source: "cli",
            })
            .unwrap()
    }

    /// The label of FakeAgent's live login, if any.
    pub fn fake_live_label(&self) -> Option<String> {
        self.fake
            .live_identity(&self.fx.env)
            .present()
            .map(|i| i.label)
    }
}
```

- [ ] **Step 8: Run the engine tests to verify they pass**

Run:
```bash
cargo test -p tagteam-engine --features test-hooks --test fake_agent
cargo test -p tagteam-engine --features test-hooks --test invariant
```
Expected: PASS. The Claude Code invariant (`every_m1_command_writes_only_the_identity_surface_*`
and the negative `the_comparison_catches_*` tests) is unchanged, because the old method delegates
with Claude Code's surface exactly as before. If a `fake_agent.rs` test fails, look for a Claude
Code assumption in the engine (a kind string, an email-only identity path, a CC-only axis), fix
it there, and name the fix in the commit message.

- [ ] **Step 9: Run the whole workspace**

Run:
```bash
cargo fmt --all
cargo test --workspace --features tagteam/test-support
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
```
Expected: every test passes; no warnings.

- [ ] **Step 10: Commit**

```bash
git add Cargo.toml Cargo.lock crates/tagteam-fake crates/tagteam-engine/Cargo.toml \
  crates/tagteam-engine/tests/common/mod.rs crates/tagteam-engine/tests/fake_agent.rs
git commit -m "Add the FakeAgent test provider and run the engine against it"
```

---

### Task 7: The HTTP oracle, its limits, and offline metadata commands

§7.6 over the network. Claude Code resolves who owns a live access token through
`GET /api/oauth/profile`, and `FakeAgent` through its own `whoami` endpoint. The engine's
`HttpOracle` asks the provider; `CachingOracle` makes sure a process asks at most once per
credential. Metadata commands (`alias`, `disable`, `enable`, `move`) recover from fingerprints
alone and never make a network call. The CLI swaps `NoOracle` for the real one, and every CLI
test is pointed at a dead local port so nothing reaches the network.

**Files:**
- Create: `crates/tagteam-cc/src/endpoints.rs`, `crates/tagteam-cc/src/oauth.rs`,
  `crates/tagteam-cc/tests/oauth.rs`, `crates/tagteam-fake/tests/network.rs`,
  `crates/tagteam-engine/tests/oracle.rs`
- Modify: `crates/tagteam-cc/src/lib.rs`, `crates/tagteam-cc/src/provider.rs`,
  `crates/tagteam-provider/src/provider.rs` (trait method), `crates/tagteam-fake/src/provider.rs`,
  `crates/tagteam-core/src/classify.rs` (tests only), `crates/tagteam-engine/src/oracle.rs`,
  `crates/tagteam-engine/src/engine.rs`, `crates/tagteam-engine/src/lifecycle.rs`,
  `crates/tagteam-engine/tests/common/mod.rs`, `crates/tagteam-engine/tests/recover.rs`
  (`any_mutation`), `crates/tagteam/src/app.rs`,
  `crates/tagteam/Cargo.toml`, `crates/tagteam/tests/common/mod.rs`,
  `crates/tagteam/tests/app.rs`, `crates/tagteam/tests/cli.rs`

**Interfaces:**
- Consumes:
  - Task 2: `tagteam_provider::http::{Http, HttpRequest, HttpResponse, HttpError, Method, ScriptedHttp}`
    (`HttpRequest::get(url, timeout).bearer(token)`, `HttpResponse::json()`,
    `ScriptedHttp::{push_json, count, requests}`), and
    `tagteam_provider::mock_server::{MockServer, MockReply}` (feature `mock-server`).
  - Task 3: `tagteam_engine::net::UreqHttp`, `EngineConfig.http`, `Fx.http: Arc<ScriptedHttp>`.
  - Task 4: `tagteam_cc::shape::{access_token, is_expired, scopes}` (and the existing
    `is_api_key`).
  - Task 6: `tagteam_fake::{FakeAgent, KIND_STATIC, identity_json, credential_json}`, and
    `FakeAgent::whoami_url()`.
- Produces:
  - `tagteam_cc::endpoints::{CLIENT_ID, Endpoints}` with `Endpoints::production()` and
    `Endpoints::with_base(base: &str)`, and `ClaudeCode::with_endpoints(self, Endpoints) -> Self`.
  - `tagteam_cc::oauth::{PROFILE_TIMEOUT, profile_request, parse_profile}`, plus the private
    `owner_identity` helper. Task 8's token reply uses its own `token_owner`, since a token reply
    may name only an organization (§7.4), while the profile oracle needs the uuid (§7.6).
  - `Provider::resolve_owner(&self, http: &dyn Http, cred: &Credential, now_ms: i64) -> Option<Identity>`.
  - `tagteam_engine::oracle::{HttpOracle, CachingOracle}`.
  - `Engine::metadata_guard(&self) -> Result<MutationGuard, EngineError>` (`pub(crate)`),
    `Engine::guard_recovering(&self, ask_oracle: bool)`.
  - `Fx::endpoints()`, `Fx::script_profile(&self, email)`.
  - `crates/tagteam/tests/common::OFFLINE_API_BASE`.
  - `Context.api_base: Option<String>`.
  - The `TAGTEAM_TEST_API_BASE` override (test-support builds only).

- [ ] **Step 1: Write the failing Claude Code tests**

Create `crates/tagteam-cc/tests/oauth.rs`:

```rust
//! Appendix A.5 and §7.6: the profile request, its reply, and when the provider may ask at all.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tagteam_cc::endpoints::Endpoints;
use tagteam_cc::live::{LiveStore, Platform};
use tagteam_cc::oauth::{PROFILE_TIMEOUT, parse_profile, profile_request};
use tagteam_cc::provider::ClaudeCode;
use tagteam_cc::shape::setup_token_credential;
use tagteam_provider::http::{HttpError, HttpResponse, Method, ScriptedHttp};
use tagteam_provider::{Credential, FakeKeychain, Provider};

/// A recorded reply (Task 1): `{"status", "headers", "body", "synthetic"}`.
fn fixture(raw: &str) -> (HttpResponse, Value) {
    let v: Value = serde_json::from_str(raw).unwrap();
    let body = v["body"].clone();
    let headers = v["headers"]
        .as_object()
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| Some((k.to_lowercase(), v.as_str()?.to_owned())))
                .collect()
        })
        .unwrap_or_default();
    let resp = HttpResponse {
        status: v["status"].as_u64().unwrap() as u16,
        headers,
        body: serde_json::to_vec(&body).unwrap(),
    };
    (resp, body)
}

fn reply(status: u16, body: &Value) -> HttpResponse {
    HttpResponse {
        status,
        headers: vec![],
        body: serde_json::to_vec(body).unwrap(),
    }
}

const NOW: i64 = 1_790_000_000_000;

fn cc() -> ClaudeCode {
    ClaudeCode::with_store(
        LiveStore::new(Arc::new(FakeKeychain::new()), Platform::MacOs)
            .with_retry_delay(Duration::ZERO),
    )
}

fn oauth(expires_at: Value) -> Vec<u8> {
    json!({"claudeAiOauth": {"accessToken": "at-live", "refreshToken": "rt-live", "expiresAt": expires_at}})
        .to_string()
        .into_bytes()
}

#[test]
fn production_and_test_endpoints_follow_appendix_a5() {
    let p = Endpoints::production();
    assert_eq!(p.token, "https://platform.claude.com/v1/oauth/token");
    assert_eq!(p.profile, "https://api.anthropic.com/api/oauth/profile");
    assert_eq!(p.usage, "https://api.anthropic.com/api/oauth/usage");
    let t = Endpoints::with_base("http://127.0.0.1:9/");
    assert_eq!(t.token, "http://127.0.0.1:9/v1/oauth/token");
    assert_eq!(t.profile, "http://127.0.0.1:9/api/oauth/profile");
    assert_eq!(t.usage, "http://127.0.0.1:9/api/oauth/usage");
}

#[test]
fn the_profile_request_is_a_bearer_get_with_a_five_second_timeout() {
    let req = profile_request(&Endpoints::production(), "at-secret");
    assert_eq!(req.method, Method::Get);
    assert_eq!(req.url, "https://api.anthropic.com/api/oauth/profile");
    assert_eq!(req.timeout, Duration::from_secs(5));
    assert_eq!(PROFILE_TIMEOUT, Duration::from_secs(5));
    assert!(req.body.is_none());
    assert!(
        req.headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("authorization") && v == "Bearer at-secret")
    );
    assert!(
        !format!("{req:?}").contains("at-secret"),
        "the bearer never reaches Debug"
    );
}

#[test]
fn the_recorded_profile_reply_resolves_to_its_account() {
    let (resp, body) = fixture(include_str!("fixtures/endpoints/profile-200.json"));
    let id = parse_profile(&resp).expect("the recorded reply resolves");
    assert_eq!(id.account_uuid.as_deref(), body["account"]["uuid"].as_str());
    assert_eq!(id.email.as_deref(), body["account"]["email"].as_str());
    assert_eq!(
        id.org_uuid,
        body["organization"]["uuid"].as_str().unwrap_or_default()
    );
    // `raw` is shaped like CC's `oauthAccount`, so a displaced row records it the same way.
    assert_eq!(id.raw["accountUuid"], body["account"]["uuid"]);
    assert_eq!(id.raw["emailAddress"], body["account"]["email"]);
}

#[test]
fn a_profile_reply_resolves_only_with_a_non_empty_account_uuid() {
    let ok = json!({"account": {"uuid": "u-1", "email": "a@x.co"}, "organization": {"uuid": "o-1", "name": "Org"}});
    let id = parse_profile(&reply(200, &ok)).unwrap();
    assert_eq!(
        (
            id.account_uuid.as_deref(),
            id.email.as_deref(),
            id.org_uuid.as_str(),
            id.org_name.as_deref(),
            id.label.as_str()
        ),
        (Some("u-1"), Some("a@x.co"), "o-1", Some("Org"), "a@x.co")
    );
    let personal = json!({"account": {"uuid": "u-1", "email": "a@x.co"}, "organization": null});
    assert_eq!(parse_profile(&reply(200, &personal)).unwrap().org_uuid, "");
    for body in [
        json!({"account": {"uuid": "", "email": "a@x.co"}}),
        json!({"account": {"email": "a@x.co"}}),
        json!({"account": {"uuid": 7}}),
        json!({}),
    ] {
        assert!(parse_profile(&reply(200, &body)).is_none(), "{body}");
    }
    assert!(parse_profile(&reply(401, &ok)).is_none(), "only a 200 resolves");
    let garbage = HttpResponse {
        status: 200,
        headers: vec![],
        body: b"<html>captive portal</html>".to_vec(),
    };
    assert!(parse_profile(&garbage).is_none());
}

#[test]
fn resolve_owner_asks_the_profile_endpoint_for_a_live_oauth_token() {
    let http = ScriptedHttp::new();
    let url = Endpoints::production().profile;
    http.push_json(
        Method::Get,
        &url,
        200,
        json!({"account": {"uuid": "u-1", "email": "a@x.co"}, "organization": {"uuid": ""}}),
    );
    let owner = cc().resolve_owner(
        &http,
        &Credential::fresh(oauth(json!(NOW + 3_600_000))),
        NOW,
    );
    assert_eq!(owner.unwrap().account_uuid.as_deref(), Some("u-1"));
    let sent = http.requests();
    assert_eq!(sent.len(), 1);
    assert!(
        sent[0]
            .headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("authorization") && v == "Bearer at-live")
    );
}

#[test]
fn resolve_owner_never_asks_without_a_token_it_can_show() {
    // §7.6: an expired access token, a setup token (its only scope is `user:inference`), an API
    // key, or no access token at all: no answer, and no request.
    let http = ScriptedHttp::new();
    let url = Endpoints::production().profile;
    http.push_json(
        Method::Get,
        &url,
        200,
        json!({"account": {"uuid": "u-1", "email": "a@x.co"}}),
    );
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("expired", oauth(json!(NOW + 60_000))), // within §7.2's 5-minute buffer
        ("setup token", setup_token_credential("sk-ant-oat01-setup")),
        ("api key", b"sk-ant-api03-key".to_vec()),
        (
            "no access token",
            json!({"claudeAiOauth": {"refreshToken": "rt"}})
                .to_string()
                .into_bytes(),
        ),
    ];
    for (what, bytes) in cases {
        assert!(
            cc().resolve_owner(&http, &Credential::fresh(bytes), NOW)
                .is_none(),
            "{what}"
        );
    }
    assert_eq!(http.count(Method::Get, &url), 0);
}

#[test]
fn resolve_owner_asks_for_an_oauth_blob_that_has_no_refresh_token() {
    // §7.6 skips only an expired token, a setup token and an API key. A blob with a live access
    // token and profile scopes but no refresh token is none of those, so it is shown.
    let http = ScriptedHttp::new();
    let url = Endpoints::production().profile;
    http.push_json(
        Method::Get,
        &url,
        200,
        json!({"account": {"uuid": "u-1", "email": "a@x.co"}}),
    );
    let blob = json!({"claudeAiOauth": {
        "accessToken": "at-live",
        "expiresAt": NOW + 3_600_000,
        "scopes": ["user:inference", "user:profile"],
    }})
    .to_string()
    .into_bytes();
    let owner = cc().resolve_owner(&http, &Credential::fresh(blob), NOW);
    assert_eq!(owner.unwrap().account_uuid.as_deref(), Some("u-1"));
    assert_eq!(http.count(Method::Get, &url), 1);
}

#[test]
fn a_non_numeric_expiry_counts_as_unexpired() {
    // §7.2: a non-numeric `expiresAt` is not expired, so the token may be shown.
    let http = ScriptedHttp::new();
    let url = Endpoints::production().profile;
    http.push_json(
        Method::Get,
        &url,
        200,
        json!({"account": {"uuid": "u-1", "email": "a@x.co"}}),
    );
    let owner = cc().resolve_owner(&http, &Credential::fresh(oauth(json!("soon"))), NOW);
    assert!(owner.is_some());
}

#[test]
fn a_transport_failure_is_no_answer() {
    let http = ScriptedHttp::new();
    let url = Endpoints::production().profile;
    http.push(
        Method::Get,
        &url,
        Err(HttpError::Ambiguous("timed out".into())),
    );
    assert!(
        cc().resolve_owner(
            &http,
            &Credential::fresh(oauth(json!(NOW + 3_600_000))),
            NOW
        )
        .is_none()
    );
}

#[test]
fn with_endpoints_redirects_every_request() {
    let http = ScriptedHttp::new();
    let base = Endpoints::with_base("http://127.0.0.1:4321");
    http.push_json(
        Method::Get,
        &base.profile,
        200,
        json!({"account": {"uuid": "u-1", "email": "a@x.co"}}),
    );
    let owner = cc().with_endpoints(base.clone()).resolve_owner(
        &http,
        &Credential::fresh(oauth(json!(NOW + 3_600_000))),
        NOW,
    );
    assert!(owner.is_some());
    assert_eq!(http.count(Method::Get, &base.profile), 1);
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p tagteam-cc --test oauth`
Expected: FAIL to compile: `could not find endpoints in tagteam_cc`, `no method named
resolve_owner`.

- [ ] **Step 3: Add the endpoints and the profile half of `oauth.rs`**

Create `crates/tagteam-cc/src/endpoints.rs`:

```rust
//! Appendix A.5. The production URLs are the only ones a release build uses; the CLI's
//! test-support build points them at a local server (`with_base`).

/// Claude Code's OAuth client id, sent with every token refresh (Appendix A.5).
pub const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    pub token: String,
    pub profile: String,
    pub usage: String,
}

impl Endpoints {
    pub fn production() -> Self {
        Self {
            token: "https://platform.claude.com/v1/oauth/token".into(),
            profile: "https://api.anthropic.com/api/oauth/profile".into(),
            usage: "https://api.anthropic.com/api/oauth/usage".into(),
        }
    }

    /// Every endpoint under one base URL, with the production paths.
    pub fn with_base(base: &str) -> Self {
        let b = base.trim_end_matches('/');
        Self {
            token: format!("{b}/v1/oauth/token"),
            profile: format!("{b}/api/oauth/profile"),
            usage: format!("{b}/api/oauth/usage"),
        }
    }
}
```

Create `crates/tagteam-cc/src/oauth.rs`:

```rust
//! Appendix A.5: the requests Claude Code's endpoints take and what their replies mean. The
//! provider builds and parses them; the engine owns every lock, write and verdict (§4.5).

use std::time::Duration;

use serde_json::{Value, json};
use tagteam_provider::Identity;
use tagteam_provider::http::{HttpRequest, HttpResponse};

use crate::endpoints::Endpoints;

/// §7.6, Appendix A.5.
pub const PROFILE_TIMEOUT: Duration = Duration::from_secs(5);

/// `GET /api/oauth/profile`, with the access token as the bearer.
pub fn profile_request(e: &Endpoints, access_token: &str) -> HttpRequest {
    HttpRequest::get(e.profile.clone(), PROFILE_TIMEOUT).bearer(access_token)
}

fn non_empty(v: &Value) -> Option<&str> {
    v.as_str().filter(|s| !s.is_empty())
}

/// An owner named by an endpoint: resolved only with a non-empty account uuid (§7.6). `raw` is
/// shaped like CC's `oauthAccount`, so a displaced row records it as it records a live login.
pub(crate) fn owner_identity(uuid: &Value, email: &Value, org: &Value) -> Option<Identity> {
    let uuid = non_empty(uuid)?.to_owned();
    let email = non_empty(email).map(str::to_owned);
    let org_uuid = org["uuid"].as_str().unwrap_or_default().to_owned();
    let org_name = org["name"].as_str().map(str::to_owned);
    Some(Identity {
        label: email.clone().unwrap_or_else(|| uuid.clone()),
        raw: json!({
            "emailAddress": email,
            "accountUuid": uuid,
            "organizationUuid": org_uuid,
            "organizationName": org_name,
        }),
        email,
        org_uuid,
        org_name,
        account_uuid: Some(uuid),
    })
}

/// Who a 200 profile reply says owns the token: `account.uuid`, `account.email`,
/// `organization.uuid` (Appendix A.5). Anything else is no answer.
pub fn parse_profile(resp: &HttpResponse) -> Option<Identity> {
    if resp.status != 200 {
        return None;
    }
    let body = resp.json()?;
    owner_identity(
        &body["account"]["uuid"],
        &body["account"]["email"],
        &body["organization"],
    )
}
```

In `crates/tagteam-cc/src/lib.rs`, add the two modules after `mod crash;`:

```rust
pub mod endpoints;
pub mod oauth;
```

- [ ] **Step 4: Add `resolve_owner` to the trait**

In `crates/tagteam-provider/src/provider.rs`, add to the imports:

```rust
use crate::http::Http;
```

and add this method at the end of `pub trait Provider` (after `write_identity`):

```rust
    /// Who owns this credential's access token (§7.6). Advisory: a failure is `None`, never an
    /// error. `None`, with no request sent, when the credential has no token it can show:
    /// an expired access token (`now_ms + 5 min ≥ expiresAt`), or a kind the provider cannot
    /// resolve.
    fn resolve_owner(&self, http: &dyn Http, cred: &Credential, now_ms: i64)
    -> Option<Identity>;
```

- [ ] **Step 5: Implement it for Claude Code**

In `crates/tagteam-cc/src/provider.rs`:

1. Add to the imports:

```rust
use tagteam_provider::http::Http;

use crate::endpoints::Endpoints;
use crate::oauth;
```

`shape` is already imported.

2. Add a field to `pub struct ClaudeCode` (keep every existing field):

```rust
    /// Appendix A.5's URLs; `Endpoints::production()` except when the CLI's test-support
    /// build points them at a local server.
    endpoints: Endpoints,
```

and initialise it in `with_store`'s struct literal:

```rust
            endpoints: Endpoints::production(),
```

3. Add this builder to the `impl ClaudeCode` block that holds `new` and `with_store`:

```rust
    /// Sends every request to `endpoints` instead of production (the CLI's test-support build).
    pub fn with_endpoints(mut self, endpoints: Endpoints) -> Self {
        self.endpoints = endpoints;
        self
    }
```

4. Add to `impl Provider for ClaudeCode`:

```rust
    fn resolve_owner(&self, http: &dyn Http, cred: &Credential, now_ms: i64) -> Option<Identity> {
        let bytes = cred.bytes();
        // §7.6 skips exactly these: an API key (no profile), an expired access token, and a
        // setup token (its only scope is `user:inference`, which the profile endpoint refuses).
        // An OAuth blob with no refresh token is none of them, so it is shown.
        if shape::is_api_key(bytes)
            || shape::is_expired(bytes, now_ms)
            || shape::scopes(bytes) == ["user:inference"]
        {
            return None;
        }
        let token = shape::access_token(bytes)?;
        let reply = http
            .send(&oauth::profile_request(&self.endpoints, &token))
            .ok()?;
        oauth::parse_profile(&reply)
    }
```

- [ ] **Step 6: Run the Claude Code tests to verify they pass**

Run: `cargo test -p tagteam-cc --test oauth`
Expected: PASS (10 tests). `cargo build --workspace` now fails in `tagteam-fake`: `not all trait
items implemented, missing: resolve_owner`. Step 7 fixes that.

- [ ] **Step 7: Write the failing `FakeAgent` test**

Create `crates/tagteam-fake/tests/network.rs`:

```rust
//! `FakeAgent`'s network half: its own endpoints, shapes and verdicts, deliberately unlike
//! Claude Code's (§15.2).

use serde_json::json;
use tagteam_fake::{FakeAgent, credential_json, identity_json};
use tagteam_provider::http::{Method, ScriptedHttp};
use tagteam_provider::{Credential, Provider};

const NOW: i64 = 1_790_000_000_000;

fn token(expires: Option<i64>) -> Credential {
    Credential::fresh(
        credential_json("fa-tok", Some("fa-renew"), expires)
            .to_string()
            .into_bytes(),
    )
}

#[test]
fn resolve_owner_asks_whoami_with_the_bearer_token() {
    let fa = FakeAgent::new();
    let http = ScriptedHttp::new();
    http.push_json(
        Method::Get,
        &fa.whoami_url(),
        200,
        json!({"uid": "fa-u1", "handle": "neo", "workspace": "zion"}),
    );
    let owner = fa
        .resolve_owner(&http, &token(Some(NOW + 3_600_000)), NOW)
        .unwrap();
    assert_eq!(owner.account_uuid.as_deref(), Some("fa-u1"));
    assert_eq!(owner.email, None, "FakeAgent identities have no email");
    assert_eq!(owner.label, "neo@zion");
    assert_eq!(owner.org_uuid, "zion");
    assert_eq!(owner.raw, identity_json("neo", "zion", "fa-u1"));
    let sent = http.requests();
    assert_eq!(sent.len(), 1);
    assert!(
        sent[0]
            .headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("authorization") && v == "Bearer fa-tok")
    );
}

#[test]
fn resolve_owner_never_asks_for_an_expired_or_static_credential() {
    let fa = FakeAgent::new();
    let http = ScriptedHttp::new();
    http.push_json(
        Method::Get,
        &fa.whoami_url(),
        200,
        json!({"uid": "fa-u1", "handle": "neo", "workspace": "zion"}),
    );
    assert!(fa.resolve_owner(&http, &token(Some(NOW + 1_000)), NOW).is_none());
    let static_cred = Credential::fresh(
        credential_json("fa-static-tok", None, None)
            .to_string()
            .into_bytes(),
    );
    assert!(fa.resolve_owner(&http, &static_cred, NOW).is_none());
    assert_eq!(http.count(Method::Get, &fa.whoami_url()), 0);
}

#[test]
fn a_whoami_reply_without_a_uid_resolves_nothing() {
    let fa = FakeAgent::new();
    let http = ScriptedHttp::new();
    http.push_json(
        Method::Get,
        &fa.whoami_url(),
        200,
        json!({"uid": "", "handle": "neo", "workspace": "zion"}),
    );
    assert!(fa.resolve_owner(&http, &token(None), NOW).is_none());
}
```

Run: `cargo test -p tagteam-fake --test network`
Expected: FAIL to compile (`resolve_owner` is not implemented for `FakeAgent`).

- [ ] **Step 8: Implement it for `FakeAgent`**

In `crates/tagteam-fake/src/provider.rs`, add only these two imports (`Duration`, `Value`,
`Credential`, `Identity` and `KIND_STATIC` are already imported by Task 6, and importing them again
is a compile error):

```rust
use tagteam_provider::http::{Http, HttpRequest};

use crate::identity_json;
```

Add this free function above `impl Provider for FakeAgent`:

```rust
/// `fa.token`, unless the credential has expired by §7.2's rule (`now + 5 min ≥ expires`).
/// A non-numeric or absent `expires` never expires.
fn showable_token(bytes: &[u8], now_ms: i64) -> Option<String> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    if v["fa"]["expires"]
        .as_i64()
        .is_some_and(|e| now_ms + 300_000 >= e)
    {
        return None;
    }
    v["fa"]["token"]
        .as_str()
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
}
```

and add to `impl Provider for FakeAgent`:

```rust
    fn resolve_owner(&self, http: &dyn Http, cred: &Credential, now_ms: i64) -> Option<Identity> {
        // A static credential has no owner endpoint behind it: nothing is sent (§7.6).
        if self.classify(cred.bytes()) == KIND_STATIC {
            return None;
        }
        let token = showable_token(cred.bytes(), now_ms)?;
        let req = HttpRequest::get(self.whoami_url(), Duration::from_secs(5)).bearer(&token);
        let reply = http.send(&req).ok()?;
        if reply.status != 200 {
            return None;
        }
        let body = reply.json()?;
        let uid = body["uid"].as_str().filter(|s| !s.is_empty())?;
        let handle = body["handle"].as_str().unwrap_or_default();
        let workspace = body["workspace"].as_str().unwrap_or_default();
        Some(Identity {
            label: format!("{handle}@{workspace}"),
            email: None,
            org_uuid: workspace.to_owned(),
            org_name: None,
            account_uuid: Some(uid.to_owned()),
            raw: identity_json(handle, workspace, uid),
        })
    }
```

Run: `cargo test -p tagteam-fake --test network && cargo build --workspace --all-targets`
Expected: PASS (3 tests), and the workspace builds.

- [ ] **Step 9: Commit the provider half**

```bash
git add crates/tagteam-cc/src/endpoints.rs crates/tagteam-cc/src/oauth.rs crates/tagteam-cc/src/lib.rs \
  crates/tagteam-cc/src/provider.rs crates/tagteam-cc/tests/oauth.rs \
  crates/tagteam-provider/src/provider.rs crates/tagteam-fake/src/provider.rs \
  crates/tagteam-fake/tests/network.rs
git commit -m "Resolve a live token's owner through each provider's profile endpoint"
```

- [ ] **Step 10: Write the failing engine tests**

First, pin L300's combined-input precedence in the pure policy. Add to the `tests` module of
`crates/tagteam-core/src/classify.rs`:

```rust
    #[test]
    fn precedence_follows_the_table_order_when_facts_combine() {
        // L300: a vault match outranks any oracle answer; a wiped or tokenless credential
        // outranks the oracle; and a foreign answer is displaced whatever else holds.
        let ours = OutgoingFacts {
            fp_equal_vault: true,
            oracle: OracleVerdict::OtherIdentity,
            lacks_refresh_over_complete: true,
            ..facts()
        };
        assert_eq!(
            decide_outgoing(&ours),
            (OutgoingClass::Ours, OutgoingAction::Nothing)
        );
        let wiped = OutgoingFacts {
            wiped: true,
            oracle: OracleVerdict::OtherIdentity,
            ..facts()
        };
        assert_eq!(
            decide_outgoing(&wiped),
            (OutgoingClass::Wiped, OutgoingAction::Nothing)
        );
        let foreign = OutgoingFacts {
            oracle: OracleVerdict::OtherIdentity,
            lacks_refresh_over_complete: true,
            ..facts()
        };
        assert_eq!(
            decide_outgoing(&foreign),
            (OutgoingClass::Foreign, OutgoingAction::Displace)
        );
    }
```

Then add the fixture helpers to `crates/tagteam-engine/tests/common/mod.rs`. Add
`use tagteam_cc::endpoints::Endpoints;` and `use tagteam_provider::http::Method;` to its imports
(Task 3 already imported `ScriptedHttp`; do not import it again), and add to `impl Fx` (next to `oauth_account`):

```rust
    /// The endpoints the fixture's `ClaudeCode` sends to: production URLs, answered by
    /// `self.http` (a `ScriptedHttp`), so nothing leaves the machine.
    pub fn endpoints() -> Endpoints {
        Endpoints::production()
    }

    /// Queues a 200 profile reply naming `email`'s login as `Fx::oauth_account` shapes it
    /// (uuid `uuid-{email}`, personal org), built from the recorded reply so every field the
    /// real endpoint sends is present.
    pub fn script_profile(&self, email: &str) {
        let recorded: Value = serde_json::from_str(include_str!(
            "../../../tagteam-cc/tests/fixtures/endpoints/profile-200.json"
        ))
        .unwrap();
        let mut body = recorded["body"].clone();
        body["account"]["uuid"] = json!(format!("uuid-{email}"));
        body["account"]["email"] = json!(email);
        body["organization"]["uuid"] = json!("");
        self.http
            .push_json(Method::Get, &Self::endpoints().profile, 200, body);
    }
```

Create `crates/tagteam-engine/tests/oracle.rs`:

```rust
//! §7.6: the profile oracle over HTTP, how often it is asked, and which commands may ask.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use common::{Fx, crash_row, mutation_lock_free};
use serde_json::json;
use tagteam_cc::endpoints::Endpoints;
use tagteam_cc::live::{LiveStore, Platform};
use tagteam_cc::ClaudeCode;
use tagteam_core::{AccountId, CLAUDE_CODE, ProviderId};
use tagteam_engine::account_lock::AccountLock;
use tagteam_engine::net::UreqHttp;
use tagteam_engine::oracle::{CachingOracle, HttpOracle, Oracle};
use tagteam_engine::registry::ProviderRegistry;
use tagteam_engine::vault::{KeychainVault, SERVICE, Vault};
use tagteam_engine::{Engine, EngineConfig, EngineError};
use tagteam_provider::http::{Http, Method};
use tagteam_provider::mock_server::{MockReply, MockServer};
use tagteam_provider::{Credential, Identity, Provider};

/// An engine over the fixture whose oracle is the real `HttpOracle`, answered by `fx.http`.
fn http_engine(fx: &Fx) -> Engine {
    fx.engine_with_oracle(Arc::new(HttpOracle::new(fx.http.clone(), fx.clock.clone())))
}

fn profile_asks(fx: &Fx) -> usize {
    fx.http.count(Method::Get, &Fx::endpoints().profile)
}

fn live_bytes(fx: &Fx) -> Vec<u8> {
    fx.live_credential().unwrap().to_string().into_bytes()
}

/// Forgets a stored uuid, as an account added before its uuid was known would have none.
fn forget_uuid(fx: &Fx, id: &AccountId) {
    fx.engine.store().unwrap();
    rusqlite::Connection::open(fx.env.data_dir().join("tagteam.db"))
        .unwrap()
        .execute(
            "UPDATE accounts SET account_uuid = NULL WHERE id = ?1",
            [id.as_str()],
        )
        .unwrap();
}

#[test]
fn http_oracle_resolves_through_the_provider() {
    let fx = Fx::new();
    fx.login("b@x.co", "rt-b");
    fx.script_profile("b@x.co");
    let oracle = HttpOracle::new(fx.http.clone(), fx.clock.clone());
    let owner = oracle
        .resolve(fx.cc.as_ref(), &Credential::fresh(live_bytes(&fx)))
        .unwrap();
    assert_eq!(owner.account_uuid.as_deref(), Some("uuid-b@x.co"));
    assert_eq!(profile_asks(&fx), 1);
}

/// Counts the calls that reach it; answers nothing, as a failed request would.
struct Counting(Arc<AtomicUsize>);

impl Oracle for Counting {
    fn resolve(&self, _p: &dyn Provider, _c: &Credential) -> Option<Identity> {
        self.0.fetch_add(1, Ordering::SeqCst);
        None
    }
}

#[test]
fn a_process_asks_at_most_once_per_credential() {
    // §7.6: keyed by the credential's fingerprint; no answer is remembered too.
    let fx = Fx::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let cache = CachingOracle::new(Counting(calls.clone()));
    let one = Credential::fresh(Fx::credential_json("a@x.co", "rt-1").to_string().into_bytes());
    let two = Credential::fresh(Fx::credential_json("a@x.co", "rt-2").to_string().into_bytes());
    assert!(cache.resolve(fx.cc.as_ref(), &one).is_none());
    assert!(cache.resolve(fx.cc.as_ref(), &one).is_none());
    assert!(cache.resolve(fx.cc.as_ref(), &two).is_none());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn an_attributed_rotation_is_captured_and_backfills_the_uuid() {
    // §9.4 step 4 OursRotated, decided by the oracle over HTTP: the vault takes the rotated
    // generation, `.prev` keeps the old one, and the missing uuid is backfilled.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live: b
    forget_uuid(&fx, &b);
    fx.rotate_live("rt-b2");
    fx.script_profile("b@x.co");
    http_engine(&fx).switch(fx.switch_request(&a, false)).unwrap();
    assert_eq!(profile_asks(&fx), 1);
    assert_eq!(fx.vault_refresh_token(&b).as_deref(), Some("rt-b2"));
    let prev = fx.kc.get(SERVICE, &format!("{b}.prev")).unwrap();
    assert!(String::from_utf8(prev).unwrap().contains("rt-b"));
    let row = fx.engine.store().unwrap().account(&b).unwrap().unwrap();
    assert_eq!(row.account_uuid.as_deref(), Some("uuid-b@x.co"));
    assert!(fx.displaced().is_empty());
}

#[test]
fn a_rotation_attributed_to_someone_else_is_displaced_and_the_vault_kept() {
    // L301: the outcome, not only the action: the vault still holds rt-b, and the displaced
    // file holds exactly the foreign bytes.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.rotate_live("someone-elses-token");
    let foreign = live_bytes(&fx);
    fx.script_profile("z@x.co");
    let out = http_engine(&fx).switch(fx.switch_request(&a, false)).unwrap();
    assert_eq!(fx.vault_refresh_token(&b).as_deref(), Some("rt-b"));
    let displaced = fx.displaced();
    assert_eq!(displaced.len(), 1);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&displaced[0]).unwrap(),
        serde_json::from_slice::<serde_json::Value>(&foreign).unwrap()
    );
    assert!(out.warnings.iter().any(|w| w.contains("displaced")));
}

#[test]
fn an_attributed_blob_without_a_refresh_token_is_displaced_not_captured() {
    // L301 for §6.2: even with a positive answer, a live credential without a refresh token
    // never replaces the vault's complete one. §7.6 does not exempt such a blob from the profile
    // oracle (only an expired token, a setup token and an API key), so the oracle is asked
    // and its positive answer is what the `lacks_refresh_over_complete` rule overrides.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let mut v = fx.live_credential().unwrap();
    v["claudeAiOauth"]["refreshToken"] = json!(null);
    v["claudeAiOauth"]["accessToken"] = json!("only-access");
    fx.set_live_credential(v.to_string().as_bytes());
    fx.script_profile("b@x.co");
    http_engine(&fx).switch(fx.switch_request(&a, false)).unwrap();
    assert_eq!(profile_asks(&fx), 1, "the blob is attributed, not skipped");
    assert_eq!(fx.vault_refresh_token(&b).as_deref(), Some("rt-b"));
    assert_eq!(fx.displaced().len(), 1);
}

#[test]
fn an_outgoing_credential_the_vault_holds_is_never_sent_to_the_oracle() {
    // L300 at the engine: `Ours` is decided by bytes first, so no request is made at all.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    http_engine(&fx).switch(fx.switch_request(&a, false)).unwrap();
    assert_eq!(profile_asks(&fx), 0);
}

#[test]
fn metadata_commands_recover_without_asking_the_oracle() {
    // §7.6, §9.6: alias, disable, enable and move retry recovery from fingerprints alone.
    // An account-changing command asks.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let row = crash_row(&fx, &a, &b);
    fx.engine.store().unwrap().insert_journal(&row).unwrap();
    fx.rotate_live("rt-unknown"); // neither generation the row names: only the oracle could say
    let engine = http_engine(&fx);
    engine.set_alias(&a, Some("work")).unwrap();
    engine.set_disabled(&a, true).unwrap();
    engine.set_disabled(&a, false).unwrap();
    engine.move_to(&a, 2).unwrap();
    assert_eq!(profile_asks(&fx), 0, "no metadata command may reach the network");
    assert!(matches!(engine.remove(&a), Err(EngineError::InterruptedSwitch(_))));
    assert!(profile_asks(&fx) >= 1, "remove is account-changing: it asks");
}

#[test]
fn a_hanging_profile_endpoint_delays_a_switch_by_its_timeout_and_holds_no_lock() {
    // Review Focus 5: a captive portal that never answers. The switch waits out the 5 s
    // profile timeout with no lock held, then captures the rotation as Unresolved, `.prev`
    // keeping the old generation.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.rotate_live("rt-b2");
    let server = MockServer::start();
    server.on("GET", "/api/oauth/profile", MockReply::Hang);
    let cc = Arc::new(
        ClaudeCode::with_store(
            LiveStore::new(fx.kc.clone(), Platform::MacOs).with_retry_delay(Duration::ZERO),
        )
        .with_endpoints(Endpoints::with_base(&server.base_url())),
    );
    let http: Arc<dyn Http> = Arc::new(UreqHttp::new());
    let engine = Engine::new(EngineConfig {
        env: fx.env.clone(),
        registry: ProviderRegistry::new().with(cc),
        vault: Vault::new(Box::new(KeychainVault::new(fx.kc.clone()))),
        oracle: Arc::new(HttpOracle::new(http.clone(), fx.clock.clone())),
        clock: fx.clock.clone(),
        http,
        default_provider: ProviderId::new(CLAUDE_CODE),
    });
    let env = fx.env.clone();
    let watched = b.clone();
    let watcher = thread::spawn(move || {
        thread::sleep(Duration::from_secs(2)); // well inside the hang
        (
            mutation_lock_free(&env),
            AccountLock::try_acquire(&env, &watched).unwrap().is_some(),
        )
    });
    let started = Instant::now();
    engine.switch(fx.switch_request(&a, false)).unwrap();
    let took = started.elapsed();
    assert_eq!(
        watcher.join().unwrap(),
        (true, true),
        "no lock is held while the oracle waits (§7.6)"
    );
    assert!(took >= Duration::from_millis(4_500), "{took:?}");
    assert!(took < Duration::from_secs(15), "{took:?}");
    assert_eq!(server.hits("GET", "/api/oauth/profile"), 1);
    assert_eq!(fx.vault_refresh_token(&b).as_deref(), Some("rt-b2"));
    let prev = fx.kc.get(SERVICE, &format!("{b}.prev")).unwrap();
    assert!(String::from_utf8(prev).unwrap().contains("rt-b"));
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
}
```

- [ ] **Step 11: Run them to verify they fail**

Run: `cargo test -p tagteam-engine --test oracle && cargo test -p tagteam-core classify`
Expected: FAIL to compile: `could not find HttpOracle in oracle`,
`could not find CachingOracle in oracle`. The core test passes once it compiles, because it
pins existing behaviour.

- [ ] **Step 12: Implement `HttpOracle` and `CachingOracle`**

In `crates/tagteam-engine/src/oracle.rs`, replace the imports with:

```rust
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tagteam_core::OracleVerdict;
use tagteam_provider::http::Http;
use tagteam_provider::{Clock, Credential, Identity, Provider};

use crate::store::AccountRow;
```

and add after `impl Oracle for NoOracle`:

```rust
/// §7.6 over HTTP: the provider asks its own profile endpoint. Never called under a lock (the
/// engine's callers guarantee that); a failure is no answer, logged at DEBUG.
pub struct HttpOracle {
    http: Arc<dyn Http>,
    clock: Arc<dyn Clock>,
}

impl HttpOracle {
    pub fn new(http: Arc<dyn Http>, clock: Arc<dyn Clock>) -> Self {
        Self { http, clock }
    }
}

impl Oracle for HttpOracle {
    fn resolve(&self, provider: &dyn Provider, credential: &Credential) -> Option<Identity> {
        let answer = provider.resolve_owner(self.http.as_ref(), credential, self.clock.now_ms());
        if answer.is_none() {
            tracing::debug!(provider = %provider.id(), "the profile oracle gave no answer");
        }
        answer
    }
}

/// Asks `inner` at most once per process for a given credential (§7.6), keyed by provider and
/// fingerprint. No answer is remembered too: a retry within one command would only repeat a
/// failure the command already treats as advisory. The lock is never held across `inner`.
pub struct CachingOracle<O: Oracle> {
    inner: O,
    answers: Mutex<HashMap<(String, String), Option<Identity>>>,
}

impl<O: Oracle> CachingOracle<O> {
    pub fn new(inner: O) -> Self {
        Self {
            inner,
            answers: Mutex::new(HashMap::new()),
        }
    }
}

impl<O: Oracle> Oracle for CachingOracle<O> {
    fn resolve(&self, provider: &dyn Provider, credential: &Credential) -> Option<Identity> {
        let Some(fp) = provider.fingerprint(credential.bytes()) else {
            return self.inner.resolve(provider, credential);
        };
        let key = (provider.id().to_string(), fp.as_str().to_owned());
        if let Some(answer) = self.answers.lock().unwrap().get(&key) {
            return answer.clone();
        }
        let answer = self.inner.resolve(provider, credential);
        self.answers.lock().unwrap().insert(key, answer.clone());
        answer
    }
}
```

- [ ] **Step 13: Keep metadata commands off the network**

First, move the recovery tests off the metadata path they use today. In
`crates/tagteam-engine/tests/recover.rs`, every test triggers recovery through `any_mutation`,
which runs `set_disabled`. Four of them depend on the oracle's answer:
- `a_rotated_target_the_oracle_attributes_finishes_forward` (`oracle_says`, line 72)
- `a_rotated_outgoing_credential_the_oracle_attributes_finishes_backward` (line 108)
- `live_bytes_beat_a_contradicting_oracle_both_ways` (line 413)
- `an_oracle_answer_without_a_uuid_never_decides` (line 432). It would still pass, but it would
  no longer prove its rule, because the oracle would never be asked.

Once `set_disabled` recovers from fingerprints alone (below), the first three rows would stay
undecidable.
Replace the helper, keeping its name so no call site changes:

```rust
fn any_mutation(fx: &Fx, _id: &AccountId) {
    // An account-changing command's mutation lock: it recovers, asking the oracle before it
    // locks (§9.6, §7.6). Metadata commands recover from fingerprints alone (Task 7), which
    // `metadata_commands_recover_without_asking_the_oracle` in tests/oracle.rs covers.
    drop(fx.engine.mutation_guard().unwrap());
}
```

`crates/tagteam-engine/tests/destroyed.rs:214` also recovers through `set_disabled`, but its
row is decided by fingerprint (`crashed_switch` plus `write_target_credential`), with no oracle
answer, so it keeps passing unchanged and also covers the offline path.

Then, in `crates/tagteam-engine/src/engine.rs`:

1. Replace `guard_recovering`'s signature, doc comment and hint collection:

```rust
    /// `mutation_guard`, with the refusal for each row whose recovery could not take its
    /// provider's live locks (`RecoveryBlocked`), by provider. With `ask_oracle` false the
    /// rows are recovered from fingerprints alone: no network call (§7.6, §9.6).
    fn guard_recovering(
        &self,
        ask_oracle: bool,
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
```

   The rest of the body stays as it is.

2. In `guard_or_refuse`, change `let (guard, blocked) = self.guard_recovering()?;` to
   `let (guard, blocked) = self.guard_recovering(true)?;`.

3. Replace `mutation_guard` with both guards:

```rust
    /// tagteam's mutation lock. Before returning it, recovers every interrupted switch whose
    /// holder has died (§9.6). The oracle is asked before the lock is taken (§7.6).
    pub fn mutation_guard(&self) -> Result<MutationGuard, EngineError> {
        Ok(self.guard_recovering(true)?.0)
    }

    /// The mutation lock for commands that change only store metadata (`alias`, `disable`,
    /// `enable`, `move`): recovery still runs, but from fingerprints alone, so these commands
    /// never make a network call (§7.6).
    pub(crate) fn metadata_guard(&self) -> Result<MutationGuard, EngineError> {
        Ok(self.guard_recovering(false)?.0)
    }
```

In `crates/tagteam-engine/src/lifecycle.rs`, in each of `set_alias`, `set_disabled` and
`move_to`, change `let _guard = self.mutation_guard()?;` to
`let _guard = self.metadata_guard()?;`. Add this sentence to each of the three doc comments:
`It never asks the oracle (§7.6).`

- [ ] **Step 14: Run the engine tests to verify they pass**

Run: `cargo test -p tagteam-engine --test oracle && cargo test -p tagteam-engine --test manage && cargo test -p tagteam-core classify`
Expected: PASS. `a_hanging_profile_endpoint…` takes about 5 s.

Run: `cargo test -p tagteam-engine --features test-hooks --test recover --test destroyed`
Expected: PASS: every recovery test, the four oracle-dependent ones included, through
`mutation_guard`.

- [ ] **Step 15: Commit the engine half**

```bash
git add crates/tagteam-core/src/classify.rs crates/tagteam-engine/src/oracle.rs \
  crates/tagteam-engine/src/engine.rs crates/tagteam-engine/src/lifecycle.rs \
  crates/tagteam-engine/tests/common/mod.rs crates/tagteam-engine/tests/oracle.rs \
  crates/tagteam-engine/tests/recover.rs
git commit -m "Ask the profile oracle over HTTP at most once per credential, never from metadata commands"
```

- [ ] **Step 16: Write the failing CLI tests**

In `crates/tagteam/tests/common/mod.rs`, add below `LOCKED`:

```rust
/// Where every endpoint points unless a test starts a `MockServer`: a local port nothing
/// listens on, so a request fails at once as `PreSend` and no test reaches the network.
pub const OFFLINE_API_BASE: &str = "http://127.0.0.1:9";
```

and add `.env("TAGTEAM_TEST_API_BASE", OFFLINE_API_BASE)` to `std_cmd`'s builder chain, after
the `TAGTEAM_TEST_PLATFORM` line.

In `crates/tagteam/tests/app.rs`, the in-process harness builds `Context` itself. In `H::run`,
add the field to the literal:

```rust
        let ctx = Context {
            env: self.env.clone(),
            keychain: self.kc.clone(),
            platform: Platform::MacOs,
            api_base: Some(common::OFFLINE_API_BASE.into()),
        };
```

Append to `crates/tagteam/tests/cli.rs` (add `use tagteam_provider::Keychain;` and
`use tagteam_provider::mock_server::{MockReply, MockServer};` to its imports):

```rust
#[test]
fn a_switch_asks_the_configured_profile_endpoint() {
    // §7.6 through the binary: the oracle answer attributes b's rotation to b, so the switch
    // captures it into b's vault; the request carried the bearer and tagteam's User-Agent.
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    let kc = FileKeychain::new(d.path().join("keychain"));
    seed_home(&env);
    login(&env, &kc, "a@x.co", "", "rt-a");
    cmd(d.path()).arg("add").assert().success();
    login(&env, &kc, "b@x.co", "", "rt-b");
    cmd(d.path()).arg("add").assert().success();
    login(&env, &kc, "b@x.co", "", "rt-b2"); // CC rotated b in place
    let server = MockServer::start();
    server.on(
        "GET",
        "/api/oauth/profile",
        MockReply::Json {
            status: 200,
            body: json!({"account": {"uuid": "uuid-b@x.co-", "email": "b@x.co"}, "organization": {"uuid": ""}}),
        },
    );
    cmd(d.path())
        .env("TAGTEAM_TEST_API_BASE", server.base_url())
        .args(["switch", "1", "--json"])
        .assert()
        .success();
    assert_eq!(server.hits("GET", "/api/oauth/profile"), 1);
    let req = &server.requests()[0];
    let header = |name: &str| {
        req.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    };
    assert_eq!(header("authorization").as_deref(), Some("Bearer at"));
    assert!(header("user-agent").unwrap().starts_with("tagteam/"));
    let list: Value = serde_json::from_slice(
        &cmd(d.path())
            .args(["list", "--json"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let b_id = list["accounts"][1]["id"].as_str().unwrap().to_owned();
    let stored = kc.find("tagteam", &b_id).present().unwrap();
    assert!(String::from_utf8(stored).unwrap().contains("rt-b2"));
}

#[test]
fn by_default_the_test_binary_never_reaches_the_network() {
    // The harness points every endpoint at OFFLINE_API_BASE: a switch that asks the oracle
    // gets `PreSend` at once and still completes (the oracle is advisory, §7.6).
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    let kc = FileKeychain::new(d.path().join("keychain"));
    seed_home(&env);
    login(&env, &kc, "a@x.co", "", "rt-a");
    cmd(d.path()).arg("add").assert().success();
    login(&env, &kc, "b@x.co", "", "rt-b");
    cmd(d.path()).arg("add").assert().success();
    login(&env, &kc, "b@x.co", "", "rt-b2");
    let started = std::time::Instant::now();
    cmd(d.path()).args(["switch", "1"]).assert().success();
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
}
```

In `crates/tagteam/Cargo.toml`, make the `test-support` feature enable the mock server and give
the dev-dependency the same feature. No earlier task touches this file; Task 18 relies on this
edit:

```toml
test-support = ["tagteam-provider/file-keychain", "tagteam-provider/mock-server", "tagteam-engine/test-hooks", "tagteam-cc/test-hooks"]
```

```toml
tagteam-provider = { workspace = true, features = ["file-keychain", "mock-server"] }
```

- [ ] **Step 17: Run them to verify they fail**

Run: `cargo test -p tagteam --features test-support --test cli a_switch_asks_the_configured && cargo test -p tagteam --test app`
Expected: FAIL. `app.rs` does not compile (`struct Context has no field named api_base`).
`a_switch_asks…` gets 0 hits, because the binary still uses `NoOracle` and ignores
`TAGTEAM_TEST_API_BASE`.

- [ ] **Step 18: Wire the real oracle and the test override into the CLI**

In `crates/tagteam/src/app.rs`:

1. Imports: replace `use tagteam_engine::oracle::NoOracle;` with
   `use tagteam_engine::oracle::{CachingOracle, HttpOracle};`, and add
   `use tagteam_cc::endpoints::Endpoints;` and
   `use tagteam_provider::{Clock, http::Http};` (merge with the existing `tagteam_provider`
   import; Task 3 already imports `tagteam_engine::net::UreqHttp`).

2. Add the constant next to the other two test variables:

```rust
#[cfg(any(test, feature = "test-support"))]
const TEST_API_BASE: &str = "TAGTEAM_TEST_API_BASE";
```

3. Add the field to `Context`:

```rust
pub struct Context {
    pub env: Env,
    pub keychain: Arc<dyn Keychain>,
    pub platform: Platform,
    /// Every endpoint under this base instead of production; only a test-support build sets it.
    pub api_base: Option<String>,
}
```

4. Replace the `Overrides` type alias and both `test_overrides` functions with:

```rust
#[derive(Default)]
struct Overrides {
    keychain: Option<Arc<dyn Keychain>>,
    platform: Option<Platform>,
    api_base: Option<String>,
}

/// The test harness's keychain, platform and endpoint base, read through `var` so a test can
/// supply the environment without mutating the process's.
#[cfg(feature = "test-support")]
fn test_overrides(var: &dyn Fn(&str) -> Option<OsString>) -> Overrides {
    let keychain = var(TEST_KEYCHAIN_DIR).map(|d| {
        Arc::new(tagteam_provider::FileKeychain::new(
            std::path::PathBuf::from(d),
        )) as Arc<dyn Keychain>
    });
    let platform = match var(TEST_PLATFORM).as_deref().and_then(|v| v.to_str()) {
        Some("linux") => Some(Platform::Linux),
        Some("macos") => Some(Platform::MacOs),
        _ => None,
    };
    let api_base = var(TEST_API_BASE).and_then(|v| v.into_string().ok());
    Overrides {
        keychain,
        platform,
        api_base,
    }
}

/// A release build has no test overrides, whatever the environment holds.
#[cfg(not(feature = "test-support"))]
fn test_overrides(_var: &dyn Fn(&str) -> Option<OsString>) -> Overrides {
    Overrides::default()
}
```

5. Replace `Context::from_process`:

```rust
impl Context {
    pub fn from_process() -> Self {
        let o = test_overrides(&|k| std::env::var_os(k));
        Self {
            env: Env::from_process(),
            keychain: o.keychain.unwrap_or_else(|| Arc::new(SecurityCli::new())),
            platform: o.platform.unwrap_or_else(Platform::current),
            api_base: o.api_base,
        }
    }
}
```

6. Replace the whole `build_engine` function, including whatever Task 3 left in it, with:

```rust
fn build_engine(ctx: Context) -> Engine {
    let mut cc = ClaudeCode::new(ctx.keychain.clone(), ctx.platform);
    if let Some(base) = &ctx.api_base {
        cc = cc.with_endpoints(Endpoints::with_base(base));
    }
    let vault = match ctx.platform {
        Platform::MacOs => Vault::new(Box::new(KeychainVault::new(ctx.keychain))),
        Platform::Linux => Vault::new(Box::new(FileVault::new(ctx.env.data_dir().join("vault")))),
    };
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let http: Arc<dyn Http> = Arc::new(UreqHttp::new());
    Engine::new(EngineConfig {
        env: ctx.env,
        registry: ProviderRegistry::new().with(Arc::new(cc)),
        vault,
        // §7.6: asked at most once per credential within one command.
        oracle: Arc::new(CachingOracle::new(HttpOracle::new(http.clone(), clock.clone()))),
        clock,
        http,
        default_provider: ProviderId::new(CLAUDE_CODE),
    })
}
```

7. In the `tests` module, replace `both_set` and `the_test_overrides_exist_only_with_test_support`:

```rust
    /// An environment that sets every test override.
    fn all_set(k: &str) -> Option<OsString> {
        match k {
            TEST_KEYCHAIN_DIR => Some("/nonexistent/keychain".into()),
            TEST_PLATFORM => Some("linux".into()),
            TEST_API_BASE => Some("http://127.0.0.1:9".into()),
            _ => None,
        }
    }

    /// Runs under both builds: with `test-support` the overrides are honoured; without it (a
    /// release build) they are ignored, whatever the environment holds.
    #[test]
    fn the_test_overrides_exist_only_with_test_support() {
        let o = test_overrides(&all_set);
        let honoured = cfg!(feature = "test-support");
        assert_eq!(o.keychain.is_some(), honoured);
        assert_eq!(o.platform, honoured.then_some(Platform::Linux));
        assert_eq!(
            o.api_base.as_deref(),
            honoured.then_some("http://127.0.0.1:9")
        );
    }
```

- [ ] **Step 19: Run the CLI tests to verify they pass**

Run:
```bash
cargo test -p tagteam --lib
cargo test -p tagteam --features test-support
```
Expected: PASS, including the release-build branch of
`the_test_overrides_exist_only_with_test_support` under `--lib` without features. The existing
`cli.rs`, `kill.rs` and `app.rs` suites are unchanged in outcome and add no network traffic:
every oracle request goes to `127.0.0.1:9` and fails as `PreSend`.

- [ ] **Step 20: Commit**

```bash
git add crates/tagteam/src/app.rs crates/tagteam/Cargo.toml crates/tagteam/tests/common/mod.rs \
  crates/tagteam/tests/app.rs crates/tagteam/tests/cli.rs
git commit -m "Use the HTTP oracle in the CLI and keep its tests off the network"
```

---

### Task 8: Token refresh request and response (Claude Code and `FakeAgent`)

The provider's half of §7.3: build the token request, send it through the port, classify the
reply, and compose the successor in the provider's stored shape. Nothing here touches the
vault, a lock or `rescue/`; the gate (Tasks 10–11) owns all of that. The rule this task carries
into the parser is §7.3's: **a successor that tagteam received is never discarded**. A reply that
names a new refresh token yields `Refreshed` even when the rest of it is incomplete.

**Files:**
- Modify: `crates/tagteam-provider/src/provider.rs` (the refresh types and the trait method),
  `crates/tagteam-provider/src/lib.rs` (re-exports), `crates/tagteam-cc/src/shape.rs`,
  `crates/tagteam-cc/src/oauth.rs`, `crates/tagteam-cc/src/provider.rs`,
  `crates/tagteam-cc/tests/oauth.rs`, `crates/tagteam-fake/src/provider.rs`,
  `crates/tagteam-fake/tests/network.rs`, `crates/tagteam-engine/tests/common/mod.rs`
- Read: `crates/tagteam-cc/tests/fixtures/endpoints/token-200.json` (created by Task 1)

**Interfaces:**
- Consumes:
  - Task 2: `tagteam_provider::http::{Http, HttpRequest, HttpResponse, HttpError, Method, ScriptedHttp}`
    (`HttpRequest::post_json(url, &Value, timeout)`, `HttpResponse::json()`).
  - Task 4: `tagteam_cc::shape::{access_token, scopes}`.
  - Task 7: `tagteam_cc::endpoints::{CLIENT_ID, Endpoints}`, `ClaudeCode`'s `endpoints`
    field, and `Fx::endpoints()`.
  - Task 6: `tagteam_fake::{FakeAgent, credential_json}`, `FakeAgent::renew_url()`.
  - Task 1: `token-200.json` (synthetic), `token-invalid-grant.json`, `token-invalid-client.json`.
- Produces:
  - `tagteam_provider::provider::{DeadReason, TransientKind, RefreshResult}`, also re-exported
    at the crate root, with `DeadReason::as_str`, `TransientKind::token`, and a redacting
    `Debug`.
  - `Provider::refresh(&self, http: &dyn Http, cred: &FreshCredential, now_ms: i64, timeout: Duration) -> RefreshResult`.
  - `tagteam_cc::shape::{TokenFields, apply_refresh, refresh_token}`.
  - `tagteam_cc::oauth::{TOKEN_TIMEOUT, refresh_request, parse_refresh}`, and the
    `pub(crate)` `token_owner`: a token reply names its owner by account uuid or by
    organization alone (§7.4), unlike the profile oracle, which needs the uuid (§7.6).
  - `Fx::script_refresh(&self, new_rt: Option<&str>)`,
    `Fx::script_token_error(&self, status: u16, error: &str)`.

- [ ] **Step 1: Check the synthetic success fixture**

A successful refresh cannot be recorded without spending a real token, so Task 1 built
`crates/tagteam-cc/tests/fixtures/endpoints/token-200.json` from Appendix A.5 (Task 1, Step 3).
This task does not write it. The test in Step 4 (`the_synthetic_success_reply_yields_its_successor_and_owner`) pins its exact values:

| Field | Value |
|---|---|
| `access_token` | `sk-ant-oat01-synthetic-access-token` |
| `refresh_token` | `sk-ant-ort01-synthetic-refresh-token` |
| `expires_in` | `28800` |
| `refresh_token_expires_in` | `7776000` |
| `scope` | `user:inference user:profile` |
| `account.uuid` | `00000000-0000-4000-8000-000000000001` |
| `account.email_address` | `probe1@example.com` |
| `organization.uuid` | `00000000-0000-4000-8000-000000000002` |

Run: `grep -c synthetic-refresh-token crates/tagteam-cc/tests/fixtures/endpoints/token-200.json`
Expected: `1`. If the file is missing, or its values differ, stop: Task 1 is not done.

- [ ] **Step 2: Write the failing shape tests**

Add to the `tests` module of `crates/tagteam-cc/src/shape.rs`:

```rust
    fn fields(rt: Option<&str>) -> TokenFields {
        TokenFields {
            access_token: "at-new".into(),
            refresh_token: rt.map(str::to_owned),
            expires_at: 2_000,
            scopes: Some(vec!["user:inference".into(), "user:profile".into()]),
            refresh_token_expires_at: Some(9_000),
        }
    }

    #[test]
    fn apply_refresh_replaces_the_tokens_and_keeps_every_other_key_in_place() {
        let old = json!({
            "claudeAiOauth": {
                "accessToken": "at-old",
                "refreshToken": "rt-old",
                "expiresAt": 1,
                "scopes": ["user:inference"],
                "subscriptionType": "max",
                "rateLimitTier": "t"
            },
            "trustedDeviceToken": "d",
            "mcpOAuth": {"srv": {"token": "machine-shared"}}
        });
        let out: Value = serde_json::from_slice(
            &apply_refresh(old.to_string().as_bytes(), &fields(Some("rt-new"))).unwrap(),
        )
        .unwrap();
        assert_eq!(
            out,
            json!({
                "claudeAiOauth": {
                    "accessToken": "at-new",
                    "refreshToken": "rt-new",
                    "expiresAt": 2_000,
                    "scopes": ["user:inference", "user:profile"],
                    "subscriptionType": "max",
                    "rateLimitTier": "t",
                    "refreshTokenExpiresAt": 9_000
                },
                "trustedDeviceToken": "d",
                "mcpOAuth": {"srv": {"token": "machine-shared"}}
            })
        );
        let keys: Vec<&str> = out["claudeAiOauth"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            [
                "accessToken",
                "refreshToken",
                "expiresAt",
                "scopes",
                "subscriptionType",
                "rateLimitTier",
                "refreshTokenExpiresAt"
            ],
            "existing keys keep their place; new ones are appended"
        );
    }

    #[test]
    fn a_reply_without_a_refresh_token_keeps_the_stored_one() {
        // Review Focus 3: the lineage is unchanged, so the fingerprint is too.
        let old = json!({"claudeAiOauth": {"accessToken": "at-old", "refreshToken": "rt-old", "expiresAt": 1}});
        let out = apply_refresh(old.to_string().as_bytes(), &fields(None)).unwrap();
        assert_eq!(refresh_token(&out).as_deref(), Some("rt-old"));
        assert_eq!(
            fingerprint(&out),
            fingerprint(old.to_string().as_bytes())
        );
        assert_eq!(access_token(&out).as_deref(), Some("at-new"));
    }

    #[test]
    fn apply_refresh_refuses_a_stored_value_that_is_not_an_object() {
        assert!(apply_refresh(b"sk-ant-api03-key", &fields(Some("rt"))).is_err());
        assert!(
            apply_refresh(br#"{"claudeAiOauth": "x"}"#, &fields(Some("rt"))).is_err()
        );
    }

    #[test]
    fn token_fields_never_print_their_tokens() {
        let shown = format!("{:?}", fields(Some("rt-secret-value")));
        assert!(!shown.contains("rt-secret-value") && !shown.contains("at-new"), "{shown}");
    }
```

Run: `cargo test -p tagteam-cc --lib shape`
Expected: FAIL to compile (`cannot find struct TokenFields`, `cannot find function apply_refresh`,
`cannot find function refresh_token`).

- [ ] **Step 3: Implement the shape half**

Add to `crates/tagteam-cc/src/shape.rs`, after `login_expires_at`:

```rust
/// The stored refresh token, if the credential has one.
pub fn refresh_token(bytes: &[u8]) -> Option<String> {
    oauth_obj(bytes).and_then(|o| token(&o, "refreshToken").map(str::to_owned))
}

/// A refresh reply's token fields (Appendix A.5), ready to apply to the stored credential.
#[derive(Clone, PartialEq, Eq)]
pub struct TokenFields {
    pub access_token: String,
    /// `None` when the reply carried no refresh token: the stored one is kept.
    pub refresh_token: Option<String>,
    pub expires_at: i64,
    /// `None` when the reply carried no `scope`: the stored scopes are kept.
    pub scopes: Option<Vec<String>>,
    pub refresh_token_expires_at: Option<i64>,
}

/// Never shows a token.
impl std::fmt::Debug for TokenFields {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenFields")
            .field("refresh_token", &self.refresh_token.as_ref().map(|_| "<redacted>"))
            .field("expires_at", &self.expires_at)
            .field("scopes", &self.scopes)
            .field("refresh_token_expires_at", &self.refresh_token_expires_at)
            .finish_non_exhaustive()
    }
}

/// Appendix A.5: the reply's fields replace the stored ones inside `claudeAiOauth`, the
/// refresh token only when the reply carries one. Every other key, at every level, is kept
/// where it was; new keys are appended.
pub fn apply_refresh(old: &[u8], f: &TokenFields) -> Result<Vec<u8>, ProviderError> {
    let not_an_object =
        || ProviderError::Invalid("the stored credential is not a JSON object".into());
    let mut root = match serde_json::from_slice::<Value>(old) {
        Ok(Value::Object(o)) => o,
        _ => return Err(not_an_object()),
    };
    let entry = root
        .entry("claudeAiOauth")
        .or_insert_with(|| Value::Object(Map::new()));
    let o = entry.as_object_mut().ok_or_else(not_an_object)?;
    o.insert("accessToken".into(), json!(f.access_token));
    if let Some(rt) = &f.refresh_token {
        o.insert("refreshToken".into(), json!(rt));
    }
    o.insert("expiresAt".into(), json!(f.expires_at));
    if let Some(s) = &f.scopes {
        o.insert("scopes".into(), json!(s));
    }
    if let Some(e) = f.refresh_token_expires_at {
        o.insert("refreshTokenExpiresAt".into(), json!(e));
    }
    Ok(serde_json::to_vec(&Value::Object(root)).expect("a Value always serializes"))
}
```

(`serde_json::Map` keeps insertion order under `preserve_order`, and `insert` on an existing key
keeps that key's position.)

Run: `cargo test -p tagteam-cc --lib shape`
Expected: PASS.

- [ ] **Step 4: Write the failing protocol tests**

In `crates/tagteam-cc/tests/oauth.rs`, replace the import block with:

```rust
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tagteam_cc::endpoints::{CLIENT_ID, Endpoints};
use tagteam_cc::live::{LiveStore, Platform};
use tagteam_cc::oauth::{
    PROFILE_TIMEOUT, TOKEN_TIMEOUT, parse_profile, parse_refresh, profile_request,
    refresh_request,
};
use tagteam_cc::provider::ClaudeCode;
use tagteam_cc::shape::{self, setup_token_credential};
use tagteam_provider::http::{HttpError, HttpResponse, Method, ScriptedHttp};
use tagteam_provider::provider::{DeadReason, RefreshResult, TransientKind};
use tagteam_provider::{Credential, FakeKeychain, Provider};
```

and append:

```rust
fn stored() -> Vec<u8> {
    json!({
        "claudeAiOauth": {
            "accessToken": "at-old",
            "refreshToken": "rt-old",
            "expiresAt": 1,
            "scopes": ["user:inference", "user:profile"],
            "subscriptionType": "max"
        },
        "mcpOAuth": {"srv": {"token": "machine-shared"}}
    })
    .to_string()
    .into_bytes()
}

fn successor(r: RefreshResult) -> Value {
    match r {
        RefreshResult::Refreshed { successor, .. } => serde_json::from_slice(&successor).unwrap(),
        other => panic!("expected Refreshed, got {other:?}"),
    }
}

#[test]
fn the_refresh_request_follows_appendix_a5() {
    let scopes = vec!["user:inference".to_owned(), "user:profile".to_owned()];
    let req = refresh_request(&Endpoints::production(), "rt-secret", &scopes, TOKEN_TIMEOUT);
    assert_eq!(req.method, Method::Post);
    assert_eq!(req.url, "https://platform.claude.com/v1/oauth/token");
    assert_eq!(req.timeout, Duration::from_secs(10));
    let body: Value = serde_json::from_slice(req.body.as_deref().unwrap()).unwrap();
    assert_eq!(
        body,
        json!({"grant_type": "refresh_token", "refresh_token": "rt-secret", "client_id": CLIENT_ID, "scope": "user:inference user:profile"})
    );
    assert_eq!(CLIENT_ID, "9d1c250a-e61b-44d9-88ed-5944d1962f5e");
    assert!(!format!("{req:?}").contains("rt-secret"));
    let bare = refresh_request(&Endpoints::production(), "rt", &[], Duration::from_secs(6));
    let body: Value = serde_json::from_slice(bare.body.as_deref().unwrap()).unwrap();
    assert!(body.get("scope").is_none(), "no stored scopes: none are invented");
    assert_eq!(bare.timeout, Duration::from_secs(6));
}

#[test]
fn the_synthetic_success_reply_yields_its_successor_and_owner() {
    let (resp, _) = fixture(include_str!("fixtures/endpoints/token-200.json"));
    let r = parse_refresh(&stored(), Ok(resp), NOW);
    let RefreshResult::Refreshed { successor, owner } = r else {
        panic!("expected Refreshed");
    };
    let s: Value = serde_json::from_slice(&successor).unwrap();
    let o = &s["claudeAiOauth"];
    // Task 1's exact values (token-200.json).
    assert_eq!(o["accessToken"], "sk-ant-oat01-synthetic-access-token");
    assert_eq!(o["refreshToken"], "sk-ant-ort01-synthetic-refresh-token");
    assert_eq!(o["expiresAt"], json!(NOW + 28_800_000));
    assert_eq!(o["refreshTokenExpiresAt"], json!(NOW + 7_776_000_000i64));
    assert_eq!(o["scopes"], json!(["user:inference", "user:profile"]));
    assert_eq!(o["subscriptionType"], "max", "untouched keys stay");
    assert_eq!(s["mcpOAuth"], json!({"srv": {"token": "machine-shared"}}));
    let owner = owner.expect("the reply names its account");
    assert_eq!(
        owner.account_uuid.as_deref(),
        Some("00000000-0000-4000-8000-000000000001")
    );
    assert_eq!(owner.email.as_deref(), Some("probe1@example.com"));
    assert_eq!(owner.org_uuid, "00000000-0000-4000-8000-000000000002");
}

#[test]
fn the_recorded_error_replies_are_dead_and_systemic() {
    let (grant, _) = fixture(include_str!("fixtures/endpoints/token-invalid-grant.json"));
    assert!(matches!(
        parse_refresh(&stored(), Ok(grant), NOW),
        RefreshResult::Dead(DeadReason::InvalidGrant)
    ));
    // The real unknown-client answer: a 400 with a nested `invalid_request_error`, not RFC
    // 6749's top-level `invalid_client` (Appendix A.5). The server's message is quoted.
    let (client, body) = fixture(include_str!("fixtures/endpoints/token-invalid-client.json"));
    assert_eq!(client.status, 400);
    assert!(body["error"].is_object(), "the recording is the nested shape");
    let RefreshResult::Systemic(message) = parse_refresh(&stored(), Ok(client), NOW) else {
        panic!("expected Systemic");
    };
    assert_eq!(message, body["error"]["message"].as_str().unwrap());
    assert!(message.starts_with("Client with id "), "{message}");
}

#[test]
fn both_client_refusal_shapes_are_systemic_and_quote_the_server() {
    let systemic = |status: u16, body: Value| match parse_refresh(&stored(), Ok(reply(status, &body)), NOW) {
        RefreshResult::Systemic(m) => m,
        other => panic!("expected Systemic, got {other:?}"),
    };
    // RFC 6749: top-level `invalid_client`, any status; `error_description` when present,
    // else the code.
    for status in [400, 401, 403] {
        assert_eq!(
            systemic(status, json!({"error": "invalid_client", "error_description": "bad client"})),
            "bad client"
        );
        assert_eq!(systemic(status, json!({"error": "invalid_client"})), "invalid_client");
    }
    assert_eq!(
        systemic(400, json!({"error": "invalid_client", "error_description": ""})),
        "invalid_client",
        "an empty description is no message"
    );
    // The endpoint's own shape: a 400 with a nested `invalid_request_error`.
    let nested = |message: Option<&str>| {
        let mut error = json!({"type": "invalid_request_error"});
        if let Some(m) = message {
            error["message"] = json!(m);
        }
        json!({"type": "error", "error": error, "request_id": "req_1"})
    };
    assert_eq!(systemic(400, nested(Some("Client with id x not found"))), "Client with id x not found");
    assert_eq!(systemic(400, nested(None)), "invalid_request_error");
    // Only a 400 carrying that exact nested type is a client refusal; everything else keeps
    // its old row.
    let transient = |status: u16, body: Value| match parse_refresh(&stored(), Ok(reply(status, &body)), NOW) {
        RefreshResult::Transient(k) => k,
        other => panic!("expected Transient, got {other:?}"),
    };
    assert_eq!(transient(401, nested(Some("m"))), TransientKind::Http(401));
    assert_eq!(transient(500, nested(Some("m"))), TransientKind::Http(500));
    assert_eq!(
        transient(400, json!({"type": "error", "error": {"type": "api_error", "message": "m"}})),
        TransientKind::Http(400),
    );
    assert_eq!(
        transient(400, json!({"error": {"message": "no type"}})),
        TransientKind::Http(400),
    );
    // A nested error is never a strike: it can't be `invalid_grant`.
    assert_eq!(
        transient(400, json!({"error": {"type": "invalid_grant"}})),
        TransientKind::Http(400),
    );
}

#[test]
fn every_row_of_the_verdict_table() {
    // §7.3 step 7, the provider's half (the engine re-reads the lineage before quarantining).
    let err = |e: &str| json!({"error": e});
    for status in [400, 401, 403] {
        assert!(
            matches!(
                parse_refresh(&stored(), Ok(reply(status, &err("invalid_grant"))), NOW),
                RefreshResult::Dead(DeadReason::InvalidGrant)
            ),
            "{status}"
        );
    }
    let transient = |r: RefreshResult| match r {
        RefreshResult::Transient(k) => k,
        other => panic!("expected Transient, got {other:?}"),
    };
    // Only 400/401/403 with `invalid_grant` is a strike.
    assert_eq!(
        transient(parse_refresh(&stored(), Ok(reply(500, &err("invalid_grant"))), NOW)),
        TransientKind::Http(500)
    );
    let k = transient(parse_refresh(&stored(), Ok(reply(401, &err("unauthorized"))), NOW));
    assert_eq!((k.clone(), k.token()), (TransientKind::Http(401), "http-401".to_owned()));
    assert_eq!(
        transient(parse_refresh(&stored(), Ok(reply(429, &json!({}))), NOW)),
        TransientKind::Http(429)
    );
    for status in [400, 401] {
        // RFC 6749's top-level shape; the nested one is pinned by the recorded fixture and by
        // `both_client_refusal_shapes_are_systemic_and_quote_the_server`.
        assert!(matches!(
            parse_refresh(&stored(), Ok(reply(status, &err("invalid_client"))), NOW),
            RefreshResult::Systemic(m) if m == "invalid_client"
        ));
    }
    let pre = transient(parse_refresh(
        &stored(),
        Err(HttpError::PreSend("dns".into())),
        NOW,
    ));
    assert_eq!((pre.clone(), pre.token()), (TransientKind::PreSend, "pre-send".to_owned()));
    let amb = transient(parse_refresh(
        &stored(),
        Err(HttpError::Ambiguous("reset".into())),
        NOW,
    ));
    assert_eq!((amb.clone(), amb.token()), (TransientKind::Ambiguous, "ambiguous".to_owned()));
}

#[test]
fn a_success_reply_with_nothing_usable_is_a_bad_response() {
    // Review Focus 2: no token of either kind, or no JSON at all: nothing was received, so
    // nothing may be persisted, quarantined or rescued.
    let transient = |r: RefreshResult| match r {
        RefreshResult::Transient(k) => k,
        other => panic!("expected Transient, got {other:?}"),
    };
    let empty = transient(parse_refresh(
        &stored(),
        Ok(reply(200, &json!({"token_type": "Bearer", "expires_in": 28800}))),
        NOW,
    ));
    assert_eq!((empty.clone(), empty.token()), (TransientKind::BadResponse, "bad-response".to_owned()));
    let html = HttpResponse {
        status: 200,
        headers: vec![],
        body: b"<html>proxy login</html>".to_vec(),
    };
    assert_eq!(
        transient(parse_refresh(&stored(), Ok(html), NOW)),
        TransientKind::BadResponse
    );
}

#[test]
fn a_reply_naming_a_refresh_token_is_never_discarded() {
    // §7.3: a successor tagteam received is persisted, even from an incomplete reply. Without
    // an access token or an expiry, it is stamped expired, so the next use refreshes again.
    let s = successor(parse_refresh(
        &stored(),
        Ok(reply(200, &json!({"refresh_token": "rt-new"}))),
        NOW,
    ));
    assert_eq!(s["claudeAiOauth"]["refreshToken"], "rt-new");
    assert_eq!(s["claudeAiOauth"]["expiresAt"], json!(NOW));
    let s = successor(parse_refresh(
        &stored(),
        Ok(reply(200, &json!({"access_token": "at-new", "refresh_token": "rt-new"}))),
        NOW,
    ));
    assert_eq!(s["claudeAiOauth"]["expiresAt"], json!(NOW), "no expires_in: expires now");
}

#[test]
fn a_reply_without_a_refresh_token_keeps_the_lineage() {
    // Review Focus 3.
    let s = successor(parse_refresh(
        &stored(),
        Ok(reply(200, &json!({"access_token": "at-new", "expires_in": 60}))),
        NOW,
    ));
    let bytes = serde_json::to_vec(&s).unwrap();
    assert_eq!(shape::fingerprint(&bytes), shape::fingerprint(&stored()));
    assert_eq!(s["claudeAiOauth"]["accessToken"], "at-new");
    assert_eq!(s["claudeAiOauth"]["expiresAt"], json!(NOW + 60_000));
    assert_eq!(
        s["claudeAiOauth"]["scopes"],
        json!(["user:inference", "user:profile"]),
        "no scope in the reply: the stored scopes stay"
    );
}

#[test]
fn refresh_sends_the_stored_token_and_scopes() {
    let http = ScriptedHttp::new();
    let url = Endpoints::production().token;
    http.push_json(
        Method::Post,
        &url,
        200,
        json!({"access_token": "at-new", "refresh_token": "rt-new", "expires_in": 28800}),
    );
    let cred = Credential::fresh(stored()).into_fresh().unwrap();
    let r = cc().refresh(&http, &cred, NOW, Duration::from_secs(6));
    assert_eq!(successor(r)["claudeAiOauth"]["refreshToken"], "rt-new");
    let sent = http.requests();
    assert_eq!(sent.len(), 1);
    let body: Value = serde_json::from_slice(sent[0].body.as_deref().unwrap()).unwrap();
    assert_eq!(body["refresh_token"], "rt-old");
    assert_eq!(body["scope"], "user:inference user:profile");
}

#[test]
fn a_credential_without_a_refresh_token_is_dead_and_never_sent() {
    let http = ScriptedHttp::new();
    let url = Endpoints::production().token;
    let access_only = Credential::fresh(
        json!({"claudeAiOauth": {"accessToken": "only-access"}})
            .to_string()
            .into_bytes(),
    )
    .into_fresh()
    .unwrap();
    assert!(matches!(
        cc().refresh(&http, &access_only, NOW, TOKEN_TIMEOUT),
        RefreshResult::Dead(DeadReason::NoRefreshToken)
    ));
    let setup = Credential::fresh(setup_token_credential("sk-ant-oat01-x"))
        .into_fresh()
        .unwrap();
    assert!(matches!(
        cc().refresh(&http, &setup, NOW, TOKEN_TIMEOUT),
        RefreshResult::Dead(DeadReason::NoRefreshToken)
    ));
    assert_eq!(http.count(Method::Post, &url), 0);
}

#[test]
fn an_organization_alone_names_the_owner() {
    // §7.4: an organization that disagrees is a conflict even when the reply names no
    // account, so the owner is built from the organization alone.
    let r = parse_refresh(
        &stored(),
        Ok(reply(
            200,
            &json!({"access_token": "at-2", "refresh_token": "rt-2", "expires_in": 60,
                    "organization": {"uuid": "org-other"}}),
        )),
        NOW,
    );
    let RefreshResult::Refreshed { owner, .. } = r else {
        panic!("expected Refreshed");
    };
    let owner = owner.expect("the organization names an owner");
    assert_eq!((owner.account_uuid.as_deref(), owner.org_uuid.as_str()), (None, "org-other"));
    let r = parse_refresh(
        &stored(),
        Ok(reply(200, &json!({"access_token": "at-2", "refresh_token": "rt-2", "expires_in": 60}))),
        NOW,
    );
    let RefreshResult::Refreshed { owner, .. } = r else {
        panic!("expected Refreshed");
    };
    assert!(owner.is_none(), "a reply naming neither has no owner");
}

#[test]
fn refresh_results_never_print_a_token() {
    let r = parse_refresh(
        &stored(),
        Ok(reply(200, &json!({"access_token": "at-SENTINEL", "refresh_token": "rt-SENTINEL", "expires_in": 1}))),
        NOW,
    );
    let shown = format!("{r:?}");
    assert!(!shown.contains("SENTINEL"), "{shown}");
    assert_eq!(DeadReason::InvalidGrant.as_str(), "invalid_grant");
    assert_eq!(DeadReason::NoRefreshToken.as_str(), "no_refresh_token");
}
```

Run: `cargo test -p tagteam-cc --test oauth`
Expected: FAIL to compile (`TOKEN_TIMEOUT`, `refresh_request`, `parse_refresh`, `RefreshResult`
and the other new names do not exist yet).

- [ ] **Step 5: Add the refresh types and the trait method**

In `crates/tagteam-provider/src/provider.rs`, change `use crate::credential::Credential;` to
`use crate::credential::{Credential, FreshCredential};`. (`Duration` is already imported by
Task 5's `use std::time::{Duration, Instant};`; do not add it again.) Then add the types, before
`pub trait Provider`:

```rust
/// Why a stored login can never refresh again (§7.3 step 7, §7.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeadReason {
    /// The token endpoint answered `invalid_grant` for the generation that was sent.
    InvalidGrant,
    /// The credential has no refresh token to send.
    NoRefreshToken,
}

impl DeadReason {
    /// The `quarantine_reason` column's value (§6.1).
    pub fn as_str(self) -> &'static str {
        match self {
            DeadReason::InvalidGrant => "invalid_grant",
            DeadReason::NoRefreshToken => "no_refresh_token",
        }
    }
}

/// A failure that may succeed on retry (§7.3 step 7). `PreSend` means nothing left the
/// machine; `Ambiguous` means the request may have been processed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransientKind {
    PreSend,
    Ambiguous,
    Http(u16),
    BadResponse,
}

impl TransientKind {
    /// `usage_state.last_error`'s kind token (§6.1).
    pub fn token(&self) -> String {
        match self {
            TransientKind::PreSend => "pre-send".into(),
            TransientKind::Ambiguous => "ambiguous".into(),
            TransientKind::Http(code) => format!("http-{code}"),
            TransientKind::BadResponse => "bad-response".into(),
        }
    }
}

/// The provider's verdict on one token request (§7.3 steps 5 and 7). The engine owns what
/// happens next: persistence, rescue, the lineage re-read and quarantine.
pub enum RefreshResult {
    /// `successor` is the whole new credential in the provider's stored shape; `owner` is the
    /// identity the reply named, if it named one (§7.4 `identity_conflict`).
    Refreshed {
        successor: Vec<u8>,
        owner: Option<Identity>,
    },
    Dead(DeadReason),
    /// The token endpoint refused the request itself: a top-level `invalid_client`, or a 400
    /// `invalid_request_error` (an unknown client id, Appendix A.5). Never a strike (§7.3).
    /// The string is the server's own message, else the error code.
    Systemic(String),
    Transient(TransientKind),
}

/// Never prints the successor's bytes, and names the owner by uuid only.
impl fmt::Debug for RefreshResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RefreshResult::Refreshed { successor, owner } => f
                .debug_struct("Refreshed")
                .field("successor", &format_args!("<{} bytes>", successor.len()))
                .field(
                    "owner_uuid",
                    &owner.as_ref().and_then(|o| o.account_uuid.as_deref()),
                )
                .finish(),
            RefreshResult::Dead(r) => f.debug_tuple("Dead").field(r).finish(),
            RefreshResult::Systemic(m) => f.debug_tuple("Systemic").field(m).finish(),
            RefreshResult::Transient(k) => f.debug_tuple("Transient").field(k).finish(),
        }
    }
}
```

Add the method at the end of `pub trait Provider`:

```rust
    /// §7.3 steps 5 and 7, the provider's half: sends the credential's refresh token with
    /// `timeout`, classifies the reply, and composes the successor (`now_ms` stamps its
    /// expiry). A credential without a refresh token is `Dead(NoRefreshToken)`, and nothing is
    /// sent. The engine calls this only for a kind whose `KindTraits::refreshable` is true.
    fn refresh(
        &self,
        http: &dyn Http,
        cred: &FreshCredential,
        now_ms: i64,
        timeout: Duration,
    ) -> RefreshResult;
```

In `crates/tagteam-provider/src/lib.rs`, add `DeadReason`, `RefreshResult` and `TransientKind`
to the `pub use provider::{…}` list.

- [ ] **Step 6: Implement the protocol half in `oauth.rs`**

In `crates/tagteam-cc/src/oauth.rs`, replace the imports with:

```rust
use std::time::Duration;

use serde_json::{Value, json};
use tagteam_provider::http::{HttpError, HttpRequest, HttpResponse};
use tagteam_provider::provider::{DeadReason, RefreshResult, TransientKind};
use tagteam_provider::Identity;

use crate::endpoints::{CLIENT_ID, Endpoints};
use crate::shape::{self, TokenFields};
```

and append:

```rust
/// §7.3: the gate's bound on its token request. Active-token refresh passes 6 s instead (§7.5).
pub const TOKEN_TIMEOUT: Duration = Duration::from_secs(10);

/// Appendix A.5's refresh request. `scope` is the stored scopes joined by a space; with none
/// stored, none are invented.
pub fn refresh_request(
    e: &Endpoints,
    refresh_token: &str,
    scopes: &[String],
    timeout: Duration,
) -> HttpRequest {
    let mut body = json!({
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
        "client_id": CLIENT_ID,
    });
    if !scopes.is_empty() {
        body["scope"] = json!(scopes.join(" "));
    }
    HttpRequest::post_json(e.token.clone(), &body, timeout)
}

/// Who a token reply says owns the new token (Appendix A.5): its `account.uuid`,
/// `account.email_address` and `organization.uuid`. Unlike the profile oracle (§7.6), either
/// the account uuid or the organization alone is enough: §7.4 quarantines on an organization
/// that disagrees even when the reply names no account. `None` only when it names neither.
pub(crate) fn token_owner(body: &Value) -> Option<Identity> {
    let text = |v: &Value| v.as_str().filter(|s| !s.is_empty()).map(str::to_owned);
    let uuid = text(&body["account"]["uuid"]);
    let org_uuid = text(&body["organization"]["uuid"]);
    if uuid.is_none() && org_uuid.is_none() {
        return None;
    }
    let email = text(&body["account"]["email_address"]);
    let org_name = text(&body["organization"]["name"]);
    let org_uuid = org_uuid.unwrap_or_default();
    Some(Identity {
        label: email
            .clone()
            .or_else(|| uuid.clone())
            .unwrap_or_else(|| org_uuid.clone()),
        raw: json!({
            "emailAddress": email,
            "accountUuid": uuid,
            "organizationUuid": org_uuid,
            "organizationName": org_name,
        }),
        email,
        org_uuid,
        org_name,
        account_uuid: uuid,
    })
}

/// Whole seconds, whether the reply wrote them as an integer or a float.
fn seconds(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))
}

/// The server's own words for a refusal: `error_description` (RFC 6749) or the nested
/// `error.message`, else `fallback` (the error code), so the text is never empty.
fn refusal_message(body: Option<&Value>, fallback: &str) -> String {
    let text = |v: &Value| v.as_str().filter(|s| !s.is_empty()).map(str::to_owned);
    body.and_then(|b| text(&b["error_description"]).or_else(|| text(&b["error"]["message"])))
        .unwrap_or_else(|| fallback.to_owned())
}

/// §7.3 step 7, the provider's half, and Appendix A.5's reply handling.
///
/// - `invalid_grant` from 400, 401 or 403 is `Dead`; the engine re-reads the source that was
///   sent before it quarantines anything.
/// - A refusal of the request itself is `Systemic`, never a strike, quoting the server's
///   message: a top-level `error == "invalid_client"` whatever the status (RFC 6749), or a 400
///   whose nested `error.type` is `invalid_request_error` (what the endpoint answers for an
///   unknown client id, Appendix A.5). The message is `error_description` or the nested
///   `error.message`, else the error code.
/// - A 200 that names an access or a refresh token is `Refreshed`. Nothing received is ever
///   discarded: without an access token the stored one is kept, and without `expires_in` the
///   successor is stamped as expiring now, so the next use refreshes again.
/// - Anything else is `Transient`.
pub fn parse_refresh(
    old: &[u8],
    reply: Result<HttpResponse, HttpError>,
    now_ms: i64,
) -> RefreshResult {
    let resp = match reply {
        Ok(r) => r,
        Err(HttpError::PreSend(_)) => return RefreshResult::Transient(TransientKind::PreSend),
        Err(HttpError::Ambiguous(_)) => return RefreshResult::Transient(TransientKind::Ambiguous),
    };
    let body = resp.json();
    let error = body.as_ref().map(|b| &b["error"]);
    let code = error.and_then(Value::as_str);
    let nested_type = error.and_then(|e| e["type"].as_str());
    match (resp.status, code, nested_type) {
        (400 | 401 | 403, Some("invalid_grant"), _) => {
            return RefreshResult::Dead(DeadReason::InvalidGrant);
        }
        (_, Some("invalid_client"), _) => {
            return RefreshResult::Systemic(refusal_message(body.as_ref(), "invalid_client"));
        }
        (400, None, Some("invalid_request_error")) => {
            return RefreshResult::Systemic(refusal_message(
                body.as_ref(),
                "invalid_request_error",
            ));
        }
        (200, ..) => {}
        (status, ..) => return RefreshResult::Transient(TransientKind::Http(status)),
    }
    let Some(body) = body else {
        return RefreshResult::Transient(TransientKind::BadResponse);
    };
    let non_empty = |k: &str| {
        body[k]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let access = non_empty("access_token");
    let refresh = non_empty("refresh_token");
    if access.is_none() && refresh.is_none() {
        return RefreshResult::Transient(TransientKind::BadResponse);
    }
    let expires_at = match (&access, seconds(&body["expires_in"])) {
        (Some(_), Some(s)) => now_ms.saturating_add(s.saturating_mul(1000)),
        _ => now_ms,
    };
    let fields = TokenFields {
        access_token: access
            .or_else(|| shape::access_token(old))
            .unwrap_or_default(),
        refresh_token: refresh,
        expires_at,
        scopes: body["scope"].as_str().map(|s| {
            s.split(' ')
                .filter(|x| !x.is_empty())
                .map(str::to_owned)
                .collect()
        }),
        refresh_token_expires_at: seconds(&body["refresh_token_expires_in"])
            .map(|s| now_ms.saturating_add(s.saturating_mul(1000))),
    };
    // `refresh` only sends a credential that parsed as an object, so the first form always
    // applies. The second keeps a received successor even if that ever changes.
    let successor = shape::apply_refresh(old, &fields)
        .or_else(|_| shape::apply_refresh(b"{}", &fields))
        .expect("an empty object always accepts the token fields");
    let owner = token_owner(&body);
    RefreshResult::Refreshed { successor, owner }
}
```

- [ ] **Step 7: Implement `refresh` for Claude Code**

In `crates/tagteam-cc/src/provider.rs`, add `use std::time::Duration;` (if not already there),
and add `FreshCredential` and `RefreshResult`, `DeadReason` to the imports
(`use tagteam_provider::provider::{DeadReason, RefreshResult};`,
`use tagteam_provider::FreshCredential;`). Then add to `impl Provider for ClaudeCode`:

```rust
    fn refresh(
        &self,
        http: &dyn Http,
        cred: &FreshCredential,
        now_ms: i64,
        timeout: Duration,
    ) -> RefreshResult {
        let old = cred.credential().bytes();
        let Some(rt) = shape::refresh_token(old) else {
            return RefreshResult::Dead(DeadReason::NoRefreshToken);
        };
        let req = oauth::refresh_request(&self.endpoints, &rt, &shape::scopes(old), timeout);
        oauth::parse_refresh(old, http.send(&req), now_ms)
    }
```

- [ ] **Step 8: Write the failing `FakeAgent` tests, then implement its `refresh`**

Append to `crates/tagteam-fake/tests/network.rs` (add
`use std::time::Duration;`, `use serde_json::Value;` and
`use tagteam_provider::provider::{DeadReason, RefreshResult};` to its imports):

```rust
fn renewable() -> tagteam_provider::FreshCredential {
    token(Some(NOW - 1)).into_fresh().unwrap()
}

#[test]
fn refresh_posts_the_renew_token_and_keeps_the_machine_shared_key() {
    let fa = FakeAgent::new();
    let http = ScriptedHttp::new();
    http.push_json(
        Method::Post,
        &fa.renew_url(),
        200,
        json!({"token": "fa-tok2", "renew": "fa-renew2", "expires_in": 60, "owner": {"uid": "fa-u1", "workspace": "zion"}}),
    );
    let r = fa.refresh(&http, &renewable(), NOW, Duration::from_secs(10));
    let RefreshResult::Refreshed { successor, owner } = r else {
        panic!("expected Refreshed, got {r:?}");
    };
    let s: Value = serde_json::from_slice(&successor).unwrap();
    assert_eq!(s["fa"]["token"], "fa-tok2");
    assert_eq!(s["fa"]["renew"], "fa-renew2");
    assert_eq!(s["fa"]["expires"], json!(NOW + 60_000));
    assert_eq!(s["device"], json!({"id": "machine-shared"}));
    let owner = owner.unwrap();
    assert_eq!(
        (owner.account_uuid.as_deref(), owner.org_uuid.as_str()),
        (Some("fa-u1"), "zion")
    );
    let body: Value =
        serde_json::from_slice(http.requests()[0].body.as_deref().unwrap()).unwrap();
    assert_eq!(body, json!({"renew": "fa-renew"}));
}

#[test]
fn fake_agent_verdicts() {
    let fa = FakeAgent::new();
    let http = ScriptedHttp::new();
    http.push_json(Method::Post, &fa.renew_url(), 401, json!({"error": "invalid_grant"}));
    assert!(matches!(
        fa.refresh(&http, &renewable(), NOW, Duration::from_secs(10)),
        RefreshResult::Dead(DeadReason::InvalidGrant)
    ));
    let static_cred = Credential::fresh(
        credential_json("fa-static-tok", None, None)
            .to_string()
            .into_bytes(),
    )
    .into_fresh()
    .unwrap();
    let before = http.count(Method::Post, &fa.renew_url());
    assert!(matches!(
        fa.refresh(&http, &static_cred, NOW, Duration::from_secs(10)),
        RefreshResult::Dead(DeadReason::NoRefreshToken)
    ));
    assert_eq!(http.count(Method::Post, &fa.renew_url()), before, "nothing sent");
}
```

Run: `cargo test -p tagteam-fake --test network`
Expected: FAIL to compile (`refresh` is not implemented for `FakeAgent`).

In `crates/tagteam-fake/src/provider.rs`, add only these imports (`json`, `Value`, `Duration` and
`HttpRequest` are already imported by Tasks 6 and 7, and importing them again is a compile error):
`use tagteam_provider::http::HttpError;`,
`use tagteam_provider::provider::{DeadReason, RefreshResult, TransientKind};` and
`use tagteam_provider::FreshCredential;`. Then add to `impl Provider for FakeAgent`:

```rust
    fn refresh(
        &self,
        http: &dyn Http,
        cred: &FreshCredential,
        now_ms: i64,
        timeout: Duration,
    ) -> RefreshResult {
        let Ok(Value::Object(mut root)) = serde_json::from_slice::<Value>(cred.credential().bytes())
        else {
            return RefreshResult::Dead(DeadReason::NoRefreshToken);
        };
        let Some(renew) = root
            .get("fa")
            .and_then(|fa| fa["renew"].as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
        else {
            return RefreshResult::Dead(DeadReason::NoRefreshToken);
        };
        let req = HttpRequest::post_json(self.renew_url(), &json!({"renew": renew}), timeout);
        let resp = match http.send(&req) {
            Ok(r) => r,
            Err(HttpError::PreSend(_)) => return RefreshResult::Transient(TransientKind::PreSend),
            Err(HttpError::Ambiguous(_)) => {
                return RefreshResult::Transient(TransientKind::Ambiguous);
            }
        };
        let body = resp.json();
        match (resp.status, body.as_ref().and_then(|b| b["error"].as_str())) {
            (400 | 401, Some("invalid_grant")) => return RefreshResult::Dead(DeadReason::InvalidGrant),
            (_, Some("invalid_client")) => {
                return RefreshResult::Systemic("the renew endpoint rejected the client".into());
            }
            (200, _) => {}
            (status, _) => return RefreshResult::Transient(TransientKind::Http(status)),
        }
        let Some(body) = body else {
            return RefreshResult::Transient(TransientKind::BadResponse);
        };
        // §7.3: a reply that names a new renew token delivered a successor, so it is kept even
        // without a new access token (the stored one stays, expiring now), as Claude Code's
        // parser does. Only a reply naming neither is a bad response.
        let new_token = body["token"].as_str().filter(|s| !s.is_empty());
        let new_renew = body["renew"].as_str().filter(|s| !s.is_empty());
        if new_token.is_none() && new_renew.is_none() {
            return RefreshResult::Transient(TransientKind::BadResponse);
        }
        let expires = match new_token {
            Some(_) => now_ms.saturating_add(
                body["expires_in"].as_i64().unwrap_or(0).saturating_mul(1000),
            ),
            None => now_ms,
        };
        let fa = root.entry("fa").or_insert_with(|| json!({}));
        if let Some(t) = new_token {
            fa["token"] = json!(t);
        }
        if let Some(r) = new_renew {
            fa["renew"] = json!(r);
        }
        fa["expires"] = json!(expires);
        // Like Claude Code's (§7.4): the uid or the workspace alone names an owner.
        let text = |k: &str| body["owner"][k].as_str().filter(|s| !s.is_empty()).map(str::to_owned);
        let (uid, workspace) = (text("uid"), text("workspace"));
        let owner = (uid.is_some() || workspace.is_some()).then(|| Identity {
            label: uid.clone().or_else(|| workspace.clone()).unwrap_or_default(),
            email: None,
            org_uuid: workspace.clone().unwrap_or_default(),
            org_name: None,
            raw: json!({"uid": uid, "workspace": workspace}),
            account_uuid: uid,
        });
        RefreshResult::Refreshed {
            successor: serde_json::to_vec(&Value::Object(root)).expect("a Value always serializes"),
            owner,
        }
    }
```

- [ ] **Step 9: Add the fixture helpers**

Add to `impl Fx` in `crates/tagteam-engine/tests/common/mod.rs`:

```rust
    /// Queues a 200 token reply: a new access token `at-<rt>` (or `at-same`), 8 h of validity,
    /// and the refresh token `new_rt` when `Some` (a reply without one keeps the lineage).
    pub fn script_refresh(&self, new_rt: Option<&str>) {
        let mut body = json!({
            "token_type": "Bearer",
            "access_token": format!("at-{}", new_rt.unwrap_or("same")),
            "expires_in": 28800,
            "scope": "user:inference user:profile",
        });
        if let Some(rt) = new_rt {
            body["refresh_token"] = json!(rt);
        }
        self.http
            .push_json(Method::Post, &Self::endpoints().token, 200, body);
    }

    /// Queues a token error reply, `{"error": error}` with `status`.
    pub fn script_token_error(&self, status: u16, error: &str) {
        self.http.push_json(
            Method::Post,
            &Self::endpoints().token,
            status,
            json!({"error": error}),
        );
    }
```

- [ ] **Step 10: Run everything this task touched**

Run:
```bash
cargo test -p tagteam-cc
cargo test -p tagteam-fake
cargo build --workspace --all-targets --features tagteam/test-support
```
Expected: PASS, and the workspace builds. The fixture helpers have no caller until Task 10.
`tests/common/mod.rs` carries `#![allow(dead_code)]`, so they do not warn.

- [ ] **Step 11: Commit**

```bash
git add crates/tagteam-provider/src/provider.rs crates/tagteam-provider/src/lib.rs \
  crates/tagteam-cc/src/shape.rs crates/tagteam-cc/src/oauth.rs crates/tagteam-cc/src/provider.rs \
  crates/tagteam-cc/tests/oauth.rs \
  crates/tagteam-fake/src/provider.rs crates/tagteam-fake/tests/network.rs \
  crates/tagteam-engine/tests/common/mod.rs
git commit -m "Build token refresh requests and classify their replies in each provider"
```

---

### Task 9: Quarantine, rescue files, and `persist_generation`

The storage the refresh gate needs before it can send anything (§6.2, §6.3, §7.4):
- **Quarantine** with its events.
- **`rescue/` files.** Each holds a received successor whose vault write failed, in a 0600 JSON
  envelope that records which generation it succeeds.
- **`persist_generation`**, the one vault writer for a new generation. It records
  `login_expires_at` and clears a quarantine exactly when the fingerprint changes.
- **`settle_rescues`**, §6.2's "Pending rescues before activation" and §7.3 step 3's adoption.
  The gate (Task 10) and the switch (Task 13) call it.

These are `pub(crate)`, so they are tested inside the crate, over a small in-crate harness.

**Files:**
- Create: `crates/tagteam-engine/src/quarantine.rs`, `crates/tagteam-engine/src/rescue.rs`,
  `crates/tagteam-engine/src/refresh.rs` (it begins with `persist_generation`; Tasks 10 and 11
  add the gate), `crates/tagteam-engine/src/testutil.rs` (test-only)
- Modify: `crates/tagteam-engine/src/lib.rs`, `crates/tagteam-engine/src/store/mod.rs`,
  `crates/tagteam-engine/src/error.rs`, `crates/tagteam-engine/tests/store.rs`,
  `crates/tagteam-engine/tests/engine_basics.rs`, `crates/tagteam-engine/tests/common/mod.rs`

**Interfaces:**
- Consumes:
  - Task 3: `EngineConfig.http` and `tagteam_provider::http::NoHttp`.
  - Task 8: `tagteam_provider::DeadReason`, for `From<DeadReason> for QuarantineReason`.
- Produces (exact contract signatures):
  - Store:
    - `Store::set_quarantine(&self, id: &AccountId, reason: &str, fp: &str, at: i64) -> Result<(), StoreError>`
    - `Store::clear_quarantine(&self, id: &AccountId) -> Result<bool, StoreError>`
    - `Store::set_login_expires_at(&self, id: &AccountId, at: Option<i64>) -> Result<(), StoreError>`
    - `AccountRow.quarantine_at: Option<i64>`
  - `tagteam_engine::quarantine::QuarantineReason`, with `as_str`, `parse`, and
    `From<DeadReason>`.
  - `Engine::quarantine(&self, row: &AccountRow, reason: QuarantineReason, fp: &str) -> Result<(), EngineError>`
    and `Engine::unquarantine(&self, row: &AccountRow) -> Result<bool, EngineError>`, both
    `pub(crate)`.
  - `pub(crate) struct RescueEntry { path, predecessor_fp, credential }`
    and `pub(crate) enum RescueFile { Entry(RescueEntry), Unreadable { path, detail } }`.
  - Rescue methods, all `pub(crate)`:
    - `Engine::write_rescue(&self, id: &AccountId, login_epoch: i64, predecessor_fp: &str, successor: &[u8], successor_fp: &Fingerprint) -> Result<PathBuf, EngineError>`
    - `Engine::rescues_for(&self, id: &AccountId) -> Vec<RescueFile>`
    - `Engine::delete_rescue(&self, path: &Path) -> Result<(), EngineError>`
    - `Engine::settle_rescues(&self, p: &dyn Provider, row: &AccountRow, lock: &AccountLock) -> Result<(), EngineError>`
  - `Engine::persist_generation(&self, p: &dyn Provider, row: &AccountRow, lock: &AccountLock, bytes: &[u8]) -> Result<(), EngineError>`
    (`pub(crate)`).
  - `EngineError::RescuePending { position: u32, label: String, detail: String }`, kind
    `rescue-pending`.
  - `crate::testutil::T`: the in-crate test harness (`#[cfg(test)]` only), which later tasks'
    unit tests may reuse.

- [ ] **Step 1: Write the failing store tests**

In `crates/tagteam-engine/tests/engine_basics.rs`, add `quarantine_at: None,` after
`quarantine_fp: None,` in `row_for`'s `AccountRow` literal.

Append to `crates/tagteam-engine/tests/store.rs`:

```rust
#[test]
fn quarantines_are_set_bound_to_a_fingerprint_and_cleared() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("t.db")).unwrap();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    s.set_quarantine(&a, "invalid_grant", "sha256:sent", 42).unwrap();
    let row = s.account(&a).unwrap().unwrap();
    assert_eq!(
        (
            row.quarantine_reason.as_deref(),
            row.quarantine_fp.as_deref(),
            row.quarantine_at
        ),
        (Some("invalid_grant"), Some("sha256:sent"), Some(42))
    );
    assert!(s.clear_quarantine(&a).unwrap(), "one was set");
    assert!(!s.clear_quarantine(&a).unwrap(), "nothing left to clear");
    let row = s.account(&a).unwrap().unwrap();
    assert_eq!(
        (row.quarantine_reason, row.quarantine_fp, row.quarantine_at),
        (None, None, None)
    );
    assert!(matches!(
        s.set_quarantine(&AccountId::from_string("nobody"), "invalid_grant", "sha256:x", 1),
        Err(StoreError::NoSuchAccount)
    ));
}

#[test]
fn login_expiry_is_recorded_on_its_own() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("t.db")).unwrap();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    s.set_login_expires_at(&a, Some(1_797_000_000_000)).unwrap();
    assert_eq!(
        s.account(&a).unwrap().unwrap().login_expires_at,
        Some(1_797_000_000_000)
    );
    s.set_login_expires_at(&a, None).unwrap();
    assert_eq!(s.account(&a).unwrap().unwrap().login_expires_at, None);
}
```

Run: `cargo test -p tagteam-engine --test store --test engine_basics`
Expected: FAIL to compile (`no method named set_quarantine`, `no field quarantine_at`).

- [ ] **Step 2: Implement the store half**

In `crates/tagteam-engine/src/store/mod.rs`:

1. Add the field to `AccountRow`, after `quarantine_fp`:

```rust
    pub quarantine_at: Option<i64>,
```

2. Add `quarantine_at` to `ACCOUNT_COLUMNS`, which becomes:

```rust
const ACCOUNT_COLUMNS: &str = "id, provider, position, identity_key, label, email, org_uuid, org_name, \
    account_uuid, kind, alias, disabled, identity_json, login_expires_at, login_epoch, replacing_fp, \
    quarantine_reason, quarantine_fp, quarantine_at, added_at";
```

   and read it in `account_from_row`, after `quarantine_fp`:

```rust
        quarantine_at: r.get("quarantine_at")?,
```

3. Replace the doc comment on `APPLY_LOGIN_SQL`:

```rust
/// Installs a login's identity fields and clears any quarantine: shared by `update_login` and
/// the replacement `finish_replacement` records, which land the same fields. Clearing here is
/// §7.4's rule, not an exception to it. Every caller installs a login that replaces the vault's:
/// `add`, `add-token` and `import` clear a quarantine explicitly, and the switch's outgoing
/// capture only ever writes a generation whose fingerprint differs from the vault's (it is not
/// `Ours`, §9.4 step 4).
```

4. Add to `impl Store`, after `backfill_account_uuid`:

```rust
    /// §7.4: one strike, bound to the fingerprint that was sent.
    pub fn set_quarantine(
        &self,
        id: &AccountId,
        reason: &str,
        fp: &str,
        at: i64,
    ) -> Result<(), StoreError> {
        let n = self.exec(
            "UPDATE accounts SET quarantine_reason = ?2, quarantine_fp = ?3, quarantine_at = ?4 \
             WHERE id = ?1",
            &[&id.as_str(), &reason, &fp, &at],
        )?;
        if n == 0 {
            Err(StoreError::NoSuchAccount)
        } else {
            Ok(())
        }
    }

    /// Clears the quarantine; `true` when there was one.
    pub fn clear_quarantine(&self, id: &AccountId) -> Result<bool, StoreError> {
        let n = self.exec(
            "UPDATE accounts SET quarantine_reason = NULL, quarantine_fp = NULL, quarantine_at = NULL \
             WHERE id = ?1 AND quarantine_reason IS NOT NULL",
            &[&id.as_str()],
        )?;
        Ok(n > 0)
    }

    /// The login's own expiry (CC: `refreshTokenExpiresAt`), after a new generation lands.
    pub fn set_login_expires_at(&self, id: &AccountId, at: Option<i64>) -> Result<(), StoreError> {
        self.exec(
            "UPDATE accounts SET login_expires_at = ?2 WHERE id = ?1",
            &[&id.as_str(), &at],
        )?;
        Ok(())
    }
```

In `crates/tagteam-engine/tests/common/mod.rs`, replace `Fx::quarantine` (doc comment and body):

```rust
    /// Quarantines an account row directly, bound to `fp` (§7.4).
    pub fn quarantine(&self, id: &AccountId, reason: &str, fp: &str) {
        self.engine
            .store()
            .unwrap()
            .set_quarantine(id, reason, fp, 1)
            .unwrap();
    }
```

Run: `cargo test -p tagteam-engine --test store --test engine_basics --test add`
Expected: PASS (`add_clears_a_quarantine` now primes through the new writer).

- [ ] **Step 3: Commit the store half**

```bash
git add crates/tagteam-engine/src/store/mod.rs crates/tagteam-engine/tests/store.rs \
  crates/tagteam-engine/tests/engine_basics.rs crates/tagteam-engine/tests/common/mod.rs
git commit -m "Record quarantines and login expiry through the store"
```

- [ ] **Step 4: Add the in-crate test harness**

Create `crates/tagteam-engine/src/testutil.rs`:

```rust
//! A small engine over Claude Code, a fake Keychain and a fixed clock, for unit tests of the
//! crate-private machinery (rescue, quarantine, the gate). Test-only.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tagteam_cc::ClaudeCode;
use tagteam_cc::live::{LiveStore, Platform};
use tagteam_core::{AccountId, CLAUDE_CODE, ProviderId};
use tagteam_provider::http::NoHttp;
use tagteam_provider::{Env, FakeClock, FakeKeychain, Identity, Provider};

use crate::account_lock::AccountLock;
use crate::engine::{Engine, EngineConfig};
use crate::oracle::NoOracle;
use crate::registry::ProviderRegistry;
use crate::store::{AccountRow, NewAccount};
use crate::vault::{KeychainVault, SERVICE, Vault};

pub(crate) const NOW: i64 = 1_790_000_000_000;

/// A stored OAuth credential of generation `rt`, with a login expiry of `rte`.
pub(crate) fn cred(rt: &str, rte: i64) -> Vec<u8> {
    json!({"claudeAiOauth": {"accessToken": format!("at-{rt}"), "refreshToken": rt,
        "expiresAt": NOW + 3_600_000, "refreshTokenExpiresAt": rte}})
    .to_string()
    .into_bytes()
}

pub(crate) struct T {
    pub _dir: tempfile::TempDir,
    pub env: Env,
    pub kc: Arc<FakeKeychain>,
    pub cc: Arc<ClaudeCode>,
    pub engine: Engine,
}

impl T {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path());
        let kc = Arc::new(FakeKeychain::new());
        let cc = Arc::new(ClaudeCode::with_store(
            LiveStore::new(kc.clone(), Platform::MacOs).with_retry_delay(Duration::ZERO),
        ));
        let engine = Engine::new(EngineConfig {
            env: env.clone(),
            registry: ProviderRegistry::new().with(cc.clone()),
            vault: Vault::new(Box::new(KeychainVault::new(kc.clone()))),
            oracle: Arc::new(NoOracle),
            clock: Arc::new(FakeClock::new(NOW)),
            http: Arc::new(NoHttp),
            default_provider: ProviderId::new(CLAUDE_CODE),
        });
        T {
            _dir: dir,
            env,
            kc,
            cc,
            engine,
        }
    }

    /// Stores an OAuth account at the next position with `bytes` in its vault.
    pub fn account(&self, email: &str, bytes: &[u8]) -> AccountRow {
        let store = self.engine.store().unwrap();
        let provider = ProviderId::new(CLAUDE_CODE);
        let id = AccountId::from_string(format!("id-{email}"));
        let identity = Identity {
            label: email.into(),
            email: Some(email.into()),
            org_uuid: String::new(),
            org_name: None,
            account_uuid: Some(format!("uuid-{email}")),
            raw: json!({"emailAddress": email, "accountUuid": format!("uuid-{email}")}),
        };
        let key = self.cc.identity_key(&identity);
        store
            .insert_account(&NewAccount {
                id: &id,
                provider: &provider,
                position: store.next_position(&provider).unwrap(),
                identity_key: key.as_str(),
                identity: &identity,
                kind: "oauth",
                alias: None,
                login_expires_at: None,
                added_at: 1,
            })
            .unwrap();
        self.kc.put(SERVICE, id.as_str(), bytes);
        self.row(&id)
    }

    pub fn row(&self, id: &AccountId) -> AccountRow {
        self.engine.store().unwrap().account(id).unwrap().unwrap()
    }

    pub fn lock(&self, id: &AccountId) -> AccountLock {
        AccountLock::acquire(&self.env, id, AccountLock::WAIT).unwrap()
    }

    /// The refresh token in the vault's current (`prev` false) or previous generation.
    pub fn vault_rt(&self, id: &AccountId, prev: bool) -> Option<String> {
        let key = if prev {
            format!("{id}.prev")
        } else {
            id.to_string()
        };
        let v: Value = serde_json::from_slice(&self.kc.get(SERVICE, &key)?).ok()?;
        v["claudeAiOauth"]["refreshToken"].as_str().map(str::to_owned)
    }

    pub fn fp(&self, bytes: &[u8]) -> String {
        self.cc.fingerprint(bytes).unwrap().as_str().to_owned()
    }
}
```

In `crates/tagteam-engine/src/lib.rs`, add the new modules (keep the list alphabetical):

```rust
pub mod quarantine;
pub mod refresh;
mod rescue;
#[cfg(test)]
mod testutil;
```

- [ ] **Step 5: Write the failing quarantine and persistence tests**

Create `crates/tagteam-engine/src/quarantine.rs` with its tests first, and a stub the tests
compile against:

```rust
//! §7.4: one strike quarantines an account, bound to the fingerprint that was sent.

use serde_json::json;
use tagteam_provider::DeadReason;

use crate::engine::Engine;
use crate::error::EngineError;
use crate::store::{AccountRow, EventRow};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{T, cred};

    #[test]
    fn reasons_round_trip_their_column_values() {
        for r in [
            QuarantineReason::InvalidGrant,
            QuarantineReason::NoRefreshToken,
            QuarantineReason::IdentityConflict,
            QuarantineReason::SuccessorLost,
        ] {
            assert_eq!(QuarantineReason::parse(r.as_str()), Some(r));
        }
        assert_eq!(QuarantineReason::InvalidGrant.as_str(), "invalid_grant");
        assert_eq!(QuarantineReason::NoRefreshToken.as_str(), "no_refresh_token");
        assert_eq!(QuarantineReason::IdentityConflict.as_str(), "identity_conflict");
        assert_eq!(QuarantineReason::SuccessorLost.as_str(), "successor_lost");
        assert_eq!(QuarantineReason::parse("refresh failed"), None);
        assert_eq!(
            QuarantineReason::from(DeadReason::InvalidGrant),
            QuarantineReason::InvalidGrant
        );
        assert_eq!(
            QuarantineReason::from(DeadReason::NoRefreshToken),
            QuarantineReason::NoRefreshToken
        );
    }

    #[test]
    fn quarantine_and_unquarantine_record_events() {
        let t = T::new();
        let row = t.account("a@x.co", &cred("rt-1", 9));
        t.engine
            .quarantine(&row, QuarantineReason::InvalidGrant, "sha256:sent")
            .unwrap();
        let q = t.row(&row.id);
        assert_eq!(
            (q.quarantine_reason.as_deref(), q.quarantine_fp.as_deref()),
            (Some("invalid_grant"), Some("sha256:sent"))
        );
        assert!(t.engine.unquarantine(&q).unwrap());
        assert!(!t.engine.unquarantine(&q).unwrap(), "nothing left to clear");
        let events = t.engine.store().unwrap().events().unwrap();
        let kinds: Vec<&str> = events.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(kinds, ["quarantine", "unquarantine"]);
        assert_eq!(events[0].to_id.as_ref(), Some(&row.id));
        assert_eq!(events[0].detail, Some(json!({"reason": "invalid_grant"})));
        assert_eq!(events[0].source, "cli");
    }
}
```

Create `crates/tagteam-engine/src/refresh.rs` with its tests:

```rust
//! §7.3, the refresh gate, and the one writer of a new generation it shares with rescue
//! adoption and the switch (§6.2, §7.4).

use tagteam_provider::{Provider, Read};

use crate::account_lock::AccountLock;
use crate::engine::Engine;
use crate::error::EngineError;
use crate::store::AccountRow;

#[cfg(test)]
mod tests {
    use crate::testutil::{T, cred};

    fn same_generation_new_access(rt: &str) -> Vec<u8> {
        serde_json::json!({"claudeAiOauth": {"accessToken": "at-other", "refreshToken": rt,
            "expiresAt": 1, "refreshTokenExpiresAt": 7}})
        .to_string()
        .into_bytes()
    }

    #[test]
    fn a_new_generation_rotates_prev_records_expiry_and_clears_the_quarantine() {
        let t = T::new();
        let row = t.account("a@x.co", &cred("rt-1", 5));
        t.engine
            .store()
            .unwrap()
            .set_quarantine(&row.id, "invalid_grant", &t.fp(&cred("rt-1", 5)), 1)
            .unwrap();
        let lock = t.lock(&row.id);
        t.engine
            .persist_generation(t.cc.as_ref(), &t.row(&row.id), &lock, &cred("rt-2", 99))
            .unwrap();
        assert_eq!(t.vault_rt(&row.id, false).as_deref(), Some("rt-2"));
        assert_eq!(t.vault_rt(&row.id, true).as_deref(), Some("rt-1"));
        let after = t.row(&row.id);
        assert_eq!(after.login_expires_at, Some(99));
        assert_eq!(after.quarantine_reason, None, "§7.4: a fingerprint change clears it");
        let kinds: Vec<String> = t
            .engine
            .store()
            .unwrap()
            .events()
            .unwrap()
            .into_iter()
            .map(|e| e.kind)
            .collect();
        assert_eq!(kinds, ["unquarantine"]);
    }

    #[test]
    fn the_same_generation_keeps_prev_and_the_quarantine() {
        // Review Focus 3's persistence half: a reply without a refresh token keeps the lineage,
        // so `.prev` does not rotate and the strike, bound to that lineage, stands.
        let t = T::new();
        let row = t.account("a@x.co", &cred("rt-1", 5));
        let fp = t.fp(&cred("rt-1", 5));
        t.engine
            .store()
            .unwrap()
            .set_quarantine(&row.id, "invalid_grant", &fp, 1)
            .unwrap();
        let lock = t.lock(&row.id);
        let next = same_generation_new_access("rt-1");
        t.engine
            .persist_generation(t.cc.as_ref(), &t.row(&row.id), &lock, &next)
            .unwrap();
        assert_eq!(t.kc.get(crate::vault::SERVICE, row.id.as_str()), Some(next));
        assert_eq!(t.vault_rt(&row.id, true), None, "no `.prev`");
        let after = t.row(&row.id);
        assert_eq!(after.quarantine_fp.as_deref(), Some(fp.as_str()));
        assert_eq!(after.login_expires_at, Some(7));
    }

    #[test]
    fn a_failed_vault_write_changes_nothing_in_the_store() {
        let t = T::new();
        let row = t.account("a@x.co", &cred("rt-1", 5));
        t.kc.set_fail_write(crate::vault::SERVICE, true);
        let lock = t.lock(&row.id);
        assert!(
            t.engine
                .persist_generation(t.cc.as_ref(), &row, &lock, &cred("rt-2", 99))
                .is_err()
        );
        t.kc.set_fail_write(crate::vault::SERVICE, false);
        assert_eq!(t.vault_rt(&row.id, false).as_deref(), Some("rt-1"));
        assert_eq!(t.row(&row.id).login_expires_at, None);
    }
}
```

Run: `cargo test -p tagteam-engine --lib -- quarantine refresh`
Expected: FAIL to compile (`QuarantineReason`, `quarantine`, `persist_generation` are not
defined).

- [ ] **Step 6: Implement quarantine and `persist_generation`**

Add to `crates/tagteam-engine/src/quarantine.rs`, above the tests:

```rust
/// The `quarantine_reason` column's values (§6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuarantineReason {
    InvalidGrant,
    NoRefreshToken,
    IdentityConflict,
    /// A refresh received a successor and could store it nowhere (§7.3 `Unpersisted`): the
    /// vault's generation is consumed (§7.4).
    SuccessorLost,
}

impl QuarantineReason {
    pub fn as_str(self) -> &'static str {
        match self {
            QuarantineReason::InvalidGrant => "invalid_grant",
            QuarantineReason::NoRefreshToken => "no_refresh_token",
            QuarantineReason::IdentityConflict => "identity_conflict",
            QuarantineReason::SuccessorLost => "successor_lost",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        [
            QuarantineReason::InvalidGrant,
            QuarantineReason::NoRefreshToken,
            QuarantineReason::IdentityConflict,
            QuarantineReason::SuccessorLost,
        ]
        .into_iter()
        .find(|r| r.as_str() == s)
    }
}

impl From<DeadReason> for QuarantineReason {
    fn from(r: DeadReason) -> Self {
        match r {
            DeadReason::InvalidGrant => QuarantineReason::InvalidGrant,
            DeadReason::NoRefreshToken => QuarantineReason::NoRefreshToken,
        }
    }
}

impl Engine {
    fn quarantine_event(&self, row: &AccountRow, kind: &str, reason: Option<&str>) -> Result<(), EngineError> {
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

    /// Sets the quarantine bound to `fp`, the fingerprint that was actually sent, and records a
    /// `quarantine` event (§7.4). The log names the position and ID only (§4.4).
    pub(crate) fn quarantine(
        &self,
        row: &AccountRow,
        reason: QuarantineReason,
        fp: &str,
    ) -> Result<(), EngineError> {
        self.store()?
            .set_quarantine(&row.id, reason.as_str(), fp, self.now_ms())?;
        self.quarantine_event(row, "quarantine", Some(reason.as_str()))?;
        tracing::warn!(
            position = row.position,
            account = %row.id,
            reason = reason.as_str(),
            "quarantined: the account needs a new login"
        );
        Ok(())
    }

    /// Clears the quarantine and records `unquarantine`; `false` when there was none.
    pub(crate) fn unquarantine(&self, row: &AccountRow) -> Result<bool, EngineError> {
        let cleared = self.store()?.clear_quarantine(&row.id)?;
        if cleared {
            self.quarantine_event(row, "unquarantine", None)?;
        }
        Ok(cleared)
    }
}
```

Add to `crates/tagteam-engine/src/refresh.rs`, above the tests:

```rust
impl Engine {
    /// Writes a new generation of `row`'s login under `lock` (§6.2): `.prev` rotates only when
    /// the lineage fingerprint changes, and the write is verified. Then `login_expires_at` is
    /// recorded, and a quarantine is cleared when the fingerprint changed (§7.4). Every writer
    /// of a received or adopted generation goes through here.
    pub(crate) fn persist_generation(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        lock: &AccountLock,
        bytes: &[u8],
    ) -> Result<(), EngineError> {
        debug_assert_eq!(lock.id(), &row.id, "the caller holds this account's lock");
        let before = match self.vault.read(&row.id) {
            Read::Present(b) => p.fingerprint(&b),
            Read::Absent => None,
            Read::Unreadable(e) => return Err(EngineError::Unreadable(e)),
        };
        self.vault.store(lock, bytes, &|b| p.fingerprint(b))?;
        self.store()?
            .set_login_expires_at(&row.id, p.login_expires_at(bytes))?;
        if p.fingerprint(bytes) != before {
            self.unquarantine(row)?;
        }
        Ok(())
    }
}
```

Run: `cargo test -p tagteam-engine --lib -- quarantine refresh`
Expected: PASS (5 tests).

- [ ] **Step 7: Write the failing rescue tests**

Create `crates/tagteam-engine/src/rescue.rs` with the imports, the error the tests expect, and
the tests:

```rust
//! §6.3 `rescue/`: a received successor whose vault write failed, in a 0600 envelope that
//! records which generation it succeeds; and §6.2's rule that pending rescues settle before any
//! activation.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use tagteam_core::{AccountId, Fingerprint};
use tagteam_provider::atomic::{ensure_private_dir, write_atomic_private};
use tagteam_provider::{Provider, Read};

use crate::account_lock::AccountLock;
use crate::engine::Engine;
use crate::error::EngineError;
use crate::store::AccountRow;

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::testutil::{T, cred};
    use crate::vault::SERVICE;

    fn mode(p: &Path) -> u32 {
        fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    fn entries(t: &T, id: &AccountId) -> Vec<RescueEntry> {
        t.engine
            .rescues_for(id)
            .into_iter()
            .map(|r| match r {
                RescueFile::Entry(e) => e,
                RescueFile::Unreadable { path, detail } => {
                    panic!("{} unreadable: {detail}", path.display())
                }
            })
            .collect()
    }

    #[test]
    fn an_envelope_round_trips_in_a_private_file() {
        let t = T::new();
        let id = AccountId::from_string("0192-acct");
        let succ = cred("rt-2", 9);
        let succ_fp = t.cc.fingerprint(&succ).unwrap();
        // A real fingerprint: the parser accepts only the canonical `sha256:<64 hex>` form.
        let pred = t.fp(&cred("rt-1", 9));
        let path = t
            .engine
            .write_rescue(&id, 3, &pred, &succ, &succ_fp)
            .unwrap();
        assert_eq!(
            path,
            t.env
                .data_dir()
                .join("rescue")
                .join(format!("0192-acct-3-{}.json", succ_fp.short12()))
        );
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(path.parent().unwrap()), 0o700);
        let v: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            v,
            json!({"format": "tagteam-rescue", "version": 1, "accountId": "0192-acct",
                "loginEpoch": 3, "predecessorFp": pred,
                "credential": String::from_utf8(succ.clone()).unwrap()})
        );
        let got = entries(&t, &id);
        assert_eq!(got.len(), 1);
        // The envelope's accountId and loginEpoch (asserted above) are validated on read, not kept.
        assert_eq!(
            (&got[0].path, got[0].predecessor_fp.as_str(), &got[0].credential),
            (&path, pred.as_str(), &succ)
        );
        t.engine.delete_rescue(&path).unwrap();
        assert!(entries(&t, &id).is_empty());
        t.engine.delete_rescue(&path).unwrap(); // already gone: fine
    }

    #[test]
    fn looking_never_creates_the_directory() {
        let t = T::new();
        assert!(t.engine.rescues_for(&AccountId::from_string("x")).is_empty());
        assert!(!t.env.data_dir().join("rescue").exists());
    }

    #[test]
    fn only_this_accounts_files_are_listed_and_damaged_ones_are_unreadable() {
        let t = T::new();
        let id = AccountId::from_string("acct");
        let other = AccountId::from_string("other");
        let succ = cred("rt-2", 9);
        let fp = t.cc.fingerprint(&succ).unwrap();
        let pred = t.fp(&cred("rt-1", 9));
        t.engine.write_rescue(&other, 0, &pred, &succ, &fp).unwrap();
        let dir = t.env.data_dir().join("rescue");
        fs::write(dir.join("acct-0-truncated.json"), b"{\"format\":\"tagteam-res").unwrap();
        fs::write(
            dir.join("acct-0-wrongfmt.json"),
            json!({"format": "something-else", "version": 1}).to_string(),
        )
        .unwrap();
        fs::write(
            dir.join("acct-0-mislabelled.json"),
            json!({"format": "tagteam-rescue", "version": 1, "accountId": "other",
                "loginEpoch": 0, "predecessorFp": fp.as_str(), "credential": "x"})
            .to_string(),
        )
        .unwrap();
        let found = t.engine.rescues_for(&id);
        assert_eq!(found.len(), 3);
        for r in &found {
            assert!(matches!(r, RescueFile::Unreadable { .. }));
        }
        assert_eq!(entries(&t, &other).len(), 1);
    }

    #[test]
    fn a_rescue_succeeding_the_vault_generation_is_adopted() {
        let t = T::new();
        let row = t.account("a@x.co", &cred("rt-1", 5));
        t.engine
            .store()
            .unwrap()
            .set_quarantine(&row.id, "invalid_grant", "sha256:old", 1)
            .unwrap();
        let succ = cred("rt-2", 77);
        let pred = t.fp(&cred("rt-1", 5));
        let path = t
            .engine
            .write_rescue(&row.id, 0, &pred, &succ, &t.cc.fingerprint(&succ).unwrap())
            .unwrap();
        let lock = t.lock(&row.id);
        t.engine
            .settle_rescues(t.cc.as_ref(), &t.row(&row.id), &lock)
            .unwrap();
        assert_eq!(t.vault_rt(&row.id, false).as_deref(), Some("rt-2"));
        assert_eq!(t.vault_rt(&row.id, true).as_deref(), Some("rt-1"));
        assert!(!path.exists(), "deleted after the verified vault write");
        let after = t.row(&row.id);
        assert_eq!(after.quarantine_reason, None);
        assert_eq!(after.login_expires_at, Some(77));
    }

    #[test]
    fn a_superseded_rescue_is_left_alone() {
        let t = T::new();
        let row = t.account("a@x.co", &cred("rt-3", 5));
        let succ = cred("rt-2", 9);
        let path = t
            .engine
            .write_rescue(&row.id, 0, &t.fp(&cred("rt-1", 5)), &succ, &t.cc.fingerprint(&succ).unwrap())
            .unwrap();
        let lock = t.lock(&row.id);
        t.engine
            .settle_rescues(t.cc.as_ref(), &row, &lock)
            .unwrap();
        assert_eq!(t.vault_rt(&row.id, false).as_deref(), Some("rt-3"));
        assert!(path.exists());
    }

    #[test]
    fn an_unreadable_rescue_blocks_until_it_is_settled() {
        // §6.2: it may hold the successor of the vault's generation.
        let t = T::new();
        let row = t.account("a@x.co", &cred("rt-1", 5));
        let dir = t.env.data_dir().join("rescue");
        fs::create_dir_all(&dir).unwrap();
        let damaged = dir.join(format!("{}-0-abcdef012345.json", row.id));
        fs::write(&damaged, b"not json").unwrap();
        let lock = t.lock(&row.id);
        let err = t
            .engine
            .settle_rescues(t.cc.as_ref(), &row, &lock)
            .unwrap_err();
        match &err {
            EngineError::RescuePending { position, detail, .. } => {
                assert_eq!(*position, row.position);
                assert!(detail.contains(&damaged.display().to_string()), "{detail}");
            }
            other => panic!("expected RescuePending, got {other:?}"),
        }
        assert_eq!(err.kind(), "rescue-pending");
        assert_eq!(t.vault_rt(&row.id, false).as_deref(), Some("rt-1"));
    }

    #[test]
    fn a_failed_adoption_keeps_the_rescue_and_refuses() {
        let t = T::new();
        let row = t.account("a@x.co", &cred("rt-1", 5));
        let succ = cred("rt-2", 9);
        let path = t
            .engine
            .write_rescue(&row.id, 0, &t.fp(&cred("rt-1", 5)), &succ, &t.cc.fingerprint(&succ).unwrap())
            .unwrap();
        t.kc.set_fail_write(SERVICE, true);
        let lock = t.lock(&row.id);
        assert!(matches!(
            t.engine.settle_rescues(t.cc.as_ref(), &row, &lock),
            Err(EngineError::RescuePending { .. })
        ));
        t.kc.set_fail_write(SERVICE, false);
        assert!(path.exists(), "a rescue is deleted only after a verified vault write");
        assert_eq!(t.vault_rt(&row.id, false).as_deref(), Some("rt-1"));
    }
}
```

Add the variant the tests expect to `EngineError` in `crates/tagteam-engine/src/error.rs`, after
`RecoveryBlocked`:

```rust
    /// §6.2: a refreshed successor sits in `rescue/` and could not be adopted, or a rescue file
    /// for the account cannot be read. Activating or refreshing the vault's generation would
    /// use a token the server has already consumed.
    #[error(
        "{label} (position {position}) has a refreshed token that is not in the vault yet: {detail}; retry once the vault can be written"
    )]
    RescuePending {
        position: u32,
        label: String,
        detail: String,
    },
```

map it in `kind()`:

```rust
            EngineError::RescuePending { .. } => "rescue-pending",
```

and pin it in `kind_is_pinned_for_every_variant`'s `cases`, before the `RolledBack` row:

```rust
            (
                EngineError::RescuePending {
                    position: 1,
                    label: "a".into(),
                    detail: "d".into(),
                },
                "rescue-pending",
            ),
```

Run: `cargo test -p tagteam-engine --lib rescue`
Expected: FAIL to compile (`write_rescue`, `rescues_for`, `RescueFile` and the other rescue
items are not defined).

- [ ] **Step 8: Implement `rescue.rs`**

Add to `crates/tagteam-engine/src/rescue.rs`, above the tests:

```rust
const FORMAT: &str = "tagteam-rescue";
const VERSION: i64 = 1;

/// One readable rescue envelope (§6.3). It holds a secret, so it has no `Debug`.
pub(crate) struct RescueEntry {
    pub path: PathBuf,
    /// The generation that was sent: this rescue succeeds it.
    pub predecessor_fp: String,
    pub credential: Vec<u8>,
}

pub(crate) enum RescueFile {
    Entry(RescueEntry),
    /// Could not be read or parsed. `detail` never quotes the file's bytes.
    Unreadable { path: PathBuf, detail: String },
}

/// Why `bytes` is not account `id`'s rescue envelope. Never quotes the bytes.
fn parse(path: &Path, bytes: &[u8], id: &AccountId) -> Result<RescueEntry, String> {
    let v: Value = serde_json::from_slice(bytes).map_err(|_| "it is not JSON".to_owned())?;
    if v["format"].as_str() != Some(FORMAT) || v["version"].as_i64() != Some(VERSION) {
        return Err("it is not a version 1 tagteam rescue envelope".into());
    }
    if v["accountId"].as_str() != Some(id.as_str()) {
        return Err("it names a different account".into());
    }
    if v["loginEpoch"].as_i64().is_none() {
        return Err("it has no loginEpoch".into());
    }
    let predecessor_fp = v["predecessorFp"]
        .as_str()
        .filter(|s| Fingerprint::parse(s).is_some())
        .ok_or_else(|| "its predecessorFp is not a fingerprint".to_owned())?
        .to_owned();
    let credential = v["credential"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "it holds no credential".to_owned())?
        .as_bytes()
        .to_vec();
    Ok(RescueEntry {
        path: path.to_path_buf(),
        predecessor_fp,
        credential,
    })
}

impl Engine {
    fn rescue_dir(&self) -> PathBuf {
        self.env.data_dir().join("rescue")
    }

    /// Writes a received successor to `rescue/<id>-<epoch>-<fp12>.json` (§5): a plain 0600 file,
    /// never a Keychain item, created beside its final name and renamed into place (§6.3). The
    /// directory is created 0700 on first use. The credential is stored verbatim as a UTF-8
    /// string, which every provider's credential is.
    pub(crate) fn write_rescue(
        &self,
        id: &AccountId,
        login_epoch: i64,
        predecessor_fp: &str,
        successor: &[u8],
        successor_fp: &Fingerprint,
    ) -> Result<PathBuf, EngineError> {
        let credential = std::str::from_utf8(successor).map_err(|_| {
            EngineError::Io(io::Error::other("the refreshed credential is not UTF-8"))
        })?;
        let envelope = json!({
            "format": FORMAT,
            "version": VERSION,
            "accountId": id.as_str(),
            "loginEpoch": login_epoch,
            "predecessorFp": predecessor_fp,
            "credential": credential,
        });
        let dir = self.rescue_dir();
        ensure_private_dir(&dir)?;
        let path = dir.join(format!("{id}-{login_epoch}-{}.json", successor_fp.short12()));
        write_atomic_private(
            &path,
            &serde_json::to_vec(&envelope).expect("a Value always serializes"),
            0o600,
        )?;
        Ok(path)
    }

    /// Every rescue file for `id` (named `<id>-…json`), in name order. Never creates
    /// `rescue/`. A directory that cannot be listed is itself unreadable: it may hide one.
    pub(crate) fn rescues_for(&self, id: &AccountId) -> Vec<RescueFile> {
        let dir = self.rescue_dir();
        let listing = match fs::read_dir(&dir) {
            Ok(l) => l,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return vec![],
            Err(e) => {
                return vec![RescueFile::Unreadable {
                    path: dir,
                    detail: e.to_string(),
                }];
            }
        };
        let prefix = format!("{id}-");
        let mut paths: Vec<PathBuf> = listing
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(&prefix) && n.ends_with(".json"))
            })
            .collect();
        paths.sort();
        paths
            .into_iter()
            .map(|path| match fs::read(&path) {
                Err(e) => RescueFile::Unreadable {
                    path,
                    detail: e.to_string(),
                },
                Ok(bytes) => match parse(&path, &bytes, id) {
                    Ok(entry) => RescueFile::Entry(entry),
                    Err(detail) => RescueFile::Unreadable { path, detail },
                },
            })
            .collect()
    }

    /// Absent is success.
    pub(crate) fn delete_rescue(&self, path: &Path) -> Result<(), EngineError> {
        match fs::remove_file(path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    /// §6.2 "Pending rescues before activation", and §7.3 step 3's adoption. A rescue whose
    /// predecessor is the vault's current generation holds the only live successor: it is
    /// written to the vault (verified), and then its file is deleted. A rescue that cannot be
    /// read might be that one, so it refuses, as does a failed adoption. A rescue whose
    /// predecessor is any other generation is superseded and left alone. The caller holds
    /// `lock`.
    pub(crate) fn settle_rescues(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        lock: &AccountLock,
    ) -> Result<(), EngineError> {
        let rescues = self.rescues_for(&row.id);
        if rescues.is_empty() {
            return Ok(());
        }
        let pending = |detail: String| EngineError::RescuePending {
            position: row.position,
            label: row.label.clone(),
            detail,
        };
        if let Some(e) = rescues.iter().find_map(|r| match r {
            RescueFile::Unreadable { path, detail } => {
                Some(format!("{} is unreadable: {detail}", path.display()))
            }
            RescueFile::Entry(_) => None,
        }) {
            return Err(pending(e));
        }
        let current = match self.vault.read(&row.id) {
            Read::Present(b) => p.fingerprint(&b).map(|f| f.as_str().to_owned()),
            Read::Absent => None,
            Read::Unreadable(e) => return Err(EngineError::Unreadable(e)),
        };
        for r in rescues {
            let RescueFile::Entry(e) = r else { continue };
            if current.as_deref() != Some(e.predecessor_fp.as_str()) {
                continue;
            }
            self.persist_generation(p, row, lock, &e.credential)
                .map_err(|err| {
                    pending(format!("{} could not be adopted: {err}", e.path.display()))
                })?;
            // Once adopted it is superseded, so a failed delete is harmless: the next pass
            // sees its predecessor is no longer current and leaves it alone.
            if let Err(err) = self.delete_rescue(&e.path) {
                tracing::warn!(
                    position = row.position,
                    account = %row.id,
                    "an adopted rescue file could not be deleted: {err}"
                );
            }
            return Ok(());
        }
        Ok(())
    }
}
```

- [ ] **Step 9: Run the task's tests to verify they pass**

Run:
```bash
cargo test -p tagteam-engine --lib
cargo test -p tagteam-engine --test store --test engine_basics --test add
cargo clippy -p tagteam-engine --all-targets -- -D warnings
```
Expected: PASS, no warnings. Nothing outside the tests calls these until Task 10 (or Task 11 for
`write_rescue`): `persist_generation`, `quarantine`, `unquarantine`, `quarantine_event`,
`rescues_for`, `delete_rescue`, `settle_rescues` and `write_rescue`. If rustc or clippy reports
`dead_code` for them, put `#[cfg_attr(not(test), allow(dead_code))]` on each of those items (an
allowed item counts as used, so what it calls is live too, `RescueEntry`'s fields included), and
remove the attributes in the task that first calls each: Task 10 for all of them but
`write_rescue`, Task 11 for `write_rescue`. `RescueEntry` deliberately keeps no `account_id` or
`login_epoch` field: the parser validates both, and nothing would read them.

- [ ] **Step 10: Commit**

```bash
git add crates/tagteam-engine/src/lib.rs crates/tagteam-engine/src/testutil.rs \
  crates/tagteam-engine/src/quarantine.rs crates/tagteam-engine/src/refresh.rs \
  crates/tagteam-engine/src/rescue.rs crates/tagteam-engine/src/error.rs
git commit -m "Add quarantine, rescue files and the single writer of a new generation"
```

---

### Task 10: The refresh gate: lock, ownership, adoption, request, verdict

**Files:**
- Modify: `crates/tagteam-engine/src/refresh.rs` (Task 9 created it with `persist_generation`)
- Modify: `crates/tagteam-engine/src/lib.rs` (confirm `pub mod refresh;`, `pub mod quarantine;`)
- Modify: `crates/tagteam-engine/src/rescue.rs`, `crates/tagteam-engine/src/quarantine.rs` (drop
  Task 9's `dead_code` allowances on what the gate now calls)
- Modify: `crates/tagteam-engine/tests/common/mod.rs` (`Fx::put_vault`, `Fx::expire_access`, and
  the shared free helpers `token_requests`, `due`, `quarantine_of`)
- Test: `crates/tagteam-engine/tests/gate.rs`

**Interfaces:**
- Consumes:
  - `Engine.http` and `Fx.http: Arc<ScriptedHttp>` (Task 3)
  - `Provider::kind_traits`, `Provider::access_expires_at` and `Provider::access_fingerprint`
    (Task 4)
  - `Fx::endpoints()` (Task 7)
  - `Provider::refresh(http, &FreshCredential, now_ms, timeout) -> RefreshResult`,
    `DeadReason`, `TransientKind::token()`, `Fx::script_refresh(Option<&str>)` and
    `Fx::script_token_error(u16, &str)` (Task 8)
  - `Engine::persist_generation`, `Engine::quarantine`, `QuarantineReason` (with `parse`),
    `Engine::settle_rescues`, `EngineError::RescuePending`, the `rescue/<id>-…` file naming and
    `Fx::quarantine` (Task 9)
  - `AccountLock::try_acquire`, `Engine::reconcile_replacement`, `displace::displace` (M1)
- Produces:
  - `pub mod refresh` in `tagteam_engine`, with:
    - `pub enum GateOutcome` and `pub enum OwnedBy`, exactly as the Interface Contract lists
      them
    - `pub const GATE_TIMEOUT: Duration` (10 s)
    - `pub(crate) const EXPIRY_BUFFER_MS: i64` and
      `pub(crate) fn expired(p: &dyn Provider, bytes: &[u8], now_ms: i64) -> bool` (§7.2's
      test; Task 16 reuses them)
    - `pub fn Engine::refresh_stored(&self, p: &dyn Provider, id: &AccountId, snapshot: &[u8])
      -> Result<GateOutcome, EngineError>`
  - `refresh_stored` is `pub`, not `pub(crate)`. The integration tests in `tests/` call it; the
    CLI never does.
  - Hook points `gate-before-request` and `gate-after-response`
  - `pub(crate) fn Engine::keep_foreign(&self, row: &AccountRow, successor: &[u8],
    fp: Option<&Fingerprint>, owner: &Identity) -> Result<(), EngineError>`: a successor that
    belongs to another account, to `displaced/`. Task 11's `Received::keep` calls it; Task 11
    replaces this task's `displace_foreign` with its own foreign path.
  - `Fx::put_vault(&self, id, bytes)` and `Fx::expire_access(&self, id)`
  - Shared test helpers in `tests/common/mod.rs` (free `pub fn`s, reused by Tasks 11, 14 and 16):
    `token_requests(&Fx) -> usize`, `due(&Fx) -> AccountId` (`a` stored and inactive with a due
    access token, `b` live), `quarantine_of(&Fx, &AccountId) -> (Option<String>, Option<String>)`
    (reason, bound fingerprint)
  - Transient kinds this task produces: `vault-absent`, `vault-unreadable`,
    `rescue-unreadable`, `refresh-failed`, `not-refreshable`, plus `TransientKind::token()`'s
    `pre-send` / `ambiguous` / `http-<code>` / `bad-response`
- Decisions this task fixes:
  - **An identity-conflicting successor is displaced, never stored** (§7.3 step 6, §7.4). A
    `Refreshed` response whose owner names another account delivered that account's token, not
    this one's. It is checked before any persistence. The gate writes it to `displaced/` (reason
    `identity-conflict`, naming the owner the response gave), never to this account's vault or
    to `rescue/`, where a later switch could adopt it. The account is quarantined with reason
    `identity_conflict`, bound to the fingerprint that was sent, which the vault still holds.
    The gate returns `Dead(IdentityConflict)`. If the displacement fails, the successor is
    lost: `Unpersisted`, with the quarantine still set.
  - A uuid is compared only when both sides know one; an organization only when both are
    non-empty. Either alone is enough: a response naming only a different organization
    conflicts (Task 8's `token_owner` builds the owner from either).
  - **An unreadable live identity counts as live.** It may be this account's login, and the gate
    never refreshes what might be the live token (§7.3 step 2).
  - **A quarantine reason that does not parse** is reported as `Dead(InvalidGrant)`: the account
    is quarantined either way, and nothing is sent.
  - Until Task 11, a `Refreshed` result persists with `persist_generation` directly. Task 11
    replaces that path with the compare-and-swap and rescue.

- [ ] **Step 1: Add the fixture helpers**

In `crates/tagteam-engine/tests/common/mod.rs`, add to `impl Fx`, after `vault_refresh_token`:

```rust
    /// Replaces `id`'s current vault generation directly, as another tagteam process would.
    pub fn put_vault(&self, id: &AccountId, bytes: &[u8]) {
        match self.platform {
            Platform::MacOs => self.kc.put(SERVICE, id.as_str(), bytes),
            Platform::Linux => {
                let dir = self.env.data_dir().join("vault");
                fs::create_dir_all(&dir).unwrap();
                fs::write(dir.join(format!("{id}.json")), bytes).unwrap();
            }
        }
    }

    /// Moves `id`'s stored access token to expire one minute from the fixture clock's now:
    /// inside the 10-minute freshen window (§7.2), and already "expired" by §7.2's 5-minute
    /// buffer. The refresh token, and so the fingerprint, are unchanged.
    pub fn expire_access(&self, id: &AccountId) {
        let mut v: Value = serde_json::from_slice(&self.vault_bytes(id).unwrap()).unwrap();
        v["claudeAiOauth"]["expiresAt"] = json!(self.clock.now_ms() + 60_000);
        self.put_vault(id, v.to_string().as_bytes());
    }
```

`FakeClock` needs `use tagteam_provider::Clock;` in scope for `now_ms()`; add `Clock` to the
existing `use tagteam_provider::{…}` line of `tests/common/mod.rs` if it is not there yet.

Then add these free functions at the end of `tests/common/mod.rs`. Several test files use them
(this task's `gate.rs`, and Tasks 11, 14 and 16), so they live here, not in each file:

```rust
/// How many token-endpoint requests the fixture's scripted port has seen.
pub fn token_requests(fx: &Fx) -> usize {
    fx.http.count(Method::Post, &Fx::endpoints().token)
}

/// `a` stored and inactive, with an access token that is due; `b` is the live login.
pub fn due(fx: &Fx) -> AccountId {
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    fx.expire_access(&a);
    a
}

/// `id`'s quarantine reason and the fingerprint it is bound to.
pub fn quarantine_of(fx: &Fx, id: &AccountId) -> (Option<String>, Option<String>) {
    let row = fx.engine.store().unwrap().account(id).unwrap().unwrap();
    (row.quarantine_reason, row.quarantine_fp)
}
```

- [ ] **Step 2: Write the failing tests**

`crates/tagteam-engine/tests/gate.rs`:

```rust
mod common;

use std::fs;
use std::time::Duration;

use common::{Fx, crashed_switch, due, quarantine_of, token_requests, vault_fp};
use serde_json::{Value, json};
use tagteam_core::AccountId;
use tagteam_engine::account_lock::AccountLock;
use tagteam_engine::quarantine::QuarantineReason;
use tagteam_engine::refresh::{GateOutcome, OwnedBy};
use tagteam_engine::vault::SERVICE;
use tagteam_provider::http::{HttpError, Method};
use tagteam_provider::{Clock, Keychain};

/// The gate on `id`, with the vault's current bytes as the caller's snapshot.
fn gate(fx: &Fx, id: &AccountId) -> GateOutcome {
    let snapshot = fx.vault_bytes(id).unwrap();
    fx.engine
        .refresh_stored(fx.cc.as_ref(), id, &snapshot)
        .unwrap()
}

#[test]
fn a_due_account_is_refreshed_once_and_its_successor_stored() {
    let fx = Fx::new();
    let a = due(&fx);
    fx.script_refresh(Some("rt-a2"));
    let GateOutcome::Refreshed(bytes) = gate(&fx, &a) else {
        panic!("expected Refreshed")
    };
    assert_eq!(fx.vault_bytes(&a).unwrap(), bytes);
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"));
    let prev: Value =
        serde_json::from_slice(&fx.kc.get(SERVICE, &format!("{a}.prev")).unwrap()).unwrap();
    assert_eq!(prev["claudeAiOauth"]["refreshToken"], "rt-a", "the old generation is .prev");
    assert_eq!(token_requests(&fx), 1);
    let sent: Value = serde_json::from_slice(
        fx.http.requests().last().unwrap().body.as_deref().unwrap(),
    )
    .unwrap();
    assert_eq!(sent["grant_type"], "refresh_token");
    assert_eq!(sent["refresh_token"], "rt-a");
    assert_eq!(quarantine_of(&fx, &a), (None, None));
}

#[test]
fn another_holder_of_the_account_lock_makes_it_busy() {
    let fx = Fx::new();
    let a = due(&fx);
    fx.script_refresh(Some("rt-a2"));
    let _held = AccountLock::acquire(&fx.env, &a, Duration::ZERO).unwrap();
    assert!(matches!(gate(&fx, &a), GateOutcome::Busy));
    assert_eq!(token_requests(&fx), 0);
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
}

#[test]
fn the_live_login_is_owned_and_never_sent() {
    let fx = Fx::new();
    fx.add("b@x.co", "rt-b");
    let a = fx.add("a@x.co", "rt-a"); // live: a
    fx.expire_access(&a);
    fx.script_refresh(Some("rt-a2"));
    assert!(matches!(gate(&fx, &a), GateOutcome::Owned(OwnedBy::Live)));
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn an_unreadable_live_identity_is_treated_as_live() {
    let fx = Fx::new();
    let a = due(&fx);
    fx.script_refresh(Some("rt-a2"));
    fs::write(fx.paths().global_config, "{\n  \"oauthAccount\": ").unwrap(); // torn
    assert!(matches!(gate(&fx, &a), GateOutcome::Owned(OwnedBy::Live)));
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn an_account_named_in_a_journal_row_is_owned() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.expire_access(&a);
    fx.script_refresh(Some("rt-a2"));
    crashed_switch(&fx, &b, &a);
    assert!(matches!(gate(&fx, &a), GateOutcome::Owned(OwnedBy::Journal)));
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn a_quarantined_account_is_never_sent() {
    let fx = Fx::new();
    let a = due(&fx);
    fx.script_refresh(Some("rt-a2"));
    fx.quarantine(&a, "invalid_grant", &vault_fp(&fx, &a));
    assert!(matches!(
        gate(&fx, &a),
        GateOutcome::Dead(QuarantineReason::InvalidGrant)
    ));
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn a_token_another_process_already_refreshed_is_returned_without_a_request() {
    let fx = Fx::new();
    let a = due(&fx);
    fx.script_refresh(Some("rt-never-sent"));
    let snapshot = fx.vault_bytes(&a).unwrap();
    let mut elsewhere: Value = serde_json::from_slice(&snapshot).unwrap();
    elsewhere["claudeAiOauth"]["accessToken"] = json!("at-refreshed-elsewhere");
    elsewhere["claudeAiOauth"]["refreshToken"] = json!("rt-a2");
    elsewhere["claudeAiOauth"]["expiresAt"] = json!(fx.clock.now_ms() + 3_600_000);
    fx.put_vault(&a, elsewhere.to_string().as_bytes());
    let out = fx
        .engine
        .refresh_stored(fx.cc.as_ref(), &a, &snapshot)
        .unwrap();
    let GateOutcome::AlreadyFresh(bytes) = out else {
        panic!("expected AlreadyFresh, got {out:?}")
    };
    assert_eq!(bytes, elsewhere.to_string().into_bytes());
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn invalid_grant_quarantines_bound_to_the_generation_sent() {
    let fx = Fx::new();
    let a = due(&fx);
    let sent = vault_fp(&fx, &a);
    fx.script_token_error(400, "invalid_grant");
    assert!(matches!(
        gate(&fx, &a),
        GateOutcome::Dead(QuarantineReason::InvalidGrant)
    ));
    assert_eq!(
        quarantine_of(&fx, &a),
        (Some("invalid_grant".into()), Some(sent))
    );
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"), "the vault is kept");
    let events = fx.engine.store().unwrap().events().unwrap();
    assert!(events.iter().any(|e| e.kind == "quarantine"), "{events:?}");
}

#[test]
fn a_credential_without_a_refresh_token_is_dead_without_a_request() {
    let fx = Fx::new();
    let a = due(&fx);
    fx.script_refresh(Some("rt-never-sent"));
    let blob = json!({"claudeAiOauth": {"accessToken": "at-only", "expiresAt": fx.clock.now_ms() + 60_000}});
    fx.put_vault(&a, blob.to_string().as_bytes());
    assert!(matches!(
        gate(&fx, &a),
        GateOutcome::Dead(QuarantineReason::NoRefreshToken)
    ));
    assert_eq!(token_requests(&fx), 0);
    assert_eq!(
        quarantine_of(&fx, &a),
        (Some("no_refresh_token".into()), Some(vault_fp(&fx, &a)))
    );
}

#[test]
fn invalid_client_is_systemic_and_never_a_strike() {
    let fx = Fx::new();
    let a = due(&fx);
    fx.script_token_error(400, "invalid_client");
    assert!(matches!(gate(&fx, &a), GateOutcome::Systemic(_)));
    assert_eq!(quarantine_of(&fx, &a), (None, None));
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
}

#[test]
fn transport_failures_are_transient_and_change_nothing() {
    let cases: [(&str, Box<dyn Fn(&Fx)>); 3] = [
        (
            "pre-send",
            Box::new(|fx| {
                fx.http.push(
                    Method::Post,
                    &Fx::endpoints().token,
                    Err(HttpError::PreSend("dns".into())),
                )
            }),
        ),
        (
            "ambiguous",
            Box::new(|fx| {
                fx.http.push(
                    Method::Post,
                    &Fx::endpoints().token,
                    Err(HttpError::Ambiguous("reset".into())),
                )
            }),
        ),
        (
            "http-500",
            Box::new(|fx| {
                fx.http.push_json(
                    Method::Post,
                    &Fx::endpoints().token,
                    500,
                    json!({"error": "server_error"}),
                )
            }),
        ),
    ];
    for (want, script) in cases {
        let fx = Fx::new();
        let a = due(&fx);
        script(&fx);
        match gate(&fx, &a) {
            GateOutcome::Transient { kind, rescued } => {
                assert_eq!((kind.as_str(), rescued), (want, false))
            }
            other => panic!("{want}: {other:?}"),
        }
        assert_eq!(quarantine_of(&fx, &a), (None, None), "{want}");
        assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"), "{want}");
    }
}

/// `a` as `due` makes it, but in organization `org-mine`, so an organization can disagree.
fn due_in_org(fx: &Fx) -> AccountId {
    common::splice_oauth_account(
        &fx.paths().global_config,
        &json!({"emailAddress": "a@x.co", "organizationUuid": "org-mine",
                "organizationName": null, "accountUuid": "uuid-a@x.co"}),
    );
    fx.set_live_credential(Fx::credential_json("a@x.co", "rt-a").to_string().as_bytes());
    let a = fx.engine.add_live(fx.add_options()).unwrap().account.id;
    fx.add("b@x.co", "rt-b");
    fx.expire_access(&a);
    a
}

/// Every file in `displaced/`, parsed.
fn displaced(fx: &Fx) -> Vec<Value> {
    fx.displaced()
        .iter()
        .map(|b| serde_json::from_slice(b).unwrap())
        .collect()
}

#[test]
fn a_successor_naming_another_account_is_displaced_and_quarantined() {
    let fx = Fx::new();
    let a = due(&fx);
    let sent = vault_fp(&fx, &a);
    fx.http.push_json(
        Method::Post,
        &Fx::endpoints().token,
        200,
        json!({
            "access_token": "at-a2",
            "refresh_token": "rt-a2",
            "expires_in": 28800,
            "scope": "user:inference user:profile",
            "account": {"uuid": "uuid-someone-else", "email_address": "else@x.co"},
            "organization": {"uuid": ""}
        }),
    );
    assert!(matches!(
        gate(&fx, &a),
        GateOutcome::Dead(QuarantineReason::IdentityConflict)
    ));
    assert_eq!(
        fx.vault_refresh_token(&a).as_deref(),
        Some("rt-a"),
        "another account's token never enters this account's vault (§7.3 step 6)"
    );
    assert_eq!(
        quarantine_of(&fx, &a),
        (Some("identity_conflict".into()), Some(sent)),
        "bound to the generation that was sent, which the vault still holds"
    );
    let kept = displaced(&fx);
    assert_eq!(kept.len(), 1, "the successor is never discarded: it is displaced");
    assert_eq!(kept[0]["claudeAiOauth"]["refreshToken"], "rt-a2");
    assert!(
        !fx.env.data_dir().join("rescue").exists(),
        "never an adoptable rescue"
    );
}

#[test]
fn an_organization_alone_that_disagrees_is_a_conflict() {
    // §7.4: either the uuid or the organization is enough; this reply names no account.
    let fx = Fx::new();
    let a = due_in_org(&fx);
    let sent = vault_fp(&fx, &a);
    fx.http.push_json(
        Method::Post,
        &Fx::endpoints().token,
        200,
        json!({"access_token": "at-a2", "refresh_token": "rt-a2", "expires_in": 28800,
               "organization": {"uuid": "org-other"}}),
    );
    assert!(matches!(
        gate(&fx, &a),
        GateOutcome::Dead(QuarantineReason::IdentityConflict)
    ));
    assert_eq!(
        quarantine_of(&fx, &a),
        (Some("identity_conflict".into()), Some(sent))
    );
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
    assert_eq!(displaced(&fx).len(), 1);
}

#[test]
fn a_successor_whose_response_names_no_owner_is_not_a_conflict() {
    let fx = Fx::new();
    let a = due(&fx);
    fx.http.push_json(
        Method::Post,
        &Fx::endpoints().token,
        200,
        json!({"access_token": "at-a2", "refresh_token": "rt-a2", "expires_in": 28800}),
    );
    assert!(matches!(gate(&fx, &a), GateOutcome::Refreshed(_)));
    assert_eq!(quarantine_of(&fx, &a), (None, None));
}

#[test]
fn an_unreadable_or_absent_vault_is_transient_without_a_request() {
    let fx = Fx::new();
    let a = due(&fx);
    fx.script_refresh(Some("rt-a2"));
    let snapshot = fx.vault_bytes(&a).unwrap();
    fx.kc.set_unreadable(SERVICE, a.as_str(), true);
    let out = fx
        .engine
        .refresh_stored(fx.cc.as_ref(), &a, &snapshot)
        .unwrap();
    assert!(
        matches!(&out, GateOutcome::Transient { kind, rescued: false } if kind == "vault-unreadable"),
        "{out:?}"
    );
    fx.kc.set_unreadable(SERVICE, a.as_str(), false);
    fx.kc.delete(SERVICE, a.as_str()).unwrap();
    let out = fx
        .engine
        .refresh_stored(fx.cc.as_ref(), &a, &snapshot)
        .unwrap();
    assert!(
        matches!(&out, GateOutcome::Transient { kind, rescued: false } if kind == "vault-absent"),
        "{out:?}"
    );
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn an_unreadable_rescue_blocks_the_request() {
    let fx = Fx::new();
    let a = due(&fx);
    fx.script_refresh(Some("rt-a2"));
    let dir = fx.env.data_dir().join("rescue");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{a}-0-000000000000.json")), "not json").unwrap();
    let out = gate(&fx, &a);
    assert!(
        matches!(&out, GateOutcome::Transient { kind, rescued: false } if kind == "rescue-unreadable"),
        "{out:?}"
    );
    assert_eq!(token_requests(&fx), 0);
}

#[cfg(feature = "test-hooks")]
mod hooked {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;
    use crate::common::mutation_lock_free;

    #[test]
    fn only_the_account_lock_is_held_across_the_request() {
        let fx = Fx::new();
        let a = due(&fx);
        fx.script_refresh(Some("rt-a2"));
        let checked = Arc::new(AtomicBool::new(false));
        let (env, id, refresh_lock, flag) = (
            fx.env.clone(),
            a.clone(),
            fx.paths().refresh_lock,
            checked.clone(),
        );
        fx.engine.on_point(
            "gate-before-request",
            Box::new(move || {
                assert!(mutation_lock_free(&env), "the mutation lock is never held (§4.3)");
                assert!(!refresh_lock.exists(), "no CC lock is held (§4.3)");
                assert!(
                    AccountLock::try_acquire(&env, &id).unwrap().is_none(),
                    "the account lock is held across the request (§7.3 step 1)"
                );
                flag.store(true, Ordering::SeqCst);
            }),
        );
        assert!(matches!(gate(&fx, &a), GateOutcome::Refreshed(_)));
        assert!(checked.load(Ordering::SeqCst));
    }

    #[test]
    fn invalid_grant_after_the_lineage_moved_is_not_a_strike() {
        let fx = Fx::new();
        let a = due(&fx);
        fx.script_token_error(400, "invalid_grant");
        let mut moved: Value = serde_json::from_slice(&fx.vault_bytes(&a).unwrap()).unwrap();
        moved["claudeAiOauth"]["refreshToken"] = json!("rt-a-written-meanwhile");
        let (kc, id, bytes) = (fx.kc.clone(), a.clone(), moved.to_string().into_bytes());
        fx.engine.on_point(
            "gate-after-response",
            Box::new(move || kc.put(SERVICE, id.as_str(), &bytes)),
        );
        let out = gate(&fx, &a);
        assert!(
            matches!(&out, GateOutcome::Transient { kind, rescued: false } if kind == "refresh-failed"),
            "{out:?}"
        );
        assert_eq!(quarantine_of(&fx, &a), (None, None));
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p tagteam-engine --features test-hooks --test gate`
Expected: FAIL to compile: no `GateOutcome` in `tagteam_engine::refresh` and no method named
`refresh_stored`.

- [ ] **Step 4: Make the module public**

Task 9 already declared both modules public in `crates/tagteam-engine/src/lib.rs`
(`pub mod quarantine;` and `pub mod refresh;`), which the tests need: they name
`tagteam_engine::refresh::GateOutcome` and `tagteam_engine::quarantine::QuarantineReason`.
Confirm both lines read exactly that; `rescue` stays private (`mod rescue;`).

Then remove each `#[cfg_attr(not(test), allow(dead_code))]` that Task 9 (Step 9) put, if any,
on an item this task now calls: `settle_rescues`, `rescues_for`, `delete_rescue`,
`persist_generation`, `quarantine`, `unquarantine` and `quarantine_event`. `write_rescue` keeps its
attribute until Task 11.

- [ ] **Step 5: Implement the gate**

In `crates/tagteam-engine/src/refresh.rs`, merge these imports with the ones Task 9 wrote:

```rust
use std::fmt;
use std::time::Duration;

use tagteam_core::{AccountId, Fingerprint};
use tagteam_provider::provider::{DeadReason, RefreshResult};
use tagteam_provider::{Credential, Identity, Provider, Read};

use crate::account_lock::AccountLock;
use crate::displace::displace;
use crate::engine::Engine;
use crate::error::EngineError;
use crate::hooks;
use crate::quarantine::QuarantineReason;
use crate::store::{AccountRow, Store};
```

Then add, below Task 9's `persist_generation`:

```rust
/// §7.3 step 5: the token request's bound. The account lock is held across it, which is why
/// every other vault writer waits up to 15 s (§6.2).
pub const GATE_TIMEOUT: Duration = Duration::from_secs(10);

/// §7.2: an access token counts as expired this long before its `expiresAt`. Shared with
/// active-token refresh (Task 16).
pub(crate) const EXPIRY_BUFFER_MS: i64 = 5 * 60 * 1000;

/// Who the gate left an account's token to (§7.3 step 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnedBy {
    /// It is the live login: only active-token refresh (§7.5) may refresh it.
    Live,
    /// An unresolved switch names it: until recovery decides, CC may be running on it (§9.6).
    Journal,
    /// A `tagteam run` session owns it (§12.5). Never produced before M4.
    Session,
}

/// What the refresh gate did (§7.3). Credential bytes never reach `Debug`.
pub enum GateOutcome {
    /// Refreshed now; the vault holds these bytes.
    Refreshed(Vec<u8>),
    /// Step 4: another process already refreshed it; the vault holds these bytes.
    AlreadyFresh(Vec<u8>),
    /// Another process holds the account lock (step 1).
    Busy,
    Owned(OwnedBy),
    /// A session profile's provenance conflicts (§12.5). Never produced before M4.
    Conflict,
    /// Quarantined, by this pass or an earlier one (§7.4).
    Dead(QuarantineReason),
    /// The token endpoint refused the request itself (`invalid_client`, or an unknown client
    /// id's `invalid_request_error`), quoting its message: never a strike.
    Systemic(String),
    /// Nothing decisive happened. `rescued` means a successor was received but is only in
    /// `rescue/`: the vault's generation is consumed, and the caller must not activate it.
    Transient { kind: String, rescued: bool },
    /// A successor was received and neither the vault nor `rescue/` could store it.
    Unpersisted,
}

impl fmt::Debug for GateOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GateOutcome::Refreshed(b) => write!(f, "Refreshed(<{} bytes>)", b.len()),
            GateOutcome::AlreadyFresh(b) => write!(f, "AlreadyFresh(<{} bytes>)", b.len()),
            GateOutcome::Busy => f.write_str("Busy"),
            GateOutcome::Owned(by) => write!(f, "Owned({by:?})"),
            GateOutcome::Conflict => f.write_str("Conflict"),
            GateOutcome::Dead(reason) => write!(f, "Dead({reason:?})"),
            GateOutcome::Systemic(m) => write!(f, "Systemic({m:?})"),
            GateOutcome::Transient { kind, rescued } => {
                write!(f, "Transient {{ kind: {kind:?}, rescued: {rescued} }}")
            }
            GateOutcome::Unpersisted => f.write_str("Unpersisted"),
        }
    }
}

fn transient(kind: &str) -> GateOutcome {
    GateOutcome::Transient {
        kind: kind.to_owned(),
        rescued: false,
    }
}

/// §7.2's test for an access token, with a non-numeric `expiresAt` counting as not expired.
/// Shared with active-token refresh (Task 16).
pub(crate) fn expired(p: &dyn Provider, bytes: &[u8], now_ms: i64) -> bool {
    p.access_expires_at(bytes)
        .is_some_and(|at| now_ms + EXPIRY_BUFFER_MS >= at)
}

/// The lineage fingerprint as the store records it; empty for bytes that carry no token.
fn fp_str(p: &dyn Provider, bytes: &[u8]) -> String {
    p.fingerprint(bytes)
        .map(|f| f.as_str().to_owned())
        .unwrap_or_default()
}

/// §7.4 `identity_conflict`: the token response names another account. A uuid is compared
/// only when both sides know one, and an organization only when both are non-empty; either
/// disagreeing alone is a conflict. A successor that conflicts is displaced, never stored
/// (§7.3 step 6).
pub(crate) fn names_another_account(owner: &Identity, row: &AccountRow) -> bool {
    let uuid = match (
        owner.account_uuid.as_deref().filter(|u| !u.is_empty()),
        row.account_uuid.as_deref(),
    ) {
        (Some(theirs), Some(ours)) => theirs != ours,
        _ => false,
    };
    let org = !owner.org_uuid.is_empty()
        && !row.org_uuid.is_empty()
        && owner.org_uuid != row.org_uuid;
    uuid || org
}

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
        if let Some(reason) = &row.quarantine_reason {
            return Ok(GateOutcome::Dead(
                QuarantineReason::parse(reason).unwrap_or(QuarantineReason::InvalidGrant),
            ));
        }
        if !p.kind_traits(&row.kind).refreshable {
            return Ok(transient("not-refreshable"));
        }
        // 2.
        if let Some(by) = self.owner_of(p, &store, &row)? {
            return Ok(GateOutcome::Owned(by));
        }
        // 3. The vault, then any rescue that succeeds it, then the vault again.
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
        let outcome = match p.refresh(self.http.as_ref(), &fresh, now, GATE_TIMEOUT) {
            RefreshResult::Refreshed { successor, owner } => {
                hooks::point(self, "gate-after-response")?;
                self.persist_refreshed(p, &row, &lock, &sent_fp, successor, owner.as_ref())?
            }
            other => {
                hooks::point(self, "gate-after-response")?;
                self.verdict(p, &row, &sent_fp, other)?
            }
        };
        drop(lock);
        Ok(outcome)
    }

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
        if self.session_owned(row) {
            return Ok(Some(OwnedBy::Session));
        }
        Ok(None)
    }

    /// §12.5: whether a `tagteam run` session owns the account. Profiles arrive with M4; until
    /// then nothing is session-owned.
    fn session_owned(&self, _row: &AccountRow) -> bool {
        false
    }

    /// Step 7 for every result but a successor.
    fn verdict(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        sent_fp: &str,
        result: RefreshResult,
    ) -> Result<GateOutcome, EngineError> {
        Ok(match result {
            RefreshResult::Refreshed { .. } => unreachable!("handled by the caller"),
            RefreshResult::Dead(DeadReason::InvalidGrant) => {
                // A strike only if the generation sent is still the vault's: if another writer
                // moved the lineage meanwhile, the refusal says nothing about the new one.
                let unchanged = matches!(
                    self.vault.read(&row.id),
                    Read::Present(b) if fp_str(p, &b) == sent_fp
                );
                if !unchanged {
                    return Ok(transient("refresh-failed"));
                }
                self.quarantine(row, QuarantineReason::InvalidGrant, sent_fp)?;
                GateOutcome::Dead(QuarantineReason::InvalidGrant)
            }
            RefreshResult::Dead(DeadReason::NoRefreshToken) => {
                self.quarantine(row, QuarantineReason::NoRefreshToken, sent_fp)?;
                GateOutcome::Dead(QuarantineReason::NoRefreshToken)
            }
            RefreshResult::Systemic(message) => GateOutcome::Systemic(message),
            RefreshResult::Transient(kind) => transient(&kind.token()),
        })
    }

    /// A received successor (§7.3 step 6). One the token endpoint says belongs to another
    /// account (§7.4) is checked first, before anything is stored: it is displaced and the
    /// account quarantined, and it never reaches this account's vault. Task 11 replaces the
    /// rest with the compare-and-swap and rescue of step 6.
    fn persist_refreshed(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        lock: &AccountLock,
        sent_fp: &str,
        successor: Vec<u8>,
        owner: Option<&Identity>,
    ) -> Result<GateOutcome, EngineError> {
        if let Some(owner) = owner.filter(|o| names_another_account(o, row)) {
            return self.displace_foreign(p, row, sent_fp, &successor, owner);
        }
        self.persist_generation(p, row, lock, &successor)?;
        Ok(GateOutcome::Refreshed(successor))
    }

    /// §7.3 step 6 and §7.4: a successor that belongs to another account is displaced, never
    /// written to this account's vault or to `rescue/` (where a later switch could adopt it),
    /// and the account is quarantined, bound to the generation that was sent, which the vault
    /// still holds. If the displacement fails the successor is lost: `Unpersisted`, with the
    /// quarantine still set.
    pub(crate) fn displace_foreign(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        sent_fp: &str,
        successor: &[u8],
        owner: &Identity,
    ) -> Result<GateOutcome, EngineError> {
        let kept = self.keep_foreign(row, successor, p.fingerprint(successor).as_ref(), owner);
        let quarantined = self.quarantine(row, QuarantineReason::IdentityConflict, sent_fp);
        match (kept, quarantined) {
            // A lost successor is reported first (§7.3 step 6).
            (Err(e), _) => {
                tracing::error!(
                    position = row.position,
                    account = %row.id,
                    "a refreshed token that belongs to another account could not be kept: {e}"
                );
                Ok(GateOutcome::Unpersisted)
            }
            (Ok(()), Err(e)) => Err(e),
            (Ok(()), Ok(())) => Ok(GateOutcome::Dead(QuarantineReason::IdentityConflict)),
        }
    }

    /// Writes a successor that belongs to another account to `displaced/` (§6.3, reason
    /// `identity-conflict`), naming the owner the token response gave.
    pub(crate) fn keep_foreign(
        &self,
        row: &AccountRow,
        successor: &[u8],
        fp: Option<&Fingerprint>,
        owner: &Identity,
    ) -> Result<(), EngineError> {
        displace(
            self,
            &row.provider,
            successor,
            fp,
            "identity-conflict",
            Some(&owner.raw),
        )
        .map(drop)
    }
}
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p tagteam-engine --features test-hooks --test gate`
Expected: PASS, 18 tests.

Run: `cargo test -p tagteam-engine --test gate`
Expected: PASS, 16 tests (the `hooked` module compiles away).

- [ ] **Step 7: Run the engine suite and clippy**

Run: `cargo test -p tagteam-engine --features test-hooks && cargo clippy -p tagteam-engine --all-targets --features test-hooks -- -D warnings`
Expected: every test passes, no warnings.

- [ ] **Step 8: Commit**

```bash
git add crates/tagteam-engine/src/refresh.rs crates/tagteam-engine/src/lib.rs \
  crates/tagteam-engine/src/rescue.rs crates/tagteam-engine/src/quarantine.rs \
  crates/tagteam-engine/tests/common/mod.rs crates/tagteam-engine/tests/gate.rs
git commit -m "Add the refresh gate's lock, ownership, adoption and verdict"
```

---

### Task 11: Gate persistence: compare-and-swap, rescue, `Unpersisted`, the pinned invariant

**Files:**
- Modify: `crates/tagteam-engine/src/refresh.rs`, `crates/tagteam-engine/src/rescue.rs` (a
  leftover `dead_code` allowance)
- Modify: `crates/tagteam-engine/src/engine.rs` (the store slot's lock tolerates poisoning, so
  `Received::drop` can record a loss while unwinding)
- Modify: `crates/tagteam-engine/tests/common/mod.rs` (`Fx::engine_with_vault`, and the shared
  free helpers `block_rescue`, `unblock_rescue`)
- Test: `crates/tagteam-engine/tests/gate_persist.rs`

**Interfaces:**
- Consumes:
  - `Engine::write_rescue(id, login_epoch, predecessor_fp, successor, &successor_fp)` and the
    rescue envelope's field names (`predecessorFp`, `credential`) (Task 9)
  - `Engine::settle_rescues` adopting a rescue on the next pass (Task 9)
  - `Engine::persist_generation` (Task 9)
  - `refresh_stored`, `GateOutcome`, `persist_refreshed`, `displace_foreign`, `keep_foreign`
    and `names_another_account` (Task 10)
  - `QuarantineReason::SuccessorLost` (Task 9)
  - `Engine::fail_at` and `on_point` (M1, `test-hooks`)
- Produces:
  - Step 6 of §7.3 inside `refresh_stored`. `persist_refreshed` and `displace_foreign` are
    replaced by `persist_successor` and `lose`. `abandon` keeps a successor on an early error
    return; the `Received` guard's `Drop` keeps it on a panic, or records its loss.
  - Shared with active-token refresh (Task 16), all in `crate::refresh`:
    - `pub(crate) struct Received<'e>` with
      `pub(crate) fn new(engine: &'e Engine, p: &dyn Provider, row: &AccountRow, predecessor_fp: &str, bytes: Vec<u8>, foreign: Option<Identity>) -> Self`,
      `pub(crate) fn is_foreign(&self) -> bool` and
      `pub(crate) fn keep(&mut self) -> Result<(), EngineError>` (`rescue/`, or `displaced/`
      for a foreign successor). Task 16 adds the `bytes()` accessor it needs, so
      nothing here is dead code;
    - `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub(crate) enum Persisted { Vault, Rescued, Unpersisted }`;
    - `pub(crate) fn Engine::persist_received(&self, p: &dyn Provider, row: &AccountRow, lock: &AccountLock, received: &mut Received<'_>) -> Persisted`
      (never for a foreign successor). `Persisted::Unpersisted` is not yet a loss: the caller
      decides, and records one with `record_loss`;
    - `pub(crate) fn Engine::abandon(&self, row: &AccountRow, sent_fp: &str, received: &mut Received<'_>, cause: EngineError) -> Abandoned`,
      with `pub(crate) enum Abandoned { Kept(EngineError), Lost }`: an error after receipt;
    - `pub(crate) fn Engine::record_loss(&self, row: &AccountRow, sent_fp: &str, cause: &dyn std::fmt::Display)`
      (§7.4 `successor_lost`: logs at ERROR and quarantines, best effort, never panics) and
      `pub(crate) fn Engine::quarantine_best_effort(&self, row: &AccountRow, reason: QuarantineReason, fp: &str)`;
    - `pub(crate) fn log_lost(row: &AccountRow, cause: &dyn std::fmt::Display)`;
    - `pub(crate) enum Displacement { Kept, Lost }` and
      `pub(crate) fn Engine::displace_received(&self, row: &AccountRow, sent_fp: &str, received: &mut Received<'_>) -> Result<Displacement, EngineError>`:
      the foreign-successor path (`keep`, then the `identity_conflict` quarantine), so the gate
      and Task 16 do not each carry a copy.
  - The hook point `gate-before-vault-write`.
  - The Transient kinds `vault-write` and `vault-unreadable` with `rescued: true`.
  - `Fx::engine_with_vault(&self, vault: Vault) -> Engine`.
  - Shared test helpers in `tests/common/mod.rs` (free `pub fn`s, reused by Task 16):
    `block_rescue(&Fx)` and `unblock_rescue(&Fx)`.
- Consumes, from Task 10's `tests/common/mod.rs`: `due`, `quarantine_of`, `token_requests`.
- Decisions this task fixes:
  - **The vault moved between steps 3 and 6** (only a defence: every writer holds the account
    lock). The successor is written to `rescue/` and logged at ERROR, and the gate returns
    `AlreadyFresh(<the vault's newer bytes>)`, the vault's newer credential (§7.3 step 6). If
    that rescue write fails too, the gate returns `Unpersisted`.
  - **`Unpersisted` carries no payload.** The gate logs it at ERROR, naming the position and
    account ID, never the email. The caller turns it into what the user sees: the switch's
    refusal (Task 14), and in M2b the collector's stderr notice. The CLI prints either on
    stderr, naming the position (§7.3 step 6).
  - **A failed `persist_generation` whose vault write landed is not a loss.** The vault is
    re-read, and if it holds the successor, the result is `Refreshed` and the metadata failure
    is logged at ERROR. Nothing is rescued.
  - **An error after the response keeps the successor explicitly** (§7.3 step 6, as amended):
    `abandon` writes it to `rescue/` (or `displaced/`) and returns the error. If that write
    fails too, the gate returns `Unpersisted`, never the error, so the loss is always reported.
    A panic is caught by the `Received` guard's `Drop`, which keeps it the same way. A panic
    while `rescue/` (or `displaced/`) is unwritable cannot return `Unpersisted`: `Drop` logs the
    loss at ERROR, which reaches stderr, and quarantines the account `successor_lost`
    (`identity_conflict` for a foreign successor), best effort (§7.3 step 6, as amended).
    `Drop` must never panic itself, since a second panic while unwinding aborts the process: it
    makes only store writes and log lines through calls that return errors, and
    `Engine::store`'s slot lock tolerates poisoning. Only a kill (SIGKILL) loses a received
    successor with nothing recorded.
  - **A successor that belongs to another account is marked before anything else**
    (`Received::new`'s `foreign`), so no path, `Drop` and `abandon` included, ever stores it in
    this account's vault or in `rescue/`. It is displaced, and the account is quarantined
    `identity_conflict`, bound to the fingerprint that was sent (Task 10's rule). A lost one is
    reported as `Unpersisted`, which takes precedence over the conflict.
  - **`Unpersisted` quarantines the account** (§7.4 `successor_lost`), bound to the fingerprint
    that was sent: the vault's generation is consumed. This is best effort: a store write that
    fails too is logged, and the loss is still reported. The one exception is the vault having
    moved to a newer generation during the request: that generation was not consumed, so a loss
    there quarantines nothing.

- [ ] **Step 1: Add the fixture helpers**

In `crates/tagteam-engine/tests/common/mod.rs`, add to `impl Fx`, after
`engine_with_vault_probe`:

```rust
    /// An engine over the same Env, Keychain, oracle, clock and HTTP port, with a
    /// caller-supplied vault: for making the vault itself misbehave.
    pub fn engine_with_vault(&self, vault: Vault) -> Engine {
        self.engine_over(self.env.clone(), vault, self.oracle.clone())
    }
```

and add these free functions at the end of the module (Task 16's tests reuse them):

```rust
/// A `rescue/` that lists fine but cannot be written to (0500). The gate's step 3 still finds
/// no rescue, so the request is sent; only the write after the response fails. (A plain file
/// in its place would make step 3 report `rescue-unreadable` before any request.)
pub fn block_rescue(fx: &Fx) {
    let dir = fx.env.data_dir().join("rescue");
    fs::create_dir_all(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
}

/// Undoes `block_rescue`, so the temporary directory can be cleaned up.
pub fn unblock_rescue(fx: &Fx) {
    let dir = fx.env.data_dir().join("rescue");
    if dir.is_dir() {
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    }
}
```

- [ ] **Step 2: Write the failing tests**

`crates/tagteam-engine/tests/gate_persist.rs`:

```rust
mod common;

use std::fs;

use common::{Fx, block_rescue, due, quarantine_of, unblock_rescue, vault_fp};
use serde_json::{Value, json};
use tagteam_core::AccountId;
use tagteam_engine::refresh::GateOutcome;
use tagteam_engine::vault::{KeychainVault, SERVICE, Vault, VaultBackend, VaultError};
use tagteam_provider::Read;
use tagteam_provider::http::Method;

fn refresh(fx: &Fx, id: &AccountId, snapshot: &[u8]) -> GateOutcome {
    fx.engine
        .refresh_stored(fx.cc.as_ref(), id, snapshot)
        .unwrap()
}

/// Every rescue envelope on disk (§6.3), parsed. None when `rescue/` does not exist or is
/// not a directory.
fn rescues(fx: &Fx) -> Vec<Value> {
    let Ok(dir) = fs::read_dir(fx.env.data_dir().join("rescue")) else {
        return vec![];
    };
    dir.map(|e| serde_json::from_slice(&fs::read(e.unwrap().path()).unwrap()).unwrap())
        .collect()
}

/// The refresh token inside each rescue envelope's credential.
fn rescued_refresh_tokens(fx: &Fx) -> Vec<String> {
    rescues(fx)
        .iter()
        .map(|envelope| {
            let cred: Value =
                serde_json::from_str(envelope["credential"].as_str().unwrap()).unwrap();
            cred["claudeAiOauth"]["refreshToken"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect()
}

/// A vault that stores `{}` whenever `target`'s current generation is written, as a Keychain
/// that silently kept something else would: the read-back never matches.
struct GarblingVault {
    inner: KeychainVault,
    target: String,
}

impl VaultBackend for GarblingVault {
    fn read(&self, key: &str) -> Read<Vec<u8>> {
        self.inner.read(key)
    }
    fn write(&self, key: &str, bytes: &[u8]) -> Result<(), VaultError> {
        if key == self.target {
            self.inner.write(key, b"{}")
        } else {
            self.inner.write(key, bytes)
        }
    }
    fn delete(&self, key: &str) -> Result<(), VaultError> {
        self.inner.delete(key)
    }
}

fn garbling_vault(fx: &Fx, id: &AccountId) -> Vault {
    Vault::new(Box::new(GarblingVault {
        inner: KeychainVault::new(fx.kc.clone()),
        target: id.to_string(),
    }))
}

#[test]
fn a_vault_that_refuses_the_write_leaves_the_successor_in_rescue_for_the_next_pass() {
    let fx = Fx::new();
    let a = due(&fx);
    let sent = vault_fp(&fx, &a);
    let snapshot = fx.vault_bytes(&a).unwrap();
    fx.script_refresh(Some("rt-a2"));
    fx.kc.set_fail_write(SERVICE, true);
    let out = refresh(&fx, &a, &snapshot);
    assert!(
        matches!(&out, GateOutcome::Transient { kind, rescued: true } if kind == "vault-write"),
        "{out:?}"
    );
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
    let envelopes = rescues(&fx);
    assert_eq!(envelopes.len(), 1);
    assert_eq!(envelopes[0]["predecessorFp"], json!(sent));
    assert_eq!(rescued_refresh_tokens(&fx), ["rt-a2"]);

    // The next pass adopts it without a second request (§7.3 step 3).
    fx.kc.set_fail_write(SERVICE, false);
    let out = refresh(&fx, &a, &snapshot);
    assert!(matches!(out, GateOutcome::AlreadyFresh(_)), "{out:?}");
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"));
    assert!(rescues(&fx).is_empty(), "an adopted rescue is deleted");
    assert_eq!(fx.http.count(Method::Post, &Fx::endpoints().token), 1);
}

#[test]
fn a_vault_that_stores_something_else_leaves_the_successor_in_rescue() {
    let fx = Fx::new();
    let a = due(&fx);
    let snapshot = fx.vault_bytes(&a).unwrap();
    fx.script_refresh(Some("rt-a2"));
    let engine = fx.engine_with_vault(garbling_vault(&fx, &a));
    let out = engine
        .refresh_stored(fx.cc.as_ref(), &a, &snapshot)
        .unwrap();
    assert!(
        matches!(&out, GateOutcome::Transient { kind, rescued: true } if kind == "vault-write"),
        "{out:?}"
    );
    assert_eq!(rescued_refresh_tokens(&fx), ["rt-a2"]);
}

#[test]
fn both_writes_failing_is_reported_as_unpersisted_and_quarantines() {
    let fx = Fx::new();
    let a = due(&fx);
    let sent = vault_fp(&fx, &a);
    let snapshot = fx.vault_bytes(&a).unwrap();
    fx.script_refresh(Some("rt-a2"));
    fx.kc.set_fail_write(SERVICE, true);
    block_rescue(&fx);
    assert!(matches!(
        refresh(&fx, &a, &snapshot),
        GateOutcome::Unpersisted
    ));
    unblock_rescue(&fx);
    assert_eq!(fx.http.count(Method::Post, &Fx::endpoints().token), 1, "the request was sent");
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
    assert_eq!(
        quarantine_of(&fx, &a),
        (Some("successor_lost".into()), Some(sent)),
        "the vault's generation is consumed (§7.4 successor_lost)"
    );
}

#[test]
fn a_response_without_a_refresh_token_keeps_the_lineage() {
    // Review Focus 3, persistence half: the old refresh token is kept, so the fingerprint is
    // unchanged and `.prev` does not rotate.
    let fx = Fx::new();
    let a = due(&fx);
    let before = vault_fp(&fx, &a);
    let snapshot = fx.vault_bytes(&a).unwrap();
    fx.script_refresh(None);
    assert!(matches!(
        refresh(&fx, &a, &snapshot),
        GateOutcome::Refreshed(_)
    ));
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
    assert_eq!(vault_fp(&fx, &a), before, "the lineage is unchanged");
    assert!(
        fx.kc.get(SERVICE, &format!("{a}.prev")).is_none(),
        ".prev rotates only on a lineage change (§6.2)"
    );
    let stored: Value = serde_json::from_slice(&fx.vault_bytes(&a).unwrap()).unwrap();
    assert_eq!(stored["claudeAiOauth"]["accessToken"], "at-same");
}

#[cfg(feature = "test-hooks")]
mod hooked {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use tagteam_engine::Engine;

    use super::*;

    #[test]
    fn the_vault_moving_during_the_request_keeps_the_newer_generation() {
        let fx = Fx::new();
        let a = due(&fx);
        let sent = vault_fp(&fx, &a);
        let snapshot = fx.vault_bytes(&a).unwrap();
        fx.script_refresh(Some("rt-a2"));
        let mut newer: Value = serde_json::from_slice(&snapshot).unwrap();
        newer["claudeAiOauth"]["refreshToken"] = json!("rt-a-written-meanwhile");
        let newer = newer.to_string().into_bytes();
        let (kc, id, bytes) = (fx.kc.clone(), a.clone(), newer.clone());
        fx.engine.on_point(
            "gate-after-response",
            Box::new(move || kc.put(SERVICE, id.as_str(), &bytes)),
        );
        let GateOutcome::AlreadyFresh(returned) = refresh(&fx, &a, &snapshot) else {
            panic!("expected AlreadyFresh")
        };
        assert_eq!(returned, newer, "the vault's newer credential is returned");
        assert_eq!(rescued_refresh_tokens(&fx), ["rt-a2"]);
        assert_eq!(rescues(&fx)[0]["predecessorFp"], json!(sent));
    }

    #[derive(Debug, Clone, Copy)]
    enum Fault {
        VaultWrite,
        VaultVerify,
        VaultAndRescue,
        ErrorAfterResponse,
        PanicAfterResponse,
        ErrorBeforeVaultWrite,
        PanicBeforeVaultWrite,
        /// An error after the response while `rescue/` is unwritable: the successor cannot be
        /// kept, and the gate must say so rather than return the error.
        ErrorAfterResponseRescueBlocked,
        /// Panics while `rescue/` is unwritable: the guard's `Drop` can neither keep the
        /// successor nor return `Unpersisted`, so it records the loss (§7.3 step 6, amended).
        PanicAfterResponseRescueBlocked,
        PanicBeforeVaultWriteRescueBlocked,
    }

    const FAULTS: [Fault; 10] = [
        Fault::VaultWrite,
        Fault::VaultVerify,
        Fault::VaultAndRescue,
        Fault::ErrorAfterResponse,
        Fault::PanicAfterResponse,
        Fault::ErrorBeforeVaultWrite,
        Fault::PanicBeforeVaultWrite,
        Fault::ErrorAfterResponseRescueBlocked,
        Fault::PanicAfterResponseRescueBlocked,
        Fault::PanicBeforeVaultWriteRescueBlocked,
    ];

    /// §15.3 "Refresh tokens" and §1.1 criterion 2: for every fault injected after the token
    /// response is received, the successor ends up in the vault or in `rescue/`; when every
    /// write fails, the gate reports `Unpersisted`, never the injected error, and quarantines
    /// the account `successor_lost`. A panic that nothing can keep the successor through still
    /// records the loss: the same quarantine, and an ERROR log line (§7.3 step 6, §7.4).
    #[test]
    fn a_received_successor_is_never_discarded() {
        for fault in FAULTS {
            let fx = Fx::new();
            let a = due(&fx);
            let sent = vault_fp(&fx, &a);
            let snapshot = fx.vault_bytes(&a).unwrap();
            fx.script_refresh(Some("rt-a2"));
            let garbling;
            let engine: &Engine = match fault {
                Fault::VaultVerify => {
                    garbling = fx.engine_with_vault(garbling_vault(&fx, &a));
                    &garbling
                }
                _ => &fx.engine,
            };
            match fault {
                Fault::VaultWrite => fx.kc.set_fail_write(SERVICE, true),
                Fault::VaultVerify => {}
                Fault::VaultAndRescue => {
                    fx.kc.set_fail_write(SERVICE, true);
                    block_rescue(&fx);
                }
                Fault::ErrorAfterResponse => engine.fail_at(Some("gate-after-response")),
                Fault::PanicAfterResponse => engine.fail_at(Some("panic:gate-after-response")),
                Fault::ErrorBeforeVaultWrite => engine.fail_at(Some("gate-before-vault-write")),
                Fault::PanicBeforeVaultWrite => {
                    engine.fail_at(Some("panic:gate-before-vault-write"))
                }
                Fault::ErrorAfterResponseRescueBlocked => {
                    engine.fail_at(Some("gate-after-response"));
                    block_rescue(&fx);
                }
                Fault::PanicAfterResponseRescueBlocked => {
                    engine.fail_at(Some("panic:gate-after-response"));
                    block_rescue(&fx);
                }
                Fault::PanicBeforeVaultWriteRescueBlocked => {
                    engine.fail_at(Some("panic:gate-before-vault-write"));
                    block_rescue(&fx);
                }
            }
            let result = catch_unwind(AssertUnwindSafe(|| {
                engine.refresh_stored(fx.cc.as_ref(), &a, &snapshot)
            }));
            let unpersisted = matches!(result, Ok(Ok(GateOutcome::Unpersisted)));
            let kept = fx.vault_refresh_token(&a).as_deref() == Some("rt-a2")
                || rescued_refresh_tokens(&fx).iter().any(|rt| rt == "rt-a2");
            let loss_recorded =
                quarantine_of(&fx, &a) == (Some("successor_lost".into()), Some(sent.clone()));
            match fault {
                Fault::PanicAfterResponseRescueBlocked
                | Fault::PanicBeforeVaultWriteRescueBlocked => {
                    assert!(result.is_err(), "{fault:?}: the injected panic propagates");
                    assert!(!kept, "{fault:?}: nothing could keep the successor");
                    assert!(
                        loss_recorded,
                        "{fault:?}: the loss quarantines the account (§7.4 successor_lost)"
                    );
                }
                _ => {
                    assert!(
                        kept || unpersisted,
                        "{fault:?}: the successor was discarded ({result:?})"
                    );
                    if unpersisted {
                        assert!(loss_recorded, "{fault:?}: Unpersisted quarantines successor_lost");
                    }
                }
            }
            if matches!(
                fault,
                Fault::VaultAndRescue | Fault::ErrorAfterResponseRescueBlocked
            ) {
                assert!(unpersisted, "{fault:?}: {result:?}");
            }
            unblock_rescue(&fx);
        }
    }

    /// §7.3 step 6 and §7.4: a successor the response says belongs to another account is never
    /// an adoptable rescue, even when an error interrupts the gate before it could be
    /// displaced. It is displaced instead, the account quarantined, and the error returned.
    #[test]
    fn a_foreign_successor_is_displaced_never_rescued_even_when_interrupted() {
        let fx = Fx::new();
        let a = due(&fx);
        let sent = vault_fp(&fx, &a);
        let snapshot = fx.vault_bytes(&a).unwrap();
        fx.http.push_json(
            Method::Post,
            &Fx::endpoints().token,
            200,
            json!({"access_token": "at-a2", "refresh_token": "rt-a2", "expires_in": 28800,
                   "account": {"uuid": "uuid-someone-else"}}),
        );
        fx.engine.fail_at(Some("gate-after-response"));
        assert!(
            fx.engine
                .refresh_stored(fx.cc.as_ref(), &a, &snapshot)
                .is_err(),
            "the injected error is returned once the successor is kept"
        );
        assert!(rescues(&fx).is_empty(), "never an adoptable rescue");
        assert_eq!(fx.displaced().len(), 1, "displaced instead");
        assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
        assert_eq!(
            quarantine_of(&fx, &a),
            (Some("identity_conflict".into()), Some(sent))
        );
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p tagteam-engine --features test-hooks --test gate_persist`
Expected: FAIL.
- `a_vault_that_refuses_the_write…`, `a_vault_that_stores_something_else…` and
  `both_writes_failing…`: Task 10's `persist_refreshed` returns the vault error through `?`, so
  the test's `.unwrap()` panics (`vault write failed` / `the vault entry did not read back as
  written`).
- `a_received_successor_is_never_discarded` fails at `Fault::VaultWrite`.
- `a_foreign_successor_is_displaced_never_rescued_even_when_interrupted` fails: Task 10 returns
  the injected error before anything keeps the successor, so nothing is displaced.
- `a_response_without_a_refresh_token_keeps_the_lineage` already passes: `persist_generation`
  keeps `.prev` on an unchanged fingerprint (Task 9).

- [ ] **Step 4: Implement step 6 and the `Received` guard**

In `crates/tagteam-engine/src/rescue.rs`, remove the `#[cfg_attr(not(test), allow(dead_code))]`
Task 9 (Step 9) put on `write_rescue`, if it is there: the `Received` guard below calls it. After
this step no Task 9 item carries that attribute.

In `crates/tagteam-engine/src/refresh.rs` (Task 10's imports already bring in
`Fingerprint` and `Identity`), add, above `impl Engine` (next to `names_another_account`):

```rust
/// A successor a refresh has received and not yet persisted: the gate's (§7.3) and the
/// active-token refresh's (§7.5, Task 16). §7.3's rule is that it is never discarded: dropped
/// while still armed, by a panic or an early return, it keeps itself (`keep`). It holds a
/// secret, so it has no `Debug`.
pub(crate) struct Received<'e> {
    engine: &'e Engine,
    row: AccountRow,
    predecessor_fp: String,
    bytes: Vec<u8>,
    fp: Fingerprint,
    /// The owner the response named, when it is another account (§7.4). Such a successor is
    /// kept in `displaced/`, never in this account's vault or in `rescue/`.
    foreign: Option<Identity>,
    armed: bool,
}

impl<'e> Received<'e> {
    /// Arms the guard. Build it the moment the response is parsed, before any fallible step.
    /// `foreign` is the response's owner when `names_another_account` says it is not this
    /// account; it is fixed here, so no later path can store the successor as this account's.
    pub(crate) fn new(
        engine: &'e Engine,
        p: &dyn Provider,
        row: &AccountRow,
        predecessor_fp: &str,
        bytes: Vec<u8>,
        foreign: Option<Identity>,
    ) -> Self {
        let fp = p
            .fingerprint(&bytes)
            .unwrap_or_else(|| Fingerprint::of_secret(&bytes));
        Self {
            engine,
            row: row.clone(),
            predecessor_fp: predecessor_fp.to_owned(),
            bytes,
            fp,
            foreign,
            armed: true,
        }
    }

    /// Whether the response said the successor belongs to another account (§7.4).
    pub(crate) fn is_foreign(&self) -> bool {
        self.foreign.is_some()
    }

    /// One attempt to keep the successor outside the vault: `rescue/`, or `displaced/` when it
    /// belongs to another account (§7.3 step 6). It disarms whatever the result, so `Drop`
    /// never repeats it.
    pub(crate) fn keep(&mut self) -> Result<(), EngineError> {
        self.armed = false;
        match &self.foreign {
            Some(owner) => self
                .engine
                .keep_foreign(&self.row, &self.bytes, Some(&self.fp), owner),
            None => self
                .engine
                .write_rescue(
                    &self.row.id,
                    self.row.login_epoch,
                    &self.predecessor_fp,
                    &self.bytes,
                    &self.fp,
                )
                .map(drop),
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for Received<'_> {
    /// Runs only while armed: a panic unwound past the successor before it was stored. It
    /// cannot return `Unpersisted`, so it keeps the successor if it can, and otherwise records
    /// the loss: an ERROR log line, which reaches stderr, and the account's quarantine (§7.3
    /// step 6, §7.4). It must never panic itself, since a second panic while unwinding aborts
    /// the process, so it makes only store writes and log lines, through calls that return
    /// errors instead of panicking.
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let engine = self.engine;
        let row = self.row.clone();
        let sent_fp = self.predecessor_fp.clone();
        let foreign = self.is_foreign();
        let kept = self.keep();
        if foreign {
            engine.quarantine_best_effort(&row, QuarantineReason::IdentityConflict, &sent_fp);
        }
        match kept {
            Ok(()) => tracing::error!(
                position = row.position,
                account = %row.id,
                "a refresh was interrupted before its token was stored; the token was kept outside the vault"
            ),
            Err(e) if foreign => log_lost(&row, &e),
            Err(e) => engine.record_loss(&row, &sent_fp, &e),
        }
    }
}

/// Where a received successor landed (§7.3 step 6, §7.5 step 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Persisted {
    /// The vault holds it.
    Vault,
    /// The vault could not take it, so `rescue/` holds it. The vault's generation is consumed.
    Rescued,
    /// Neither could. Not yet a loss: the caller decides, since active-token refresh may still
    /// publish it to the live store (§7.5 step 5), and records one with `record_loss`.
    Unpersisted,
}

/// What became of a successor an error interrupted before it was stored (§7.3 step 6).
pub(crate) enum Abandoned {
    /// Kept in `rescue/` (or `displaced/`): the caller returns the error.
    Kept(EngineError),
    /// Kept nowhere, and recorded: the caller reports `Unpersisted`, never the error.
    Lost,
}

/// What became of a successor that belongs to another account (§7.4).
pub(crate) enum Displacement {
    /// In `displaced/`, and the account quarantined `identity_conflict`.
    Kept,
    /// Kept nowhere: the caller reports `Unpersisted`.
    Lost,
}

/// A successor is lost (§7.3 step 6). Logged at ERROR, naming the account by position and ID
/// only (§4.4); the caller's refusal or notice carries it to the user.
pub(crate) fn log_lost(row: &AccountRow, cause: &dyn std::fmt::Display) {
    tracing::error!(
        position = row.position,
        account = %row.id,
        "a refreshed token was lost: {cause}"
    );
}
```

In `refresh_stored`, replace the `Refreshed` arm:

```rust
            RefreshResult::Refreshed { successor, owner } => {
                hooks::point(self, "gate-after-response")?;
                self.persist_refreshed(p, &row, &lock, &sent_fp, successor, owner.as_ref())?
            }
```

with:

```rust
            RefreshResult::Refreshed { successor, owner } => {
                // §7.4: a successor the response says belongs to another account is marked
                // first, so no path, `Drop` included, ever stores it as this account's.
                let foreign = owner.filter(|o| names_another_account(o, &row));
                // From here on the successor is never discarded (§7.3): `received` keeps it if
                // this unwinds, and `abandon` if an error returns early.
                let mut received = Received::new(self, p, &row, &sent_fp, successor, foreign);
                let persisted = hooks::point(self, "gate-after-response").and_then(|()| {
                    self.persist_successor(p, &row, &lock, &sent_fp, &mut received)
                });
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
```

Replace the `persist_refreshed` and `displace_foreign` methods (Task 10) with these. Both
refresh paths share `persist_received`, `abandon`, `record_loss` and `quarantine_best_effort`
(Task 16 reuses them); `persist_successor` and `lose` are the gate's. `keep_foreign` stays:
`Received::keep` calls it.

```rust
    /// §7.3 step 6 and §7.5 step 5: the received successor goes to the vault, or to `rescue/`
    /// when the vault cannot take it. A failed `persist_generation` whose vault write landed is
    /// no loss: the metadata failure is logged and the result is `Vault`. `received` is
    /// disarmed whatever the result, so its `Drop` never writes a second copy. `Unpersisted` is
    /// not yet a loss: the gate records one at once (`lose`); active-token refresh only when the
    /// live store did not take the successor either (§7.5 step 5). A successor that belongs to
    /// another account never comes here: its caller keeps it with `Received::keep` (§7.4).
    pub(crate) fn persist_received(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        lock: &AccountLock,
        received: &mut Received<'_>,
    ) -> Persisted {
        debug_assert!(!received.is_foreign(), "a foreign successor is never stored");
        let Err(e) = self.persist_generation(p, row, lock, &received.bytes) else {
            received.disarm();
            return Persisted::Vault;
        };
        // The vault write may have landed before recording it failed; then nothing is lost.
        if matches!(self.vault.read(&row.id), Read::Present(b) if b == received.bytes) {
            tracing::error!(
                position = row.position,
                account = %row.id,
                "a refreshed token was stored, but recording it failed: {e}"
            );
            received.disarm();
            return Persisted::Vault;
        }
        tracing::error!(
            position = row.position,
            account = %row.id,
            "the vault could not store a refreshed token: {e}"
        );
        match received.keep() {
            Ok(()) => Persisted::Rescued,
            Err(e) => {
                tracing::error!(
                    position = row.position,
                    account = %row.id,
                    "neither the vault nor rescue/ could store a refreshed token: {e}"
                );
                Persisted::Unpersisted
            }
        }
    }

    /// §7.3 step 6 for the gate: a successor that belongs to another account is displaced and
    /// the account quarantined; any other is persisted compare-and-swap style. Every path ends
    /// with the successor in the vault, in `rescue/` or `displaced/`, or reported as
    /// `Unpersisted`.
    fn persist_successor(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        lock: &AccountLock,
        sent_fp: &str,
        received: &mut Received<'_>,
    ) -> Result<GateOutcome, EngineError> {
        if received.is_foreign() {
            return Ok(match self.displace_received(row, sent_fp, received)? {
                Displacement::Kept => GateOutcome::Dead(QuarantineReason::IdentityConflict),
                Displacement::Lost => GateOutcome::Unpersisted,
            });
        }
        // Every vault writer holds the account lock, so this comparison cannot fail; it stays
        // as a defence.
        match self.vault.read(&row.id) {
            Read::Present(now) if fp_str(p, &now) == sent_fp => {}
            Read::Present(now) => {
                tracing::error!(
                    position = row.position,
                    account = %row.id,
                    "the vault moved while its refresh was in flight; the successor was kept in rescue/"
                );
                // The vault moved on to a generation this refresh did not consume, so losing
                // the successor here quarantines nothing.
                return Ok(match received.keep() {
                    Ok(()) => GateOutcome::AlreadyFresh(now),
                    Err(e) => {
                        log_lost(row, &e);
                        GateOutcome::Unpersisted
                    }
                });
            }
            Read::Absent | Read::Unreadable(_) => {
                return Ok(match received.keep() {
                    Ok(()) => GateOutcome::Transient {
                        kind: "vault-unreadable".into(),
                        rescued: true,
                    },
                    Err(e) => self.lose(row, sent_fp, &e),
                });
            }
        }
        hooks::point(self, "gate-before-vault-write")?;
        Ok(match self.persist_received(p, row, lock, received) {
            Persisted::Vault => GateOutcome::Refreshed(std::mem::take(&mut received.bytes)),
            Persisted::Rescued => GateOutcome::Transient {
                kind: "vault-write".into(),
                rescued: true,
            },
            Persisted::Unpersisted => {
                self.lose(row, sent_fp, &"neither the vault nor rescue/ could store it")
            }
        })
    }

    /// §7.3 step 6 and §7.4 for a successor that belongs to another account: it is displaced,
    /// never stored, and the account is quarantined `identity_conflict`, bound to `sent_fp`.
    /// A successor that could not even be displaced is reported first (`Lost`); the account is
    /// then still quarantined, so the loss is only logged. Shared by the gate and active-token
    /// refresh (Task 16).
    pub(crate) fn displace_received(
        &self,
        row: &AccountRow,
        sent_fp: &str,
        received: &mut Received<'_>,
    ) -> Result<Displacement, EngineError> {
        let kept = received.keep();
        let quarantined = self.quarantine(row, QuarantineReason::IdentityConflict, sent_fp);
        match (kept, quarantined) {
            (Err(e), _) => {
                log_lost(row, &e);
                Ok(Displacement::Lost)
            }
            (Ok(()), Err(e)) => Err(e),
            (Ok(()), Ok(())) => Ok(Displacement::Kept),
        }
    }

    /// Quarantines `row`, best effort: a failure is logged (position and ID only), never
    /// returned. Never panics, so `Received::drop` may call it while unwinding.
    pub(crate) fn quarantine_best_effort(
        &self,
        row: &AccountRow,
        reason: QuarantineReason,
        fp: &str,
    ) {
        if let Err(e) = self.quarantine(row, reason, fp) {
            tracing::error!(
                position = row.position,
                account = %row.id,
                reason = reason.as_str(),
                "could not quarantine the account: {e}"
            );
        }
    }

    /// §7.3 step 6 and §7.4 `successor_lost`: a received successor could be kept nowhere, and
    /// the generation that was sent, still the vault's, is consumed. Logs the loss and
    /// quarantines the account, bound to `sent_fp`, best effort: a store that cannot record it
    /// either leaves the loss reported only. Shared by the gate, `Received::drop` and
    /// active-token refresh (Task 16). Never panics.
    pub(crate) fn record_loss(
        &self,
        row: &AccountRow,
        sent_fp: &str,
        cause: &dyn std::fmt::Display,
    ) {
        log_lost(row, cause);
        self.quarantine_best_effort(row, QuarantineReason::SuccessorLost, sent_fp);
    }

    /// The gate's `Unpersisted`, recorded (`record_loss`).
    fn lose(&self, row: &AccountRow, sent_fp: &str, cause: &dyn std::fmt::Display) -> GateOutcome {
        self.record_loss(row, sent_fp, cause);
        GateOutcome::Unpersisted
    }

    /// §7.3 step 6: an error after the response was received, before the successor was
    /// stored. The successor is kept first (`Received::keep`) and the caller returns the error.
    /// If keeping it fails too, the loss is recorded (`record_loss`; only logged for a foreign
    /// successor, whose account is quarantined `identity_conflict` instead) and the caller
    /// reports `Unpersisted`, never the error. Shared by the gate and active-token refresh
    /// (Task 16).
    pub(crate) fn abandon(
        &self,
        row: &AccountRow,
        sent_fp: &str,
        received: &mut Received<'_>,
        cause: EngineError,
    ) -> Abandoned {
        let foreign = received.is_foreign();
        let kept = received.keep();
        if foreign {
            self.quarantine_best_effort(row, QuarantineReason::IdentityConflict, sent_fp);
        }
        match kept {
            Ok(()) => Abandoned::Kept(cause),
            Err(e) => {
                tracing::error!(
                    position = row.position,
                    account = %row.id,
                    "the refresh failed after its token was received: {cause}"
                );
                if foreign {
                    log_lost(row, &e);
                } else {
                    self.record_loss(row, sent_fp, &e);
                }
                Abandoned::Lost
            }
        }
    }
```

In `crates/tagteam-engine/src/engine.rs`, add `PoisonError` to the `std::sync` import, and in
both `store()` and `existing_store()` replace

```rust
        let mut slot = self.store.lock().unwrap();
```

with

```rust
        let mut slot = self.store.lock().unwrap_or_else(PoisonError::into_inner);
```

The slot only caches an `Arc<Store>`, so a poisoned lock holds nothing half-written. This keeps
`Received::drop`'s quarantine from ever panicking a second time while unwinding (which would
abort); `Store`'s own connection lock already tolerates poisoning.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p tagteam-engine --features test-hooks --test gate_persist --test gate`
Expected: PASS: 7 tests in `gate_persist`, and Task 10's 18 in `gate` still pass
(`a_successor_naming_another_account_is_displaced_and_quarantined` and
`an_organization_alone_that_disagrees_is_a_conflict` now go through `persist_successor`'s
foreign path).

- [ ] **Step 6: Run the engine suite and clippy**

Run: `cargo test -p tagteam-engine --features test-hooks && cargo clippy -p tagteam-engine --all-targets --features test-hooks -- -D warnings`
Expected: every test passes, no warnings.

- [ ] **Step 7: Pin the vault writers racing the gate (§15.2)**

This pins behaviour Tasks 10 and 11 already give: every vault writer takes the account lock,
and the gate holds it across its request. Expect the test to pass on its first run. If it
fails, the lock discipline is broken; fix that, not the test.

Add to the `hooked` module's imports in `crates/tagteam-engine/tests/gate_persist.rs`:

```rust
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    use tagteam_engine::EngineError;
```

and append this test to the `hooked` module:

```rust
    /// §15.2 "every vault writer racing the refresh gate", for the writers of an inactive
    /// account that M2a has: a switch to it, and `remove`. Each runs in a second engine (a
    /// second tagteam process) while the gate holds the account lock across its request. It
    /// waits for that lock, then acts on the gate's successor, never on the generation the
    /// gate consumed (§6.2), and never makes a second request.
    #[test]
    fn a_vault_writer_waits_for_the_gate_then_builds_on_its_successor() {
        for writer in ["switch", "remove"] {
            let fx = Fx::new();
            let a = due(&fx);
            let snapshot = fx.vault_bytes(&a).unwrap();
            fx.script_refresh(Some("rt-a2"));
            let other = Mutex::new(Some(fx.engine_with_env(fx.env.clone())));
            let running: Arc<Mutex<Option<thread::JoinHandle<Result<(), EngineError>>>>> =
                Arc::default();
            let (slot, target, request) =
                (running.clone(), a.clone(), fx.switch_request(&a, false));
            fx.engine.on_point(
                "gate-before-request",
                Box::new(move || {
                    let engine = other.lock().unwrap().take().expect("one request per gate");
                    let (target, request) = (target.clone(), request.clone());
                    let handle = thread::spawn(move || match writer {
                        "switch" => engine.switch(request).map(drop),
                        _ => engine.remove(&target).map(drop),
                    });
                    thread::sleep(Duration::from_millis(300));
                    assert!(
                        !handle.is_finished(),
                        "the {writer} must wait for the account lock the gate holds (§6.2)"
                    );
                    *slot.lock().unwrap() = Some(handle);
                }),
            );
            assert!(
                matches!(refresh(&fx, &a, &snapshot), GateOutcome::Refreshed(_)),
                "{writer}"
            );
            let handle = running.lock().unwrap().take().expect("the writer ran");
            handle.join().unwrap().unwrap();
            match writer {
                "switch" => assert_eq!(
                    fx.live_refresh_token().as_deref(),
                    Some("rt-a2"),
                    "the switch activated the gate's successor, not the generation it consumed"
                ),
                _ => assert!(
                    fx.vault_bytes(&a).is_none(),
                    "remove ran after the gate, on what the gate stored"
                ),
            }
            assert!(rescues(&fx).is_empty(), "{writer}: the successor reached the vault");
            assert_eq!(
                fx.http.count(Method::Post, &Fx::endpoints().token),
                1,
                "{writer}: exactly one refresh request"
            );
        }
    }
```

Run: `cargo test -p tagteam-engine --features test-hooks --test gate_persist a_vault_writer_waits`
Expected: PASS (1 test; 8 in `gate_persist` in all).

- [ ] **Step 8: Commit**

```bash
git add crates/tagteam-engine/src/refresh.rs crates/tagteam-engine/src/rescue.rs \
  crates/tagteam-engine/src/engine.rs crates/tagteam-engine/tests/common/mod.rs \
  crates/tagteam-engine/tests/gate_persist.rs
git commit -m "Persist a refreshed token by compare-and-swap, rescuing it on any failure"
```

---

### Task 12: Rotation: lazy walk, quarantined accounts skipped, unreadable account named

**Files:**
- Modify: `crates/tagteam-core/src/rotation.rs`, `crates/tagteam-core/src/lib.rs`
- Modify: `crates/tagteam-provider/src/read.rs` (`ReadError` implements `std::error::Error`)
- Modify: `crates/tagteam-engine/src/switch.rs` (`rotation`, `has_login`; `is_switchable` is
  removed)
- Modify: `crates/tagteam-engine/src/error.rs` (`UnreadableAccount`)
- Modify: `crates/tagteam-engine/tests/switch.rs` (the M1 unreadable-vault test)
- Test: `crates/tagteam-engine/tests/rotation.rs`

**Interfaces:**
- Consumes: `AccountRow.quarantine_reason` (M1), `Fx::quarantine` (Task 9: now through
  `store.set_quarantine`), `Fx::engine_with_vault_probe` (M1).
- Produces:
  - `tagteam_core::rotation_order(positions: &[u32], anchor: Option<u32>) -> Vec<u32>`. It
    replaces `next_in_rotation`, which is removed.
  - `EngineError::UnreadableAccount { position: u32, label: String, source: ReadError }`, with
    kind `"unreadable"`. A rotation raises it for an unreadable vault met before its pick, and a
    direct switch for an unreadable target.
  - The rotation reads vaults lazily, so a bare `switch` never reads the vault of an account it
    does not end up switching between (M-8).
  - Tasks 13 and 14 keep that property: they read only the target's vault.
- Decisions this task fixes:
  - **Counting candidates.** With a managed live anchor, a rotation counts its candidates from
    the store: enabled, unquarantined, with an `identity_json` object. The live account counts
    if it qualifies, as it did in M1. Fewer than two gives `only-one-account`.
  - **A walk that finds no credential** gives `only-one-account` with a managed live anchor,
    and `no-valid-target` without one (M1's reason for that case). This is the spec's §9.3 as
    clarified in `ac25b2d` (the fresh-machine case of §9.2 takes precedence when there is no
    live login to anchor on); M1's `a_fresh_machine_activates_the_first_switchable_account`
    pins it, and no code here changes for it.
  - **`ReadError` gains an empty `std::error::Error` impl.** `thiserror` treats a field named
    `source` as the error's source, which requires one. It is additive and harmless.

- [ ] **Step 1: Write the failing core tests**

Replace the whole of `crates/tagteam-core/src/rotation.rs` with the new function's tests
first, keeping the old function so the crate still builds:

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

#[cfg(test)]
mod tests {
    use super::rotation_order;

    #[test]
    fn positions_after_the_anchor_come_first_and_wrap() {
        let positions = [1, 2, 3, 5];
        assert_eq!(rotation_order(&positions, Some(1)), [2, 3, 5]);
        assert_eq!(rotation_order(&positions, Some(3)), [5, 1, 2]);
        assert_eq!(rotation_order(&positions, Some(5)), [1, 2, 3]);
    }

    #[test]
    fn an_anchor_outside_the_list_still_orders_from_after_it() {
        assert_eq!(rotation_order(&[1, 3, 5], Some(4)), [5, 1, 3]);
    }

    #[test]
    fn no_anchor_takes_every_position_from_the_first() {
        assert_eq!(rotation_order(&[4, 2], None), [2, 4]);
    }

    #[test]
    fn unsorted_or_repeated_input_is_tried_once_in_order() {
        assert_eq!(rotation_order(&[5, 1, 3, 1], Some(3)), [5, 1]);
    }

    #[test]
    fn only_the_anchor_yields_nothing() {
        assert!(rotation_order(&[1], Some(1)).is_empty());
        assert!(rotation_order(&[], None).is_empty());
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p tagteam-core rotation`
Expected: FAIL to compile: `unresolved import super::rotation_order`.

- [ ] **Step 3: Replace `next_in_rotation` with `rotation_order`**

In `crates/tagteam-core/src/rotation.rs`, replace the `next_in_rotation` function (keep the
tests module) with:

```rust
/// §9.3: the positions a rotation tries, in order. With an anchor, every position after it,
/// wrapping around, and never the anchor itself; with none, every position from the first.
/// The input need not be sorted, and a repeated position is tried once.
pub fn rotation_order(positions: &[u32], anchor: Option<u32>) -> Vec<u32> {
    let mut sorted = positions.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let Some(anchor) = anchor else {
        return sorted;
    };
    let (before, after): (Vec<u32>, Vec<u32>) = sorted
        .into_iter()
        .filter(|p| *p != anchor)
        .partition(|p| *p < anchor);
    after.into_iter().chain(before).collect()
}
```

In `crates/tagteam-core/src/lib.rs`, replace `pub use rotation::next_in_rotation;` with:

```rust
pub use rotation::rotation_order;
```

- [ ] **Step 4: Run the core tests**

Run: `cargo test -p tagteam-core`
Expected: PASS (5 rotation tests). `tagteam-engine` does not build yet: it still imports
`next_in_rotation`. The next steps fix it.

- [ ] **Step 5: Write the failing engine tests**

`crates/tagteam-engine/tests/rotation.rs`:

```rust
mod common;

use std::fs;
use std::sync::{Arc, Mutex};

use common::{Fx, vault_fp};
use tagteam_cc::{ItemKind, keychain_account, keychain_service};
use tagteam_core::AccountId;
use tagteam_engine::EngineError;
use tagteam_engine::switch::{SwitchOutcome, SwitchReason};
use tagteam_engine::vault::SERVICE;
use tagteam_provider::Keychain;

fn rotate(fx: &Fx) -> Result<SwitchOutcome, EngineError> {
    fx.engine.switch(fx.rotation_request(false))
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
fn a_quarantined_account_is_skipped() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.add("c@x.co", "rt-c"); // live: the walk would wrap to a
    fx.quarantine(&a, "invalid_grant", &vault_fp(&fx, &a));
    assert_eq!(rotate(&fx).unwrap().to.unwrap().id, b);
}

#[test]
fn a_quarantined_account_does_not_count_toward_two() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b"); // live
    fx.quarantine(&a, "invalid_grant", &vault_fp(&fx, &a));
    let out = rotate(&fx).unwrap();
    assert_eq!(
        (out.switched, out.reason),
        (false, SwitchReason::OnlyOneAccount)
    );
}

#[test]
fn an_account_whose_vault_holds_nothing_is_skipped() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.add("c@x.co", "rt-c"); // live
    fx.kc.delete(SERVICE, a.as_str()).unwrap();
    assert_eq!(rotate(&fx).unwrap().to.unwrap().id, b);
}

#[test]
fn a_walk_that_finds_no_credential_is_only_one_account() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b"); // live
    fx.kc.delete(SERVICE, a.as_str()).unwrap();
    let out = rotate(&fx).unwrap();
    assert_eq!(
        (out.switched, out.reason),
        (false, SwitchReason::OnlyOneAccount)
    );
}

#[test]
fn an_unreadable_vault_before_the_pick_names_the_account() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    fx.add("c@x.co", "rt-c"); // live: the walk tries a first
    fx.kc.set_unreadable(SERVICE, a.as_str(), true);
    let err = rotate(&fx).unwrap_err();
    assert!(
        matches!(&err, EngineError::UnreadableAccount { position: 1, label, .. } if label == "a@x.co"),
        "{err}"
    );
    assert_eq!(err.kind(), "unreadable");
    let msg = err.to_string();
    assert!(
        msg.contains("a@x.co") && msg.contains("position 1"),
        "{msg}"
    );
    assert_eq!(fx.live_email().as_deref(), Some("c@x.co"), "nothing switched");
}

#[test]
fn an_unreadable_vault_after_the_pick_is_never_read() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.add("c@x.co", "rt-c"); // live: the walk picks a before it reaches b
    fx.kc.set_unreadable(SERVICE, b.as_str(), true);
    assert_eq!(rotate(&fx).unwrap().to.unwrap().id, a);
}

#[test]
fn a_bare_switch_reads_only_the_vaults_it_switches_between() {
    // M-8: M1's rotation read every account's vault, twice. The walk reads only as far as its
    // pick, and the transaction only the outgoing account and the target.
    let fx = Fx::new();
    let ids: Vec<AccountId> = (0..10)
        .map(|n| fx.add(&format!("a{n}@x.co"), &format!("rt-{n}")))
        .collect();
    // live: a9, at position 10; the walk wraps to a0, at position 1.
    let reads = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen = reads.clone();
    let engine =
        fx.engine_with_vault_probe(move |key| seen.lock().unwrap().push(key.to_owned()));
    let out = engine.switch(fx.rotation_request(false)).unwrap();
    assert_eq!(out.to.unwrap().id, ids[0]);
    let reads = reads.lock().unwrap();
    for id in &ids[1..9] {
        assert!(
            !reads.iter().any(|key| key.starts_with(id.as_str())),
            "{id} was read: {reads:?}"
        );
    }
}

#[test]
fn with_no_live_login_a_quarantined_active_account_is_skipped() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live and the store's active account
    fx.quarantine(&b, "invalid_grant", &vault_fp(&fx, &b));
    log_out(&fx);
    assert_eq!(rotate(&fx).unwrap().to.unwrap().id, a);
}
```

In `crates/tagteam-engine/tests/switch.rs`, replace the whole M1 test
`an_unreadable_vault_is_reported_as_unreadable_never_as_missing` with:

```rust
#[test]
fn an_unreadable_vault_is_reported_as_unreadable_never_as_missing() {
    // §4.3: an unreadable vault item is not an absent one. A direct target says so, and a
    // rotation that meets it before its pick neither skips the account nor calls the other one
    // the only switchable account: both name the account (§9.3).
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b"); // live: b
    fx.kc.set_unreadable(SERVICE, a.as_str(), true);
    for target in [SwitchTarget::Rotation, to(&a)] {
        let err = switch(&fx, target.clone(), false).unwrap_err();
        assert!(
            matches!(&err, EngineError::UnreadableAccount { position: 1, label, .. } if label == "a@x.co"),
            "{target:?}: {err}"
        );
        assert!(!err.to_string().contains("no stored credential"), "{err}");
    }
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    fx.kc.set_unreadable(SERVICE, a.as_str(), false);
    assert_eq!(switch(&fx, to(&a), false).unwrap().to.unwrap().id, a);
}
```

In `crates/tagteam-engine/src/error.rs`, add to the `cases` vector of
`kind_is_pinned_for_every_variant`:

```rust
            (
                EngineError::UnreadableAccount {
                    position: 1,
                    label: "a".into(),
                    source: ReadError::new("k", "d"),
                },
                "unreadable",
            ),
```

- [ ] **Step 6: Run the engine tests to verify they fail**

Run: `cargo test -p tagteam-engine --test rotation`
Expected: FAIL to compile: `unresolved import tagteam_core::next_in_rotation` in `switch.rs`,
and `no variant named UnreadableAccount`.

- [ ] **Step 7: Let `ReadError` be an error source**

In `crates/tagteam-provider/src/read.rs`, after `impl fmt::Display for ReadError { … }`, add:

```rust
impl std::error::Error for ReadError {}
```

- [ ] **Step 8: Add the error variant**

In `crates/tagteam-engine/src/error.rs`, add to `EngineError`, after `Unreadable(ReadError)`:

```rust
    /// A vault that could not be read where the switch needed it: a direct target, or an
    /// account a rotation met before its pick, which could have been the pick (§9.3). It is
    /// named, never skipped and never taken for a missing credential.
    #[error("the stored credential for {label} (position {position}) is unreadable: {source}")]
    UnreadableAccount {
        position: u32,
        label: String,
        source: ReadError,
    },
```

and to `kind()`, after the `EngineError::Unreadable(_)` arm:

```rust
            EngineError::UnreadableAccount { .. } => "unreadable",
```

- [ ] **Step 9: Walk the rotation lazily**

In `crates/tagteam-engine/src/switch.rs`, change the `tagteam_core` import:

```rust
use tagteam_core::{
    AccountId, OracleVerdict, OutgoingAction, OutgoingClass, OutgoingFacts, ProviderId,
    decide_outgoing, rotation_order,
};
```

Add a free function after `login_of`:

```rust
/// A rotation candidate by the store alone (§9.3 "Reading the vault lazily"): enabled, not
/// quarantined, and with an identity. Whether its vault holds a credential is read only when
/// the walk reaches it.
fn is_candidate(row: &AccountRow) -> bool {
    !row.disabled && row.quarantine_reason.is_none() && row.identity_json.is_object()
}
```

Replace `has_login` and `is_switchable`:

```rust
    /// An identity and a non-empty vault credential. A vault that cannot be read is reported,
    /// never taken for a missing credential (§4.3).
    fn has_login(&self, row: &AccountRow) -> Result<bool, EngineError> {
        if !row.identity_json.is_object() {
            return Ok(false);
        }
        match self.vault.read(&row.id) {
            Read::Present(b) => Ok(!b.is_empty()),
            Read::Absent => Ok(false),
            Read::Unreadable(e) => Err(EngineError::Unreadable(e)),
        }
    }

    /// "Switchable": a vault credential and an identity, and not disabled (§9.3).
    fn is_switchable(&self, row: &AccountRow) -> Result<bool, EngineError> {
        Ok(!row.disabled && self.has_login(row)?)
    }
```

with:

```rust
    /// Whether `row`'s vault holds a credential. A vault that cannot be read is reported,
    /// naming the account, never taken for a missing credential (§4.3, §9.3).
    fn vault_holds_login(&self, row: &AccountRow) -> Result<bool, EngineError> {
        match self.vault.read(&row.id) {
            Read::Present(b) => Ok(!b.is_empty()),
            Read::Absent => Ok(false),
            Read::Unreadable(source) => Err(EngineError::UnreadableAccount {
                position: row.position,
                label: row.label.clone(),
                source,
            }),
        }
    }

    /// An identity and a non-empty vault credential.
    fn has_login(&self, row: &AccountRow) -> Result<bool, EngineError> {
        Ok(row.identity_json.is_object() && self.vault_holds_login(row)?)
    }
```

Replace the whole `rotation` method (its doc comment included) with:

```rust
    /// §9.3 rotation, reading the vault lazily.
    ///
    /// - The candidates are counted from the store: with a managed live anchor and fewer than
    ///   two of them, it stays put (§9.2).
    /// - The walk starts after the live account when it is managed (`live_row`), even if the
    ///   store's active account disagrees (§6.1: the live identity wins). With no live login,
    ///   or an unmanaged one, it starts at the store's active account if that is a candidate,
    ///   then goes on from the first position.
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
        let accounts = store.accounts(provider)?;
        let candidates: Vec<&AccountRow> = accounts.iter().filter(|a| is_candidate(a)).collect();
        if live_row.is_some() && candidates.len() < 2 {
            return Ok(Rotation::Stay(SwitchReason::OnlyOneAccount, ONLY_ONE));
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
        for position in order {
            let Some(row) = candidates.iter().find(|a| a.position == position) else {
                continue;
            };
            if self.vault_holds_login(row)? {
                return Ok(Rotation::To((*row).clone()));
            }
        }
        Ok(match live_row {
            Some(_) => Rotation::Stay(SwitchReason::OnlyOneAccount, ONLY_ONE),
            None => Rotation::Stay(SwitchReason::NoValidTarget, "no account can be activated"),
        })
    }
```

- [ ] **Step 10: Run the tests to verify they pass**

Run: `cargo test -p tagteam-engine --test rotation --test switch`
Expected: PASS. That's 8 tests in `rotation`, plus every test in `switch`, including M1's
`rotation_skips_disabled_accounts_but_direct_targets_may_be_disabled`,
`a_bare_switch_rotates_on_from_a_managed_live_login`,
`without_a_managed_live_login_the_rotation_falls_back_to_the_store_s_active_account`,
`a_fresh_machine_activates_the_first_switchable_account` and
`the_rotation_roster_is_read_before_cc_s_locks`.

Run: `cargo test -p tagteam-engine --lib error`
Expected: PASS (`kind_is_pinned_for_every_variant`).

- [ ] **Step 11: Run the workspace suite and clippy**

Run: `cargo test --workspace --features tagteam/test-support && cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings`
Expected: every test passes, no warnings. `rg next_in_rotation crates` finds nothing.

- [ ] **Step 12: Commit**

```bash
git add crates/tagteam-core/src/rotation.rs crates/tagteam-core/src/lib.rs \
  crates/tagteam-provider/src/read.rs crates/tagteam-engine/src/switch.rs \
  crates/tagteam-engine/src/error.rs crates/tagteam-engine/tests/switch.rs \
  crates/tagteam-engine/tests/rotation.rs
git commit -m "Walk the rotation lazily, skipping quarantined accounts and naming an unreadable one"
```

---

### Task 13: Pending rescues before activation

A rescue file means the vault's current generation has already been sent to the token
endpoint (§6.3). Activating the vault alone would hand Claude Code a spent refresh token,
which CC's next refresh turns into `invalid_grant` and a wiped login. §6.2 (amended) requires
anything that activates an account's vault generation to settle that account's `rescue/`
entries first, under its account lock. §9.4 steps 2 and 5 name the switch's two places. This
task wires Task 9's `settle_rescues` into the switch transaction, before either branch reads
the target and before the journal row is written.

**Files:**
- Modify: `crates/tagteam-engine/src/switch.rs` (`transact`)
- Modify: `crates/tagteam-engine/tests/common/mod.rs` (`Fx::plant_rescue`, and the shared free
  helpers `credential`, `two_accounts`)
- Create: `crates/tagteam-engine/tests/rescue_switch.rs`

**Interfaces:**
- Consumes:
  - `Engine::settle_rescues(&self, p: &dyn Provider, row: &AccountRow, lock: &AccountLock) -> Result<(), EngineError>` (Task 9)
  - `EngineError::RescuePending { position, label, detail }`, kind `"rescue-pending"` (Task 9). Its `detail` names the rescue file's path for an unreadable file, and says the vault write failed for an adoption that failed.
  - The rescue envelope and filename of §6.3 and the Interface Contract: `rescue/<id>-<epoch>-<fp12>.json`, `{"format": "tagteam-rescue", "version": 1, "accountId", "loginEpoch", "predecessorFp", "credential"}`
- Produces:
  - `switch` never activates a target while a rescue for it is pending.
  - `Fx::plant_rescue(&self, id: &AccountId, predecessor_fp: &str, successor: &[u8]) -> PathBuf`
    (test fixture; Task 14 uses it too).
  - Shared free helpers in `tests/common/mod.rs` (Task 14 reuses them):
    `credential(email, rt) -> Vec<u8>` (`Fx::credential_json` as bytes) and
    `two_accounts(&Fx) -> AccountId` (`a` at position 1 with `rt-a`, `b` at position 2 and live
    with `rt-b`).

- [ ] **Step 1: Add the fixture helpers**

In `crates/tagteam-engine/tests/common/mod.rs`, add inside the first `impl Fx` block, after
`vault_refresh_token`:

```rust
    /// Leaves a rescue file for `id` exactly as a gate whose vault write failed would (§6.3):
    /// the envelope names the generation that was sent (`predecessor_fp`) and holds
    /// `successor` verbatim. Written directly, as another tagteam process's gate would have.
    pub fn plant_rescue(&self, id: &AccountId, predecessor_fp: &str, successor: &[u8]) -> PathBuf {
        let epoch = self
            .engine
            .store()
            .unwrap()
            .account(id)
            .unwrap()
            .expect("a rescue belongs to a stored account")
            .login_epoch;
        let dir = self.env.data_dir().join("rescue");
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        let fp = self.cc.fingerprint(successor).unwrap();
        let path = dir.join(format!("{id}-{epoch}-{}.json", fp.short12()));
        let envelope = json!({
            "format": "tagteam-rescue",
            "version": 1,
            "accountId": id.as_str(),
            "loginEpoch": epoch,
            "predecessorFp": predecessor_fp,
            "credential": String::from_utf8(successor.to_vec()).unwrap(),
        });
        fs::write(&path, envelope.to_string()).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        path
    }
```

(`fs`, `PathBuf`, `PermissionsExt` and `json!` are already imported at the top of the module.)

and add these free functions at the end of the module (Task 14's tests use them too):

```rust
/// `Fx::credential_json` as the bytes a vault stores.
pub fn credential(email: &str, rt: &str) -> Vec<u8> {
    Fx::credential_json(email, rt).to_string().into_bytes()
}

/// `a` at position 1 (`rt-a`), `b` at position 2 and live (`rt-b`).
pub fn two_accounts(fx: &Fx) -> AccountId {
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    a
}
```

- [ ] **Step 2: Write the failing tests**

`crates/tagteam-engine/tests/rescue_switch.rs`:
```rust
//! §6.2 "Pending rescues before activation": a switch settles its target's `rescue/` entries
//! under the target's account lock before it reads the target, in both branches of §9.4.

mod common;

use std::fs;
use std::thread;
use std::time::Duration;

use common::{Fx, credential, journal, two_accounts, vault_fp};
use tagteam_engine::account_lock::AccountLock;
use tagteam_engine::vault::SERVICE;
use tagteam_provider::Provider;

#[test]
fn a_rescued_successor_is_adopted_and_is_what_claude_code_receives() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let rescue = fx.plant_rescue(&a, &vault_fp(&fx, &a), &credential("a@x.co", "rt-a-2"));
    fx.switch_to(&a, false).unwrap();
    assert_eq!(
        fx.live_refresh_token().as_deref(),
        Some("rt-a-2"),
        "never the spent rt-a"
    );
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a-2"));
    let prev: serde_json::Value =
        serde_json::from_slice(&fx.kc.get(SERVICE, &format!("{a}.prev")).unwrap()).unwrap();
    assert_eq!(prev["claudeAiOauth"]["refreshToken"], "rt-a", ".prev keeps the old generation");
    assert!(!rescue.exists(), "deleted once the vault write was verified");
}

#[test]
fn a_rescue_written_while_the_switch_waits_for_the_account_lock_is_adopted() {
    // The cross-review's Busy path. Another process's gate holds the target's account lock
    // while it refreshes; its vault write fails, so it rescues the successor, then releases
    // the lock. The switch that was waiting for that lock must activate the successor, not
    // the vault's generation, which that refresh has spent.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let held = AccountLock::acquire(&fx.env, &a, Duration::from_secs(1)).unwrap();
    let predecessor = vault_fp(&fx, &a);
    let successor = credential("a@x.co", "rt-a-2");
    let (fx_ref, a_ref) = (&fx, &a);
    thread::scope(|s| {
        s.spawn(move || {
            thread::sleep(Duration::from_millis(300));
            fx_ref.plant_rescue(a_ref, &predecessor, &successor);
            drop(held);
        });
        fx.switch_to(&a, false).unwrap();
    });
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a-2"));
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a-2"));
}

#[test]
fn a_damaged_rescue_file_refuses_the_switch_and_names_the_file() {
    // Review Focus 4: a truncated file, and a hand-edited one missing its credential.
    let damaged: [&[u8]; 2] = [
        br#"{"format":"tagteam-rescue","vers"#,
        br#"{"format":"tagteam-rescue","version":1}"#,
    ];
    for bytes in damaged {
        let fx = Fx::new();
        let a = two_accounts(&fx);
        let path = fx.plant_rescue(&a, &vault_fp(&fx, &a), &credential("a@x.co", "rt-a-2"));
        fs::write(&path, bytes).unwrap();
        let err = fx.switch_to(&a, false).unwrap_err();
        assert_eq!(err.kind(), "rescue-pending", "{err}");
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(err.to_string().contains(name), "names the file: {err}");
        assert_eq!(fx.live_email().as_deref(), Some("b@x.co"), "nothing activated");
        assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-b"));
        assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
        assert!(journal(&fx).is_none(), "refused before the journal row");
        assert!(path.exists(), "a damaged rescue is never deleted");
    }
}

#[test]
fn a_rescue_whose_adoption_fails_refuses_the_switch() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let path = fx.plant_rescue(&a, &vault_fp(&fx, &a), &credential("a@x.co", "rt-a-2"));
    fx.kc.set_fail_write(SERVICE, true);
    let err = fx.switch_to(&a, false).unwrap_err();
    fx.kc.set_fail_write(SERVICE, false);
    assert_eq!(err.kind(), "rescue-pending", "{err}");
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-b"));
    assert!(journal(&fx).is_none());
    assert!(path.exists(), "kept until a verified vault write");
    // Once the vault accepts writes again, the same switch adopts and activates it.
    fx.switch_to(&a, false).unwrap();
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a-2"));
    assert!(!path.exists());
}

#[test]
fn a_superseded_rescue_does_not_block_the_switch() {
    // Its predecessor is not the vault's generation: the vault moved on (a capture of a newer
    // lineage), so this rescue is superseded. It neither blocks nor gets activated.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let older = fx.cc.fingerprint(&credential("a@x.co", "rt-a-0")).unwrap();
    let path = fx.plant_rescue(&a, older.as_str(), &credential("a@x.co", "rt-a-stale"));
    fx.switch_to(&a, false).unwrap();
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
    assert!(path.exists(), "left alone, never activated");
}

#[test]
fn the_direct_branch_settles_the_target_s_rescues_too() {
    // §9.4 step 2: an unmanaged live login, forced, takes the direct branch.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.login("stranger@x.co", "rt-s");
    fx.plant_rescue(&a, &vault_fp(&fx, &a), &credential("a@x.co", "rt-a-2"));
    fx.switch_to(&a, true).unwrap();
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a-2"));
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p tagteam-engine --test rescue_switch`
Expected: FAIL.
- `a_rescued_successor_is_adopted…`, `a_rescue_written_while…` and `the_direct_branch…` fail
  with `left: Some("rt-a")`, `right: Some("rt-a-2")`: the switch activates the vault's spent
  generation.
- `a_damaged_rescue_file…` and `a_rescue_whose_adoption_fails…` panic in `unwrap_err`, because
  the switch succeeds.
- `a_superseded_rescue_does_not_block_the_switch` passes already. It guards against the fix
  over-blocking.

- [ ] **Step 4: Settle the target's rescues in `transact`**

In `crates/tagteam-engine/src/switch.rs`, `fn transact`, the opening reads today are:

```rust
        let provider = &req.provider;
        let target_identity = p.parse_identity(&target.identity_json)?;
        let live = p.read_live_auth(&self.env);
        refuse_unsafe_live_reads(&live)?;
        let doomed = p.doomed(&self.env, locks, LiveChange::Write(&target.kind));
        refuse_unreadable(&doomed)?;

        let mut warnings = Vec::new();
```

Insert between `refuse_unreadable(&doomed)?;` and `let mut warnings`:

```rust
        // §6.2 "Pending rescues before activation": a rescue has already spent the vault's
        // generation. It is settled here, under the target's account lock and before anything
        // is written, so neither branch below reads the spent generation (§9.4 steps 2 and 5).
        // A rescue that cannot be read or adopted refuses the switch before its journal row
        // exists, so there is nothing to roll back.
        let target_lock = account_locks
            .iter()
            .find(|l| l.id() == &target.id)
            .expect("the target is locked");
        self.settle_rescues(p, &target, target_lock)?;
```

Both branches read the target through `self.read_target(&target)` after this point, so both
see the adopted successor. The direct branch reads it before comparing it with the live
bytes. The normal branch reads it after `settle_outgoing`. A self-switch's outgoing capture
(step 4) then classifies the live credential against the adopted generation, which is
correct: the adopted successor is the newest generation tagteam holds.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p tagteam-engine --test rescue_switch`
Expected: PASS (6 tests).

Run: `cargo test -p tagteam-engine`
Expected: PASS. No switch or recovery test plants a rescue, so none changes behaviour.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-engine/src/switch.rs crates/tagteam-engine/tests/common/mod.rs \
  crates/tagteam-engine/tests/rescue_switch.rs
git commit -m "Settle a target's pending rescues before a switch activates it"
```

---

### Task 14: Freshen before activation

§7.2 (amended): a target whose access token expires within 10 minutes is refreshed through the
gate (§7.3) before it is activated. This happens after the target is resolved and before the
switch takes `MutationGuard`, the last point where a manual switch may use the network (§4.3).
The manual-switch table in §7.2 decides what each gate outcome does:

| Gate outcome | Manual switch |
|---|---|
| Refreshed / AlreadyFresh | Activate the new generation |
| Dead | Quarantined by the gate. A direct switch refuses (`relogin-required`); a rotation plans again, and the walk skips it (§9.3) |
| `Busy` | Proceed. The switch waits for the account lock; Task 13's pending-rescue settle and the locked vault read pick up the other refresh |
| Transient (not rescued, not `rescue-unreadable`), Systemic | Proceed with the vault's generation and a warning |
| Transient with `rescued: true`, `rescue-unreadable` | Refuse (`rescue-pending`): the successor is in `rescue/`, and the next pass adopts it |
| `Unpersisted` | Refuse (`relogin-required`): the successor is lost and the vault's generation is consumed, so only a new login brings the account back |
| `Owned`, `Conflict` | The matching §9.2 refusal |

A quarantined target is never refreshed (§7.4). One that would need freshening is refused like
Dead. Otherwise it is activated with a warning. The same rule applies to a target quarantined
*while this switch waited for its account lock*: a concurrent refresh that lost its successor
quarantines it `successor_lost` (§7.3 step 6, §7.4). §9.4 step 1 (amended) re-reads the
target's quarantine under the locks, so the switch never activates the generation that refresh
spent.

**Readings of the spec this task commits to:**
- **`--force` still freshens.** §7.2 makes no exception for it, and a forced switch still
  activates the target's vault generation. `--force` only overrides whose *live* credential
  is displaced (§9.2, §9.4 step 3). Under `--force`, `Owned(Journal)` (the target is named by
  the undecidable row the forced switch is about to supersede) proceeds with a warning instead
  of refusing.
- **A self-switch never freshens.** The account is live, so the gate would return
  `Owned(Live)` (§7.3 step 2): only CC, or §7.5, refreshes the live token.
- **A kind whose `kind_traits().refreshable` is false never freshens.** Examples are setup
  tokens and API keys (§7.1: no refresh token, no expiry).
- **No freshen when planning again under the mutation lock.** The mutation lock is contended,
  so no network may be used there (§4.3). A pick that changes under the lock is activated with
  the vault's generation, and CC refreshes it.

**Files:**
- Modify: `crates/tagteam-engine/src/switch.rs` (`Plan.warnings`, `freshen_plan`, `freshen`,
  `switch`, `rederive`, `Locked.warnings`, `transact`)
- Modify: `crates/tagteam-engine/src/error.rs` (`EngineError::NeedsRelogin` and its kind)
- Modify: `crates/tagteam-engine/tests/common/mod.rs` (the shared free helper `rescue_files`)
- Create: `crates/tagteam-engine/tests/freshen.rs`

**Interfaces:**
- Consumes:
  - `Engine::refresh_stored(&self, p: &dyn Provider, id: &AccountId, snapshot: &[u8]) -> Result<GateOutcome, EngineError>`, plus `GateOutcome` and `OwnedBy` (Tasks 10–11, `crate::refresh`)
  - `Engine::rescues_for(&self, id) -> Vec<RescueFile>` and `RescueFile::Unreadable { path, .. }` (Task 9, `crate::rescue`)
  - `EngineError::RescuePending` (Task 9)
  - `Provider::kind_traits` and `Provider::access_expires_at` (Task 4)
  - The Task 12 rotation walk, which skips quarantined accounts
  - `AccountRow::quarantine_reason`
  - Fixture: `Fx::expire_access`, `Fx::script_refresh`, `Fx::script_token_error`, `Fx::endpoints()`, `Fx::quarantine`, `Fx.http: Arc<ScriptedHttp>` (Tasks 3, 7–10), `Fx::plant_rescue` (Task 13)
  - Shared free helpers in `tests/common/mod.rs`: `token_requests`, `quarantine_of` (Task 10),
    `block_rescue`, `unblock_rescue` (Task 11), `credential`, `two_accounts` (Task 13)
- Produces:
  - `rescue_files(&Fx) -> usize` in `tests/common/mod.rs` (a free `pub fn`, the number of files
    in `rescue/`; Task 16 reuses it)
  - `EngineError::NeedsRelogin { position: u32, label: String }`, kind `"relogin-required"`
  - `SwitchOutcome.warnings` carries freshen warnings, each worded exactly:
    - `could not refresh <label> first (<kind or detail>); Claude Code will refresh it when it is online`
    - `<label> (position <N>) needs a new login: its stored refresh token can no longer be used; it works only until its current access token expires`
  - `const FRESHEN_WINDOW_MS: i64 = 600_000` and `fn due(&self, p, vault: &[u8]) -> bool`
    (both private to `switch.rs`; `freshen` and `rederive` share `due`)

- [ ] **Step 1: Pin the new error kind (failing)**

In `crates/tagteam-engine/src/error.rs`, `kind_is_pinned_for_every_variant`, add to `cases`
after the `IdentityConflict` entry:

```rust
            (
                EngineError::NeedsRelogin {
                    position: 1,
                    label: "a@b.co".into(),
                },
                "relogin-required",
            ),
```

- [ ] **Step 2: Write the failing behaviour tests**

First add this free function at the end of `crates/tagteam-engine/tests/common/mod.rs` (Task 16's
tests use it too):

```rust
/// How many files `rescue/` holds; 0 when it does not exist.
pub fn rescue_files(fx: &Fx) -> usize {
    fs::read_dir(fx.env.data_dir().join("rescue")).map_or(0, |d| d.count())
}
```

`crates/tagteam-engine/tests/freshen.rs`:
```rust
//! §7.2 freshen before activation, through a manual `switch`: every row of the manual-switch
//! table, quarantined targets, and when no request may be made at all.

mod common;

use std::fs;
use std::thread;
use std::time::Duration;

use common::{
    API_KEY, Fx, block_rescue, credential, journal, quarantine_of, rescue_files, token_requests,
    two_accounts, unblock_rescue, vault_fp,
};
use serde_json::json;
use tagteam_engine::account_lock::AccountLock;
use tagteam_engine::vault::SERVICE;
use tagteam_provider::http::{HttpError, Method};

fn cannot_refresh(why: &str) -> String {
    format!("could not refresh a@x.co first ({why}); Claude Code will refresh it when it is online")
}

#[test]
fn an_expiring_target_is_refreshed_before_it_is_activated() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.expire_access(&a);
    fx.script_refresh(Some("rt-a-2"));
    let out = fx.switch_to(&a, false).unwrap();
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    assert_eq!(token_requests(&fx), 1);
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a-2"));
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a-2"));
}

#[test]
fn no_request_is_made_outside_the_window_for_a_self_switch_or_an_unrefreshable_kind() {
    // Outside the 10-minute window.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.switch_to(&a, false).unwrap();
    // A self-switch: a is live now, and only CC (or §7.5) refreshes the live token.
    fx.expire_access(&a);
    fx.switch_to(&a, false).unwrap();
    // An API key has no refresh token and no expiry (§7.1).
    let k = fx.add_api_key(API_KEY);
    fx.switch_to(&k, false).unwrap();
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn a_dead_direct_target_is_quarantined_and_refused() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.expire_access(&a);
    fx.script_token_error(400, "invalid_grant");
    let err = fx.switch_to(&a, false).unwrap_err();
    assert_eq!(err.kind(), "relogin-required", "{err}");
    assert_eq!(
        err.to_string(),
        "a@x.co (position 1) needs a new login: its stored refresh token can no longer be used; log in with `claude`, then run `tagteam add`"
    );
    assert_eq!(quarantine_of(&fx, &a).0.as_deref(), Some("invalid_grant"));
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"), "nothing activated");
    assert!(journal(&fx).is_none());
    assert!(fx.displaced().is_empty());
}

#[test]
fn a_rotation_whose_pick_turns_out_dead_moves_on_to_the_next_account() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.add("c@x.co", "rt-c"); // live: c, so the rotation's pick is a (it wraps)
    fx.expire_access(&a);
    fx.script_token_error(400, "invalid_grant");
    let out = fx.engine.switch(fx.rotation_request(false)).unwrap();
    assert_eq!(out.to.as_ref().map(|r| &r.id), Some(&b));
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    assert_eq!(quarantine_of(&fx, &a).0.as_deref(), Some("invalid_grant"));
    assert_eq!(token_requests(&fx), 1, "b was not in the window");
}

#[test]
fn a_busy_gate_lets_the_switch_wait_for_the_other_refresh() {
    // The cross-review's Busy path. Another process's gate holds a's account lock, spends
    // rt-a, cannot write the vault and rescues rt-a-2. This switch sends nothing, waits for
    // the lock, and activates rt-a-2.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.expire_access(&a);
    let held = AccountLock::acquire(&fx.env, &a, Duration::from_secs(1)).unwrap();
    let predecessor = vault_fp(&fx, &a);
    let successor = credential("a@x.co", "rt-a-2");
    let (fx_ref, a_ref) = (&fx, &a);
    let out = thread::scope(|s| {
        s.spawn(move || {
            thread::sleep(Duration::from_millis(300));
            fx_ref.plant_rescue(a_ref, &predecessor, &successor);
            drop(held);
        });
        fx.switch_to(&a, false).unwrap()
    });
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    assert_eq!(token_requests(&fx), 0, "single-flight: the other refresh was the one");
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a-2"));
}

#[test]
fn a_target_quarantined_while_the_switch_waits_is_never_activated_spent() {
    // Codex round 1: another process's gate holds a's account lock, spends rt-a, and can store
    // the successor nowhere (§7.3 `Unpersisted`), so it quarantines a `successor_lost`. This
    // switch got `Busy`, waited for the lock, and must not activate the spent rt-a: §9.4 step 1
    // re-reads the quarantine under the lock and applies §7.2's quarantined-target rule.
    for due in [true, false] {
        let fx = Fx::new();
        let a = two_accounts(&fx);
        if due {
            fx.expire_access(&a);
        }
        let held = AccountLock::acquire(&fx.env, &a, Duration::from_secs(1)).unwrap();
        let spent = vault_fp(&fx, &a);
        let (fx_ref, a_ref) = (&fx, &a);
        let result = thread::scope(|s| {
            s.spawn(move || {
                thread::sleep(Duration::from_millis(300));
                fx_ref.quarantine(a_ref, "successor_lost", &spent);
                drop(held);
            });
            fx.switch_to(&a, false)
        });
        assert_eq!(token_requests(&fx), 0, "due: {due}");
        if due {
            let err = result.unwrap_err();
            assert_eq!(err.kind(), "relogin-required", "{err}");
            assert_eq!(fx.live_email().as_deref(), Some("b@x.co"), "the spent rt-a is never activated");
        } else {
            // Its access token still works: activated, with §7.2's warning.
            let out = result.unwrap();
            assert_eq!(
                out.warnings,
                ["a@x.co (position 1) needs a new login: its stored refresh token can no longer be used; it works only until its current access token expires"]
            );
            assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
        }
    }
}

#[test]
fn offline_the_switch_proceeds_with_the_vault_generation_and_a_warning() {
    // Review Focus 1, and the other transient kinds.
    let replies: [(Result<(), HttpError>, &str); 3] = [
        (Err(HttpError::PreSend("dns lookup failed".into())), "pre-send"),
        (Err(HttpError::Ambiguous("connection reset".into())), "ambiguous"),
        (Ok(()), "http-500"),
    ];
    for (reply, kind) in replies {
        let fx = Fx::new();
        let a = two_accounts(&fx);
        fx.expire_access(&a);
        let token = Fx::endpoints().token;
        match reply {
            Err(e) => fx.http.push(Method::Post, &token, Err(e)),
            Ok(()) => fx.http.push_json(Method::Post, &token, 500, json!({"error": "overloaded"})),
        }
        let out = fx.switch_to(&a, false).unwrap();
        assert_eq!(out.warnings, [cannot_refresh(kind)], "{kind}");
        assert_eq!(token_requests(&fx), 1, "{kind}");
        assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"), "{kind}");
        assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"), "{kind}");
        assert_eq!(quarantine_of(&fx, &a).0, None, "{kind}");
        assert_eq!(rescue_files(&fx), 0, "no successor was received: {kind}");
    }
}

#[test]
fn a_systemic_refusal_is_never_a_strike_and_the_switch_proceeds() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.expire_access(&a);
    fx.script_token_error(400, "invalid_client");
    let out = fx.switch_to(&a, false).unwrap();
    assert_eq!(out.warnings.len(), 1, "{:?}", out.warnings);
    assert!(out.warnings[0].starts_with("could not refresh a@x.co first ("));
    assert!(out.warnings[0].ends_with("); Claude Code will refresh it when it is online"));
    assert_eq!(quarantine_of(&fx, &a).0, None);
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
}

#[test]
fn a_rescued_successor_refuses_the_switch_until_the_vault_takes_it() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.expire_access(&a);
    fx.script_refresh(Some("rt-a-2"));
    fx.kc.set_fail_write(SERVICE, true);
    let err = fx.switch_to(&a, false).unwrap_err();
    fx.kc.set_fail_write(SERVICE, false);
    assert_eq!(err.kind(), "rescue-pending", "{err}");
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"), "the spent rt-a is never activated");
    assert_eq!(rescue_files(&fx), 1);
    // Next time, the gate adopts the rescue (§7.3 step 3) and needs no request.
    fx.switch_to(&a, false).unwrap();
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a-2"));
    assert_eq!(token_requests(&fx), 1);
}

#[test]
fn an_unpersisted_successor_refuses_the_switch_and_asks_for_a_new_login() {
    // The successor is lost and the vault's generation is spent (§7.3 step 6): retrying
    // cannot help, and activating would hand CC a used refresh token.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.expire_access(&a);
    fx.script_refresh(Some("rt-a-2"));
    fx.kc.set_fail_write(SERVICE, true);
    // `rescue/` lists fine but cannot be written to (0500), so the gate still sends the
    // request, then has nowhere to keep the successor.
    block_rescue(&fx);
    let err = fx.switch_to(&a, false).unwrap_err();
    fx.kc.set_fail_write(SERVICE, false);
    unblock_rescue(&fx);
    assert_eq!(err.kind(), "relogin-required", "{err}");
    assert!(err.to_string().contains("can no longer be used"), "{err}");
    assert_eq!(token_requests(&fx), 1, "the request was sent");
    assert_eq!(
        quarantine_of(&fx, &a).0.as_deref(),
        Some("successor_lost"),
        "the spent generation is quarantined (§7.4)"
    );
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
}

#[test]
fn an_unreadable_rescue_refuses_without_a_request_and_names_the_file() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.expire_access(&a);
    let path = fx.plant_rescue(&a, &vault_fp(&fx, &a), &credential("a@x.co", "rt-a-2"));
    fs::write(&path, b"{\"format\":\"tagteam-res").unwrap();
    let err = fx.switch_to(&a, false).unwrap_err();
    assert_eq!(err.kind(), "rescue-pending", "{err}");
    assert!(
        err.to_string().contains(path.file_name().unwrap().to_str().unwrap()),
        "{err}"
    );
    assert_eq!(token_requests(&fx), 0);
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
}

#[test]
fn a_rescue_the_vault_cannot_adopt_refuses_with_a_detail_that_says_so() {
    // The gate reports a failed adoption of a readable rescue as `rescue-unreadable` too
    // (Task 10), but no file is unreadable, so the refusal must not name an empty list.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.expire_access(&a);
    fx.plant_rescue(&a, &vault_fp(&fx, &a), &credential("a@x.co", "rt-a-2"));
    fx.kc.set_fail_write(SERVICE, true);
    let err = fx.switch_to(&a, false).unwrap_err();
    fx.kc.set_fail_write(SERVICE, false);
    assert_eq!(err.kind(), "rescue-pending", "{err}");
    let shown = err.to_string();
    assert!(shown.contains("a pending rescue could not be adopted"), "{shown}");
    assert!(shown.ends_with("retry once the vault can be written"), "{shown}");
    assert!(!shown.contains(" cannot be read"), "{shown}");
    assert_eq!(token_requests(&fx), 0);
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    // Once the vault can be written, the next switch adopts the rescue and activates it.
    fx.switch_to(&a, false).unwrap();
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a-2"));
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn a_quarantined_target_works_until_its_access_token_expires() {
    // Outside the window: activated, with a warning.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.quarantine(&a, "invalid_grant", &vault_fp(&fx, &a));
    let out = fx.switch_to(&a, false).unwrap();
    assert_eq!(
        out.warnings,
        ["a@x.co (position 1) needs a new login: its stored refresh token can no longer be used; it works only until its current access token expires"]
    );
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));

    // Inside the window: refused, and never refreshed (§7.4).
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.quarantine(&a, "invalid_grant", &vault_fp(&fx, &a));
    fx.expire_access(&a);
    let err = fx.switch_to(&a, false).unwrap_err();
    assert_eq!(err.kind(), "relogin-required", "{err}");
    assert_eq!(token_requests(&fx), 0);
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
}

#[test]
fn a_forced_switch_still_freshens_its_target() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.login("stranger@x.co", "rt-s"); // unmanaged: --force takes the direct branch
    fx.expire_access(&a);
    fx.script_refresh(Some("rt-a-2"));
    fx.switch_to(&a, true).unwrap();
    assert_eq!(token_requests(&fx), 1);
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a-2"));
}
```

`AlreadyFresh` needs no switch-level test of its own. The switch treats it exactly like
Refreshed, and `a_rescued_successor_refuses_the_switch_until_the_vault_takes_it` reaches it
through the gate's step 3 adoption followed by step 4. Tasks 10–11 test the gate's step 4
directly.

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p tagteam-engine --lib error && cargo test -p tagteam-engine --test freshen`
Expected: FAIL. `error.rs` does not compile: `no variant named NeedsRelogin`. Once Step 4's
variant exists, `freshen.rs` fails in:
- `an_expiring_target_is_refreshed…` and `a_forced_switch_still_freshens…`:
  `left: 0, right: 1` token requests.
- `a_dead_direct_target…`, `a_rescued_successor…` and `an_unpersisted…`: `unwrap_err` on an `Ok`.
- `a_rotation_whose_pick_turns_out_dead…`: lands on a, not b.
- `offline…` and `a_systemic…`: no warning.
- `a_quarantined_target…`: no warning, and the second half succeeds.
- `a_rescue_the_vault_cannot_adopt…`: the refusal's detail names the file and the failed
  adoption, not the phrase "a pending rescue could not be adopted".

`no_request_is_made_outside_the_window…`, `a_busy_gate…` and
`an_unreadable_rescue_refuses_without_a_request_and_names_the_file` pass already. The first two
pin that the fix adds no request. The third passes because Task 13's pending-rescue settle
already refuses with `rescue-pending`, names the file and sends nothing.
`a_target_quarantined_while_the_switch_waits…` fails in both halves: the switch activates a
without re-reading its quarantine.

- [ ] **Step 4: Add `EngineError::NeedsRelogin`**

In `crates/tagteam-engine/src/error.rs`, add after `IdentityConflict { label: String },`:

```rust
    /// A target whose stored refresh token can no longer be used: rejected (§7.4, it is
    /// quarantined), or spent by a refresh whose successor could be stored nowhere (§7.3
    /// step 6, `Unpersisted`). Only a new login brings it back.
    #[error(
        "{label} (position {position}) needs a new login: its stored refresh token can no longer be used; log in with `claude`, then run `tagteam add`"
    )]
    NeedsRelogin { position: u32, label: String },
```

and in `kind()` after the `IdentityConflict` arm:

```rust
            EngineError::NeedsRelogin { .. } => "relogin-required",
```

- [ ] **Step 5: Carry warnings in the plan**

In `crates/tagteam-engine/src/switch.rs`:

1. Add to the imports:

```rust
use crate::refresh::{GateOutcome, OwnedBy};
use crate::rescue::RescueFile;
```

2. Below `const ATTEMPTS: usize = 3;` add:

```rust
/// §7.2: a target whose access token expires within this many milliseconds is refreshed
/// before it is activated. Twice CC's own 5-minute buffer.
const FRESHEN_WINDOW_MS: i64 = 10 * 60 * 1000;
```

3. `struct Plan` gains a field:

```rust
struct Plan {
    target: AccountRow,
    strategy: &'static str,
    self_switch: bool,
    hint: Option<OracleHint>,
    /// What freshening the target decided to tell the user (§7.2), carried into the outcome.
    warnings: Vec<String>,
}
```

4. In `fn plan`, the final literal becomes:

```rust
        Ok(Planned::Go(Plan {
            target,
            strategy,
            self_switch,
            hint,
            warnings: vec![],
        }))
```

5. In `fn transact`, replace `let mut warnings = Vec::new();` with:

```rust
        let mut warnings = plan.warnings.clone();
```

- [ ] **Step 6: Freshen the plan's target**

In `crates/tagteam-engine/src/switch.rs`, add below `fn already_active`:

```rust
/// What freshening a plan's target decided (§7.2, the manual-switch table).
enum Freshened {
    /// Go ahead, with these warnings for the outcome.
    Go(Vec<String>),
    /// A rotation's pick turned out dead and is quarantined now: plan again; the walk skips
    /// it (§9.3).
    Replan,
}

fn needs_relogin(target: &AccountRow) -> EngineError {
    EngineError::NeedsRelogin {
        position: target.position,
        label: target.label.clone(),
    }
}

fn cannot_refresh(label: &str, why: &str) -> String {
    format!("could not refresh {label} first ({why}); Claude Code will refresh it when it is online")
}

fn works_until_expiry(target: &AccountRow) -> String {
    format!(
        "{} (position {}) needs a new login: its stored refresh token can no longer be used; it works only until its current access token expires",
        target.label, target.position
    )
}
```

In `impl Engine`, add after `fn plan`:

```rust
    /// §7.2: refreshes the plan's target through the gate when its access token is about to
    /// expire. This runs before any lock is taken (§4.3). A rotation whose pick turns out dead
    /// plans again from the current roster, which now skips it; every round quarantines one
    /// more account, so the rounds are bounded by the roster.
    fn freshen_plan(
        &self,
        p: &dyn Provider,
        store: &Store,
        req: &SwitchRequest,
        mut plan: Plan,
    ) -> Result<Planned, EngineError> {
        for _ in 0..=store.accounts(&req.provider)?.len() {
            match self.freshen(p, req, &plan)? {
                Freshened::Go(warnings) => {
                    plan.warnings.extend(warnings);
                    return Ok(Planned::Go(plan));
                }
                Freshened::Replan => {
                    plan = match self.plan(p, store, req, Ask::Reuse(plan.hint.take()))? {
                        Planned::Done(outcome) => return Ok(Planned::Done(outcome)),
                        Planned::Go(next) => next,
                    };
                }
            }
        }
        Err(EngineError::InvalidInput(
            "no account in the rotation could be refreshed".into(),
        ))
    }

    /// §7.2: whether `vault`'s access token expires within the freshen window. An unknown or
    /// non-numeric expiry is never due.
    fn due(&self, p: &dyn Provider, vault: &[u8]) -> bool {
        p.access_expires_at(vault)
            .is_some_and(|at| self.now_ms() + FRESHEN_WINDOW_MS >= at)
    }

    /// One row of §7.2's manual-switch table, for the plan's target.
    fn freshen(
        &self,
        p: &dyn Provider,
        req: &SwitchRequest,
        plan: &Plan,
    ) -> Result<Freshened, EngineError> {
        let target = &plan.target;
        // A self-switch activates what is already live: only CC, or §7.5, refreshes that token.
        if plan.self_switch || !p.kind_traits(&target.kind).refreshable {
            return Ok(Freshened::Go(vec![]));
        }
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
            // live, the gate leaves its refresh to CC (§7.3 step 2).
            GateOutcome::Transient { kind, .. } => Freshened::Go(vec![cannot_refresh(label, &kind)]),
            GateOutcome::Systemic(detail) => Freshened::Go(vec![cannot_refresh(label, &detail)]),
            // The successor is lost and the vault's generation is spent (§7.3 step 6): no
            // retry helps, and activating would hand CC a used refresh token.
            GateOutcome::Unpersisted => return Err(needs_relogin(target)),
            // The forced switch is about to supersede the undecidable row that names it.
            GateOutcome::Owned(OwnedBy::Journal) if req.force => {
                Freshened::Go(vec![cannot_refresh(label, "an interrupted switch names it")])
            }
            GateOutcome::Owned(OwnedBy::Journal) => {
                return Err(EngineError::InterruptedSwitch(req.provider.to_string()));
            }
            // It became the live login meanwhile: `rederive` plans the self-switch again.
            GateOutcome::Owned(OwnedBy::Live) => Freshened::Go(vec![]),
            // Unreachable before M4, which introduces sessions and provenance. M4 gives these
            // two the spec's `session-owned` and `profile-conflict` kinds (§7.2's table); until
            // then they refuse with `invalid-input`.
            GateOutcome::Owned(OwnedBy::Session) => {
                return Err(EngineError::InvalidInput(format!(
                    "{label} is in use by a `tagteam run` session; exit it first"
                )));
            }
            GateOutcome::Conflict => {
                return Err(EngineError::InvalidInput(format!(
                    "{label}'s session profile holds a login that conflicts with the vault; refusing to activate it"
                )));
            }
        })
    }
```

- [ ] **Step 7: Call it from `switch`, and carry warnings through a re-plan**

In `fn switch`, the opening plan today is:

```rust
        let mut plan = match self.plan(p, &store, &req, Ask::Oracle)? {
            Planned::Done(outcome) => return Ok(outcome),
            Planned::Go(plan) => plan,
        };
```

Replace it with:

```rust
        // §7.2: before the mutation lock, the only place a manual switch may use the network.
        let mut plan = match self.plan(p, &store, &req, Ask::Oracle)? {
            Planned::Done(outcome) => return Ok(outcome),
            Planned::Go(plan) => match self.freshen_plan(p, &store, &req, plan)? {
                Planned::Done(outcome) => return Ok(outcome),
                Planned::Go(plan) => plan,
            },
        };
```

In the attempt loop, the re-plan today is:

```rust
            if attempt > 1 {
                plan = match self.plan(p, &store, &req, Ask::Reuse(plan.hint.take()))? {
                    Planned::Done(outcome) => return Ok(outcome),
                    Planned::Go(plan) => plan,
                };
            }
```

Replace it with:

```rust
            if attempt > 1 {
                // No freshen here: this runs under the mutation lock, where no network is
                // allowed (§4.3). A pick that changed is activated with the vault's
                // generation, and CC refreshes it.
                let warnings = std::mem::take(&mut plan.warnings);
                plan = match self.plan(p, &store, &req, Ask::Reuse(plan.hint.take()))? {
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
```

and the `Rederived::Done` arm becomes:

```rust
                Rederived::Done(mut outcome) => {
                    outcome.warnings.extend(plan.warnings.clone());
                    return Ok(outcome);
                }
```

- [ ] **Step 8: Re-read the target's quarantine under the locks (§9.4 step 1, amended)**

A refresh that finished while this switch waited for the target's account lock may have
quarantined it (§7.4 `successor_lost`). `rederive` runs under every lock, so it re-reads the
row there. A target quarantined since planning follows §7.2's rule: a rotation plans again
(the walk skips it); a direct target is refused when it would need freshening, and otherwise
activated with the warning. No request is made here: the mutation lock is held (§4.3).

In `crates/tagteam-engine/src/switch.rs`, `struct Locked` gains a field:

```rust
/// The live login and the rows the plan was made for, re-read under every lock.
struct Locked {
    live_identity: Option<Identity>,
    target: AccountRow,
    outgoing: Option<AccountRow>,
    /// What the locked re-read decided to tell the user (§9.4 step 1).
    warnings: Vec<String>,
}
```

In `fn rederive`, after

```rust
        let Some(target) = store.account(&plan.target.id)? else {
            return Ok(Rederived::Replan);
        };
```

insert:

```rust
        // §9.4 step 1 (amended): a refresh that finished while this switch waited for the
        // target's account lock may have quarantined it (§7.4 `successor_lost`). A target
        // quarantined since planning follows §7.2's quarantined-target rule: a rotation plans
        // again, and the walk skips it.
        let mut warnings = Vec::new();
        if target.quarantine_reason.is_some() && plan.target.quarantine_reason.is_none() {
            if matches!(req.target, SwitchTarget::Rotation) {
                return Ok(Rederived::Replan);
            }
            let vault = self.read_target(&target)?;
            if self.due(p, &vault) {
                return Err(needs_relogin(&target));
            }
            warnings.push(works_until_expiry(&target));
        }
```

and the `Rederived::Go` literal at its end becomes:

```rust
            Rederived::Go(Locked {
                live_identity,
                target,
                outgoing: again,
                warnings,
            })
```

In `fn transact`, the destructuring becomes

```rust
        let Locked {
            live_identity,
            target,
            outgoing,
            warnings: locked_warnings,
        } = locked;
```

and the line Step 5 wrote, `let mut warnings = plan.warnings.clone();`, becomes:

```rust
        let mut warnings = plan.warnings.clone();
        warnings.extend(locked_warnings);
```

- [ ] **Step 9: Run the tests to verify they pass**

Run: `cargo test -p tagteam-engine --lib error && cargo test -p tagteam-engine --test freshen`
Expected: PASS (14 tests in `freshen.rs`).

Run: `cargo test -p tagteam-engine`
Expected: PASS. The other suites' accounts carry `expiresAt: 1_790_003_600_000`, an hour after
the fixture clock, so nothing enters the freshen window and no suite gains a request.

Run: `cargo clippy -p tagteam-engine --all-targets --features test-hooks -- -D warnings`
Expected: no warnings.

- [ ] **Step 10: Commit**

```bash
git add crates/tagteam-engine/src/switch.rs crates/tagteam-engine/src/error.rs \
  crates/tagteam-engine/tests/common/mod.rs crates/tagteam-engine/tests/freshen.rs
git commit -m "Refresh an expiring switch target through the gate before activating it"
```

---

### Task 15: Recovery: forward capture and identity-key comparison

This task implements two amendments to §9.6.

**The forward capture (ruling L476).** A forward finish clears the auth axis the target does
not use. When an entry there holds a generation other than the journaled `from_fp`, it is
classified as §9.4 step 4 would classify it, but without step 4's `Unresolved` capture:
- If the pre-lock oracle resolved exactly those bytes to `from_id`, the entry is written to
  `from_id`'s vault, under the account lock recovery already holds. §6.2 bounds that write as
  it bounds `OursRotated`, and `account_uuid` is backfilled.
- Otherwise it is displaced, as M1 does today.

**Identity by key.** Backward recovery, and the final coherence check, compare identities by
identity key. An `oauthAccount` object that CC updated for the same identity is kept, and is
no longer overwritten by the journaled copy.

**Files:**
- Modify: `crates/tagteam-engine/src/recover.rs` (`recover_one`, `finish_forward`,
  `finish_backward`, `surfaces_agree`, new `capture_rotated_outgoing`)
- Modify: `crates/tagteam-engine/src/switch.rs` (`Held::contains`)
- Modify: `crates/tagteam-engine/tests/recover.rs`
- Modify: `crates/tagteam-engine/tests/common/mod.rs` (the shared free helper `prev_refresh_token`)

**Interfaces:**
- Consumes:
  - `Engine::persist_generation(&self, p: &dyn Provider, row: &AccountRow, lock: &AccountLock, bytes: &[u8]) -> Result<(), EngineError>` (Task 9)
  - `HttpOracle::new(http: Arc<dyn Http>, clock: Arc<dyn Clock>)` (Task 7)
  - `Engine::metadata_guard` (Task 7): metadata commands recover without the oracle
  - `Fx::script_profile`, `Fx::endpoints()`, `Fx.http` (Tasks 3, 7)
  - `core::decide_outgoing` and `OutgoingFacts` (M1)
- Produces:
  - `prev_refresh_token(&Fx, &AccountId) -> Option<String>` in `tests/common/mod.rs` (a free
    `pub fn`; Task 16 reuses it)
  - `Held::contains(&self, fp: &str) -> bool` (`pub(crate)`, `switch.rs`)
  - Forward recovery captures an attributed rotation of the outgoing account's token.
  - Backward recovery keeps a same-identity `oauthAccount`.

- [ ] **Step 1: Write the failing tests**

First add this free function at the end of `crates/tagteam-engine/tests/common/mod.rs` (Task 16's
tests use it too), and add `prev_refresh_token` to `recover.rs`'s `use common::{…}` list:

```rust
/// The refresh token inside `id`'s `.prev` vault generation, if there is one.
pub fn prev_refresh_token(fx: &Fx, id: &AccountId) -> Option<String> {
    let v: Value = serde_json::from_slice(&fx.kc.get(SERVICE, &format!("{id}.prev"))?).ok()?;
    v["claudeAiOauth"]["refreshToken"].as_str().map(str::to_owned)
}
```

Append to `crates/tagteam-engine/tests/recover.rs`. Add `use std::sync::Arc;`,
`use tagteam_engine::oracle::HttpOracle;` and `use tagteam_provider::http::Method;` to its
imports. `oracle_says`, `any_mutation` and `active` already exist in this file. Task 7 made
`any_mutation` take the mutation lock through `mutation_guard`, so it recovers as an
account-changing command does: the oracle is asked first (§7.6).

```rust
/// OAuth `a` → API key `k`, killed after step 7 stored the key and before the credential entry
/// was cleared; then CC rotated a's token. Returns `(a, k)`.
fn crashed_cross_axis_switch_then_cc_rotated(fx: &Fx) -> (AccountId, AccountId) {
    let a = fx.add("a@x.co", "rt-a");
    let k = fx.add_api_key(API_KEY); // leaves a live
    crashed_switch(fx, &a, &k);
    fx.put_managed_key(API_KEY.as_bytes());
    fx.rotate_live("rt-a-rotated-by-cc");
    (a, k)
}

#[test]
fn forward_recovery_captures_a_rotated_outgoing_token_the_oracle_attributes() {
    // Ruling L476: the rotated generation is a's newest. Capturing it keeps a usable;
    // displacing it would leave a's vault holding the spent rt-a.
    let fx = Fx::new();
    let (a, k) = crashed_cross_axis_switch_then_cc_rotated(&fx);
    oracle_says(&fx, "a@x.co");
    any_mutation(&fx, &a);
    assert_journal_cleared(&fx);
    assert_eq!(active(&fx), Some(k));
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a-rotated-by-cc"));
    assert_eq!(prev_refresh_token(&fx, &a).as_deref(), Some("rt-a"), ".prev keeps the old one");
    assert!(fx.displaced().is_empty(), "captured, so nothing to displace");
    assert_eq!(fx.live_refresh_token(), None, "the entry is still cleared for the API key");
}

#[test]
fn a_captured_rotation_backfills_a_missing_account_uuid() {
    let fx = Fx::new();
    let (a, _) = crashed_cross_axis_switch_then_cc_rotated(&fx);
    // No uuid recorded yet: attribution falls back to email and org (§7.6, oracle.rs).
    rusqlite::Connection::open(fx.env.data_dir().join("tagteam.db"))
        .unwrap()
        .execute(
            "UPDATE accounts SET account_uuid = NULL WHERE id = ?1",
            [a.as_str()],
        )
        .unwrap();
    oracle_says(&fx, "a@x.co");
    any_mutation(&fx, &a);
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a-rotated-by-cc"));
    let row = fx.engine.store().unwrap().account(&a).unwrap().unwrap();
    assert_eq!(row.account_uuid.as_deref(), Some("uuid-a@x.co"));
}

#[test]
fn forward_recovery_without_an_attribution_displaces_the_rotated_token() {
    // No answer, and an answer naming someone else: displaced, as before the amendment.
    // Step 4's Unresolved capture does not apply to recovery.
    for answer in [None, Some("stranger@x.co")] {
        let fx = Fx::new();
        let (a, _) = crashed_cross_axis_switch_then_cc_rotated(&fx);
        if let Some(email) = answer {
            oracle_says(&fx, email);
        }
        any_mutation(&fx, &a);
        assert_journal_cleared(&fx);
        assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"), "{answer:?}");
        let displaced = fx.displaced();
        assert_eq!(displaced.len(), 1, "{answer:?}");
        assert!(String::from_utf8_lossy(&displaced[0]).contains("rt-a-rotated-by-cc"));
    }
}

#[test]
fn an_attributed_token_without_a_refresh_token_never_replaces_a_complete_vault() {
    // §6.2: an automatic capture never replaces a refresh token with a credential that lacks
    // one. Such an entry is displaced instead.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let k = fx.add_api_key(API_KEY);
    crashed_switch(&fx, &a, &k);
    fx.put_managed_key(API_KEY.as_bytes());
    let access_only = serde_json::json!({
        "claudeAiOauth": {"accessToken": "at-a-only", "expiresAt": 1_790_003_600_000i64},
        "mcpOAuth": {"srv": {"token": "machine-shared"}}
    })
    .to_string()
    .into_bytes();
    fx.set_live_credential(&access_only);
    oracle_says(&fx, "a@x.co");
    any_mutation(&fx, &a);
    assert_journal_cleared(&fx);
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
    assert_eq!(fx.displaced(), [access_only]);
}

#[test]
fn a_metadata_command_recovers_by_fingerprint_without_the_network() {
    // The row is decidable by fingerprint: the managed key is the target's. A metadata command
    // settles it without asking the oracle (§7.6, §9.6), so the rotated token has no
    // attribution and is displaced. The same row under an account-changing command asks the
    // oracle over HTTP once, and captures it.
    for asks in [false, true] {
        let fx = Fx::new();
        let (a, _) = crashed_cross_axis_switch_then_cc_rotated(&fx);
        fx.script_profile("a@x.co");
        let engine =
            fx.engine_with_oracle(Arc::new(HttpOracle::new(fx.http.clone(), fx.clock.clone())));
        if asks {
            drop(engine.mutation_guard().unwrap());
        } else {
            engine.set_disabled(&a, false).unwrap();
        }
        assert_journal_cleared(&fx);
        let profile_requests = fx.http.count(Method::Get, &Fx::endpoints().profile);
        assert_eq!(profile_requests, usize::from(asks), "asks={asks}");
        if asks {
            assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a-rotated-by-cc"));
            assert!(fx.displaced().is_empty());
        } else {
            assert!(fx.http.requests().is_empty(), "no request of any kind");
            assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
            assert_eq!(fx.displaced().len(), 1);
        }
    }
}

#[test]
fn backward_recovery_keeps_a_cc_updated_oauth_account_of_the_same_identity() {
    // The switch never landed; since the crash, CC refreshed a field of b's own oauthAccount.
    // That object still names b, so recovery keeps it rather than splicing the journaled copy
    // back over it.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    crashed_switch(&fx, &b, &a);
    let mut updated = Fx::oauth_account("b@x.co");
    updated["displayName"] = Value::String("B".into());
    common::splice_oauth_account(&fx.paths().global_config, &updated);
    any_mutation(&fx, &a);
    assert_journal_cleared(&fx);
    assert_eq!(active(&fx), Some(b));
    let doc: Value = serde_json::from_slice(&fs::read(fx.paths().global_config).unwrap()).unwrap();
    assert_eq!(doc["oauthAccount"], updated, "the CC-updated object is kept");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tagteam-engine --test recover`
Expected: FAIL.
- `forward_recovery_captures_…` and `a_captured_rotation_backfills_…` fail with the vault
  still at `rt-a` and one file displaced.
- `a_metadata_command_recovers_…` fails its `asks=true` round the same way. Its `asks=false`
  round passes already, since Task 7 made metadata commands offline.
- `backward_recovery_keeps_…` fails because `oauthAccount` lost `displayName`: the journaled
  copy was spliced back.
- `forward_recovery_without_an_attribution_…` and `an_attributed_token_without_a_refresh_token_…`
  pass already. They pin that the capture stays within its bounds.

- [ ] **Step 3: Let `Held` answer without inserting**

In `crates/tagteam-engine/src/switch.rs`, `impl Held`, add after `insert`:

```rust
    /// Whether `fp` is held already, without recording it.
    pub(crate) fn contains(&self, fp: &str) -> bool {
        self.0.iter().any(|h| h == fp)
    }
```

- [ ] **Step 4: Capture an attributed rotation in `finish_forward`**

In `crates/tagteam-engine/src/recover.rs`:

1. Imports become:

```rust
use serde_json::Value;
use tagteam_core::{OracleVerdict, OutgoingAction, OutgoingFacts, decide_outgoing};
use tagteam_provider::{
    Credential, LiveAuth, LiveChange, LiveLocks, LockError, MutationGuard, Provider, ProviderError,
    Read,
};

use crate::account_lock::AccountLock;
use crate::engine::Engine;
use crate::error::EngineError;
use crate::hooks;
use crate::oracle::verdict;
use crate::store::{AccountRow, EventRow, JournalRow, Store};
use crate::switch::{
    Axis, Held, OracleHint, answer_for, refuse_unreadable, refuse_unsafe_live_reads,
};
```

2. In `recover_one`, the account locks are kept and handed down. Today:

```rust
        let _accounts = self.lock_accounts(&ids)?;
```

becomes

```rust
        let accounts = self.lock_accounts(&ids)?;
```

and the dispatch becomes:

```rust
        match self.direction(p, &store, row, &live, hints)? {
            Direction::Forward(fp) => {
                self.finish_forward(p, &store, row, &accounts, &locks, &live, hints, &fp)
            }
            Direction::Backward(fp) => self.finish_backward(p, &store, row, &locks, &live, &fp),
            Direction::Undecidable => Ok(()),
        }
```

3. `finish_forward` takes the account locks and the hints:

```rust
    #[allow(clippy::too_many_arguments)]
    fn finish_forward(
        &self,
        p: &dyn Provider,
        store: &Store,
        row: &JournalRow,
        accounts: &[AccountLock],
        locks: &LiveLocks<'_>,
        live: &LiveAuth,
        hints: &[OracleHint],
        established: &str,
    ) -> Result<(), EngineError> {
```

Its doomed-entry loop today:

```rust
        for entry in &doomed {
            if let Read::Present(bytes) = &entry.bytes {
                self.save_unheld(
                    p,
                    &row.provider,
                    bytes,
                    &mut held,
                    false,
                    live_identity.as_ref(),
                    &mut warnings,
                )?;
            }
        }
```

becomes:

```rust
        // §9.6 (amended): an entry holding a generation no set above holds is the outgoing
        // account's only when the pre-lock oracle attributed exactly those bytes to it. It is
        // then captured into that account's vault; anything else is saved to `displaced/`.
        let from = match &row.from_id {
            Some(id) => store.account(id)?,
            None => None,
        };
        for entry in &doomed {
            let Read::Present(bytes) = &entry.bytes else {
                continue;
            };
            if let Some(from) = &from {
                if self.capture_rotated_outgoing(p, store, from, bytes, hints, accounts, &mut held)? {
                    continue;
                }
            }
            self.save_unheld(
                p,
                &row.provider,
                bytes,
                &mut held,
                false,
                live_identity.as_ref(),
                &mut warnings,
            )?;
        }
```

Also update the comment above `let doomed = …`. Today it says "A generation CC rotated since
the crash is none of these, so it is saved." It becomes:

```rust
        // §9.4 step 7's rule, before the other axis is cleared: every entry the clear destroys
        // is kept first, unless its generation is held already. Held are the target's live
        // generation, the vaults of both accounts the row names, and the outgoing generation
        // the row journaled, which step 4 settled before the row was written. A generation CC
        // rotated since the crash is none of these: it is captured when the oracle attributes
        // it to the outgoing account, and saved to `displaced/` otherwise.
```

4. Add the capture, in the same `impl Engine`, after `finish_forward`:

```rust
    /// §9.6 (amended): an entry a forward finish is about to clear, holding a generation that
    /// none of the held sets does, is classified as §9.4 step 4 would classify it, but
    /// without step 4's `Unresolved` capture. Recovery can run long after the crash, even after
    /// a re-login, so a live login naming the outgoing account no longer implies the credential
    /// is its. It is captured into the outgoing account's vault only when the pre-lock oracle
    /// resolved exactly these bytes to it. The hints are asked only about fresh live reads
    /// (`Axis::live_secret`), so a degraded read can never be captured, and §6.2's
    /// refresh-token bound applies through `decide_outgoing`. Returns whether the entry was
    /// captured, and so is held now.
    #[allow(clippy::too_many_arguments)]
    fn capture_rotated_outgoing(
        &self,
        p: &dyn Provider,
        store: &Store,
        from: &AccountRow,
        bytes: &[u8],
        hints: &[OracleHint],
        accounts: &[AccountLock],
        held: &mut Held,
    ) -> Result<bool, EngineError> {
        let Some(fp) = p.fingerprint(bytes) else {
            return Ok(false);
        };
        if held.contains(fp.as_str()) {
            return Ok(false);
        }
        let resolved = hints.iter().find_map(|h| answer_for(Some(h), bytes));
        let oracle = verdict(resolved, from);
        if oracle != OracleVerdict::ThisAccount {
            return Ok(false);
        }
        let vault = match self.vault.read(&from.id) {
            Read::Present(v) => Some(v),
            Read::Absent => None,
            Read::Unreadable(e) => return Err(EngineError::Unreadable(e)),
        };
        let facts = OutgoingFacts {
            bytes_equal_vault: vault.as_deref() == Some(bytes),
            fp_equal_vault: vault.as_deref().and_then(|v| p.fingerprint(v)).as_ref() == Some(&fp),
            wiped: p.is_wiped(bytes),
            tokenless: false,
            oracle,
            lacks_refresh_over_complete: !p.has_refresh_token(bytes)
                && vault.as_deref().is_some_and(|v| p.has_refresh_token(v)),
        };
        let OutgoingAction::CaptureToVault { .. } = decide_outgoing(&facts).1 else {
            return Ok(false);
        };
        let lock = accounts
            .iter()
            .find(|l| l.id() == &from.id)
            .expect("recovery locks both accounts the row names");
        self.persist_generation(p, from, lock, bytes)?;
        if let Some(uuid) = resolved.and_then(|i| i.account_uuid.as_deref()) {
            store.backfill_account_uuid(&from.id, uuid)?;
        }
        held.insert(fp.as_str());
        Ok(true)
    }
```

- [ ] **Step 5: Compare identities by key when finishing backward and checking coherence**

In `finish_backward`, the identity step today is:

```rust
        let expected = row.from_identity.as_ref();
        if live_identity.map(|i| i.raw).as_ref() != expected {
            let identity = expected.map(|v| p.parse_identity(v)).transpose()?;
            p.write_identity(&self.env, locks, identity.as_ref())?;
        }
```

Replace it with:

```rust
        // §9.6 (amended): splice the journaled identity back only when the live one names a
        // different identity. CC may have updated other fields of the same identity's object
        // since the crash, and that object is kept.
        let expected = row.from_identity.as_ref();
        let expected_identity = expected.map(|v| p.parse_identity(v)).transpose()?;
        let key = |i: &tagteam_provider::Identity| p.identity_key(i);
        if live_identity.as_ref().map(key) != expected_identity.as_ref().map(key) {
            p.write_identity(&self.env, locks, expected_identity.as_ref())?;
        }
```

`surfaces_agree` today compares raw objects:

```rust
        let Ok(identity) = self.read_live_identity(p) else {
            return false;
        };
        locks.check_owned().is_ok()
            && identity.map(|i| i.raw).as_ref() == expected
            && axes_coherent(p, &p.read_live_auth(&self.env), own, fp)
```

It becomes (update its doc comment's first sentence to "The live identity names the same
identity as `expected`, by identity key, …"):

```rust
        let Ok(identity) = self.read_live_identity(p) else {
            return false;
        };
        // A stored identity that no longer parses agrees with nothing.
        let Ok(expected) = expected.map(|v| p.parse_identity(v)).transpose() else {
            return false;
        };
        locks.check_owned().is_ok()
            && identity.map(|i| p.identity_key(&i)) == expected.map(|i| p.identity_key(&i))
            && axes_coherent(p, &p.read_live_auth(&self.env), own, fp)
```

A forward finish has just spliced `to.identity_json` itself, so it passes this check exactly
as it passed the raw comparison. The only behaviour that changes is the backward keep.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p tagteam-engine --test recover`
Expected: PASS, the new tests included.
`a_cross_axis_switch_never_displaces_the_journaled_outgoing_generation` still displaces its
rotated token, because the fixture's oracle (`FixedOracle`, which its test leaves answering
`None`) attributes nothing. `any_mutation` does ask the oracle since Task 7; the offline path is
pinned by `a_metadata_command_recovers_by_fingerprint_without_the_network`.

Run: `cargo test -p tagteam-engine --features test-hooks`
Expected: PASS.

Run: `cargo clippy -p tagteam-engine --all-targets --features test-hooks -- -D warnings`
Expected: no warnings.

- [ ] **Step 7: Commit**

```bash
git add crates/tagteam-engine/src/recover.rs crates/tagteam-engine/src/switch.rs \
  crates/tagteam-engine/tests/common/mod.rs crates/tagteam-engine/tests/recover.rs
git commit -m "Capture an attributed rotation of the outgoing token during forward recovery"
```

---

### Task 16: Active-token refresh

§7.5 as an engine operation. Nothing in M2a calls it outside tests; M2b's usage collector wires
its two triggers (§8.1: an expired active token, and a 401 on a token still valid locally).
The engine refuses with an error, never a silent no-op, whenever the operation must not run:
the live login is unmanaged, its kind does not refresh, the read is degraded or unreadable, or
the live credential belongs to someone else.

**Files:**
- Create: `crates/tagteam-engine/src/active.rs`
- Modify: `crates/tagteam-engine/src/lib.rs` (add `pub mod active;`)
- Modify: `crates/tagteam-engine/src/error.rs` (the `ForeignLiveCredential` variant, its
  kind, and its row in `kind_is_pinned_for_every_variant`)
- Modify: `crates/tagteam-engine/src/refresh.rs` (an accessor on Task 11's `Received`)
- Modify (Steps 7–11, the `Superseded` class): `crates/tagteam-core/src/classify.rs`,
  `crates/tagteam-engine/src/switch.rs` (`settle_outgoing`),
  `crates/tagteam-engine/src/recover.rs` (Task 15's `OutgoingFacts` literal)
- Test: `crates/tagteam-engine/tests/active.rs`

**Interfaces:**
- Consumes:
  - Task 3: `Engine.http: Arc<dyn Http>`.
  - Task 4: `Provider::{kind_traits, access_expires_at, access_fingerprint}`; `write_credential`
    without its `live` parameter.
  - Task 5: `Provider::{live_lock_budget, lock_credentials, lock_config}`, `CredLocks`.
  - Task 8: `Provider::refresh(http, &FreshCredential, now_ms, timeout) -> RefreshResult`,
    `DeadReason`, `TransientKind::token`.
  - Task 9: `Engine::{persist_generation, rescues_for, delete_rescue, quarantine}`,
    `RescueEntry`, `RescueFile`, `QuarantineReason` (from `tagteam_engine::quarantine`),
    `EngineError::RescuePending`.
  - Task 10: `crate::refresh::names_another_account(owner, row)`: the identity-conflict rule
    both refresh paths share (uuid compared only when both sides know one, org only when both
    are non-empty; either alone is a conflict); `crate::refresh::expired(p, bytes, now_ms)`
    (§7.2's test, which this task does not redefine).
  - Task 11: `crate::refresh::{Received, Persisted, Abandoned, Displacement}` (`Received::new(…,
    foreign)`, `Received::is_foreign`, `Received::keep`), `Engine::displace_received(row,
    sent_fp, &mut Received) -> Result<Displacement, EngineError>` (the foreign-successor path)
    and
    `Engine::persist_received(p, row, lock, &mut Received) -> Persisted`: the vault → `rescue/`
    → `Unpersisted` step both refresh paths share, with the guard that keeps a received
    successor on a panic, and displaces one that belongs to another account (§7.4). Also
    `Engine::abandon(row, sent_fp, &mut Received, cause) -> Abandoned` for an error after
    receipt, and `Engine::record_loss(row, sent_fp, cause)`, the §7.4 `successor_lost`
    quarantine every loss records.
  - Task 13: `Fx::plant_rescue(&self, id, predecessor_fp: &str, successor: &[u8]) -> PathBuf`.
  - Task 14: `EngineError::NeedsRelogin { position, label }`.
  - M1: `switch::{Held, OracleHint, answer_for, refuse_unreadable}`,
    `Engine::{save_unheld, hold_vault, read_live_identity, guard_or_refuse, lock_account}`,
    `oracle::verdict`.
  - Test fixture: `Fx.http`, `Fx::endpoints()` (Task 7); `Fx::script_refresh`,
    `Fx::script_token_error` (Task 8); the shared free helpers in `tests/common/mod.rs`
    `token_requests`, `quarantine_of` (Task 10), `block_rescue`, `unblock_rescue` (Task 11),
    `rescue_files` (Task 14) and `prev_refresh_token` (Task 15).
- Produces:
  - `tagteam_engine::active::{ActiveTrigger, ActiveOutcome, ACTIVE_REFRESH_TIMEOUT}`.
  - `Engine::refresh_active(&ProviderId, ActiveTrigger) -> Result<ActiveOutcome, EngineError>`.
  - `Received::bytes(&self) -> &[u8]` (`pub(crate)`, in `refresh.rs`).
  - `EngineError::ForeignLiveCredential { position: u32 }`, kind `foreign-credential`.
  - `ActiveOutcome::PublishedOnly`: tagteam's storage failed, but the live store holds the
    successor, so nothing is lost.
  - `OutgoingClass::Superseded` and `OutgoingFacts::equals_vault_prev` (`tagteam-core`), and
    `settle_outgoing` leaving a superseded live generation alone (§9.4 step 4, as amended;
    Steps 7–11).
  - Hook points `active-before-request` (before the token request),
    `active-after-response` (after a successor is received and guarded, before the lock
    re-check), and `active-before-publish` (before the live write; a successor neither store
    took stays guarded by `PendingLoss` across it).
- Decisions this task fixes (Codex round 1):
  - **A quarantined generation is never sent again** (§7.4): the quarantine holds while the
    live credential or the vault carries the fingerprint it is bound to. Checked under the
    locks, before reconciliation and any request.
  - **Every read reconciliation decides on is tri-state.** An unreadable vault, `.prev` or
    rescue refuses before any write or request. An unreadable `.prev` may be the generation
    that tells an unpublished earlier pass apart from a CC rotation.
  - **A pending self-heal is published before the lineage advances.** When reconciliation
    selects a recovered generation the live store does not hold yet, it is published first,
    whether or not it then needs a refresh. If that publication fails, the operation stops with
    `PersistedNotPublished`, never `NotNeeded`, and sends nothing. Publishing releases CC's
    credential locks with the config lock, so a refresh still needed afterwards runs in a second
    pass, from a fresh re-read under freshly taken locks.
  - **A successor that belongs to another account is displaced, never stored or published**
    (§7.3 step 6, §7.4, as the gate does), and the account is quarantined, bound to the
    fingerprint that was sent. A lost successor is reported as `Unpersisted` in preference to
    the conflict or to a compromised lock.
  - **A loss is only a successor held nowhere** (§7.5 step 5, as amended, Codex round 2). When
    the vault and `rescue/` both fail but publication puts the successor in the live store, CC
    holds it and the next pass adopts it into the vault (step 3's CC-rotation row). That is
    `PublishedOnly`, and nothing is quarantined. A successor that is in none of the three
    (persistence failed, and publication was skipped or failed) is `Unpersisted`, recorded
    exactly as the gate records one (`record_loss`: an ERROR log and the `successor_lost`
    quarantine, bound to the generation sent).
  - **The next switch never captures a generation this refresh superseded** (§9.4 step 4
    `Superseded`, Codex round 2). A successor persisted but not published leaves live A, vault
    B and `.prev` A. The outgoing capture would otherwise take A for a rotation and put the
    consumed token back over B. The class belongs in this task, the first that can produce
    that state; Steps 7–11 add it in their own commit.

- [ ] **Step 1: Write the failing tests**

Create `crates/tagteam-engine/tests/active.rs`:

```rust
mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Duration;

use common::{
    Fx, block_rescue, prev_refresh_token, quarantine_of, rescue_files, token_requests,
    unblock_rescue,
};
use serde_json::{Value, json};
use tagteam_cc::ItemKind;
use tagteam_core::AccountId;
use tagteam_engine::EngineError;
use tagteam_engine::active::{ActiveOutcome, ActiveTrigger};
use tagteam_engine::quarantine::QuarantineReason;
use tagteam_engine::vault::SERVICE;
use tagteam_provider::http::Method;
use tagteam_provider::{Clock, Provider};

fn bytes(v: &Value) -> Vec<u8> {
    v.to_string().into_bytes()
}

fn cred(email: &str, rt: &str) -> Vec<u8> {
    bytes(&Fx::credential_json(email, rt))
}

fn active(fx: &Fx, trigger: ActiveTrigger) -> Result<ActiveOutcome, EngineError> {
    fx.engine.refresh_active(&fx.provider(), trigger)
}

/// The live access token as §7.2 counts it expired: `now + 5 min ≥ expiresAt`. Only
/// `expiresAt` changes, so the live generation (its refresh token) is still the vault's.
fn expire_live(fx: &Fx) {
    let mut v = fx.live_credential().unwrap();
    v["claudeAiOauth"]["expiresAt"] = json!(fx.clock.now_ms());
    fx.set_live_credential(&bytes(&v));
}

fn fp(fx: &Fx, secret: &[u8]) -> String {
    fx.cc.fingerprint(secret).unwrap().as_str().to_owned()
}

fn quarantine_reason(fx: &Fx, id: &AccountId) -> Option<String> {
    quarantine_of(fx, id).0
}

#[test]
fn an_expired_live_token_is_refreshed_persisted_and_published() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    expire_live(&fx);
    fx.script_refresh(Some("rt-a2"));

    let out = active(&fx, ActiveTrigger::Expired).unwrap();

    assert_eq!(out, ActiveOutcome::Refreshed);
    assert_eq!(token_requests(&fx), 1);
    let sent = fx.http.requests().into_iter().next().unwrap();
    let body: Value = serde_json::from_slice(sent.body.as_deref().unwrap()).unwrap();
    assert_eq!(body["grant_type"], "refresh_token");
    assert_eq!(body["refresh_token"], "rt-a");
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"));
    assert_eq!(prev_refresh_token(&fx, &a).as_deref(), Some("rt-a"));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a2"));
    // The machine-shared keys stay the machine's (§9.4 step 5).
    assert_eq!(
        fx.live_credential().unwrap()["mcpOAuth"],
        json!({"srv": {"token": "machine-shared"}})
    );
    assert!(fx.displaced().is_empty());
    assert!(!fx.paths().refresh_lock.exists() && !fx.paths().config_lock.exists());
}

#[test]
fn a_live_token_neither_expired_nor_rejected_needs_no_request() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let before = fx.kc.items();

    let out = active(&fx, ActiveTrigger::Expired).unwrap();

    assert_eq!(out, ActiveOutcome::NotNeeded { reconciled: false });
    assert_eq!(token_requests(&fx), 0);
    assert_eq!(fx.kc.items(), before);
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
}

#[test]
fn a_locally_valid_token_the_server_rejected_is_refreshed() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let live = cred("a@x.co", "rt-a");
    let rejected = fx.cc.access_fingerprint(&live).unwrap().as_str().to_owned();

    // A rejection of some other access token says nothing about this one.
    let stale = ActiveTrigger::Rejected {
        access_fp: fx.cc.access_fingerprint(&cred("a@x.co", "rt-old")).unwrap().as_str().to_owned(),
    };
    assert_eq!(
        active(&fx, stale).unwrap(),
        ActiveOutcome::NotNeeded { reconciled: false }
    );
    assert_eq!(token_requests(&fx), 0);

    fx.script_refresh(Some("rt-a2"));
    let out = active(&fx, ActiveTrigger::Rejected { access_fp: rejected }).unwrap();
    assert_eq!(out, ActiveOutcome::Refreshed);
    assert_eq!(token_requests(&fx), 1);
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a2"));
}

#[test]
fn row_vault_prev_self_heals_the_live_store_from_the_vault() {
    // An earlier pass reached the vault but not the live store.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.kc.put(SERVICE, a.as_str(), &cred("a@x.co", "rt-b"));
    fx.kc.put(SERVICE, &format!("{a}.prev"), &cred("a@x.co", "rt-a"));

    let out = active(&fx, ActiveTrigger::Expired).unwrap();

    assert_eq!(out, ActiveOutcome::NotNeeded { reconciled: true });
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-b"));
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-b"));
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn row_a_published_rescue_is_written_to_the_vault() {
    // Published to the live store, but the vault write failed.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let vault = fx.vault_bytes(&a).unwrap();
    fx.rotate_live("rt-r");
    let live = bytes(&fx.live_credential().unwrap());
    fx.plant_rescue(&a, &fp(&fx, &vault), &live);

    let out = active(&fx, ActiveTrigger::Expired).unwrap();

    assert_eq!(out, ActiveOutcome::NotNeeded { reconciled: true });
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-r"));
    assert_eq!(prev_refresh_token(&fx, &a).as_deref(), Some("rt-a"));
    assert_eq!(rescue_files(&fx), 0, "its writes are verified: retired");
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn row_in_step_publishes_a_rescue_that_succeeds_the_vault() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let vault = fx.vault_bytes(&a).unwrap();
    fx.plant_rescue(&a, &fp(&fx, &vault), &cred("a@x.co", "rt-s"));

    let out = active(&fx, ActiveTrigger::Expired).unwrap();

    assert_eq!(out, ActiveOutcome::NotNeeded { reconciled: true });
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-s"));
    assert_eq!(prev_refresh_token(&fx, &a).as_deref(), Some("rt-a"));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-s"));
    assert_eq!(rescue_files(&fx), 0);
}

#[test]
fn row_a_cc_rotation_after_a_published_rescue_is_adopted_and_retires_the_rescue() {
    // §15.2: the rescue rt-r was published, then CC rotated rt-r to rt-c. The live store is
    // where the lineage advances (§7.5 step 3), so rt-c is newest and the rescue is superseded.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let vault = fx.vault_bytes(&a).unwrap();
    fx.rotate_live("rt-r");
    fx.plant_rescue(&a, &fp(&fx, &vault), &bytes(&fx.live_credential().unwrap()));
    fx.rotate_live("rt-c");

    let out = active(&fx, ActiveTrigger::Expired).unwrap();

    assert_eq!(out, ActiveOutcome::NotNeeded { reconciled: true });
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-c"));
    assert_eq!(prev_refresh_token(&fx, &a).as_deref(), Some("rt-a"));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-c"));
    assert_eq!(rescue_files(&fx), 0);
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn row_a_cc_rotation_is_adopted_then_refreshed_when_expired() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.rotate_live("rt-c");
    expire_live(&fx);
    fx.script_refresh(Some("rt-c2"));

    let out = active(&fx, ActiveTrigger::Expired).unwrap();

    assert_eq!(out, ActiveOutcome::Refreshed);
    let sent = fx.http.requests().into_iter().next().unwrap();
    let body: Value = serde_json::from_slice(sent.body.as_deref().unwrap()).unwrap();
    assert_eq!(body["refresh_token"], "rt-c", "the newest generation is the one sent");
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-c2"));
    assert_eq!(prev_refresh_token(&fx, &a).as_deref(), Some("rt-c"));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-c2"));
}

#[test]
fn an_access_token_only_live_blob_never_replaces_the_vault_refresh_token() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let blob = json!({"claudeAiOauth": {"accessToken": "at-only", "expiresAt": fx.clock.now_ms()}});
    fx.set_live_credential(&bytes(&blob));

    let err = active(&fx, ActiveTrigger::Expired).unwrap_err();

    assert_eq!(err.kind(), "relogin-required");
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn a_dead_verdict_quarantines_the_account_bound_to_the_generation_sent() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    expire_live(&fx);
    fx.script_token_error(400, "invalid_grant");

    let out = active(&fx, ActiveTrigger::Expired).unwrap();

    assert_eq!(out, ActiveOutcome::Dead(QuarantineReason::InvalidGrant));
    let row = fx.engine.store().unwrap().account(&a).unwrap().unwrap();
    assert_eq!(row.quarantine_reason.as_deref(), Some("invalid_grant"));
    assert_eq!(
        row.quarantine_fp.as_deref(),
        Some(fp(&fx, &cred("a@x.co", "rt-a")).as_str())
    );
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
}

#[test]
fn a_transient_failure_changes_nothing() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    expire_live(&fx);
    let before = fx.kc.items();

    // Nothing is scripted: the request is never sent (`PreSend`).
    let out = active(&fx, ActiveTrigger::Expired).unwrap();

    assert_eq!(
        out,
        ActiveOutcome::Transient {
            kind: "pre-send".into()
        }
    );
    assert_eq!(fx.kc.items(), before);
    assert_eq!(quarantine_reason(&fx, &a), None);
}

#[test]
fn a_failed_vault_write_rescues_the_successor_and_still_publishes_it() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    expire_live(&fx);
    fx.script_refresh(Some("rt-a2"));
    fx.kc.set_fail_write(SERVICE, true);

    let out = active(&fx, ActiveTrigger::Expired).unwrap();

    assert_eq!(out, ActiveOutcome::Refreshed, "rescue/ is tagteam's storage too");
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a2"));
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
    assert_eq!(rescue_files(&fx), 1);

    // The next pass finds the rescue's generation live and writes it to the vault.
    fx.kc.set_fail_write(SERVICE, false);
    let out = active(&fx, ActiveTrigger::Expired).unwrap();
    assert_eq!(out, ActiveOutcome::NotNeeded { reconciled: true });
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"));
    assert_eq!(rescue_files(&fx), 0);
    assert_eq!(token_requests(&fx), 1);
}

#[test]
fn when_both_writes_fail_but_the_live_store_takes_it_nothing_is_lost() {
    // §7.5 step 5: CC holds the successor, so it is not lost and nothing is quarantined; the
    // next pass adopts it into the vault (step 3's CC-rotation row).
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    expire_live(&fx);
    fx.script_refresh(Some("rt-a2"));
    fx.kc.set_fail_write(SERVICE, true);
    block_rescue(&fx);

    let out = active(&fx, ActiveTrigger::Expired).unwrap();
    unblock_rescue(&fx);

    assert_eq!(out, ActiveOutcome::PublishedOnly);
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a2"));
    assert_eq!(quarantine_reason(&fx, &a), None, "a successor CC holds is not lost");

    fx.kc.set_fail_write(SERVICE, false);
    let out = active(&fx, ActiveTrigger::Expired).unwrap();
    assert_eq!(out, ActiveOutcome::NotNeeded { reconciled: true });
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"));
    assert_eq!(token_requests(&fx), 1);
}

/// A 200 token reply whose owner is another account (§7.4).
fn reply_for_someone_else(fx: &Fx) {
    fx.http.push_json(
        Method::Post,
        &Fx::endpoints().token,
        200,
        json!({
            "access_token": "at-z", "refresh_token": "rt-z", "expires_in": 28800,
            "account": {"uuid": "uuid-z@x.co", "email_address": "z@x.co"},
            "organization": {"uuid": ""}
        }),
    );
}

#[test]
fn a_token_response_naming_another_account_is_displaced_never_stored_or_published() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let sent = fp(&fx, &cred("a@x.co", "rt-a"));
    expire_live(&fx);
    reply_for_someone_else(&fx);

    let out = active(&fx, ActiveTrigger::Expired).unwrap();

    assert_eq!(out, ActiveOutcome::Dead(QuarantineReason::IdentityConflict));
    let row = fx.engine.store().unwrap().account(&a).unwrap().unwrap();
    assert_eq!(row.quarantine_reason.as_deref(), Some("identity_conflict"));
    assert_eq!(row.quarantine_fp.as_deref(), Some(sent.as_str()), "bound to the generation sent");
    assert_eq!(
        fx.vault_refresh_token(&a).as_deref(),
        Some("rt-a"),
        "another account's token never enters this account's vault (§7.3 step 6)"
    );
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"), "nor the live store");
    let kept: Vec<Value> = fx
        .displaced()
        .iter()
        .map(|b| serde_json::from_slice(b).unwrap())
        .collect();
    assert_eq!(kept.len(), 1, "the successor is never discarded: it is displaced");
    assert_eq!(kept[0]["claudeAiOauth"]["refreshToken"], "rt-z");
    assert_eq!(rescue_files(&fx), 0);
}

#[test]
fn a_lost_conflicting_successor_is_reported_as_unpersisted() {
    // Persistence loss takes precedence over the conflict (§7.3 step 6).
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    expire_live(&fx);
    reply_for_someone_else(&fx);
    let dir = fx.env.data_dir().join("displaced");
    fs::create_dir_all(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();

    let out = active(&fx, ActiveTrigger::Expired).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();

    assert_eq!(out, ActiveOutcome::Unpersisted);
    assert_eq!(quarantine_reason(&fx, &a).as_deref(), Some("identity_conflict"));
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
}

#[test]
fn a_quarantined_active_token_is_never_sent_again() {
    // §7.4: after `invalid_grant`, the quarantine is bound to the live (and vault) generation,
    // so a second call sends nothing.
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    expire_live(&fx);
    fx.script_token_error(400, "invalid_grant");
    for _ in 0..2 {
        assert_eq!(
            active(&fx, ActiveTrigger::Expired).unwrap(),
            ActiveOutcome::Dead(QuarantineReason::InvalidGrant)
        );
    }
    assert_eq!(token_requests(&fx), 1, "the quarantined generation is never sent again");
}

#[test]
fn an_unreadable_prev_refuses_before_any_write_or_request() {
    // Live A, vault B, `.prev` unreadable. `.prev` may be A (an earlier pass never published
    // B), so A must not be adopted over B as if CC had rotated it.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.kc.put(SERVICE, a.as_str(), &cred("a@x.co", "rt-b"));
    fx.kc.put(SERVICE, &format!("{a}.prev"), &cred("a@x.co", "rt-a"));
    fx.kc.set_unreadable(SERVICE, &format!("{a}.prev"), true);
    expire_live(&fx);

    let err = active(&fx, ActiveTrigger::Expired).unwrap_err();

    assert_eq!(err.kind(), "unreadable", "{err}");
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-b"), "B is never overwritten");
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
    assert_eq!(token_requests(&fx), 0);
}

/// Live A, vault B and `.prev` A: an earlier pass persisted B but never published it. B's
/// access token expires at `b_expires`.
fn unpublished_b(fx: &Fx, a: &AccountId, b_expires: i64) {
    let mut b = Fx::credential_json("a@x.co", "rt-b");
    b["claudeAiOauth"]["expiresAt"] = json!(b_expires);
    fx.kc.put(SERVICE, a.as_str(), &bytes(&b));
    fx.kc.put(SERVICE, &format!("{a}.prev"), &cred("a@x.co", "rt-a"));
}

#[test]
fn an_expired_recovered_generation_is_published_before_it_is_refreshed() {
    // B is published first (the self-heal), then refreshed to C in a second pass, so the live
    // store never falls two generations behind.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    unpublished_b(&fx, &a, fx.clock.now_ms());
    fx.script_refresh(Some("rt-c"));

    let out = active(&fx, ActiveTrigger::Expired).unwrap();

    assert_eq!(out, ActiveOutcome::Refreshed);
    let sent: Value =
        serde_json::from_slice(fx.http.requests()[0].body.as_deref().unwrap()).unwrap();
    assert_eq!(sent["refresh_token"], "rt-b", "the recovered generation is the one sent");
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-c"));
    assert_eq!(prev_refresh_token(&fx, &a).as_deref(), Some("rt-b"));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-c"));
    assert_eq!(token_requests(&fx), 1);
}

#[test]
fn a_self_heal_that_cannot_publish_sends_nothing_and_says_so() {
    // CC holds its config lock, so B cannot reach the live store. Whether or not B needs a
    // refresh, nothing is sent, and the outcome is `PersistedNotPublished`, never `NotNeeded`.
    for b_expired in [true, false] {
        let fx = Fx::with_lock_timeout(Duration::from_millis(300));
        let a = fx.add("a@x.co", "rt-a");
        let now = fx.clock.now_ms();
        unpublished_b(&fx, &a, if b_expired { now } else { now + 3_600_000 });
        fs::create_dir(fx.paths().config_lock).unwrap(); // CC holds it, freshly

        let out = active(&fx, ActiveTrigger::Expired).unwrap();
        fs::remove_dir(fx.paths().config_lock).unwrap();

        assert_eq!(out, ActiveOutcome::PersistedNotPublished, "B expired: {b_expired}");
        assert_eq!(token_requests(&fx), 0, "B expired: {b_expired}");
        assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"), "B expired: {b_expired}");
        assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-b"), "B expired: {b_expired}");
    }
}

#[test]
fn a_degraded_live_read_is_refused_before_any_request() {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    expire_live(&fx);
    let (svc, acct) = fx.live_item(ItemKind::OAuth);
    fx.kc.set_unreadable(&svc, &acct, true);
    fs::write(fx.paths().credentials_file, cred("a@x.co", "rt-a")).unwrap();

    let err = active(&fx, ActiveTrigger::Expired).unwrap_err();

    assert_eq!(err.kind(), "degraded-read");
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn an_unmanaged_live_login_is_refused() {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    fx.login("x@y.co", "rt-x");

    let err = active(&fx, ActiveTrigger::Expired).unwrap_err();

    assert_eq!(err.kind(), "invalid-input");
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn an_oracle_naming_someone_else_refuses_without_adopting() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.rotate_live("rt-c");
    fx.oracle
        .set(Some(fx.cc.parse_identity(&Fx::oauth_account("z@x.co")).unwrap()));

    let err = active(&fx, ActiveTrigger::Expired).unwrap_err();

    assert_eq!(err.kind(), "foreign-credential");
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
    assert_eq!(token_requests(&fx), 0);
}

#[cfg(feature = "test-hooks")]
mod hooks {
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, SystemTime};

    use super::*;

    #[test]
    fn only_the_credential_locks_are_held_across_the_request() {
        let fx = Fx::new();
        fx.add("a@x.co", "rt-a");
        expire_live(&fx);
        fx.script_refresh(Some("rt-a2"));
        let seen = Arc::new(Mutex::new(None));
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

        assert_eq!(active(&fx, ActiveTrigger::Expired).unwrap(), ActiveOutcome::Refreshed);
        assert_eq!(
            *seen.lock().unwrap(),
            Some((true, false)),
            "CC's refresh lock is held; its config lock is not (§4.3)"
        );
    }

    #[test]
    fn a_lock_taken_over_during_the_request_persists_but_never_publishes() {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        expire_live(&fx);
        fx.script_refresh(Some("rt-a2"));
        let refresh = fx.paths().refresh_lock;
        let lock_dir = refresh.clone();
        // A takeover rewrites the lock directory's mtime (§9.1 compromise detection).
        fx.engine.on_point(
            "active-after-response",
            Box::new(move || {
                fs::File::open(&lock_dir)
                    .unwrap()
                    .set_modified(SystemTime::now() + Duration::from_secs(60))
                    .unwrap();
            }),
        );

        let out = active(&fx, ActiveTrigger::Expired).unwrap();

        assert_eq!(out, ActiveOutcome::PersistedNotPublished);
        assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"));
        assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"), "not published");
        assert!(refresh.is_dir(), "a lock taken over is left to its new holder");

        // The next pass finds the vault ahead of the live store and self-heals it.
        fs::remove_dir(&refresh).unwrap();
        fx.engine.on_point("active-after-response", Box::new(|| {}));
        let out = active(&fx, ActiveTrigger::Expired).unwrap();
        assert_eq!(out, ActiveOutcome::NotNeeded { reconciled: true });
        assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a2"));
        assert_eq!(token_requests(&fx), 1);
    }

    /// Makes the credential lock look taken over once the response arrives (§9.1), so the
    /// successor is never published.
    fn take_over_after_response(fx: &Fx) -> PathBuf {
        let refresh = fx.paths().refresh_lock;
        let lock_dir = refresh.clone();
        fx.engine.on_point(
            "active-after-response",
            Box::new(move || {
                fs::File::open(&lock_dir)
                    .unwrap()
                    .set_modified(SystemTime::now() + Duration::from_secs(60))
                    .unwrap();
            }),
        );
        refresh
    }

    #[test]
    fn a_successor_held_nowhere_is_unpersisted_and_quarantines_the_account() {
        // §7.5 step 5, as amended: the vault and rescue/ both fail, and the lock was taken
        // over, so the live store is not written either. The consumed generation must never be
        // sent again: `successor_lost`, bound to it (§7.4).
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let sent = fp(&fx, &cred("a@x.co", "rt-a"));
        expire_live(&fx);
        fx.script_refresh(Some("rt-a2"));
        fx.kc.set_fail_write(SERVICE, true);
        block_rescue(&fx);
        take_over_after_response(&fx);

        let out = active(&fx, ActiveTrigger::Expired).unwrap();
        unblock_rescue(&fx);

        assert_eq!(out, ActiveOutcome::Unpersisted);
        let row = fx.engine.store().unwrap().account(&a).unwrap().unwrap();
        assert_eq!(row.quarantine_reason.as_deref(), Some("successor_lost"));
        assert_eq!(row.quarantine_fp.as_deref(), Some(sent.as_str()));
        assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"), "never published");
        assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
    }

    #[test]
    fn a_panic_while_publishing_a_successor_held_nowhere_still_records_the_loss() {
        // Once neither the vault nor rescue/ took the successor, the live write is its last
        // home; a panic inside that write must still record the loss (§7.3 step 6).
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let sent = fp(&fx, &cred("a@x.co", "rt-a"));
        expire_live(&fx);
        fx.script_refresh(Some("rt-a2"));
        fx.kc.set_fail_write(SERVICE, true);
        block_rescue(&fx);
        fx.engine.fail_at(Some("panic:active-before-publish"));

        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            active(&fx, ActiveTrigger::Expired)
        }));
        fx.engine.fail_at(None);
        unblock_rescue(&fx);

        assert!(unwound.is_err(), "the injected panic unwinds");
        let row = fx.engine.store().unwrap().account(&a).unwrap().unwrap();
        assert_eq!(row.quarantine_reason.as_deref(), Some("successor_lost"));
        assert_eq!(row.quarantine_fp.as_deref(), Some(sent.as_str()));
        assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"), "never published");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tagteam-engine --features test-hooks --test active`
Expected: FAIL to compile with "could not find `active` in `tagteam_engine`".

- [ ] **Step 3: Add the refusal for a foreign live credential**

In `crates/tagteam-engine/src/error.rs`, add this variant after `RollbackFailed`:

```rust
    /// §7.5: the oracle attributed the live credential to another identity, so tagteam neither
    /// adopts nor refreshes it. `usageStatus` calls this `foreign_credential` (§13.2).
    #[error(
        "the live credential does not belong to the account at position {position}; tagteam will not refresh it"
    )]
    ForeignLiveCredential { position: u32 },
```

In `kind()`, before `EngineError::Io(_) => "io",`:

```rust
            EngineError::ForeignLiveCredential { .. } => "foreign-credential",
```

In `kind_is_pinned_for_every_variant`, before the `Io` case:

```rust
            (
                EngineError::ForeignLiveCredential { position: 1 },
                "foreign-credential",
            ),
```

- [ ] **Step 4: Write the operation**

In `crates/tagteam-engine/src/lib.rs`, add `pub mod active;` as the first module line (above
`pub mod account_lock;`).

In `crates/tagteam-engine/src/refresh.rs`, add below `impl<'e> Received<'e> { … }` (Task 11):

```rust
impl Received<'_> {
    /// The successor's bytes, for publishing it to the live store once persisted (§7.5
    /// step 5).
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}
```

Create `crates/tagteam-engine/src/active.rs`:

```rust
//! Active-token refresh (§7.5): the one path that ever sends the live login's refresh token.

use std::path::PathBuf;
use std::time::Duration;

use tagteam_core::{Fingerprint, OracleVerdict, ProviderId};
use tagteam_provider::{
    CredLocks, Credential, LiveChange, Provenance, Provider, ProviderError, Read, RefreshResult,
    StoredLogin,
};

use crate::account_lock::AccountLock;
use crate::engine::Engine;
use crate::error::EngineError;
use crate::hooks;
use crate::oracle::verdict;
use crate::quarantine::QuarantineReason;
use crate::refresh::{Abandoned, Displacement, Persisted, Received, expired, names_another_account};
use crate::rescue::{RescueEntry, RescueFile};
use crate::store::AccountRow;
use crate::switch::{Held, OracleHint, answer_for, refuse_unreadable};

/// §7.5 step 5: how long the token request may take while CC's credential locks are held
/// (§4.3's second bounded exception).
pub const ACTIVE_REFRESH_TIMEOUT: Duration = Duration::from_secs(6);

/// Why the caller asks (§7.5): the only two reasons tagteam refreshes the live token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActiveTrigger {
    /// The live access token has expired.
    Expired,
    /// The server answered 401 to this access token although it is still valid locally
    /// (a sibling machine revoked it; §8.1 `rejected_fp`).
    Rejected { access_fp: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActiveOutcome {
    /// Reconciliation left a live token that is neither expired nor the rejected one.
    NotNeeded { reconciled: bool },
    /// The successor is in tagteam's storage (the vault, or `rescue/`) and in the live store.
    Refreshed,
    /// CC's locks were compromised when the response arrived: persisted to the vault or
    /// `rescue/`, but not published; the next pass reconciles (§7.5 step 5).
    PersistedNotPublished,
    /// Neither the vault nor `rescue/` could take the successor, but the live store holds it,
    /// so nothing is lost: the next pass adopts it into the vault (§7.5 step 3). Nothing is
    /// quarantined.
    PublishedOnly,
    Dead(QuarantineReason),
    Systemic(String),
    Transient { kind: String },
    /// The successor is held nowhere: logged at ERROR, and the account quarantined
    /// `successor_lost` (§7.5 step 5, §7.4).
    Unpersisted,
}

/// A successor neither the vault nor `rescue/` could take, while its live write is under way.
/// If that write unwinds, dropping this records the loss (§7.3 step 6: `successor_lost`, bound
/// to the generation sent). Disarmed once the outcome is settled.
struct PendingLoss<'a> {
    engine: &'a Engine,
    row: &'a AccountRow,
    sent_fp: &'a str,
    armed: bool,
}

impl Drop for PendingLoss<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.engine
                .record_loss(self.row, self.sent_fp, &"publishing it to the live store unwound");
        }
    }
}

/// What reconciliation (§7.5 step 3) settled: the generation the live store holds, or will
/// hold once `publish` writes it, and the rescue files to retire once that write reads back.
struct Reconciled {
    current: Vec<u8>,
    publish: bool,
    changed: bool,
    retire: Vec<PathBuf>,
}

impl Engine {
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

        // Step 1: the mutation lock and the account lock, held throughout.
        let guard = self.guard_or_refuse(provider)?;
        let lock = self.lock_account(&row.id)?;
        // At most two passes. Publishing a recovered generation (step 3's self-heal) releases
        // CC's credential locks along with the config lock, so a refresh that is still needed
        // afterwards runs in a second pass, from a fresh re-read under freshly taken locks. That
        // pass finds the live store in step with the vault, so it never publishes again.
        for _pass in 0..2 {
            let cred = p.lock_credentials(&self.env, &guard, p.live_lock_budget())?;

            // Step 2: the same account, read fresh, now that CC cannot rotate it.
            let (row, live) = self.active_login(p, provider)?;
            if &row.id != lock.id() {
                return Err(EngineError::LiveMoved);
            }
            // §7.4: a quarantined generation is never sent again.
            if let Some(reason) = self.active_quarantine(p, &row, &live)? {
                return Ok(ActiveOutcome::Dead(reason));
            }

            // Step 3.
            let rec = self.reconcile_active(p, &row, &lock, &live, hint.as_ref())?;

            // Step 4: a request only if the newest generation still needs one.
            let now = self.now_ms();
            let rejected = match &trigger {
                ActiveTrigger::Rejected { access_fp } => p
                    .access_fingerprint(&rec.current)
                    .is_some_and(|f| f.as_str() == access_fp),
                ActiveTrigger::Expired => false,
            };
            let needed = rejected || expired(p, &rec.current, now);
            if rec.publish {
                // The self-heal comes first, needed or not: advancing the lineage again before
                // the live store caught up would leave it two generations behind, where no
                // later pass could tell its generation from a CC rotation.
                if !self.publish(p, &row, cred, &rec.current, &rec.retire)? {
                    return Ok(ActiveOutcome::PersistedNotPublished);
                }
                if needed {
                    continue;
                }
                return Ok(ActiveOutcome::NotNeeded { reconciled: true });
            }
            if !needed {
                return Ok(ActiveOutcome::NotNeeded {
                    reconciled: rec.changed,
                });
            }

            // Step 5.
            return self.request_active(p, &row, &lock, cred, &rec, now);
        }
        Err(EngineError::LiveMoved)
    }

    /// §7.4: the active account's quarantine holds while the live credential or the vault
    /// still carries the generation it is bound to (a quarantine with no bound generation
    /// always holds). Such a token is never sent again.
    fn active_quarantine(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        live: &[u8],
    ) -> Result<Option<QuarantineReason>, EngineError> {
        let Some(reason) = &row.quarantine_reason else {
            return Ok(None);
        };
        let reason = QuarantineReason::parse(reason).unwrap_or(QuarantineReason::InvalidGrant);
        let Some(bound) = &row.quarantine_fp else {
            return Ok(Some(reason));
        };
        let carries = |b: &[u8]| p.fingerprint(b).is_some_and(|f| f.as_str() == bound);
        let vault_carries = match self.vault.read(&row.id) {
            Read::Present(v) => carries(&v),
            Read::Absent => false,
            Read::Unreadable(e) => return Err(EngineError::Unreadable(e)),
        };
        Ok((carries(live) || vault_carries).then_some(reason))
    }

    /// §7.5 step 5: the request under CC's credential locks only (6 s), then persistence, then
    /// the live write under the config lock.
    fn request_active(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        lock: &AccountLock,
        cred: CredLocks<'_>,
        rec: &Reconciled,
        now: i64,
    ) -> Result<ActiveOutcome, EngineError> {
        cred.check_owned()?;
        hooks::point(self, "active-before-request")?;
        let sent = p.fingerprint(&rec.current);
        let sent_fp = sent.as_ref().map(Fingerprint::as_str).unwrap_or_default();
        // The type cannot prove `rec.current` fresh here: it arrives as bytes through
        // `Reconciled`, not as a `FreshCredential`. Freshness rests on the runtime check in
        // `active_login`, which refuses a degraded or empty live read before anything is sent
        // (§4.3); the other sources are vault reads, which are authoritative.
        // `a_degraded_live_read_is_refused_before_any_request` pins that check, so a change to
        // it that let a degraded token through would fail that test.
        let fresh = Credential::fresh(rec.current.clone())
            .into_fresh()
            .expect("a credential built fresh is fresh");
        match p.refresh(&*self.http, &fresh, now, ACTIVE_REFRESH_TIMEOUT) {
            RefreshResult::Refreshed { successor, owner } => {
                // §7.4: a successor the response says belongs to another account is marked
                // first (Task 11's rule): it is displaced, never stored or published.
                let foreign = owner.filter(|o| names_another_account(o, row));
                // From here on the successor is never discarded (§7.3): `received` keeps it if
                // anything below unwinds before it is stored.
                let mut received = Received::new(self, p, row, sent_fp, successor, foreign);
                if let Err(e) = hooks::point(self, "active-after-response") {
                    // As the gate does (Task 11): keep it and return the error, or record the
                    // loss and report it, never the error.
                    return match self.abandon(row, sent_fp, &mut received, e) {
                        Abandoned::Kept(e) => Err(e),
                        Abandoned::Lost => Ok(ActiveOutcome::Unpersisted),
                    };
                }
                if received.is_foreign() {
                    // Task 11's shared path: displaced and quarantined `identity_conflict`; a
                    // successor that could not even be displaced is reported first (§7.3 step 6).
                    return Ok(match self.displace_received(row, sent_fp, &mut received)? {
                        Displacement::Kept => {
                            ActiveOutcome::Dead(QuarantineReason::IdentityConflict)
                        }
                        Displacement::Lost => ActiveOutcome::Unpersisted,
                    });
                }
                let owned = cred.check_owned().is_ok();
                // Task 11's shared step: the vault, else `rescue/`, else reported as lost.
                let persisted = self.persist_received(p, row, lock, &mut received);
                // `persist_received` disarmed `received`. When neither store took the
                // successor, the live write below is its last home, so the loss stays armed
                // until that write lands: a panic in between still records it (§7.3 step 6).
                let mut pending = PendingLoss {
                    engine: self,
                    row,
                    sent_fp,
                    armed: persisted == Persisted::Unpersisted,
                };
                // CC must hold the newest generation whatever became of tagteam's copy.
                let published = if owned {
                    hooks::point(self, "active-before-publish")
                        .and_then(|()| self.publish(p, row, cred, received.bytes(), &rec.retire))
                } else {
                    Ok(false)
                };
                // Settled below either way: published, or recorded by `record_loss`.
                pending.armed = false;
                // A loss takes precedence over everything else (§7.3 step 6). But a successor
                // the live store holds is not lost: CC has it, and the next pass adopts it into
                // the vault (§7.5 step 3). Only one held nowhere is recorded, as the gate
                // records it: `successor_lost`, bound to the generation sent (§7.5 step 5).
                Ok(match (persisted, published) {
                    (Persisted::Unpersisted, Ok(true)) => ActiveOutcome::PublishedOnly,
                    (Persisted::Unpersisted, published) => {
                        let cause = match &published {
                            Err(e) => format!("the live store was not written either: {e}"),
                            Ok(_) => "the live store was not written either".to_owned(),
                        };
                        self.record_loss(row, sent_fp, &cause);
                        ActiveOutcome::Unpersisted
                    }
                    (_, Err(e)) => return Err(e),
                    (_, Ok(true)) => ActiveOutcome::Refreshed,
                    (_, Ok(false)) => ActiveOutcome::PersistedNotPublished,
                })
            }
            RefreshResult::Dead(reason) => {
                // §7.3 step 7: Dead only while the source that was sent still holds the
                // generation sent; otherwise the lineage moved and this is a failed refresh.
                let holds = |bytes: Option<Vec<u8>>| {
                    sent.is_some() && bytes.as_deref().and_then(|b| p.fingerprint(b)) == sent
                };
                let live_now = p
                    .read_live_auth(&self.env)
                    .credential
                    .present()
                    .filter(|c| c.provenance() == Provenance::Fresh)
                    .map(|c| c.bytes().to_vec());
                if !holds(live_now) && !holds(self.vault.read(&row.id).present()) {
                    return Ok(ActiveOutcome::Transient {
                        kind: "refresh-failed".into(),
                    });
                }
                let reason: QuarantineReason = reason.into();
                self.quarantine(row, reason, sent_fp)?;
                Ok(ActiveOutcome::Dead(reason))
            }
            RefreshResult::Systemic(detail) => Ok(ActiveOutcome::Systemic(detail)),
            RefreshResult::Transient(kind) => Ok(ActiveOutcome::Transient { kind: kind.token() }),
        }
    }

    /// The live login's account and credential, read fresh (§7.5 step 2). Only a managed
    /// login of a kind that refreshes, from a read that is neither degraded nor empty (§4.3).
    fn active_login(
        &self,
        p: &dyn Provider,
        provider: &ProviderId,
    ) -> Result<(AccountRow, Vec<u8>), EngineError> {
        let identity = self
            .read_live_identity(p)?
            .ok_or(EngineError::NoLiveLogin)?;
        let row = match self.existing_store()? {
            Some(store) => {
                store.find_by_identity_key(provider, p.identity_key(&identity).as_str())?
            }
            None => None,
        }
        .ok_or_else(|| {
            EngineError::InvalidInput(format!(
                "the live login is not managed by tagteam; only {} refreshes it",
                p.display_name()
            ))
        })?;
        if !p.kind_traits(&row.kind).refreshable {
            return Err(EngineError::InvalidInput(format!(
                "position {} holds a credential that does not refresh",
                row.position
            )));
        }
        let bytes = match p.read_live_auth(&self.env).credential {
            Read::Present(c) if c.provenance() == Provenance::Degraded => {
                return Err(EngineError::DegradedRead);
            }
            Read::Present(c) if !c.is_empty() => c.bytes().to_vec(),
            Read::Present(_) | Read::Absent => return Err(EngineError::NoLiveLogin),
            Read::Unreadable(e) => return Err(EngineError::Unreadable(e)),
        };
        Ok((row, bytes))
    }

    /// The oracle's word on the live credential, asked before any lock and only while its
    /// access token can still be shown (§7.6). An answer naming someone else refuses at once.
    fn corroborate(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        live: &[u8],
    ) -> Result<Option<OracleHint>, EngineError> {
        if expired(p, live, self.now_ms()) {
            return Ok(None);
        }
        let resolved = self.oracle.resolve(p, &Credential::fresh(live.to_vec()));
        if verdict(resolved.as_ref(), row) == OracleVerdict::OtherIdentity {
            return Err(EngineError::ForeignLiveCredential {
                position: row.position,
            });
        }
        Ok(Some(OracleHint {
            bytes: live.to_vec(),
            resolved,
        }))
    }

    /// §7.5 step 3's table, before any request. Generation order comes from the live store and
    /// fingerprints, never from access-token expiry (B.48).
    fn reconcile_active(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        lock: &AccountLock,
        live: &[u8],
        hint: Option<&OracleHint>,
    ) -> Result<Reconciled, EngineError> {
        let fp = |b: &[u8]| p.fingerprint(b);
        let relogin = || EngineError::NeedsRelogin {
            position: row.position,
            label: row.label.clone(),
        };
        let live_fp = fp(live).ok_or_else(relogin)?;
        let vault = match self.vault.read(&row.id) {
            Read::Present(v) => Some(v),
            Read::Absent => None,
            Read::Unreadable(e) => return Err(EngineError::Unreadable(e)),
        };
        // Tri-state (B.1): an unreadable `.prev` may be the generation that tells an
        // unpublished earlier pass apart from a CC rotation, so it refuses rather than reads as
        // absent, which would adopt a consumed token as the newest.
        let prev = match self.vault.read_prev(&row.id) {
            Read::Present(v) => Some(v),
            Read::Absent => None,
            Read::Unreadable(e) => return Err(EngineError::Unreadable(e)),
        };
        let mut rescues: Vec<RescueEntry> = Vec::new();
        for file in self.rescues_for(&row.id) {
            match file {
                RescueFile::Entry(entry) => rescues.push(entry),
                RescueFile::Unreadable { path, detail } => {
                    return Err(EngineError::RescuePending {
                        position: row.position,
                        label: row.label.clone(),
                        detail: format!("{}: {detail}", path.display()),
                    });
                }
            }
        }
        let is_live = |b: &[u8]| fp(b).as_ref() == Some(&live_fp);

        // In step with the vault. A rescue that succeeds this generation is published: the
        // vault now, the live store once the config lock is taken.
        if vault.as_deref().is_some_and(is_live) {
            if let Some(r) = rescues.iter().find(|r| r.predecessor_fp == live_fp.as_str()) {
                self.persist_generation(p, row, lock, &r.credential)?;
                return Ok(Reconciled {
                    current: r.credential.clone(),
                    publish: true,
                    changed: true,
                    retire: vec![r.path.clone()],
                });
            }
            // A rescue of exactly this generation has landed everywhere.
            let landed: Vec<&RescueEntry> =
                rescues.iter().filter(|r| is_live(&r.credential)).collect();
            for r in &landed {
                self.delete_rescue(&r.path)?;
            }
            return Ok(Reconciled {
                current: live.to_vec(),
                publish: false,
                changed: !landed.is_empty(),
                retire: vec![],
            });
        }

        // The vault's `.prev`: an earlier pass reached the vault but not the live store.
        if let (Some(v), Some(pr)) = (&vault, &prev) {
            if is_live(pr) {
                let vault_fp = fp(v);
                let retire = rescues
                    .iter()
                    .filter(|r| vault_fp.is_some() && fp(&r.credential) == vault_fp)
                    .map(|r| r.path.clone())
                    .collect();
                return Ok(Reconciled {
                    current: v.clone(),
                    publish: true,
                    changed: true,
                    retire,
                });
            }
        }

        // A rescue's generation: published, but its vault write failed.
        if let Some(r) = rescues.iter().find(|r| is_live(&r.credential)) {
            self.persist_generation(p, row, lock, &r.credential)?;
            self.delete_rescue(&r.path)?;
            return Ok(Reconciled {
                current: live.to_vec(),
                publish: false,
                changed: true,
                retire: vec![],
            });
        }

        // Any other full token pair: CC rotated it, so it is the newest generation, adopted
        // whatever its access token's expiry. An access-token-only blob never replaces the
        // vault's refresh token (§6.2).
        if !p.has_refresh_token(live) {
            return Err(relogin());
        }
        if verdict(answer_for(hint, live), row) == OracleVerdict::OtherIdentity {
            return Err(EngineError::ForeignLiveCredential {
                position: row.position,
            });
        }
        self.persist_generation(p, row, lock, live)?;
        for r in &rescues {
            self.delete_rescue(&r.path)?;
        }
        Ok(Reconciled {
            current: live.to_vec(),
            publish: false,
            changed: true,
            retire: vec![],
        })
    }

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
        let not_published = |why: &dyn std::fmt::Display| {
            tracing::warn!(
                position = row.position,
                "a refreshed credential was not published to the live store: {why}"
            );
            Ok(false)
        };
        let locks = match p.lock_config(&self.env, cred, p.live_lock_budget()) {
            Ok(locks) => locks,
            Err(e) => return not_published(&e),
        };
        let doomed = p.doomed(&self.env, &locks, LiveChange::Write(&row.kind));
        if let Err(e) = refuse_unreadable(&doomed) {
            return not_published(&e);
        }
        let mut held = Held::default();
        held.hold(p, secret);
        self.hold_vault(p, &mut held, &row.id);
        let mut warnings = Vec::new();
        for entry in doomed.iter().filter(|d| !d.on_fallback) {
            if let Read::Present(bytes) = &entry.bytes {
                if let Err(e) =
                    self.save_unheld(p, &row.provider, bytes, &mut held, false, None, &mut warnings)
                {
                    return not_published(&e);
                }
            }
        }
        let login = StoredLogin {
            kind: row.kind.clone(),
            secret: secret.to_vec(),
            identity: p.parse_identity(&row.identity_json)?,
        };
        let written = {
            let mut before_fallback = |bytes: &[u8]| {
                self.save_unheld(p, &row.provider, bytes, &mut held, false, None, &mut warnings)
                    .map_err(|e| {
                        ProviderError::Invalid(format!(
                            "could not save a credential the Keychain fallback would delete: {e}"
                        ))
                    })
            };
            // The undo is dropped, never run: it would write the consumed generation back.
            p.write_credential(&self.env, &locks, &login, &mut before_fallback)
                .map(|_| ())
        };
        for w in &warnings {
            tracing::warn!(position = row.position, "publishing a refreshed credential: {w}");
        }
        if let Err(e) = written {
            return not_published(&e);
        }
        let landed = match p.read_live_auth(&self.env).credential {
            Read::Present(c) if c.provenance() == Provenance::Fresh => {
                p.fingerprint(c.bytes()).is_some() && p.fingerprint(c.bytes()) == p.fingerprint(secret)
            }
            _ => false,
        };
        if landed {
            for path in retire {
                self.delete_rescue(path)?;
            }
        }
        Ok(landed)
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p tagteam-engine --features test-hooks --test active`
Expected: PASS, 26 tests.

Run: `cargo test -p tagteam-engine --test active && cargo test -p tagteam-engine --lib error`
Expected: PASS: 22 tests without the hook module; `kind_is_pinned_for_every_variant` passes.

Run: `cargo clippy -p tagteam-engine --all-targets --features test-hooks -- -D warnings`
Expected: no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam-engine/src/active.rs crates/tagteam-engine/src/lib.rs crates/tagteam-engine/src/error.rs crates/tagteam-engine/src/refresh.rs crates/tagteam-engine/tests/active.rs
git commit -m "Refresh the active token under CC's credential locks, reconciling first"
```

- [ ] **Step 7: Write the failing tests for the `Superseded` class**

A refresh that persists B but cannot publish it leaves live A, vault B and `.prev` A. Today the
next switch's outgoing capture (§9.4 step 4) compares live A with vault B only, takes A for a
rotation (or `Unresolved`), and writes the consumed A back over B. §9.4 step 4 (as amended)
leaves such a generation alone.

Add to the `tests` module of `crates/tagteam-core/src/classify.rs`:

```rust
    #[test]
    fn a_live_generation_the_vault_superseded_is_left_alone() {
        // §9.4 step 4 `Superseded`: an active-token refresh stored a newer generation it could
        // not publish. Capturing the live one would put a consumed token back, whatever the
        // oracle says; `.prev` keeps it, so leaving it alone loses nothing.
        for oracle in [
            OracleVerdict::ThisAccount,
            OracleVerdict::OtherIdentity,
            OracleVerdict::Unavailable,
        ] {
            let f = OutgoingFacts {
                equals_vault_prev: true,
                oracle,
                ..facts()
            };
            assert_eq!(
                decide_outgoing(&f),
                (OutgoingClass::Superseded, OutgoingAction::Nothing),
                "{oracle:?}"
            );
        }
        // The vault's own generation still classifies as `Ours` first.
        let f = OutgoingFacts {
            equals_vault_prev: true,
            fp_equal_vault: true,
            ..facts()
        };
        assert_eq!(decide_outgoing(&f).0, OutgoingClass::Ours);
    }
```

Append to the `hooks` module of `crates/tagteam-engine/tests/active.rs`:

```rust
    #[test]
    fn a_switch_never_captures_the_generation_an_unpublished_refresh_superseded() {
        // Codex round 2, §9.4 step 4 `Superseded`: the refresh persists B but cannot publish
        // it, leaving live A, vault B and `.prev` A. Switching away must leave B in the vault
        // (no capture of the consumed A, nothing displaced); switching back activates B.
        let fx = Fx::new();
        let b = fx.add("b@x.co", "rt-b");
        let a = fx.add("a@x.co", "rt-a");
        expire_live(&fx);
        fx.script_refresh(Some("rt-a2"));
        let refresh = take_over_after_response(&fx);
        assert_eq!(
            active(&fx, ActiveTrigger::Expired).unwrap(),
            ActiveOutcome::PersistedNotPublished
        );
        // The lock's new holder is gone; without this the switch would wait for it.
        fs::remove_dir(&refresh).unwrap();
        assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
        assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"));
        assert_eq!(prev_refresh_token(&fx, &a).as_deref(), Some("rt-a"));

        fx.switch_to(&b, false).unwrap();
        assert_eq!(
            fx.vault_refresh_token(&a).as_deref(),
            Some("rt-a2"),
            "the consumed live generation is never captured over the newer one"
        );
        assert!(
            fx.displaced().is_empty(),
            "the vault's .prev already keeps it: nothing to displace"
        );

        fx.switch_to(&a, false).unwrap();
        assert_eq!(
            fx.live_refresh_token().as_deref(),
            Some("rt-a2"),
            "switching back activates B, not the consumed A"
        );
        assert_eq!(token_requests(&fx), 1);
    }
```

- [ ] **Step 8: Run the tests to verify they fail**

Run: `cargo test -p tagteam-core classify`
Expected: FAIL to compile: "struct `OutgoingFacts` has no field named `equals_vault_prev`".

Run: `cargo test -p tagteam-engine --features test-hooks --test active a_switch_never_captures`
Expected: FAIL at "the consumed live generation is never captured over the newer one": the
vault holds `rt-a` (captured as `Unresolved`, with `rt-a2` pushed to `.prev`).

- [ ] **Step 9: Add the class and read `.prev` in step 4**

In `crates/tagteam-core/src/classify.rs`:

1. In `OutgoingFacts`, after `pub fp_equal_vault: bool,`, add:

```rust
    /// The live credential is the vault's `.prev` generation: an active-token refresh stored a
    /// newer one it could not publish (§7.5), so capturing this one would put a consumed token
    /// back (§9.4 step 4 `Superseded`).
    pub equals_vault_prev: bool,
```

2. In `OutgoingClass`, after `Ours,`, add:

```rust
    /// The vault holds a newer generation than the live one (§9.4 step 4, amended).
    Superseded,
```

3. In `decide_outgoing`, right after the `Ours` early return, add:

```rust
    if f.equals_vault_prev {
        return (OutgoingClass::Superseded, OutgoingAction::Nothing);
    }
```

and extend its doc comment's first sentence to "The §9.4 step 4 table, `Superseded` included,
with …".

4. In the `tests` module's `facts()`, after `fp_equal_vault: false,`, add
   `equals_vault_prev: false,`.

In `crates/tagteam-engine/src/switch.rs`, in `settle_outgoing`, replace

```rust
        let fp_live = p.fingerprint(&bytes);
        let resolved = answer_for(hint, &bytes);
        let facts = OutgoingFacts {
            bytes_equal_vault: vault.as_deref() == Some(bytes.as_slice()),
            fp_equal_vault: fp_live.is_some()
                && vault.as_deref().and_then(|v| p.fingerprint(v)) == fp_live,
```

with

```rust
        let fp_live = p.fingerprint(&bytes);
        let resolved = answer_for(hint, &bytes);
        let ours = vault
            .as_deref()
            .is_some_and(|v| same_generation(p, v, &bytes));
        // §9.4 step 4 `Superseded`: the vault's `.prev` is this generation, so an active-token
        // refresh stored a newer one it could not publish (§7.5). Read only when the live
        // credential is not the vault's own, and tri-state: an unreadable `.prev` may be
        // exactly that generation, and capturing over the newer one would lose it (step 3).
        let superseded = !ours
            && match self.vault.read_prev(&out.id) {
                Read::Present(prev) => same_generation(p, &prev, &bytes),
                Read::Absent => false,
                Read::Unreadable(e) => return Err(EngineError::Unreadable(e)),
            };
        let facts = OutgoingFacts {
            bytes_equal_vault: vault.as_deref() == Some(bytes.as_slice()),
            fp_equal_vault: fp_live.is_some()
                && vault.as_deref().and_then(|v| p.fingerprint(v)) == fp_live,
            equals_vault_prev: superseded,
```

`settle_outgoing`'s `OutgoingAction::Nothing` arm already covers the new class, and `transact`
holds the live generation afterwards, so step 7's rule does not displace it either (the vault's
`.prev` keeps it).

In `crates/tagteam-engine/src/recover.rs`, in Task 15's `capture_rotated_outgoing`, add after
`fp_equal_vault: …,` in its `OutgoingFacts` literal:

```rust
            // Recovery's held set covers the vault's `.prev` (`hold_vault`), so a superseded
            // generation returned early above and never reaches this classification.
            equals_vault_prev: false,
```

- [ ] **Step 10: Run the tests to verify they pass**

Run: `cargo test -p tagteam-core && cargo test -p tagteam-engine --features test-hooks`
Expected: PASS: the new core test, and 27 tests in `active` (`a_switch_never_captures…`
included); the switch and recovery suites still pass unchanged.

Run: `cargo clippy -p tagteam-core -p tagteam-engine --all-targets --features tagteam-engine/test-hooks -- -D warnings`
Expected: no warnings.

- [ ] **Step 11: Commit**

```bash
git add crates/tagteam-core/src/classify.rs crates/tagteam-engine/src/switch.rs \
  crates/tagteam-engine/src/recover.rs crates/tagteam-engine/tests/active.rs
git commit -m "Never capture a live generation the vault has already superseded"
```

---

### Task 17: Carried-over fixes (L319, L370, L380, L421, L444, L360)

Six items from the M1 final review's triage (`.superpowers/research/m1-run/final-review.md`),
each small and independent: one failing test, one fix, one commit apiece. L360 is a build
check, not a code change.

**Files:**
- Modify: `crates/tagteam-provider/src/atomic.rs` (L319)
- Modify: `crates/tagteam-provider/src/provider.rs` (L370)
- Modify: `crates/tagteam-cc/src/naming.rs` (L380)
- Modify: `crates/tagteam-engine/src/store/mod.rs`, test `crates/tagteam-engine/tests/store.rs` (L421)
- Modify: `crates/tagteam-engine/src/lifecycle.rs`, test `crates/tagteam-engine/tests/add.rs` (L444)

**Interfaces:**
- Consumes: M1 only (`write_atomic_mode_with`, `Identity`, `StoredLogin`, `passwd_name`,
  `Store::open`, `commit_login`, `Fx::engine_with_vault_probe`), plus Task 4's `AddTokenOptions`
  path through `kind_traits` (unchanged signature).
- Produces: no new names. `Identity`'s `Debug` becomes hand-written and redacting, the contract
  shape: `Identity { label: <redacted>, email: Some(<redacted>), org_uuid, org_name: …,
  account_uuid, raw: <redacted> }`.

- [ ] **Step 1: L319 — write the failing test**

In `crates/tagteam-provider/src/atomic.rs`, add to `mod tests`:

```rust
    #[test]
    fn a_panic_before_publication_leaves_no_temp_file() {
        // L319: a write that unwinds must not leave its 0600 temp file behind.
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.json");
        fs::write(&p, "old").unwrap();
        let unwound = std::panic::catch_unwind(|| {
            let _ = write_atomic_with(&p, b"new", 0o600, || -> io::Result<()> {
                panic!("the ownership check panicked")
            });
        });
        assert!(unwound.is_err());
        assert_eq!(fs::read(&p).unwrap(), b"old");
        assert_eq!(
            fs::read_dir(d.path()).unwrap().count(),
            1,
            "no temp file left"
        );
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p tagteam-provider --lib atomic::tests::a_panic_before_publication_leaves_no_temp_file`
Expected: FAIL at `no temp file left` (left: 2, right: 1).

- [ ] **Step 3: Remove the temp file through a guard**

In `crates/tagteam-provider/src/atomic.rs`, add above `fn write_atomic_mode_with`:

```rust
/// The temporary file of one write, removed unless it was published: after an error and while
/// unwinding from a panic alike. A killed process still leaves it behind; nothing in-process
/// can prevent that, and its name (`.<name>.tagteam-<pid>-<rand>`) marks it as tagteam's.
struct Temp<'a> {
    path: &'a Path,
    published: bool,
}

impl Drop for Temp<'_> {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_file(self.path);
        }
    }
}
```

In `write_atomic_mode_with`, replace everything from the `let mut file = OpenOptions::new()`
statement to the end of the function with:

```rust
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&tmp)?;
    let mut temp = Temp {
        path: &tmp,
        published: false,
    };
    let prepared = (|| {
        // Before any byte is written, so the umask can never widen a secret file.
        file.set_permissions(Permissions::from_mode(mode))?;
        file.write_all(bytes)?;
        file.sync_all()
    })();
    prepared
        .map_err(E::from)
        .and_then(|()| before_publish())
        .and_then(|()| fs::rename(&tmp, &target).map_err(E::from))?;
    temp.published = true;
    // Published: from here on nothing may report failure.
    let _ = File::open(dir).and_then(|d| d.sync_all());
    Ok(())
}
```

- [ ] **Step 4: Run the atomic tests**

Run: `cargo test -p tagteam-provider --lib atomic`
Expected: PASS, including `a_failed_pre_publication_check_publishes_nothing` and
`no_temp_files_are_left_behind`. The new test prints the injected panic's message; that is
expected.

- [ ] **Step 5: Commit**

```bash
git add crates/tagteam-provider/src/atomic.rs
git commit -m "Remove an atomic write's temp file when the write unwinds"
```

- [ ] **Step 6: L370 — write the failing test**

Check that nothing depends on `Identity`'s current `Debug` text:

Run: `rg -n 'Identity' crates --glob '*.rs' | rg ':\?\}|\{:#\?\}|dbg!'`
Expected: no output. (Assertion failure messages print it, but none compares it.)

In `crates/tagteam-provider/src/provider.rs`, add to `mod tests`:

```rust
    #[test]
    fn identity_debug_shows_no_email_label_or_name() {
        // L370, §4.4: logs identify accounts by position and ID, never by email.
        let id = Identity {
            label: "who@example.com".into(),
            email: Some("who@example.com".into()),
            org_uuid: "org-1".into(),
            org_name: Some("Acme Holdings".into()),
            account_uuid: Some("uuid-1".into()),
            raw: serde_json::json!({"emailAddress": "who@example.com", "organizationName": "Acme Holdings"}),
        };
        let login = StoredLogin {
            kind: "oauth".into(),
            secret: b"s".to_vec(),
            identity: id.clone(),
        };
        for shown in [format!("{id:?}"), format!("{login:?}")] {
            assert!(!shown.contains("who@example.com"), "{shown}");
            assert!(!shown.contains("Acme"), "{shown}");
        }
        let shown = format!("{id:?}");
        assert!(shown.contains("uuid-1") && shown.contains("org-1"), "{shown}");
    }
```

- [ ] **Step 7: Run it to verify it fails**

Run: `cargo test -p tagteam-provider --lib provider::tests::identity_debug_shows_no_email_label_or_name`
Expected: FAIL: the derived `Debug` prints `label: "who@example.com"`.

- [ ] **Step 8: Redact `Identity` and `StoredLogin`**

In `crates/tagteam-provider/src/provider.rs`, change `Identity`'s derive from
`#[derive(Debug, Clone, PartialEq)]` to `#[derive(Clone, PartialEq)]`, and add after the
struct:

```rust
/// Stands in for a value `Debug` must not show.
struct Redacted;

impl fmt::Debug for Redacted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// Emails, labels (CC: the email), organization names and the provider's raw object never reach
/// `Debug`: logs identify accounts by position and ID (§4.4). Uuids name no one.
impl fmt::Debug for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Identity")
            .field("label", &Redacted)
            .field("email", &self.email.as_ref().map(|_| Redacted))
            .field("org_uuid", &self.org_uuid)
            .field("org_name", &self.org_name.as_ref().map(|_| Redacted))
            .field("account_uuid", &self.account_uuid)
            .field("raw", &Redacted)
            .finish()
    }
}
```

In `impl fmt::Debug for StoredLogin`, replace `.field("identity", &self.identity.label)` with
`.field("identity", &self.identity)`.

- [ ] **Step 9: Run the provider tests**

Run: `cargo test -p tagteam-provider --lib provider`
Expected: PASS, including `debug_output_never_contains_secret_bytes`.

Run: `cargo test --workspace --features tagteam/test-support`
Expected: PASS: no test relied on the old `Debug` text.

- [ ] **Step 10: Commit**

```bash
git add crates/tagteam-provider/src/provider.rs
git commit -m "Keep emails and names out of an identity's Debug output"
```

- [ ] **Step 11: L380 — pin the passwd name**

In `crates/tagteam-cc/src/naming.rs`, add to `mod tests`:

```rust
    #[test]
    fn the_passwd_name_is_this_user_s_from_any_thread() {
        let id = std::process::Command::new("id").arg("-un").output().unwrap();
        let expected = String::from_utf8(id.stdout).unwrap().trim().to_owned();
        let names: Vec<Option<String>> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..8).map(|_| s.spawn(passwd_name)).collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        for name in names {
            assert_eq!(name.as_deref(), Some(expected.as_str()));
        }
    }
```

Run: `cargo test -p tagteam-cc --lib naming::tests::the_passwd_name_is_this_user_s_from_any_thread`
Expected: PASS already. `getpwuid`'s one static buffer is corrupted only by a truly
concurrent call, which no test can provoke on demand. This test is a pin, not a regression
test: it fixes the behaviour the re-entrant call must keep, and the fix itself is verified by
reading it. The commit message says so.

- [ ] **Step 12: Use `getpwuid_r`**

In `crates/tagteam-cc/src/naming.rs`, replace `fn passwd_name` with:

```rust
/// The effective user's passwd name, through the re-entrant `getpwuid_r` (L380): tagteam's
/// threads may resolve Keychain names at the same time, and `getpwuid` answers into one shared
/// static buffer.
fn passwd_name() -> Option<String> {
    // SAFETY: `sysconf` only reads a configuration value.
    let hint = unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) };
    let mut size = usize::try_from(hint).ok().filter(|n| *n > 0).unwrap_or(1024);
    loop {
        let mut buf: Vec<libc::c_char> = vec![0; size];
        // SAFETY: `passwd` holds only integers and pointers, for which all-zero is valid; it
        // is an out-parameter that `getpwuid_r` fills.
        let mut pw: libc::passwd = unsafe { std::mem::zeroed() };
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        // SAFETY: every pointer is to memory this frame owns, `buf.len()` is its true length,
        // and `getpwuid_r` writes only within them.
        let rc = unsafe {
            libc::getpwuid_r(
                libc::geteuid(),
                &mut pw,
                buf.as_mut_ptr(),
                buf.len(),
                &mut found,
            )
        };
        if rc == libc::ERANGE && size < 1 << 20 {
            size *= 2;
            continue;
        }
        if rc != 0 || found.is_null() {
            return None;
        }
        // SAFETY: on success `pw_name` points to a NUL-terminated string inside `buf`, which
        // lives until the end of this iteration.
        let name = unsafe { CStr::from_ptr(pw.pw_name) };
        return name.to_str().ok().map(str::to_owned);
    }
}
```

- [ ] **Step 13: Run the naming tests**

Run: `cargo test -p tagteam-cc --lib naming`
Expected: PASS.

- [ ] **Step 14: Commit**

```bash
git add crates/tagteam-cc/src/naming.rs
git commit -m "Read the passwd name with the re-entrant getpwuid_r" \
  -m "The added test is a pin, not a regression test: it passes on the old code, because a concurrent getpwuid call cannot be provoked on demand."
```

- [ ] **Step 15: L421 — write the failing test**

In `crates/tagteam-engine/tests/store.rs`, add:

```rust
/// `path` with SQLite's sidecar suffix (`-wal`, `-shm`).
fn sidecar(path: &std::path::Path, suffix: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("{}{suffix}", path.display()))
}

#[test]
fn the_database_and_its_sidecars_are_created_0600() {
    // L421, §6.1: the store names every account, so it is private from creation.
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("data/tagteam.db");
    let s = Store::open(&path).unwrap();
    s.set_active(&cc(), None).unwrap(); // a write: the WAL is in use
    for p in [path.clone(), sidecar(&path, "-wal"), sidecar(&path, "-shm")] {
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{}", p.display());
    }
}
```

- [ ] **Step 16: Run it to verify it fails**

Run: `cargo test -p tagteam-engine --test store the_database_and_its_sidecars_are_created_0600`
Expected: FAIL: `tagteam.db` is `0o644` (SQLite's default mode, less the umask).

- [ ] **Step 17: Create the file 0600 before SQLite opens it**

In `crates/tagteam-engine/src/store/mod.rs`, extend the imports:

```rust
use std::fs::{OpenOptions, Permissions};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
```

Add above `impl Store`:

```rust
/// Creates the database file with mode 0600 before SQLite first opens it (§6.1): it names
/// every account. SQLite gives its `-wal` and `-shm` files the database's own mode. A file
/// that already exists is left exactly as it is; an empty one is a valid empty database.
fn create_private(path: &Path) -> io::Result<()> {
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
    {
        // At creation, before any byte, so the umask can never widen it.
        Ok(file) => file.set_permissions(Permissions::from_mode(0o600)),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e),
    }
}
```

In `Store::open`, after the `ensure_private_dir(dir)?;` block and before `let conn = connect(path, true)?;`, add:

```rust
        create_private(path)?;
```

- [ ] **Step 18: Run the store tests**

Run: `cargo test -p tagteam-engine --test store`
Expected: PASS, including the WAL-mode and newer-schema tests (an empty file migrates like a
missing one).

- [ ] **Step 19: Commit**

```bash
git add crates/tagteam-engine/src/store/mod.rs crates/tagteam-engine/tests/store.rs
git commit -m "Create the store 0600 before SQLite opens it"
```

- [ ] **Step 20: L444 — write the failing test**

In `crates/tagteam-engine/tests/add.rs`, extend the imports with
`use std::collections::HashMap;`, `use std::sync::Mutex;`, `use common::API_KEY;` and
`use tagteam_engine::vault::SERVICE;`, then add:

```rust
#[test]
fn a_new_account_whose_vault_write_fails_its_check_leaves_no_secret_behind() {
    // L444: the write lands but reads back wrong, so the new account is rolled back. Its
    // secret must go with it, never outliving the account it belongs to (§5).
    let fx = Fx::new();
    let kc = fx.kc.clone();
    let reads = Mutex::new(HashMap::<String, usize>::new());
    let engine = fx.engine_with_vault_probe(move |key| {
        let mut reads = reads.lock().unwrap();
        let n = reads.entry(key.to_owned()).or_default();
        *n += 1;
        // `Vault::store` reads a key before its write and again to verify it: tamper between.
        if *n == 2 && !key.ends_with(".prev") {
            kc.put(SERVICE, key, b"tampered");
        }
    });

    let err = engine.add_token(fx.add_token_options(API_KEY)).unwrap_err();

    assert_eq!(err.kind(), "vault");
    let left: Vec<_> = fx
        .kc
        .items()
        .into_keys()
        .filter(|(service, _)| service == SERVICE)
        .collect();
    assert!(left.is_empty(), "{left:?}");
    assert!(
        fx.engine
            .store()
            .unwrap()
            .accounts(&fx.provider())
            .unwrap()
            .is_empty()
    );
}
```

- [ ] **Step 21: Run it to verify it fails**

Run: `cargo test -p tagteam-engine --test add a_new_account_whose_vault_write_fails_its_check_leaves_no_secret_behind`
Expected: FAIL: `left` holds the new account's `tagteam` item.

- [ ] **Step 22: Delete what the failed write left**

In `crates/tagteam-engine/src/lifecycle.rs`, in `commit_login`'s `None =>` branch, replace:

```rust
                if let Err(e) = self.vault.store(lock_for(&prep.id), secret, &fp) {
                    store.delete_account(&prep.id)?;
                    return Err(e.into());
                }
```

with:

```rust
                if let Err(e) = self.vault.store(lock_for(&prep.id), secret, &fp) {
                    // The write may have landed and failed only its read-back. A new account
                    // has no earlier generation to keep, so whatever it left goes with the row:
                    // no secret outlives its account (§5, L444).
                    if let Err(cleanup) = self.vault.delete(lock_for(&prep.id)) {
                        tracing::error!(
                            id = %prep.id,
                            "could not remove the vault entry of an account that was never added: {cleanup}"
                        );
                    }
                    store.delete_account(&prep.id)?;
                    return Err(e.into());
                }
```

- [ ] **Step 23: Run the add tests**

Run: `cargo test -p tagteam-engine --test add`
Expected: PASS.

- [ ] **Step 24: Commit**

```bash
git add crates/tagteam-engine/src/lifecycle.rs crates/tagteam-engine/tests/add.rs
git commit -m "Delete a new account's vault entry when its write fails verification"
```

- [ ] **Step 25: L360 — compile the Linux-gated code**

Only `aarch64-apple-darwin` has been built so far, so the `cfg(target_os = "linux")` code (for
example `process.rs`'s `start_of`) has never been compiled. The crates checked here are pure
Rust. `tagteam-engine` is left out because it pulls in `ring`, whose C code needs a Linux
cross-compiler.

Run:
```bash
rustup target add x86_64-unknown-linux-gnu
cargo check -p tagteam-provider -p tagteam-cc -p tagteam-fake --target x86_64-unknown-linux-gnu
```
Expected: `Finished` with no errors.
- If `rustup` cannot download the target from the sandbox, this step is Michael's: he runs
  the two commands and pastes the result into the task report.
- If `cargo check` reports errors, fix each one in the `cfg(target_os = "linux")` code it
  names, then rerun until clean, and commit with
  `git commit -m "Fix the Linux build of <the item named>"`.
- If it is clean, there is nothing to commit.

---

### Task 18: The CLI: wiring, errors, markers, and the single-flight race through the binary

Task 7 wires `UreqHttp` and `CachingOracle(HttpOracle)` into `build_engine`, along with the
`TAGTEAM_TEST_API_BASE` override. This task finishes the CLI side of M2a:
- the new refusals rendered as the user sees them (they flow through `EngineError::kind` and
  `Display`, so no new code, only tests that pin them)
- a `relogin required` marker in `list` and `status`
- the freshen warning on stderr and in `--json`'s `warnings`
- the refresh gate's single flight, proved across real processes

Every binary test here sets `TAGTEAM_TEST_API_BASE` itself: either to `http://127.0.0.1:9`
(connection refused, so `PreSend`, and never the real network) or to a `MockServer`.

**Files:**
- Modify: `crates/tagteam/src/render.rs` (markers and their unit tests)
- Modify: `crates/tagteam/tests/common/mod.rs` (the shared helpers `two_accounts`,
  `expire_vault`, `live_email`)
- Test: `crates/tagteam/tests/cli.rs`
- Create: `crates/tagteam/tests/gate_race.rs`

**Interfaces:**
- Consumes:
  - Task 2: `tagteam_provider::mock_server::{MockServer, MockReply}`. `MockServer::hits`
    counts a request as soon as the server has read it, before a `Delay` or `Hang`.
  - Task 4: `AccountView.kind: KindTraits`, and the list line's kind label from
    `kind.display`.
  - Task 7: the binary honours `TAGTEAM_TEST_API_BASE` under `test-support`.
  - Task 9: `Store::set_quarantine`, `AccountRow.quarantine_at`, and
    `EngineError::RescuePending`, whose detail names the file (Review Focus 4).
  - Task 10: the gate holds the account lock across its token request; `Busy` when it is held.
  - Task 13: `settle_rescues` runs in the switch transaction.
  - Task 14: freshen before activation with §7.2's table; `EngineError::NeedsRelogin`; exactly
    one warning in `SwitchOutcome.warnings` for a transient freshen failure.
- Produces: `render::list_human` and `render::status_human` markers for a quarantined account.
  No new public API. Shared helpers in `crates/tagteam/tests/common/mod.rs`, used by `cli.rs`
  and `gate_race.rs`: `two_accounts(root) -> (String, String)`,
  `expire_vault(root, id, in_ms)` and `live_email(root) -> String`.
- The mock server is already enabled for the binary's tests: Task 7 Step 16 edited
  `crates/tagteam/Cargo.toml`.

- [ ] **Step 1: Write the failing marker unit tests**

In `crates/tagteam/src/render.rs`, extend the test module's imports with
`use tagteam_core::AccountId;` and `use tagteam_provider::KindTraits;`, then add:

```rust
    /// `a@x.co`, the live OAuth account at position 1, quarantined or not.
    fn a_view(quarantined: bool) -> AccountView {
        AccountView {
            row: AccountRow {
                id: AccountId::from_string("0192"),
                provider: ProviderId::new(CLAUDE_CODE),
                position: 1,
                identity_key: "a@x.co\n".into(),
                label: "a@x.co".into(),
                email: Some("a@x.co".into()),
                org_uuid: String::new(),
                org_name: None,
                account_uuid: None,
                kind: "oauth".into(),
                alias: None,
                disabled: false,
                identity_json: json!({}),
                login_expires_at: None,
                login_epoch: 0,
                replacing_fp: None,
                quarantine_reason: quarantined.then(|| "invalid_grant".into()),
                quarantine_fp: None,
                quarantine_at: None,
                added_at: 1,
            },
            active: true,
            kind: KindTraits {
                refreshable: true,
                managed_key_axis: false,
                default_email_prefix: None,
                display: None,
            },
        }
    }

    fn one(view: AccountView) -> [ProviderAccounts; 1] {
        [ProviderAccounts {
            provider: ProviderId::new(CLAUDE_CODE),
            active_position: Some(1),
            accounts: vec![view],
        }]
    }

    #[test]
    fn a_quarantined_account_is_marked_for_a_new_login() {
        let names = |id: &str| id.to_owned();
        assert_eq!(
            list_human(&one(a_view(true)), &names),
            "* 1  a@x.co  relogin required\n"
        );
        assert_eq!(list_human(&one(a_view(false)), &names), "* 1  a@x.co\n");
        let status = StatusView::Managed {
            account: a_view(true),
            total: 1,
        };
        assert_eq!(
            status_human(&status),
            "Live: a@x.co (position 1 of 1), relogin required\n"
        );
        let status = StatusView::Managed {
            account: a_view(false),
            total: 1,
        };
        assert_eq!(status_human(&status), "Live: a@x.co (position 1 of 1)\n");
    }
```

- [ ] **Step 2: Write the binary tests**

First add the helpers `cli.rs` and `gate_race.rs` share to `crates/tagteam/tests/common/mod.rs`.
Change its `serde_json` import to `use serde_json::{Value, json};` and its `tagteam_provider`
import to `use tagteam_provider::{Env, FileKeychain, Keychain};`, and add
`use std::time::{SystemTime, UNIX_EPOCH};` and `use tagteam_engine::vault::SERVICE;`. Then add:

```rust
/// `a@x.co` at position 1 and `b@x.co` at position 2, both added through the binary with every
/// endpoint offline (`std_cmd`'s default); `b` is live. Returns their ids.
pub fn two_accounts(root: &Path) -> (String, String) {
    let env = Env::for_test(root);
    let kc = FileKeychain::new(root.join("keychain"));
    seed_home(&env);
    login(&env, &kc, "a@x.co", "", "rt-a");
    cmd(root).arg("add").assert().success();
    login(&env, &kc, "b@x.co", "", "rt-b");
    cmd(root).arg("add").assert().success();
    let out = cmd(root).args(["list", "--json"]).output().unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let id = |i: usize| v["accounts"][i]["id"].as_str().unwrap().to_owned();
    (id(0), id(1))
}

/// Rewrites account `id`'s vault copy so its access token expires `in_ms` from now: inside
/// the 10-minute freshen window (§7.2) when `in_ms` is below 600 000.
pub fn expire_vault(root: &Path, id: &str, in_ms: i64) {
    let kc = FileKeychain::new(root.join("keychain"));
    let mut v: Value = serde_json::from_slice(&kc.find(SERVICE, id).present().unwrap()).unwrap();
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64;
    v["claudeAiOauth"]["expiresAt"] = json!(now + in_ms);
    kc.upsert(SERVICE, id, v.to_string().as_bytes()).unwrap();
}

/// The email of the `oauthAccount` Claude Code is logged in as.
pub fn live_email(root: &Path) -> String {
    let config: Value =
        serde_json::from_slice(&fs::read(root.join("home/.claude.json")).unwrap()).unwrap();
    config["oauthAccount"]["emailAddress"]
        .as_str()
        .unwrap()
        .to_owned()
}
```

In `crates/tagteam/tests/cli.rs`, add `expire_vault`, `live_email` and `two_accounts` to its
`use common::{…}` list, and extend the imports with (`Keychain` is already imported by Task 7):

```rust
use tagteam_core::AccountId;
use tagteam_engine::store::Store;
```

and add:

```rust
/// The binary with every endpoint pointed at Task 7's `common::OFFLINE_API_BASE` (connection
/// refused: every endpoint fails `PreSend`, and nothing reaches the network). `cmd` already sets
/// it; naming it here keeps each test's reliance on it visible.
fn offline(root: &Path) -> assert_cmd::Command {
    let mut c = cmd(root);
    c.env("TAGTEAM_TEST_API_BASE", common::OFFLINE_API_BASE);
    c
}

fn quarantine(root: &Path, id: &str) {
    Store::open_existing(&Env::for_test(root).data_dir().join("tagteam.db"))
        .unwrap()
        .unwrap()
        .set_quarantine(&AccountId::from_string(id), "invalid_grant", "sha256:0", 1)
        .unwrap();
}

#[test]
fn list_and_status_mark_quarantined_accounts() {
    let d = tempfile::tempdir().unwrap();
    let (a, b) = two_accounts(d.path());
    quarantine(d.path(), &a);
    quarantine(d.path(), &b);
    offline(d.path())
        .arg("list")
        .assert()
        .success()
        .stdout("  1  a@x.co  relogin required\n* 2  b@x.co  relogin required\n");
    offline(d.path())
        .arg("status")
        .assert()
        .success()
        .stdout("Live: b@x.co (position 2 of 2), relogin required\n");
    let out = offline(d.path()).args(["list", "--json"]).output().unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["accounts"][0]["usageStatus"], "relogin_required");
}

#[test]
fn a_quarantined_target_that_needs_a_refresh_is_refused_with_a_relogin_message() {
    // §7.2: a quarantined target is never refreshed; one that would need it is refused as
    // Dead is, before anything is locked or written.
    const MESSAGE: &str = "a@x.co (position 1) needs a new login: its stored refresh token can no longer be used; log in with `claude`, then run `tagteam add`";
    let d = tempfile::tempdir().unwrap();
    let (a, _) = two_accounts(d.path());
    quarantine(d.path(), &a);
    expire_vault(d.path(), &a, 60_000);
    offline(d.path())
        .args(["switch", "1"])
        .assert()
        .code(1)
        .stderr(format!("tagteam: {MESSAGE}\n"));
    let out = offline(d.path())
        .args(["switch", "1", "--json"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        serde_json::from_slice::<Value>(&out).unwrap(),
        json!({"schemaVersion": 1, "error": {"type": "relogin-required", "message": MESSAGE}})
    );
    assert_eq!(live_email(d.path()), "b@x.co");
}

#[test]
fn an_unreadable_rescue_blocks_the_switch_and_names_the_file() {
    // Review Focus 4, through the binary: the vault's generation may be consumed, so it is
    // never activated while a rescue for the account cannot be read (§6.2).
    let d = tempfile::tempdir().unwrap();
    let (a, _) = two_accounts(d.path());
    let dir = Env::for_test(d.path()).data_dir().join("rescue");
    std::fs::create_dir_all(&dir).unwrap();
    let name = format!("{a}-0-000000000000.json");
    std::fs::write(dir.join(&name), "{ truncated").unwrap();

    let out = offline(d.path())
        .args(["switch", "1", "--json"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["error"]["type"], "rescue-pending");
    let message = v["error"]["message"].as_str().unwrap();
    assert!(
        message.starts_with(
            "a@x.co (position 1) has a refreshed token that is not in the vault yet: "
        ),
        "{message}"
    );
    assert!(message.contains(&name), "{message}");
    assert!(
        message.ends_with("; retry once the vault can be written"),
        "{message}"
    );
    offline(d.path())
        .args(["switch", "1"])
        .assert()
        .code(1)
        .stderr(predicates::str::starts_with(
            "tagteam: a@x.co (position 1) has a refreshed token that is not in the vault yet: ",
        ));
    assert_eq!(live_email(d.path()), "b@x.co");
}

#[test]
fn an_offline_refresh_before_a_switch_warns_once_and_still_switches() {
    // Review Focus 1, through the binary: a transient failure proceeds with the vault's
    // generation and a warning (§7.2), on stderr and in `warnings`, and never names a token.
    let d = tempfile::tempdir().unwrap();
    let (a, _) = two_accounts(d.path());
    expire_vault(d.path(), &a, 60_000);

    let out = offline(d.path())
        .args(["switch", "1", "--json"])
        .assert()
        .success()
        .get_output()
        .clone();

    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!((v["switched"].clone(), v["to"].clone()), (json!(true), json!(1)));
    let warnings = v["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(stderr.matches("warning: ").count(), 1, "{stderr}");
    assert!(stderr.contains(warnings[0].as_str().unwrap()), "{stderr}");
    assert!(!stderr.contains("rt-a"), "never a token: {stderr}");
    assert_eq!(live_email(d.path()), "a@x.co");
}
```

- [ ] **Step 3: Run the tests to verify the marker tests fail**

Run: `cargo test -p tagteam --lib render && cargo test -p tagteam --features test-support --test cli`
Expected:
- `a_quarantined_account_is_marked_for_a_new_login` and
  `list_and_status_mark_quarantined_accounts` FAIL: there is no marker yet.
- The three switch tests PASS. They pin Tasks 13 and 14 through the binary; a failure there is
  a defect in that task's code, fixed there, not here.

- [ ] **Step 4: Add the markers**

In `crates/tagteam/src/render.rs`, in `list_human`, directly after:

```rust
            if r.disabled {
                line.push_str("  disabled");
            }
```

add:

```rust
            if r.quarantine_reason.is_some() {
                line.push_str("  relogin required");
            }
```

In `status_human`, replace the `StatusView::Managed` arm with:

```rust
        StatusView::Managed { account, total } => {
            let marker = if account.row.quarantine_reason.is_some() {
                ", relogin required"
            } else {
                ""
            };
            format!(
                "Live: {} (position {} of {total}){marker}\n",
                name(&account.row),
                account.row.position
            )
        }
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p tagteam --lib render && cargo test -p tagteam --features test-support --test cli --test app`
Expected: PASS. The `app.rs` snapshots are unchanged, since none of their accounts is quarantined.

- [ ] **Step 6: Commit**

```bash
git add crates/tagteam/src/render.rs crates/tagteam/tests/common/mod.rs crates/tagteam/tests/cli.rs
git commit -m "Mark quarantined accounts and pin the freshen refusals through the binary"
```

- [ ] **Step 7: Write the race tests**

Create `crates/tagteam/tests/gate_race.rs`:

```rust
//! §15.2 concurrency, through the real binary: the refresh gate is single-flight per account,
//! and a suspended holder is never preempted (§7.3 step 1, B.38). Needs `--features
//! test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::collections::BTreeSet;
use std::path::Path;
use std::process::{Child, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use common::{cmd, expire_vault, live_email, std_cmd, two_accounts};
use serde_json::{Value, json};
use tagteam_engine::vault::SERVICE;
use tagteam_provider::mock_server::{MockReply, MockServer};
use tagteam_provider::{FileKeychain, Keychain};

const API_BASE: &str = "TAGTEAM_TEST_API_BASE";
const TOKEN: &str = "/v1/oauth/token";

/// `a@x.co` at position 1 and `b@x.co` at 2, `b` live; `a`'s access token inside the freshen
/// window (§7.2), so `switch 1` refreshes it through the gate first. Returns `a`'s id.
fn expiring_target(root: &Path) -> String {
    let (a, _) = two_accounts(root);
    expire_vault(root, &a, 60_000);
    a
}

fn token_reply(rt: &str) -> MockReply {
    MockReply::Json {
        status: 200,
        body: json!({"access_token": format!("at-{rt}"), "refresh_token": rt,
                     "expires_in": 28800, "scope": "user:inference user:profile"}),
    }
}

fn spawn_switch(root: &Path, server: &MockServer) -> Child {
    std_cmd(root)
        .env(API_BASE, server.base_url())
        .args(["switch", "1", "--json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

/// Returns once the server has read the token request: the sender's gate now holds the
/// account lock and is waiting for the reply (§7.3 steps 1 and 5).
fn wait_for_token_request(server: &MockServer) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while server.hits("POST", TOKEN) == 0 {
        assert!(Instant::now() < deadline, "the token request never arrived");
        thread::sleep(Duration::from_millis(20));
    }
}

fn signal(child: &Child, sig: libc::c_int) {
    // SAFETY: `kill` only sends a signal to our own child, whose pid is still ours to use:
    // it has not been waited for.
    assert_eq!(unsafe { libc::kill(child.id() as libc::pid_t, sig) }, 0);
}

fn json_out(child: Child) -> (bool, Value, String) {
    let out = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let v = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}\n{stderr}", String::from_utf8_lossy(&out.stdout)));
    (out.status.success(), v, stderr)
}

fn vault_refresh_token(root: &Path, id: &str) -> String {
    let kc = FileKeychain::new(root.join("keychain"));
    let v: Value = serde_json::from_slice(&kc.find(SERVICE, id).present().unwrap()).unwrap();
    v["claudeAiOauth"]["refreshToken"].as_str().unwrap().to_owned()
}

#[test]
fn a_switch_that_finds_the_target_refreshing_waits_and_activates_its_successor() {
    // §7.2's `Busy` row: B proceeds, waits for the account lock, and its locked re-read picks
    // up A's refresh. One request in all.
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    let a = expiring_target(root);
    let server = MockServer::start();
    server.on(
        "POST",
        TOKEN,
        MockReply::Delay(Duration::from_secs(3), Box::new(token_reply("rt-a2"))),
    );

    let holder = spawn_switch(root, &server);
    wait_for_token_request(&server);
    signal(&holder, libc::SIGSTOP);
    let waiter = spawn_switch(root, &server);
    // B finds the lock held (`Busy`), takes the mutation lock, and waits for the account lock.
    thread::sleep(Duration::from_secs(1));
    // Well inside A's 10 s request timeout; the reply arrives 3 s after the request.
    signal(&holder, libc::SIGCONT);

    let (b_ok, b, b_err) = json_out(waiter);
    let (a_ok, a_out, a_err) = json_out(holder);
    assert!(a_ok && b_ok, "A: {a_out} {a_err}\nB: {b} {b_err}");
    assert_eq!(server.hits("POST", TOKEN), 1, "single flight");
    assert_eq!(vault_refresh_token(root, &a), "rt-a2");
    assert_eq!(live_email(root), "a@x.co");
    // Whichever took the mutation lock first switched; the other found the work done.
    let reasons: BTreeSet<String> = [&a_out, &b]
        .iter()
        .map(|v| v["reason"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        reasons,
        BTreeSet::from(["already-active".to_owned(), "switched".to_owned()])
    );
}

#[test]
#[ignore = "waits out the full 15 s account-lock timeout; run with --ignored"]
fn a_refresh_holder_stopped_past_every_timeout_is_never_preempted() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    let a = expiring_target(root);
    let server = MockServer::start();
    // The reply comes long after A's own 10 s request timeout: A never receives a successor.
    server.on(
        "POST",
        TOKEN,
        MockReply::Delay(Duration::from_secs(60), Box::new(token_reply("rt-a2"))),
    );

    let holder = spawn_switch(root, &server);
    wait_for_token_request(&server);
    signal(&holder, libc::SIGSTOP);

    // B: `Busy`, then the account lock A still holds, for all of `AccountLock::WAIT`.
    let started = Instant::now();
    let out = cmd(root)
        .env(API_BASE, server.base_url())
        .args(["switch", "1", "--json"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["error"]["type"], "lock-timeout", "{v}");
    assert!(started.elapsed() >= Duration::from_secs(15));
    assert_eq!(
        server.hits("POST", TOKEN),
        1,
        "B never sends the generation A holds"
    );

    signal(&holder, libc::SIGCONT);
    let (ok, v, stderr) = json_out(holder);
    // A's request was sent and its reply never read (`ambiguous`): the switch proceeds with
    // the vault's generation and one warning (§7.2).
    assert!(ok, "{v}\n{stderr}");
    assert_eq!(v["switched"], true);
    assert_eq!(v["warnings"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(server.hits("POST", TOKEN), 1);
    assert_eq!(vault_refresh_token(root, &a), "rt-a");
    assert_eq!(live_email(root), "a@x.co");
}
```

- [ ] **Step 8: Run the race tests**

Run: `cargo test -p tagteam --features test-support --test gate_race`
Expected: PASS, 1 test in about 5 s; 1 ignored.

Run: `cargo test -p tagteam --features test-support --test gate_race -- --ignored`
Expected: PASS, 1 test in about 17 s.

These tests add no product code. A failure is a defect in the gate (Task 10) or in freshen
(Task 14): fix it there and rerun.

- [ ] **Step 9: Commit**

```bash
git add crates/tagteam/tests/gate_race.rs
git commit -m "Prove the refresh gate's single flight across processes"
```

---

### Task 19: Final verification and live acceptance

**Human step (Steps 4–6).** Steps 1–3 are the implementer's. The live acceptance run spends
one real refresh token of a spare account, so Michael runs it, and the merge waits for it. The
whole-branch review and the pre-merge cross-review run after Step 3 and before the pull
request. They follow the global workflow and are not steps of this plan.

**Files:**
- Modify: `docs/superpowers/plans/2026-09-29-tagteam-m2a-network-credentials.md` (the
  `**Status:**` line)

**Interfaces:**
- Consumes: everything Tasks 2–18 produced.
- Produces: nothing new.

- [ ] **Step 1: Format, lint and test everything**

Run:
```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --features tagteam/test-support -- -D warnings
cargo test --workspace --features tagteam/test-support
cargo test --workspace --features tagteam/test-support -- --ignored
cargo test -p tagteam --lib
```
Expected:
- No diff and no warnings (`tagteam-fake` included).
- Every test passes. The `--ignored` run includes the 9 s CC-lock test and
  `gate_race`'s 15 s stopped-holder test.
- `cargo test -p tagteam --lib` runs without features, so it exercises the release branch of
  the test-override checks.

On a Mac, also run
`cargo test -p tagteam-provider --features real_keychain --test real_keychain` (PASS, no GUI
prompt).

Run: `cargo check -p tagteam-provider -p tagteam-cc -p tagteam-fake --target x86_64-unknown-linux-gnu`
Expected: `Finished`, as in Task 17 Step 25.

- [ ] **Step 2: Build the release binary and check it carries no test hooks**

Run:
```bash
cargo build --release -p tagteam
strings target/release/tagteam | grep TAGTEAM_TEST_ ; test $? -eq 1
```
Expected: the build succeeds and `grep` finds nothing (exit 1), so the final `test` exits 0.
The keychain-directory, platform and API-base overrides exist only under `test-support`.

- [ ] **Step 3: Check it read-only against the real login**

Run: `./target/release/tagteam status && ./target/release/tagteam list`
Expected:
- `status` names the live Claude Code login.
- `list` shows the stored accounts, or says there are none.
- Neither command sends a request or writes anything: neither asks the oracle nor refreshes.
Running it is Michael's call, as in M1, since it reads the real HOME.

- [ ] **Step 4: Michael — prepare the live acceptance run**

The goal: tagteam's refresh gate refreshes a real token, and Claude Code runs on the successor.
Use a spare account `S`, never the daily one `M`.

1. If `S` is not stored yet: `claude /login` as `S`, then `tagteam add`. Log back in as `M`
   (`claude /login`, or `tagteam switch <M>` if `M` is stored).
2. Read the vault without printing a secret. This helper prints only the access token's expiry
   and the refresh token's fingerprint prefix. On macOS:

```bash
tt_vault() {
  security find-generic-password -s tagteam -a "$1" -w \
    | /usr/bin/python3 -c 'import json,sys,hashlib; o=json.load(sys.stdin)["claudeAiOauth"]; print(o.get("expiresAt"), hashlib.sha256(o["refreshToken"].encode()).hexdigest()[:12])'
}
```

   On Linux, feed it `~/.local/share/tagteam/vault/<id>.json` instead of the `security` call.
   To find `S`'s id:
   `tagteam list --json | /usr/bin/python3 -c 'import json,sys; [print(a["position"], a["id"], a["email"]) for a in json.load(sys.stdin)["accounts"]]'`.
3. Leave `S` inactive until `tt_vault <S-id>` shows an `expiresAt` within 10 minutes of
   `date +%s000`, or already past it (typically a few hours after `S` was last live). Record
   that expiry `E0` and the fingerprint prefix `F0`.

- [ ] **Step 5: Michael — run the acceptance**

| # | Command | Pass when |
|---|---|---|
| 1 | `tagteam switch <S>` | `Switched to …`, with no `warning:` line (freshen refreshed `S` through the gate) |
| 2 | `tt_vault <S-id>` | `expiresAt` is hours in the future; the prefix `F1` differs from `F0` (if the endpoint kept the refresh token, `F1 = F0` with a new expiry: record which) |
| 3 | `ls ~/.local/share/tagteam/rescue ~/.local/share/tagteam/displaced 2>&1` | no rescue file; no new displaced file |
| 4 | `claude auth status --json`, then `claude -p "reply with the single word ok"` | `loggedIn: true` with `S`'s email; the reply is `ok` |
| 5 | `tagteam switch <M>` | `Switched to …`, with no warning |
| 6 | `tt_vault <S-id>` and step 3's `ls` | still `F1` (Claude Code ran on tagteam's successor, and the outgoing capture found the vault's own generation); no displaced or rescue file |
| 7 | `tagteam list` | no `relogin required` on any account |

Optional, extended: the rotation capture.
1. Leave `S` live past its access token's expiry and use Claude Code, so Claude Code refreshes
   the token itself.
2. Run `tagteam switch <M>`.
3. `tt_vault <S-id>` should now show a new prefix `F2`, captured from the live store, with no
   displaced file and no `relogin required`.

- [ ] **Step 6: Decide and record**

- **Pass** (rows 1–7): put the table into the pull request description. Record expiries as
  times relative to the run, and fingerprints as the 12-character prefixes only. Never a
  token. Then continue with Step 7.
- **Fail:** stop. Do not open or merge the pull request. Bring the table to Michael: a
  failure here means Appendix A.5, the gate (Task 10–11), or freshen (Task 14) disagrees with
  the real endpoint.

- [ ] **Step 7: Mark the plan implemented**

Once implementation and review are complete and merging is the next action, set this plan's
`**Status:**` line to `Implemented — <pull request URL>`, per the design-record lifecycle. The
spec stays `In progress` until M5.

```bash
git add docs/superpowers/plans/2026-09-29-tagteam-m2a-network-credentials.md
git commit -m "Mark the M2a plan implemented"
```

---

