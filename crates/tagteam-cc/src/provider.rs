use std::sync::Arc;

use serde_json::Value;
use tagteam_core::{CLAUDE_CODE, Fingerprint, IdentityKey, ProviderId};
use tagteam_provider::{
    Env, Identity, IdentitySurface, Keychain, LiveAuth, LiveLocks, MutationGuard, Provider,
    ProviderError, Read, StoredLogin, Undo,
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
        Self {
            live: Arc::new(store),
        }
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
        self.live
            .restore(&self.env, &self.paths, &self.snapshot, &fence)
    }

    fn what(&self) -> String {
        "restore the live credential and managed key".into()
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
        let fence = || locks.check_owned().map_err(ProviderError::from);
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
            credential_items: if mac {
                read_services(env, ItemKind::OAuth)
                    .into_iter()
                    .map(|s| (s, acct.clone()))
                    .collect()
            } else {
                vec![]
            },
            owned_items: if mac {
                read_services(env, ItemKind::ManagedKey)
                    .into_iter()
                    .map(|s| (s, acct.clone()))
                    .collect()
            } else {
                vec![]
            },
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
        live: &LiveAuth,
    ) -> Result<Box<dyn Undo + 'l>, ProviderError> {
        self.guarded(env, locks, |paths, fence| {
            if target.kind == KIND_API_KEY {
                self.live
                    .write_managed_key(env, paths, &target.secret, fence)?;
                self.live.clear_credential_account_keys(env, paths, fence)
            } else {
                let live_map = match &live.credential {
                    Read::Present(c) => serde_json::from_slice::<Value>(c.bytes())
                        .ok()
                        .and_then(|v| v.as_object().cloned()),
                    _ => None,
                };
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
        let fence = || locks.check_owned().map_err(ProviderError::from);
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
