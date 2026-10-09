use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Map, Value};
use tagteam_core::{CLAUDE_CODE, Fingerprint, IdentityKey, ProviderId};
use tagteam_provider::http::Http;
use tagteam_provider::process::{ProcessSpawner, SpawnSpec};
use tagteam_provider::profile::refuse_linked_credential;
use tagteam_provider::provider::{DeadReason, RefreshResult};
use tagteam_provider::{
    BeforeFallback, Cancel, Capabilities, Check, CredLocks, Credential, DoomedEntry, Env,
    FreshCredential, Identity, IdentitySurface, Keychain, KindTraits, LiveAuth, LiveChange,
    LiveLockSet, LiveLocks, LockError, LockState, MergeReport, MustShare, MutationGuard, Pace,
    PollBudget, Provider, ProviderError, Read, SessionEnv, SharePolicy, StoredLogin, Undo,
    UsageResult, Validity, Window, Written,
};

use crate::config;
use crate::crash;
use crate::doctor;
use crate::endpoints::Endpoints;
use crate::live::{self, Extent, Fence, LiveStore, Platform, Snapshot};
use crate::locks;
use crate::naming::{ItemKind, keychain_account, keychain_service};
use crate::oauth;
use crate::paths::{CcPaths, nfc};
use crate::session::{self, CC_MUST_SHARE, CC_PRIVATE, CC_SHARED};
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

/// `kind`'s one Keychain item for `env` (Appendix A.2), paired with the account: the
/// credential- or managed-key half of §3's identity surface. Empty off macOS, where CC never
/// touches the Keychain (Appendix A.3).
fn keychain_items(env: &Env, kind: ItemKind, acct: &str, mac: bool) -> Vec<(String, String)> {
    if !mac {
        return vec![];
    }
    vec![(keychain_service(env, kind), acct.to_owned())]
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

/// A profile's own credential as §12.3 step 4 composes from it: a JSON object, else refused,
/// as a live entry is (§9.4 step 3). Its absence is the caller's `None`.
fn entry_object(bytes: &[u8]) -> Result<Map<String, Value>, ProviderError> {
    match serde_json::from_slice::<Value>(bytes) {
        Ok(Value::Object(o)) => Ok(o),
        _ => Err(ProviderError::Invalid(live::UNPARSABLE_ENTRY.into())),
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
            // A fixed phrase (§14.2): the error may name a path or another program's message.
            if undo.undo(self.locks).is_err() {
                tracing::error!("restoring the live credential during unwinding failed");
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
            // §3's create-only row: the must-share entries, in the config home a link sync
            // shares from (§12.2).
            create_only: CC_MUST_SHARE
                .iter()
                .map(|(name, _)| paths.config_home.join(name))
                .collect(),
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

    fn export_login(
        &self,
        login: &StoredLogin,
        full: bool,
    ) -> Result<(Value, Value), ProviderError> {
        Ok((
            shape::export_identity(&login.identity),
            shape::export_credential(&login.secret, full)?,
        ))
    }

    fn import_login(
        &self,
        identity: &Value,
        credential: &Value,
    ) -> Result<StoredLogin, ProviderError> {
        let identity = shape::import_identity(identity)?;
        let secret = shape::import_credential(credential)?;
        Ok(StoredLogin {
            kind: shape::classify(&secret).into(),
            secret,
            identity,
        })
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

    fn launch_command(&self) -> &'static str {
        "claude"
    }

    fn session_dir_var(&self) -> Option<&'static str> {
        Some("CLAUDE_CONFIG_DIR")
    }

    /// Appendix A.1: an empty `CLAUDE_CONFIG_DIR` counts as unset.
    fn session_dir(&self, env: &Env) -> Option<PathBuf> {
        env.claude_config_dir
            .as_deref()
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    }

    /// Appendix A.2: the live items are named from `CLAUDE_SECURESTORAGE_CONFIG_DIR` when it is
    /// set, else from `CLAUDE_CONFIG_DIR`, NFC-normalised, an empty value counting as unset.
    fn live_item_spelling(&self, env: &Env) -> Option<String> {
        crate::naming::suffix_source(env)
    }

    fn outer_home(&self, env: &Env) -> Value {
        session::outer_home(env)
    }

    fn apply_outer_home(&self, env: &Env, outer: &Value) -> Result<Env, ProviderError> {
        session::apply_outer_home(env, outer)
    }

    /// §12.2 "One spelling": the canonical path, NFC-normalized.
    fn profile_spelling(&self, canonical: &Path) -> String {
        nfc(canonical.as_os_str())
    }

    fn share_policy(&self, env: &Env) -> SharePolicy {
        SharePolicy {
            source: CcPaths::resolve(env).config_home,
            shared: CC_SHARED.to_vec(),
            must_share: CC_MUST_SHARE
                .iter()
                .map(|&(name, kind)| MustShare { name, kind })
                .collect(),
            private: CC_PRIVATE.to_vec(),
        }
    }

    fn session_records_dir(&self, profile: &Path) -> PathBuf {
        profile.join("sessions")
    }

    fn supervisor_lock(&self, profile: &Path) -> Option<PathBuf> {
        Some(profile.join("daemon.lock"))
    }

    /// The hashed Keychain item named from `spelling`, the recorded spelling, then
    /// `.credentials.json` in `dir`, where the profile is now (Decision 19), exactly as the
    /// live read takes them (§12.3 step 2).
    fn read_profile_credential(&self, env: &Env, dir: &Path, spelling: &str) -> Read<Credential> {
        self.live.read_credential(
            &session::profile_env(env, spelling),
            &session::profile_paths(env, dir),
        )
    }

    /// The profile's credential locks are named from `spelling`, as Claude Code running there
    /// names them (M4b's `write_profile_credential` takes them the same way); the read is
    /// `read_profile_credential`'s. Releasing them ends the operation.
    fn read_profile_credential_settled(
        &self,
        env: &Env,
        dir: &Path,
        spelling: &str,
        guard: &MutationGuard,
    ) -> Result<Read<Credential>, ProviderError> {
        let held = self.lock_credentials(
            &session::profile_env(env, spelling),
            guard,
            self.lock_budget,
        )?;
        let read = self.read_profile_credential(env, dir, spelling);
        drop(held);
        Ok(read)
    }

    /// The `oauthAccount` of the config in `dir`, where the profile is now (Decision 19).
    fn profile_identity(&self, env: &Env, dir: &Path) -> Read<Identity> {
        config::live_identity(&session::profile_paths(env, dir))
    }

    /// Both axes' items for `spelling`. When `dir`, where the profile is now (Decision 19), is a
    /// real directory, the deletes hold the profile's own credential locks there, within the live
    /// locks' budget, and each delete its storage-write lock too (§9.1); otherwise no lock is
    /// taken. The profile's `.credentials.json` goes with its directory. Nothing on Linux.
    fn delete_profile_credential(
        &self,
        env: &Env,
        dir: &Path,
        spelling: &str,
    ) -> Result<(), ProviderError> {
        self.live.delete_items(
            &session::profile_env(env, spelling),
            &session::profile_paths(env, dir),
            self.lock_budget,
        )
    }

    fn invoked_by(&self, env: &Env) -> bool {
        env.var("CLAUDECODE").is_some_and(|v| v == "1") || self.session_dir(env).is_some()
    }

    fn seed_profile(
        &self,
        env: &Env,
        dir: &Path,
        identity: &Identity,
    ) -> Result<(), ProviderError> {
        session::seed(env, dir, identity, self.lock_budget)
    }

    fn has_baseline(&self, dir: &Path) -> bool {
        session::has_baseline(dir)
    }

    fn merge_back(
        &self,
        env: &Env,
        dir: &Path,
        cancel: &Cancel,
    ) -> Result<MergeReport, ProviderError> {
        session::merge_back(env, dir, self.lock_budget, cancel)
    }

    /// §12.3 step 4 (Decision 6), under the profile's own credential locks:
    /// 1. a read of its entry, the operation's last read (§9.1);
    /// 2. then the file alone, under its storage-write lock.
    ///
    /// Releasing the locks ends the operation. A file that is a link refuses first, before any
    /// lock or read: the write would land wherever it points.
    fn write_profile_credential(
        &self,
        env: &Env,
        spelling: &str,
        guard: &MutationGuard,
        bytes: &[u8],
    ) -> Result<(), ProviderError> {
        let profile = session::profile_env(env, spelling);
        let paths = CcPaths::resolve(&profile);
        refuse_linked_credential(&paths.credentials_file)?;
        let held = self.lock_credentials(&profile, guard, self.lock_budget)?;
        let fence = || held.check_owned().map_err(ProviderError::from);
        // The read the storage-write lock's re-read is held to; nothing else uses it.
        self.live.snapshot(&profile, &paths)?;
        self.live
            .write_credential_file(&profile, &paths, bytes, &fence)
    }

    fn compose_profile_credential(
        &self,
        vault: &[u8],
        profile: Option<&[u8]>,
    ) -> Result<Vec<u8>, ProviderError> {
        let live = profile.map(entry_object).transpose()?;
        shape::compose(vault, live.as_ref())
    }

    /// §12.5: the process environment's names are read here, so the token file descriptors
    /// actually set are the ones scrubbed.
    fn session_env(&self, spelling: &str) -> SessionEnv {
        session::session_env(spelling, std::env::vars_os().map(|(name, _)| name))
    }

    /// §12.3 step 8: `claude auth status --json` in the session's environment and `cwd`. It
    /// spawns `program`, the launch command `plan_run` resolved on `PATH` (§12.1), so the check
    /// runs the binary the session will (Decision 20). A token already set spawns nothing.
    fn validate_profile(
        &self,
        _env: &Env,
        spelling: &str,
        cwd: &Path,
        program: &Path,
        expect: &Identity,
        spawner: &dyn ProcessSpawner,
        cancel: &Cancel,
    ) -> Validity {
        if cancel.requested().is_some() {
            return Validity::Unknown(session::INTERRUPTED.into());
        }
        let SessionEnv { set, remove } = self.session_env(spelling);
        let spec = SpawnSpec {
            program: program.to_path_buf(),
            args: Vec::from(["auth", "status", "--json"].map(OsString::from)),
            set,
            remove,
            cwd: Some(cwd.to_path_buf()),
        };
        session::validity(
            spawner.run_captured(&spec, session::AUTH_STATUS_TIMEOUT, cancel),
            spelling,
            expect,
        )
    }

    /// §13.6's Claude Code checks (`doctor.rs`), with this provider's own Keychain.
    fn doctor_checks(
        &self,
        env: &Env,
        spawner: &dyn ProcessSpawner,
        cancel: &Cancel,
    ) -> Vec<Check> {
        doctor::checks(&self.live, env, spawner, cancel)
    }

    /// The state of the Keychain `cc.keychain` asks about: only on macOS does CC use one.
    fn keychain_lock_state(&self) -> Option<LockState> {
        (self.live.platform() == Platform::MacOs).then(|| self.live.keychain().lock_state())
    }

    /// The token, profile and usage hosts (Appendix A.5), each once, as root URLs.
    fn doctor_hosts(&self) -> Vec<String> {
        let mut hosts: Vec<String> = Vec::new();
        for url in [
            &self.endpoints.token,
            &self.endpoints.profile,
            &self.endpoints.usage,
        ] {
            if let Some(host) = doctor::origin(url) {
                if !hosts.contains(&host) {
                    hosts.push(host);
                }
            }
        }
        hosts
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
