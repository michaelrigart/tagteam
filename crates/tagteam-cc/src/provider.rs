use std::sync::Arc;

use serde_json::{Map, Value};
use tagteam_core::{CLAUDE_CODE, Fingerprint, IdentityKey, ProviderId};
use tagteam_provider::{
    Credential, Env, Identity, IdentitySurface, Keychain, LiveAuth, LiveLocks, MutationGuard,
    Provider, ProviderError, Read, StoredLogin, Undo,
};

use crate::config;
use crate::live::{self, Fence, LiveStore, Platform, Snapshot};
use crate::locks;
use crate::naming::{ItemKind, keychain_account, read_services};
use crate::paths::CcPaths;
use crate::shape::{self, KIND_API_KEY, KINDS, MACHINE_SHARED_KEYS};

/// A live read that is too stale or unreliable to compose from, but that isn't itself an
/// `Unreadable`/parse error with its own detail worth keeping (§9.4 step 3, §3).
const LIVE_NOT_FRESH: &str =
    "the live credential could not be read fresh; refusing to overwrite it";

pub struct ClaudeCode {
    live: Arc<LiveStore>,
}

impl ClaudeCode {
    pub fn new(keychain: Arc<dyn Keychain>, platform: Platform) -> Self {
        Self::with_store(LiveStore::new(keychain, platform))
    }

    pub fn with_store(store: LiveStore) -> Self {
        Self {
            live: Arc::new(store),
        }
    }
}

/// The ownership fence every protected write and restore checks immediately before mutating
/// (§9.1): a holder that lost its lock stops before writing anything.
fn fence_of<'a>(locks: &'a LiveLocks<'_>) -> impl Fn() -> Result<(), ProviderError> + 'a {
    move || locks.check_owned().map_err(ProviderError::from)
}

/// Every item a reader tries for `kind`, paired with the account: the credential- or
/// managed-key half of §3's identity surface. Empty off macOS, where CC never touches the
/// Keychain (Appendix A.3).
fn keychain_items(env: &Env, kind: ItemKind, acct: &str, mac: bool) -> Vec<(String, String)> {
    if !mac {
        return vec![];
    }
    read_services(env, kind)
        .into_iter()
        .map(|s| (s, acct.to_owned()))
        .collect()
}

/// The object to compose the target's machine-shared keys from (§9.4 step 3): `Absent` is a
/// legitimate "no live login" and composes as if there were none, but anything else that
/// cannot be trusted — an unreadable read, a degraded fallback, an empty entry, or bytes that
/// don't parse — is refused rather than silently treated as absent or used as-is. Called from
/// inside the lock fence on a read taken fresh at write time, never on a read the caller took
/// earlier: that read may have gone stale by the time the write actually happens.
fn fresh_live_object(read: Read<Credential>) -> Result<Option<Map<String, Value>>, ProviderError> {
    match read {
        Read::Absent => Ok(None),
        Read::Unreadable(e) => Err(ProviderError::Unreadable(e)),
        Read::Present(c) => match c.into_fresh() {
            Some(fresh) if !fresh.credential().is_empty() => {
                match serde_json::from_slice::<Value>(fresh.credential().bytes()) {
                    Ok(Value::Object(o)) => Ok(Some(o)),
                    _ => Err(ProviderError::Invalid(live::UNPARSABLE_ENTRY.into())),
                }
            }
            _ => Err(ProviderError::Invalid(LIVE_NOT_FRESH.into())),
        },
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
        let fence = fence_of(locks);
        self.live
            .restore(&self.env, &self.paths, &self.snapshot, &fence)
    }

    fn what(&self) -> String {
        format!(
            "restore the live credential, the managed key, and {}",
            self.paths.global_config.display()
        )
    }
}

/// Restores the snapshot if the operation unwinds before handing its undo to the engine: a
/// panic between two writes of one operation must not leave the first one in place.
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
    /// worked. The returned undo borrows `locks` for `'l` (§9.4 step 10: the credential locks
    /// must be "held throughout"), matching the `Provider` trait's writers.
    fn guarded<'l>(
        &self,
        env: &Env,
        locks: &'l LiveLocks<'_>,
        f: impl FnOnce(&CcPaths, Fence<'_>) -> Result<(), ProviderError>,
    ) -> Result<Box<dyn Undo + 'l>, ProviderError> {
        locks.check_owned()?;
        let paths = CcPaths::resolve(env);
        let snapshot = self.live.snapshot(env, &paths)?;
        let undo = Box::new(SnapshotUndo {
            live: self.live.clone(),
            env: env.clone(),
            paths: paths.clone(),
            snapshot,
        });
        let mut armed = Armed {
            undo: Some(undo),
            locks,
        };
        let fence = fence_of(locks);
        let result = f(&paths, &fence);
        let undo = armed.undo.take().expect("armed until here");
        match result {
            Ok(()) => Ok(undo),
            Err(e) => match undo.undo(locks) {
                Ok(()) => Err(e),
                Err(re) => Err(ProviderError::RestoreFailed {
                    cause: Box::new(e),
                    restore: Box::new(re),
                }),
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
        }
    }

    fn identity_key(&self, id: &Identity) -> IdentityKey {
        IdentityKey::new(format!(
            "{}\n{}",
            id.email.as_deref().unwrap_or(&id.label),
            id.org_uuid
        ))
    }

    fn credential_kinds(&self) -> &'static [&'static str] {
        &KINDS
    }

    fn parse_identity(&self, raw: &Value) -> Result<Identity, ProviderError> {
        shape::identity_from_oauth_account(raw).ok_or_else(|| {
            ProviderError::Invalid("the stored oauthAccount has no emailAddress".into())
        })
    }

    fn token_identity(&self, email: &str) -> Identity {
        shape::token_identity(email)
    }

    fn token_secret(&self, token: &str) -> (String, Vec<u8>) {
        let t = token.trim();
        if shape::is_api_key(t.as_bytes()) {
            (KIND_API_KEY.into(), t.as_bytes().to_vec())
        } else {
            (
                shape::KIND_SETUP_TOKEN.into(),
                shape::setup_token_credential(t),
            )
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

    fn lock_live<'g>(
        &self,
        env: &Env,
        g: &'g MutationGuard,
    ) -> Result<LiveLocks<'g>, ProviderError> {
        let set = locks::acquire(&CcPaths::resolve(env))?;
        Ok(LiveLocks::new(g, Box::new(set)))
    }

    fn write_credential<'l>(
        &self,
        env: &Env,
        locks: &'l LiveLocks<'_>,
        target: &StoredLogin,
        // Never composed from: a caller's read may have gone stale by the time the write
        // actually happens under the locks, so `write_credential` re-reads fresh instead
        // (see `fresh_live_object`).
        _live: &LiveAuth,
    ) -> Result<Box<dyn Undo + 'l>, ProviderError> {
        self.guarded(env, locks, |paths, fence| {
            if target.kind == KIND_API_KEY {
                self.live
                    .write_managed_key(env, paths, &target.secret, fence)?;
                self.live.clear_credential_account_keys(env, paths, fence)
            } else {
                let fresh = self.live.read_credential(env, paths);
                let live_map = fresh_live_object(fresh)?;
                let composed = shape::compose(&target.secret, live_map.as_ref())?;
                self.live
                    .write_credential_entry(env, paths, &composed, fence)?;
                self.live.clear_managed_key(env, paths, fence)
            }
        })
    }

    fn clear_other_axis<'l>(
        &self,
        env: &Env,
        locks: &'l LiveLocks<'_>,
        kept_kind: &str,
    ) -> Result<Box<dyn Undo + 'l>, ProviderError> {
        self.guarded(env, locks, |paths, fence| {
            if kept_kind == KIND_API_KEY {
                self.live.clear_credential_account_keys(env, paths, fence)
            } else {
                self.live.clear_managed_key(env, paths, fence)
            }
        })
    }

    fn write_identity<'l>(
        &self,
        env: &Env,
        locks: &'l LiveLocks<'_>,
        identity: Option<&Identity>,
    ) -> Result<Box<dyn Undo + 'l>, ProviderError> {
        let fence = fence_of(locks);
        fence()?;
        let paths = CcPaths::resolve(env);
        Ok(Box::new(config::splice_key(
            &paths.global_config,
            "oauthAccount",
            identity.map(|i| &i.raw),
            &fence,
        )?))
    }

    fn uses_file_store(&self, _env: &Env) -> bool {
        self.live.platform() == Platform::Linux || self.live.file_mode_pinned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tagteam_provider::ReadError;

    #[test]
    fn fresh_live_object_passes_through_absence() {
        assert_eq!(fresh_live_object(Read::Absent).unwrap(), None);
    }

    #[test]
    fn fresh_live_object_refuses_an_unreadable_read() {
        let err =
            fresh_live_object(Read::Unreadable(ReadError::new("keychain", "rc 36"))).unwrap_err();
        assert!(matches!(err, ProviderError::Unreadable(_)));
    }

    #[test]
    fn fresh_live_object_refuses_a_degraded_read() {
        // A degraded read is a Keychain failure covered by the plaintext file, so its bytes
        // may be a superseded generation: it is never trusted enough to compose from, even
        // though `guarded`'s snapshot happens to already refuse this on the OAuth axis today
        // (both read the same Keychain state). This is the composition-time guard directly.
        let bytes = json!({"mcpOAuth": {"m": 1}}).to_string().into_bytes();
        let err = fresh_live_object(Read::Present(Credential::degraded(bytes))).unwrap_err();
        assert!(matches!(err, ProviderError::Invalid(_)));
    }

    #[test]
    fn fresh_live_object_refuses_an_empty_entry() {
        let err = fresh_live_object(Read::Present(Credential::fresh(vec![]))).unwrap_err();
        assert!(matches!(err, ProviderError::Invalid(_)));
    }

    #[test]
    fn fresh_live_object_refuses_bytes_that_do_not_parse() {
        let err =
            fresh_live_object(Read::Present(Credential::fresh(b"not json".to_vec()))).unwrap_err();
        assert!(matches!(err, ProviderError::Invalid(_)));
    }

    #[test]
    fn fresh_live_object_accepts_a_fresh_json_object() {
        let bytes = json!({"mcpOAuth": {"m": 1}}).to_string().into_bytes();
        let obj = fresh_live_object(Read::Present(Credential::fresh(bytes)))
            .unwrap()
            .unwrap();
        assert_eq!(Value::Object(obj), json!({"mcpOAuth": {"m": 1}}));
    }
}
