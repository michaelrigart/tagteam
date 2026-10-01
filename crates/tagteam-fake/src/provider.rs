use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use tagteam_core::{Fingerprint, IdentityKey, ProviderId};
use tagteam_provider::atomic::{
    ensure_private_dir, remove_target, write_atomic_private_with, write_atomic_with,
};
use tagteam_provider::http::{Http, HttpError, HttpRequest};
use tagteam_provider::provider::{DeadReason, RefreshResult, TransientKind};
use tagteam_provider::splice::{self, render_nested};
use tagteam_provider::{
    BeforeFallback, Capabilities, CredLocks, Credential, DoomedEntry, Env, FreshCredential,
    Identity, IdentitySurface, KindTraits, LiveAuth, LiveChange, LiveLockSet, LiveLocks, LockError,
    MkdirLock, MkdirLockSpec, MutationGuard, Pace, PollBudget, Provider, ProviderError, Read,
    ReadError, SecretStore, StoredLogin, Undo, UsageResult, Window, Written,
};

use crate::FAKE_AGENT;
use crate::identity_json;
use crate::paths::FakePaths;
use crate::shape::{self, DEVICE, KIND_STATIC, KINDS};
use crate::usage;

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

    pub fn usage_url(&self) -> String {
        format!("{}/usage", self.base)
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

impl Provider for FakeAgent {
    fn id(&self) -> ProviderId {
        ProviderId::new(FAKE_AGENT)
    }

    fn display_name(&self) -> &'static str {
        "FakeAgent"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            usage: true,
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

    /// None on purpose: FakeAgent is the provider whose consume-first setting runs `best`
    /// (§4.5, §11.5). Its `monthly` meter is still a `Long` window for pace (§8.7).
    fn primary_long_window(&self) -> Option<&'static str> {
        None
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
        let spec = MkdirLockSpec::new(FakePaths::resolve(env).lock, LOCK_STALE, budget)
            .with_cancel(&env.cancel);
        Ok(CredLocks::new(
            g,
            Box::new(FakeLock(MkdirLock::acquire(&spec)?)),
        ))
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

    fn refresh(
        &self,
        http: &dyn Http,
        cred: &FreshCredential,
        now_ms: i64,
        timeout: Duration,
    ) -> RefreshResult {
        let Ok(Value::Object(mut root)) =
            serde_json::from_slice::<Value>(cred.credential().bytes())
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
            (400 | 401, Some("invalid_grant")) => {
                return RefreshResult::Dead(DeadReason::InvalidGrant);
            }
            (status, Some("invalid_client")) if status != 200 => {
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
                body["expires_in"]
                    .as_i64()
                    .unwrap_or(0)
                    .saturating_mul(1000),
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
        let text = |k: &str| {
            body["owner"][k]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        let (uid, workspace) = (text("uid"), text("workspace"));
        let owner = (uid.is_some() || workspace.is_some()).then(|| Identity {
            label: uid
                .clone()
                .or_else(|| workspace.clone())
                .unwrap_or_default(),
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

    fn fetch_usage(&self, http: &dyn Http, cred: &Credential) -> UsageResult {
        let Some(token) = shape::token(cred.bytes()) else {
            return UsageResult::NoAccessToken;
        };
        usage::parse_usage(http.send(&usage::usage_request(self.usage_url(), &token)))
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
        Some(FakePaths::resolve(env).identity)
    }
}
