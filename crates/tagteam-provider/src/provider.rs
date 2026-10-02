use std::fmt;
use std::io;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;
use tagteam_core::backoff::parse_retry_after;
use tagteam_core::pace::Pace;
use tagteam_core::poll::PollBudget;
use tagteam_core::usage::Window;
use tagteam_core::{Fingerprint, IdentityKey, ProviderId};

use crate::credential::{Credential, FreshCredential};
use crate::env::Env;
use crate::flock::MutationGuard;
use crate::http::{Http, HttpError, HttpResponse};
use crate::keychain::KeychainError;
use crate::mkdir_lock::LockError;
use crate::read::{Read, ReadError};

/// A login's identity. `raw` is the provider-owned object stored in `identity_json`
/// (CC: the `oauthAccount` object).
#[derive(Clone, PartialEq)]
pub struct Identity {
    pub label: String,
    pub email: Option<String>,
    pub org_uuid: String,
    pub org_name: Option<String>,
    pub account_uuid: Option<String>,
    pub raw: Value,
}

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
            .field("identity", &self.identity)
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

/// Both auth axes, each tri-state: the credential entry and the managed API key. `Debug` is
/// derived: `Read<T>`'s own `Debug` already redacts the payload of both fields, so a
/// hand-written impl here would only risk diverging from it.
#[derive(Debug, Clone)]
pub struct LiveAuth {
    pub credential: Read<Credential>,
    pub managed_key: Read<Vec<u8>>,
}

/// A change to the live login that destroys what it replaces (§9.4 step 7).
#[derive(Debug, Clone, Copy)]
pub enum LiveChange<'k> {
    /// `write_credential` for a target of this kind: its axis written, the other cleared.
    Write(&'k str),
    /// `clear_other_axis`, keeping this kind's axis.
    ClearOther(&'k str),
}

/// A live entry holding secrets that a `LiveChange` overwrites or deletes. `Debug` is derived:
/// `Read<T>`'s own `Debug` redacts the bytes.
#[derive(Debug, Clone)]
pub struct DoomedEntry {
    /// Its current contents.
    pub bytes: Read<Vec<u8>>,
}

/// Called by `write_credential` with each live entry a Keychain-refusal fallback is about to
/// delete, immediately before it does, under the live locks. An error aborts the write, which
/// then restores what it changed.
pub type BeforeFallback<'a> = &'a mut dyn FnMut(&[u8]) -> Result<(), ProviderError>;

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

pub trait LiveLockSet: Send {
    fn check_owned(&self) -> Result<(), LockError>;
}

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

/// Restores what one write replaced, for same-process rollback (§9.4 step 10). Every restore
/// re-checks lock ownership first: after a takeover, CC may have written since, and restoring
/// would overwrite it (§9.1).
///
/// A `Provider` returns `Box<dyn Undo + 'l>` tied to the `&'l LiveLocks<'_>` borrow that
/// produced it (§9.4 step 10: the credential locks must be "held throughout"), so the box
/// cannot outlive the locks it must run under. Dropping the locks while an undo made from
/// them is still alive is a borrow-checker error, not a runtime one:
///
/// ```compile_fail
/// use std::time::Duration;
/// use tagteam_provider::{Env, LiveLockSet, LiveLocks, LockError, MutationGuard, ProviderError, Undo};
///
/// struct AlwaysOwned;
/// impl LiveLockSet for AlwaysOwned {
///     fn check_owned(&self) -> Result<(), LockError> {
///         Ok(())
///     }
/// }
///
/// struct NoopUndo;
/// impl Undo for NoopUndo {
///     fn undo(self: Box<Self>, _locks: &LiveLocks<'_>) -> Result<(), ProviderError> {
///         Ok(())
///     }
///     fn what(&self) -> String {
///         "noop".into()
///     }
/// }
///
/// fn write_credential<'l>(locks: &'l LiveLocks<'_>) -> Box<dyn Undo + 'l> {
///     Box::new(NoopUndo)
/// }
///
/// let dir = tempfile::tempdir().unwrap();
/// let env = Env::for_test(dir.path());
/// let guard = MutationGuard::acquire(&env, Duration::from_millis(100)).unwrap();
/// let locks = LiveLocks::new(&guard, Box::new(AlwaysOwned));
/// let undo = write_credential(&locks);
/// drop(locks); // still borrowed by `undo`: cannot move out of `locks`
/// let _ = undo.what(); // `undo` is alive here, so the borrow conflict is the only error
/// ```
pub trait Undo: Send {
    fn undo(self: Box<Self>, locks: &LiveLocks<'_>) -> Result<(), ProviderError>;
    fn what(&self) -> String;
}

/// Where a credential write put the secret, as that one write decided it (Appendix A.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretStore {
    /// The OS keychain.
    Keychain,
    /// A file that is the platform's only store for it (Linux).
    File(PathBuf),
    /// A file, because the keychain refused this write or an earlier one of the same operation
    /// (Appendix A.3).
    Fallback(PathBuf),
}

/// What `Provider::write_credential` did: its undo, tied to the locks' borrow, and where the
/// secret went.
pub struct Written<'l> {
    pub undo: Box<dyn Undo + 'l>,
    pub stored_in: SecretStore,
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("{0}")]
    Unreadable(ReadError),
    /// A config file that is torn or not a JSON object is never replaced (§9.5). `remedy` is
    /// the provider's advice on repairing it.
    #[error("{} is torn or not a JSON object; {remedy}", path.display())]
    ConfigUnsplicable { path: PathBuf, remedy: &'static str },
    /// A Keychain item `remove_item` could not verify gone: after a file fallback, after a
    /// managed-key fallback, when a managed key is removed, or when a profile's credential is
    /// deleted. Claude Code may still read it (L397); the wording holds for a removal as much
    /// as for a write.
    #[error("the Keychain item {0} could not be verified gone, so Claude Code may still read it")]
    ShadowingItem(String),
    /// A write failed part-way and restoring the previous state failed too: the live state is
    /// partial, and only journal recovery may settle it.
    #[error("{cause}; restoring the previous state also failed: {restore}")]
    RestoreFailed {
        cause: Box<ProviderError>,
        restore: Box<ProviderError>,
    },
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
    #[error(transparent)]
    Lock(#[from] LockError),
    #[error(transparent)]
    Keychain(#[from] KeychainError),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("{0}")]
    Invalid(String),
}

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

/// The provider's verdict on one usage request (§8.1). The engine owns the budget, the lease,
/// the token and what is recorded (§8.3).
#[derive(Debug, Clone, PartialEq)]
pub enum UsageResult {
    /// 200 with a recognised body. Empty means "no usage" (§8.2).
    Windows(Vec<Window>),
    /// The credential has no access token; no request was sent (§8.1).
    NoAccessToken,
    /// 401 (§8.1: the caller decides between the gate and §7.5).
    Unauthorized,
    /// Anything else: `Http(429)` carries `retry_after_s`.
    Failed {
        kind: TransientKind,
        retry_after_s: Option<f64>,
    },
}

impl UsageResult {
    /// §8.1's verdict on one usage reply. A 200 whose body `normalize` accepts is `Windows`,
    /// empty when it names no window (§8.2); a 200 that is not JSON, or that `normalize`
    /// rejects, is `bad-response`. A 401 is the caller's to handle (§8.1). Any other status is
    /// `Http(status)` with `Retry-After` in its seconds form, when there is one (§8.1, §8.5).
    pub fn from_reply(
        reply: Result<HttpResponse, HttpError>,
        normalize: impl FnOnce(&Value) -> Option<Vec<Window>>,
    ) -> UsageResult {
        let failed = |kind| UsageResult::Failed {
            kind,
            retry_after_s: None,
        };
        let resp = match reply {
            Ok(r) => r,
            Err(HttpError::PreSend(_)) => return failed(TransientKind::PreSend),
            Err(HttpError::Ambiguous(_)) => return failed(TransientKind::Ambiguous),
        };
        match resp.status {
            200 => match resp.json().as_ref().and_then(normalize) {
                Some(windows) => UsageResult::Windows(windows),
                None => failed(TransientKind::BadResponse),
            },
            401 => UsageResult::Unauthorized,
            status => UsageResult::Failed {
                kind: TransientKind::Http(status),
                retry_after_s: resp.header("retry-after").and_then(parse_retry_after),
            },
        }
    }
}

pub trait Provider: Send + Sync {
    fn id(&self) -> ProviderId;
    fn display_name(&self) -> &'static str;
    fn capabilities(&self) -> Capabilities;
    fn identity_surface(&self, env: &Env) -> IdentitySurface;
    fn identity_key(&self, id: &Identity) -> IdentityKey;
    fn credential_kinds(&self) -> &'static [&'static str];
    fn kind_traits(&self, kind: &str) -> KindTraits;
    /// The window consume-first ranks on (§4.5, §11.2 step 8): the key of a `Long` window this
    /// provider's normalization produces (§8.2). `None` when it offers none; a consume-first
    /// strategy then runs `best` for this provider.
    fn primary_long_window(&self) -> Option<&'static str>;
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
    /// Epoch ms of the access token's expiry; `None` when absent or not an integer (§7.2).
    fn access_expires_at(&self, secret: &[u8]) -> Option<i64>;
    /// The access token's own fingerprint. The gate's "someone already refreshed" check
    /// (§7.3 step 4) and M2b's `rejected_fp` compare access tokens, not lineages.
    fn access_fingerprint(&self, secret: &[u8]) -> Option<Fingerprint>;

    /// `Absent` means there is no live login.
    fn live_identity(&self, env: &Env) -> Read<Identity>;
    fn read_live_auth(&self, env: &Env) -> LiveAuth;
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
    /// Every live entry holding secrets that `change` overwrites or deletes, on either auth
    /// axis, read now under `locks` (§9.4 step 7): the entries it writes or clears, and the
    /// copies of them no reader sees that go with them.
    fn doomed(&self, env: &Env, locks: &LiveLocks<'_>, change: LiveChange<'_>) -> Vec<DoomedEntry>;
    /// Composes the target (§9.4 step 5), writes it on its axis, then clears the other axis
    /// (step 7). Refuses when an entry it would overwrite cannot be read fresh. The returned
    /// undo is tied to `locks`'s borrow (§9.4 step 10: the credential locks must be "held
    /// throughout") and cannot outlive it; `stored_in` is where this write put the secret.
    fn write_credential<'l>(
        &self,
        env: &Env,
        locks: &'l LiveLocks<'_>,
        target: &StoredLogin,
        before_fallback: BeforeFallback<'_>,
    ) -> Result<Written<'l>, ProviderError>;
    /// Clears the auth axis other than `kept_kind`'s (§9.6 finish-forward).
    fn clear_other_axis<'l>(
        &self,
        env: &Env,
        locks: &'l LiveLocks<'_>,
        kept_kind: &str,
    ) -> Result<Box<dyn Undo + 'l>, ProviderError>;
    /// Splices the identity into the live config; `None` removes it (§9.4 step 8).
    fn write_identity<'l>(
        &self,
        env: &Env,
        locks: &'l LiveLocks<'_>,
        identity: Option<&Identity>,
    ) -> Result<Box<dyn Undo + 'l>, ProviderError>;

    /// Who owns this credential's access token (§7.6). Advisory: a failure is `None`, never an
    /// error. `None`, with no request sent, when the credential has no token it can show:
    /// an expired access token (`now_ms + 5 min ≥ expiresAt`), or a kind the provider cannot
    /// resolve.
    fn resolve_owner(&self, http: &dyn Http, cred: &Credential, now_ms: i64) -> Option<Identity>;

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

    /// §8.1, the provider's half: sends one usage request with the credential's access token
    /// and classifies the reply into generic windows (§8.2). `NoAccessToken`, with nothing
    /// sent, when the credential has none. It never refreshes and never checks expiry: the
    /// engine owns the gate, §7.5, the budget slot and the lease (§8.3).
    fn fetch_usage(&self, http: &dyn Http, cred: &Credential) -> UsageResult;
    /// §8.6's constants and the hourly request cap of this provider's usage endpoint.
    fn poll_budget(&self) -> PollBudget;
    /// §13.2: the provider's JSON for a row's `usage`/`lastGoodUsage`, from windows and their
    /// pace.
    fn render_usage(&self, windows: &[(Window, Pace)]) -> Value;
    /// One of this provider's window keys as its normalization describes it (§8.2): the key,
    /// label, kind and `period_s`, with `pct` 0 and no reset or detail, which the engine fills
    /// from a stored sample (§13.4's history of a window the last reading lacks). `None` for a
    /// key this provider does not produce.
    fn describe_window(&self, key: &str) -> Option<Window>;
    /// The file whose mtime and size key `live_identity_cache` (§13.5): the one `live_identity`
    /// reads. CC: `~/.claude.json`. `None` when no single file backs the live identity.
    fn live_identity_source(&self, env: &Env) -> Option<PathBuf>;

    // §4.5 "Parallel sessions" (§12). A profile's Keychain items are named from the recorded
    // spelling, never one derived again (§12.2). Its files are found by `dir`, its actual
    // directory, which differs from that spelling once the data directory has moved
    // (Decision 19).
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
    /// §8.1, §12.5: the profile's credential, read as the agent reads it (Decision 19): the
    /// Keychain item named from `spelling`, the marker's recorded spelling, then the credential
    /// file in `dir`, the profile's actual directory.
    fn read_profile_credential(&self, env: &Env, dir: &Path, spelling: &str) -> Read<Credential>;
    /// §12.5 "Identity drift": the login identity of the profile in `dir`, its actual directory
    /// (CC: its `.claude.json` `oauthAccount`; Decision 19).
    fn profile_identity(&self, env: &Env, dir: &Path) -> Read<Identity>;
    /// §10.3: deletes the agent-owned credential items for `spelling` and verifies them gone
    /// (CC macOS: the hashed Keychain items, under CC's storage-write lock anchored in `dir`,
    /// the profile's actual directory; otherwise nothing outside the directory).
    fn delete_profile_credential(
        &self,
        env: &Env,
        dir: &Path,
        spelling: &str,
    ) -> Result<(), ProviderError>;
    /// §13.5: whether `env` is a process this agent started (CC: `CLAUDECODE` or `CLAUDE_CONFIG_DIR`).
    fn invoked_by(&self, env: &Env) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(
            shown.contains("uuid-1") && shown.contains("org-1"),
            "{shown}"
        );
    }

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

    struct Held(std::cell::Cell<bool>);
    impl LiveLockSet for Held {
        fn check_owned(&self) -> Result<(), LockError> {
            if self.0.get() {
                Ok(())
            } else {
                Err(LockError::Compromised("x".into()))
            }
        }
    }

    #[test]
    fn live_locks_delegate_ownership_checks() {
        let d = tempfile::tempdir().unwrap();
        let env = Env::for_test(d.path());
        let g = MutationGuard::acquire(&env, std::time::Duration::from_millis(100)).unwrap();
        let owned = LiveLocks::new(&g, Box::new(Held(std::cell::Cell::new(true))));
        assert!(owned.check_owned().is_ok());
        let compromised = LiveLocks::new(&g, Box::new(Held(std::cell::Cell::new(false))));
        assert!(matches!(
            compromised.check_owned(),
            Err(LockError::Compromised(_))
        ));
    }

    /// A lock set that records when it is released.
    struct Recorded(
        &'static str,
        std::sync::Arc<std::sync::Mutex<Vec<&'static str>>>,
    );
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
                assert_eq!(
                    b, budget,
                    "the credential stage starts with the whole budget"
                );
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

    struct NoopUndo;
    impl Undo for NoopUndo {
        fn undo(self: Box<Self>, locks: &LiveLocks<'_>) -> Result<(), ProviderError> {
            locks.check_owned()?;
            Ok(())
        }
        fn what(&self) -> String {
            "noop".into()
        }
    }

    #[test]
    fn an_undo_runs_while_its_locks_are_held() {
        let d = tempfile::tempdir().unwrap();
        let env = Env::for_test(d.path());
        let g = MutationGuard::acquire(&env, std::time::Duration::from_millis(100)).unwrap();
        let locks = LiveLocks::new(&g, Box::new(Held(std::cell::Cell::new(true))));
        let undo: Box<dyn Undo + '_> = Box::new(NoopUndo);
        assert!(undo.undo(&locks).is_ok());
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
        let doomed = DoomedEntry {
            bytes: Read::Present(SENTINEL.as_bytes().to_vec()),
        };
        assert!(!format!("{doomed:?}").contains("SENTINEL"));
    }

    #[test]
    fn incomplete_lists_every_unrestored_entry_by_name() {
        let err = ProviderError::Incomplete {
            failed: vec![
                "Claude Code-credentials".into(),
                "/home/x/.claude.json".into(),
            ],
        };
        let msg = err.to_string();
        assert!(msg.contains("Claude Code-credentials") && msg.contains(".claude.json"));
    }

    fn stub_window() -> Window {
        Window {
            key: "5h".into(),
            label: "5h".into(),
            kind: tagteam_core::usage::WindowKind::Short,
            pct: 12.0,
            resets_at: None,
            period_s: None,
            detail: None,
        }
    }

    /// A normalizer that accepts a JSON object with `"windows": true` and rejects any other.
    fn stub_normalize(body: &Value) -> Option<Vec<Window>> {
        match body["windows"].as_bool()? {
            true => Some(vec![stub_window()]),
            false => Some(Vec::new()),
        }
    }

    fn reply(
        status: u16,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> Result<HttpResponse, HttpError> {
        Ok(HttpResponse {
            status,
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            body: body.to_vec(),
        })
    }

    fn verdict(r: Result<HttpResponse, HttpError>) -> UsageResult {
        UsageResult::from_reply(r, stub_normalize)
    }

    fn failed(kind: TransientKind, retry_after_s: Option<f64>) -> UsageResult {
        UsageResult::Failed {
            kind,
            retry_after_s,
        }
    }

    #[test]
    fn a_transport_failure_is_pre_send_or_ambiguous_and_never_calls_the_normalizer() {
        let never = |_: &Value| -> Option<Vec<Window>> { panic!("no body to normalize") };
        assert_eq!(
            UsageResult::from_reply(Err(HttpError::PreSend("no route".into())), never),
            failed(TransientKind::PreSend, None)
        );
        assert_eq!(
            UsageResult::from_reply(Err(HttpError::Ambiguous("reset".into())), never),
            failed(TransientKind::Ambiguous, None)
        );
    }

    #[test]
    fn a_200_the_normalizer_accepts_is_its_windows_and_empty_means_no_usage() {
        assert_eq!(
            verdict(reply(200, &[], br#"{"windows": true}"#)),
            UsageResult::Windows(vec![stub_window()])
        );
        assert_eq!(
            verdict(reply(200, &[], br#"{"windows": false}"#)),
            UsageResult::Windows(Vec::new())
        );
    }

    #[test]
    fn a_200_the_normalizer_rejects_or_that_is_not_json_is_bad_response() {
        assert_eq!(
            verdict(reply(200, &[], br#"{"other": 1}"#)),
            failed(TransientKind::BadResponse, None)
        );
        assert_eq!(
            verdict(reply(200, &[], b"<html>not json</html>")),
            failed(TransientKind::BadResponse, None)
        );
        assert_eq!(
            verdict(reply(200, &[], b"")),
            failed(TransientKind::BadResponse, None)
        );
    }

    #[test]
    fn a_401_is_unauthorized_whatever_it_carries() {
        assert_eq!(
            verdict(reply(401, &[("retry-after", "5")], br#"{"windows": true}"#)),
            UsageResult::Unauthorized
        );
    }

    #[test]
    fn a_429_carries_its_retry_after_in_seconds_when_it_has_one() {
        assert_eq!(
            verdict(reply(429, &[("retry-after", "120")], b"")),
            failed(TransientKind::Http(429), Some(120.0))
        );
        assert_eq!(
            verdict(reply(429, &[("Retry-After", "0.5")], b"")),
            failed(TransientKind::Http(429), Some(0.5))
        );
        assert_eq!(
            verdict(reply(429, &[], b"")),
            failed(TransientKind::Http(429), None)
        );
        assert_eq!(
            verdict(reply(
                429,
                &[("retry-after", "Wed, 21 Oct 2026 07:28:00 GMT")],
                b""
            )),
            failed(TransientKind::Http(429), None),
            "the HTTP-date form is not read"
        );
    }

    #[test]
    fn any_other_status_is_http_with_its_code() {
        assert_eq!(
            verdict(reply(500, &[], b"oops")),
            failed(TransientKind::Http(500), None)
        );
        assert_eq!(
            verdict(reply(503, &[("retry-after", "7")], b"")),
            failed(TransientKind::Http(503), Some(7.0))
        );
    }
}
