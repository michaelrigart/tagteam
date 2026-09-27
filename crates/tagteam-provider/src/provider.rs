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
        f.debug_struct("LiveAuth")
            .field("credential", &self.credential)
            .field("managed_key", &key)
            .finish()
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
    fn lock_live<'g>(
        &self,
        env: &Env,
        g: &'g MutationGuard,
    ) -> Result<LiveLocks<'g>, ProviderError>;
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
    fn clear_other_axis(
        &self,
        env: &Env,
        locks: &LiveLocks<'_>,
        kept_kind: &str,
    ) -> Result<Box<dyn Undo>, ProviderError>;
    /// Splices the identity into the live config; `None` removes it (§9.4 step 8).
    fn write_identity(
        &self,
        env: &Env,
        locks: &LiveLocks<'_>,
        identity: Option<&Identity>,
    ) -> Result<Box<dyn Undo>, ProviderError>;
    /// For the post-switch hint (§9.4 "After unlocking").
    fn uses_file_store(&self, env: &Env) -> bool;
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
