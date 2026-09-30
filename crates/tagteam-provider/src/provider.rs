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
    /// Destroyed only if the Keychain refuses the write and it falls back to a file (Appendix
    /// A.3). That is decided only as the write runs, which then reports the entry to
    /// `before_fallback` just before it deletes it.
    pub on_fallback: bool,
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
/// undo.undo(&locks).unwrap();
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
    /// A file, because the keychain refused this write or an earlier one in this process.
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
    #[error(
        "{0} is torn or not a JSON object; restore it from Claude Code's backups (~/.claude/backups/) or repair it, then retry"
    )]
    ConfigUnsplicable(PathBuf),
    #[error(
        "the credential was written to the file, but the Keychain item {0} that shadows it could not be verified gone"
    )]
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
    /// file path, never bytes.
    #[error("the previous state could not be restored for: {}", .failed.join(", "))]
    Incomplete { failed: Vec<String> },
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
    fn capabilities(&self) -> Capabilities;
    fn identity_surface(&self, env: &Env) -> IdentitySurface;
    fn identity_key(&self, id: &Identity) -> IdentityKey;
    fn credential_kinds(&self) -> &'static [&'static str];
    fn kind_traits(&self, kind: &str) -> KindTraits;
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
    fn lock_live<'g>(
        &self,
        env: &Env,
        g: &'g MutationGuard,
    ) -> Result<LiveLocks<'g>, ProviderError>;
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
        live: &LiveAuth,
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
            on_fallback: true,
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
}
