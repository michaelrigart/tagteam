use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Map, Value};
use tagteam_core::{CLAUDE_CODE, Fingerprint, IdentityKey, ProviderId};
use tagteam_provider::http::Http;
use tagteam_provider::provider::{DeadReason, RefreshResult};
use tagteam_provider::{
    BeforeFallback, Capabilities, CredLocks, Credential, DoomedEntry, Env, FreshCredential,
    Identity, IdentitySurface, Keychain, KindTraits, LiveAuth, LiveChange, LiveLockSet, LiveLocks,
    LockError, MutationGuard, Pace, PollBudget, Provider, ProviderError, Read, StoredLogin, Undo,
    UsageResult, Window, Written,
};

use crate::config;
use crate::crash;
use crate::endpoints::Endpoints;
use crate::live::{self, Extent, Fence, LiveStore, Platform, Snapshot};
use crate::locks;
use crate::naming::{ItemKind, keychain_account, read_services};
use crate::oauth;
use crate::paths::CcPaths;
use crate::shape::{self, KIND_API_KEY, KINDS, MACHINE_SHARED_KEYS};
use crate::usage;

/// A live read that is too stale or unreliable to compose from, but that isn't itself an
/// `Unreadable`/parse error with its own detail worth keeping (§9.4 step 3, §3).
const LIVE_NOT_FRESH: &str =
    "the live credential could not be read fresh; refusing to overwrite it";

/// What to do about a `~/.claude.json` that cannot be spliced (§9.5).
pub const CONFIG_REMEDY: &str =
    "restore it from Claude Code's backups (~/.claude/backups/) or repair it, then retry";

pub struct ClaudeCode {
    live: Arc<LiveStore>,
    /// How long CC's live locks may take, both stages together (§9.1):
    /// `locks::ACQUIRE_TIMEOUT`, except in tests.
    lock_budget: Duration,
    /// Appendix A.5's URLs; `Endpoints::production()` except when the CLI's test-support
    /// build points them at a local server.
    endpoints: Endpoints,
}

impl ClaudeCode {
    pub fn new(keychain: Arc<dyn Keychain>, platform: Platform) -> Self {
        Self::with_store(LiveStore::new(keychain, platform))
    }

    pub fn with_store(store: LiveStore) -> Self {
        Self {
            live: Arc::new(store),
            lock_budget: locks::ACQUIRE_TIMEOUT,
            endpoints: Endpoints::production(),
        }
    }

    /// Sends every request to `endpoints` instead of production (the CLI's test-support build).
    pub fn with_endpoints(mut self, endpoints: Endpoints) -> Self {
        self.endpoints = endpoints;
        self
    }

    /// A shorter budget for CC's locks, so a test of a held lock need not wait the full 9 s.
    #[cfg(feature = "test-hooks")]
    pub fn with_lock_timeout(mut self, timeout: Duration) -> Self {
        self.lock_budget = timeout;
        self
    }
}

/// CC's credential locks, held for one operation: a switch, a recovery, a §7.5 pass or an
/// `add`. Releasing them ends the operation, and with it the Keychain file-mode pin (Appendix
/// A.3), so the next operation tries the Keychain again, and what the operation read and wrote
/// of each credential entry (§9.1), so the next one compares against its own reads.
struct OperationLocks {
    set: locks::CcCredSet,
    live: Arc<LiveStore>,
}

impl LiveLockSet for OperationLocks {
    fn check_owned(&self) -> Result<(), LockError> {
        self.set.check_owned()
    }
}

impl Drop for OperationLocks {
    fn drop(&mut self) {
        // Runs before `set` is dropped, so the operation ends while the locks are still held.
        self.live.end_operation();
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
    /// must be "held throughout"), matching the `Provider` trait's writers; it comes with
    /// whatever `f` reported.
    fn guarded<'l, T>(
        &self,
        env: &Env,
        locks: &'l LiveLocks<'_>,
        f: impl FnOnce(&CcPaths, Fence<'_>) -> Result<T, ProviderError>,
    ) -> Result<(Box<dyn Undo + 'l>, T), ProviderError> {
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
            Ok(value) => Ok((undo, value)),
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

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            usage: true,
            refresh: true,
            api_keys: true,
            sessions: true,
            statusline: true,
        }
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

    fn kind_traits(&self, kind: &str) -> KindTraits {
        shape::kind_traits(kind)
    }

    fn primary_long_window(&self) -> Option<&'static str> {
        Some(usage::SEVEN_DAY)
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

    fn access_expires_at(&self, secret: &[u8]) -> Option<i64> {
        shape::access_expires_at(secret)
    }

    fn access_fingerprint(&self, secret: &[u8]) -> Option<Fingerprint> {
        shape::access_token(secret).map(|t| Fingerprint::of_secret(t.as_bytes()))
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

    fn live_lock_budget(&self) -> Duration {
        self.lock_budget
    }

    fn lock_credentials<'g>(
        &self,
        env: &Env,
        g: &'g MutationGuard,
        budget: Duration,
    ) -> Result<CredLocks<'g>, ProviderError> {
        let set = locks::acquire_credentials(&CcPaths::resolve(env), budget, &env.cancel)?;
        Ok(CredLocks::new(
            g,
            Box::new(OperationLocks {
                set,
                live: self.live.clone(),
            }),
        ))
    }

    fn lock_config<'g>(
        &self,
        env: &Env,
        cred: CredLocks<'g>,
        budget: Duration,
    ) -> Result<LiveLocks<'g>, ProviderError> {
        // On a timeout or an interruption, `cred` is dropped as this returns, releasing the
        // credential locks.
        let set = locks::acquire_config(&CcPaths::resolve(env), budget, &env.cancel)?;
        Ok(cred.with_config(Box::new(set)))
    }

    fn doomed(
        &self,
        env: &Env,
        _locks: &LiveLocks<'_>,
        change: LiveChange<'_>,
    ) -> Vec<DoomedEntry> {
        let (entry, managed) = match change {
            LiveChange::Write(kind) if kind == KIND_API_KEY => (Extent::Cleared, Extent::Written),
            LiveChange::Write(_) => (Extent::Written, Extent::Cleared),
            LiveChange::ClearOther(kind) if kind == KIND_API_KEY => (Extent::Cleared, Extent::None),
            LiveChange::ClearOther(_) => (Extent::None, Extent::Cleared),
        };
        self.live
            .doomed(env, &CcPaths::resolve(env), entry, managed)
    }

    fn write_credential<'l>(
        &self,
        env: &Env,
        locks: &'l LiveLocks<'_>,
        target: &StoredLogin,
        before_fallback: BeforeFallback<'_>,
    ) -> Result<Written<'l>, ProviderError> {
        let (undo, stored_in) = self.guarded(env, locks, |paths, fence| {
            let stored_in = if target.kind == KIND_API_KEY {
                let stored_in = self.live.write_managed_key(
                    env,
                    paths,
                    &target.secret,
                    fence,
                    before_fallback,
                )?;
                crash::point("after-target-axis");
                self.live.clear_credential_account_keys(env, paths, fence)?;
                stored_in
            } else {
                let fresh = self.live.read_credential(env, paths);
                let live_map = fresh_live_object(fresh)?;
                let composed = shape::compose(&target.secret, live_map.as_ref())?;
                let stored_in = self.live.write_credential_entry(
                    env,
                    paths,
                    &composed,
                    fence,
                    before_fallback,
                )?;
                crash::point("after-target-axis");
                self.live.clear_managed_key(env, paths, fence)?;
                stored_in
            };
            Ok(stored_in)
        })?;
        Ok(Written { undo, stored_in })
    }

    fn clear_other_axis<'l>(
        &self,
        env: &Env,
        locks: &'l LiveLocks<'_>,
        kept_kind: &str,
    ) -> Result<Box<dyn Undo + 'l>, ProviderError> {
        let (undo, ()) = self.guarded(env, locks, |paths, fence| {
            if kept_kind == KIND_API_KEY {
                self.live.clear_credential_account_keys(env, paths, fence)
            } else {
                self.live.clear_managed_key(env, paths, fence)
            }
        })?;
        Ok(undo)
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

    fn fetch_usage(&self, http: &dyn Http, cred: &Credential) -> UsageResult {
        // An API key has no access token, and neither has a wiped or refresh-only blob (§8.1).
        // A setup token is sent like any OAuth access token (Decision 11).
        let Some(token) = shape::access_token(cred.bytes()) else {
            return UsageResult::NoAccessToken;
        };
        usage::parse_usage(http.send(&usage::usage_request(&self.endpoints, &token)))
    }

    fn poll_budget(&self) -> PollBudget {
        PollBudget::STANDARD
    }

    fn render_usage(&self, windows: &[(Window, Pace)]) -> Value {
        usage::render(windows)
    }

    fn describe_window(&self, key: &str) -> Option<Window> {
        usage::describe(key)
    }

    fn live_identity_source(&self, env: &Env) -> Option<PathBuf> {
        Some(CcPaths::resolve(env).global_config)
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
