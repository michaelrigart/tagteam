#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Map, Value, json};
use tagteam_cc::endpoints::Endpoints;
use tagteam_cc::live::{LiveStore, Platform};
use tagteam_cc::shape::compose;
use tagteam_cc::{CcPaths, ClaudeCode, ItemKind, keychain_account, keychain_service};
use tagteam_core::{AccountId, CLAUDE_CODE, PollBudget, PollPlan, ProviderId, Window, WindowKind};
use tagteam_engine::auto::{AutoEvent, EventSink};
use tagteam_engine::lifecycle::{AddOptions, AddTokenOptions};
use tagteam_engine::oracle::Oracle;
use tagteam_engine::registry::ProviderRegistry;
use tagteam_engine::settings::Settings;
use tagteam_engine::store::{
    Activation, Eligibility, JournalRow, LoginMeta, NewAccount, Reserve, Store,
};
use tagteam_engine::switch::{SwitchOutcome, SwitchRequest, SwitchTarget};
use tagteam_engine::vault::{FileVault, KeychainVault, SERVICE, Vault, VaultBackend, VaultError};
use tagteam_engine::{Engine, EngineConfig, EngineError};
use tagteam_fake::{FAKE_AGENT, FakeAgent};
use tagteam_provider::http::Method;
use tagteam_provider::splice::{get_top_level, remove_top_level, replace_top_level};
use tagteam_provider::{
    Cancel, Clock, Credential, Env, FakeClock, FakeKeychain, Identity, IdentitySurface,
    MutationGuard, ProcessStamp, Provider, Read, ScriptedHttp,
};

pub fn identity(email: &str) -> Identity {
    Identity {
        label: email.into(),
        email: Some(email.into()),
        org_uuid: String::new(),
        org_name: None,
        account_uuid: None,
        raw: json!({"emailAddress": email}),
    }
}

/// Inserts an account whose identity key is `"<email>\n"`.
pub fn add(s: &Store, p: &ProviderId, id: &str, email: &str, pos: u32) -> AccountId {
    let aid = AccountId::from_string(id);
    let key = format!("{email}\n");
    s.insert_account(&NewAccount {
        id: &aid,
        provider: p,
        position: pos,
        identity_key: &key,
        identity: &identity(email),
        kind: "oauth",
        alias: None,
        login_expires_at: None,
        added_at: 1,
    })
    .unwrap();
    aid
}

pub fn cc() -> ProviderId {
    ProviderId::new("claude-code")
}

/// An oracle that answers whatever the test sets.
#[derive(Default)]
pub struct FixedOracle(pub Mutex<Option<Identity>>);

impl FixedOracle {
    pub fn set(&self, id: Option<Identity>) {
        *self.0.lock().unwrap() = id;
    }
}

impl Oracle for FixedOracle {
    fn resolve(&self, _p: &dyn Provider, _c: &Credential) -> Option<Identity> {
        self.0.lock().unwrap().clone()
    }
}

pub const CLAUDE_JSON: &str = r#"{
  "numStartups": 12,
  "projects": {
    "/work/app": {
      "allowedTools": [],
      "history": ["x"]
    }
  },
  "mcpServers": {
    "local": { "command": "srv" }
  },
  "userID": "user-7",
  "someFutureKey": { "n": 1e400 }
}
"#;

/// An API key an account is added with (`Fx::add_api_key`).
pub const API_KEY: &str = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz";
/// A managed key that no stored account holds.
pub const STRAY_API_KEY: &str = "sk-ant-api03-stray-key-that-no-vault-holds";
/// A second API key no stored account starts with.
pub const OTHER_API_KEY: &str = "sk-ant-api03-zyxwvutsrqponmlkjihgfedcba";
/// The unsuffixed OAuth item readers fall back to under `Fx::with_fallback_items`.
pub const FALLBACK_ITEM: &str = "Claude Code-credentials";
/// The unsuffixed managed-key item readers fall back to under `Fx::with_fallback_items`.
pub const FALLBACK_MANAGED_ITEM: &str = "Claude Code";

/// Sets one top-level key of the config at `path`, changing nothing else. A free function, so
/// a `'static` race callback can call it without borrowing the fixture.
pub fn splice_config_key(path: &Path, key: &str, value: &Value) {
    let doc = fs::read(path).unwrap();
    fs::write(path, replace_top_level(&doc, key, value).unwrap()).unwrap();
}

/// Replaces `oauthAccount` in the config at `path`, as CC does on a login.
pub fn splice_oauth_account(path: &Path, oauth_account: &Value) {
    splice_config_key(path, "oauthAccount", oauth_account);
}

/// Whether tagteam's mutation lock is free right now; takes and drops it if so. It asks under a
/// token of its own: a test that set `env`'s token would otherwise read every lock as taken.
pub fn mutation_lock_free(env: &Env) -> bool {
    let mut probe = env.clone();
    probe.cancel = Cancel::new();
    MutationGuard::acquire(&probe, Duration::ZERO).is_ok()
}

/// A process that is gone: its journal rows are recoverable (§12.6). Shared by `recover.rs`
/// and `invariant.rs`, which both need to plant a crashed switch's journal row. The pid is one
/// no process can hold (above any pid_max), so `is_live()` is false on macOS (ESRCH) and on
/// Linux (no `/proc` entry). It must not spawn: a child forked while another test holds an
/// account lock keeps a duplicate of that lock's file description and makes a re-lock Busy.
pub fn dead_holder() -> ProcessStamp {
    ProcessStamp {
        pid: i32::MAX as u32,
        start: 0,
    }
}

/// The vault's fingerprint of `id`'s stored generation.
pub fn vault_fp(fx: &Fx, id: &AccountId) -> String {
    fx.cc
        .fingerprint(&fx.vault_bytes(id).unwrap())
        .unwrap()
        .as_str()
        .to_owned()
}

/// The row a switch from `from` to `to` writes at step 6, held by a process that has died. It
/// journals the target's `login_epoch`, as the switch does (§9.4 step 6).
pub fn crash_row(fx: &Fx, from: &AccountId, to: &AccountId) -> JournalRow {
    let store = fx.engine.store().unwrap();
    let from_row = store.account(from).unwrap().unwrap();
    let to_row = store.account(to).unwrap().unwrap();
    JournalRow {
        provider: fx.provider(),
        holder: dead_holder(),
        from_id: Some(from.clone()),
        to_id: to.clone(),
        from_fp: Some(vault_fp(fx, from)),
        from_identity: Some(from_row.identity_json),
        to_fp: vault_fp(fx, to),
        to_epoch: Some(to_row.login_epoch),
        started_at: 1,
        prior: None,
    }
}

/// What step 7 leaves live: the target credential, composed with the live machine-shared keys.
pub fn write_target_credential(fx: &Fx, to: &AccountId) {
    let live: Value = fx.live_credential().unwrap();
    let composed = compose(&fx.vault_bytes(to).unwrap(), live.as_object()).unwrap();
    fx.set_live_credential(&composed);
}

/// Leaves a journal row as a switch from `from` to `to` that died after step 6.
pub fn crashed_switch(fx: &Fx, from: &AccountId, to: &AccountId) {
    let row = crash_row(fx, from, to);
    fx.engine.store().unwrap().insert_journal(&row).unwrap();
}

/// The provider's current journal row, if any.
pub fn journal(fx: &Fx) -> Option<JournalRow> {
    fx.engine.store().unwrap().journal(&fx.provider()).unwrap()
}

/// Asserts the provider's journal is clear: recovery must always finish by clearing it.
pub fn assert_journal_cleared(fx: &Fx) {
    assert!(journal(fx).is_none(), "a journal row was left behind");
}

/// A Keychain vault that runs `on_read` with each key just before reading it: for observing
/// what holds while the engine reads the vault.
pub struct ProbeVault {
    inner: KeychainVault,
    on_read: Box<dyn Fn(&str) + Send + Sync>,
}

impl VaultBackend for ProbeVault {
    fn read(&self, key: &str) -> Read<Vec<u8>> {
        (self.on_read)(key);
        self.inner.read(key)
    }
    fn write(&self, key: &str, bytes: &[u8]) -> Result<(), VaultError> {
        self.inner.write(key, bytes)
    }
    fn delete(&self, key: &str) -> Result<(), VaultError> {
        self.inner.delete(key)
    }
}

pub struct Fx {
    pub dir: tempfile::TempDir,
    pub env: Env,
    pub platform: Platform,
    pub kc: Arc<FakeKeychain>,
    pub oracle: Arc<FixedOracle>,
    pub clock: Arc<FakeClock>,
    /// Every engine this fixture builds sends through this one scripted port.
    pub http: Arc<ScriptedHttp>,
    pub cc: Arc<ClaudeCode>,
    pub engine: Engine,
}

impl Fx {
    pub fn new() -> Self {
        Self::with_platform(Platform::MacOs)
    }

    pub fn with_platform(platform: Platform) -> Self {
        Self::with(platform, |_| {})
    }

    /// A fixture whose Env is adjusted before anything is created in it.
    pub fn with(platform: Platform, adjust: impl FnOnce(&mut Env)) -> Self {
        Self::build(platform, adjust, |cc| cc)
    }

    /// A macOS fixture with an explicit `CLAUDE_CONFIG_DIR=~/.claude`: readers also try the
    /// unsuffixed items (Appendix A.2), so `FALLBACK_ITEM` is a second copy of the entry.
    pub fn with_fallback_items() -> Self {
        Self::with(Platform::MacOs, |e| {
            e.claude_config_dir = Some(e.home.join(".claude").into_os_string())
        })
    }

    pub fn put_fallback_item(&self, bytes: &[u8]) {
        self.kc
            .put(FALLBACK_ITEM, &keychain_account(&self.env), bytes);
    }

    pub fn fallback_item(&self) -> Option<Value> {
        let bytes = self.kc.get(FALLBACK_ITEM, &keychain_account(&self.env))?;
        serde_json::from_slice(&bytes).ok()
    }

    pub fn put_fallback_managed_item(&self, key: &[u8]) {
        self.kc
            .put(FALLBACK_MANAGED_ITEM, &keychain_account(&self.env), key);
    }

    pub fn fallback_managed_item(&self) -> Option<Vec<u8>> {
        self.kc
            .get(FALLBACK_MANAGED_ITEM, &keychain_account(&self.env))
    }

    /// A macOS fixture whose provider waits only `timeout` for CC's locks, so a held CC lock
    /// can be tested without the real 9 s wait.
    pub fn with_lock_timeout(timeout: Duration) -> Self {
        Self::build(Platform::MacOs, |_| {}, |cc| cc.with_lock_timeout(timeout))
    }

    fn build(
        platform: Platform,
        adjust: impl FnOnce(&mut Env),
        tune: impl FnOnce(ClaudeCode) -> ClaudeCode,
    ) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut env = Env::for_test(dir.path());
        adjust(&mut env);
        let claude = env.home.join(".claude");
        fs::create_dir_all(claude.join("projects/-work-app/memory")).unwrap();
        fs::write(
            claude.join("projects/-work-app/memory/MEMORY.md"),
            "remember this\n",
        )
        .unwrap();
        fs::create_dir_all(claude.join("skills/s")).unwrap();
        fs::write(claude.join("skills/s/SKILL.md"), "skill\n").unwrap();
        fs::create_dir_all(claude.join("plugins")).unwrap();
        fs::write(claude.join("plugins/installed.json"), "{}\n").unwrap();
        fs::write(claude.join("CLAUDE.md"), "instructions\n").unwrap();
        fs::write(claude.join("history.jsonl"), "{\"display\":\"hi\"}\n").unwrap();
        fs::write(claude.join("settings.json"), "{\"theme\":\"dark\"}\n").unwrap();
        // Wherever this Env's global config resolves (Appendix A.1), not a hard-coded path.
        fs::write(CcPaths::resolve(&env).global_config, CLAUDE_JSON).unwrap();

        let kc = Arc::new(FakeKeychain::new());
        let oracle = Arc::new(FixedOracle::default());
        let clock = Arc::new(FakeClock::new(1_790_000_000_000));
        let http = Arc::new(ScriptedHttp::new());
        let cc = Arc::new(tune(ClaudeCode::with_store(
            LiveStore::new(kc.clone(), platform).with_retry_delay(Duration::ZERO),
        )));
        let vault = match platform {
            Platform::MacOs => Vault::new(Box::new(KeychainVault::new(kc.clone()))),
            Platform::Linux => Vault::new(Box::new(FileVault::new(env.data_dir().join("vault")))),
        };
        let engine = Engine::new(EngineConfig {
            env: env.clone(),
            registry: ProviderRegistry::new().with(cc.clone()),
            vault,
            oracle: oracle.clone(),
            clock: clock.clone(),
            http: http.clone(),
            default_provider: ProviderId::new(CLAUDE_CODE),
            settings: Settings::default(),
        });
        Fx {
            dir,
            env,
            platform,
            kc,
            oracle,
            clock,
            http,
            cc,
            engine,
        }
    }

    pub fn provider(&self) -> ProviderId {
        ProviderId::new(CLAUDE_CODE)
    }

    pub fn paths(&self) -> CcPaths {
        CcPaths::resolve(&self.env)
    }

    pub fn credential_json(email: &str, rt: &str) -> Value {
        json!({
            "claudeAiOauth": {
                "accessToken": format!("at-{rt}"),
                "refreshToken": rt,
                "expiresAt": 1_790_003_600_000i64,
                "refreshTokenExpiresAt": 1_797_000_000_000i64,
                "scopes": ["user:inference", "user:profile"],
                "subscriptionType": if email.starts_with("team") { "team" } else { "max" }
            },
            "mcpOAuth": {"srv": {"token": "machine-shared"}}
        })
    }

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

    pub fn oauth_account(email: &str) -> Value {
        json!({"emailAddress": email, "organizationUuid": "", "organizationName": null, "accountUuid": format!("uuid-{email}")})
    }

    /// What `claude /login` leaves behind: `oauthAccount` plus the live credential.
    pub fn login(&self, email: &str, rt: &str) {
        splice_oauth_account(&self.paths().global_config, &Self::oauth_account(email));
        self.set_live_credential(Self::credential_json(email, rt).to_string().as_bytes());
    }

    pub fn set_live_credential(&self, bytes: &[u8]) {
        match self.platform {
            Platform::MacOs => self.kc.put(
                &keychain_service(&self.env, ItemKind::OAuth),
                &keychain_account(&self.env),
                bytes,
            ),
            Platform::Linux => fs::write(self.paths().credentials_file, bytes).unwrap(),
        }
    }

    /// The (service, account) of the live Keychain item of `kind` that CC writes.
    pub fn live_item(&self, kind: ItemKind) -> (String, String) {
        (
            keychain_service(&self.env, kind),
            keychain_account(&self.env),
        )
    }

    pub fn put_managed_key(&self, key: &[u8]) {
        let (svc, acct) = self.live_item(ItemKind::ManagedKey);
        self.kc.put(&svc, &acct, key);
    }

    pub fn managed_key(&self) -> Option<Vec<u8>> {
        let (svc, acct) = self.live_item(ItemKind::ManagedKey);
        self.kc.get(&svc, &acct)
    }

    /// The contents of every file in `displaced/`.
    pub fn displaced(&self) -> Vec<Vec<u8>> {
        fs::read_dir(self.env.data_dir().join("displaced"))
            .map(|d| d.map(|e| fs::read(e.unwrap().path()).unwrap()).collect())
            .unwrap_or_default()
    }

    /// A manual `switch <id>` from the CLI, optionally forced.
    pub fn switch_request(&self, id: &AccountId, force: bool) -> SwitchRequest {
        SwitchRequest {
            provider: self.provider(),
            target: SwitchTarget::Account(id.clone()),
            force,
            source: "cli",
            auto: None,
        }
    }

    pub fn switch_to(&self, id: &AccountId, force: bool) -> Result<SwitchOutcome, EngineError> {
        self.engine.switch(self.switch_request(id, force))
    }

    /// A manual `switch --rotate` from the CLI, optionally forced.
    pub fn rotation_request(&self, force: bool) -> SwitchRequest {
        SwitchRequest {
            provider: self.provider(),
            target: SwitchTarget::Rotation,
            force,
            source: "cli",
            auto: None,
        }
    }

    pub fn live_credential(&self) -> Option<Value> {
        let bytes = match self.platform {
            Platform::MacOs => self.kc.get(
                &keychain_service(&self.env, ItemKind::OAuth),
                &keychain_account(&self.env),
            ),
            Platform::Linux => fs::read(self.paths().credentials_file).ok(),
        }?;
        serde_json::from_slice(&bytes).ok()
    }

    /// CC rotating the live refresh token in place.
    pub fn rotate_live(&self, new_rt: &str) {
        let mut v = self.live_credential().unwrap();
        v["claudeAiOauth"]["refreshToken"] = json!(new_rt);
        self.set_live_credential(v.to_string().as_bytes());
    }

    pub fn live_refresh_token(&self) -> Option<String> {
        self.live_credential()?["claudeAiOauth"]["refreshToken"]
            .as_str()
            .map(str::to_owned)
    }

    pub fn live_email(&self) -> Option<String> {
        match self.cc.live_identity(&self.env) {
            Read::Present(i) => i.email,
            _ => None,
        }
    }

    pub fn vault_bytes(&self, id: &AccountId) -> Option<Vec<u8>> {
        self.engine_vault_read(id)
    }

    fn engine_vault_read(&self, id: &AccountId) -> Option<Vec<u8>> {
        match self.platform {
            Platform::MacOs => self.kc.get(SERVICE, id.as_str()),
            Platform::Linux => {
                fs::read(self.env.data_dir().join("vault").join(format!("{id}.json"))).ok()
            }
        }
    }

    pub fn vault_refresh_token(&self, id: &AccountId) -> Option<String> {
        let v: Value = serde_json::from_slice(&self.vault_bytes(id)?).ok()?;
        v["claudeAiOauth"]["refreshToken"]
            .as_str()
            .map(str::to_owned)
    }

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

    /// The `AddOptions` every plain `add_live` call in these tests starts from — shared by
    /// `Fx::add` and by `add.rs`'s own `add_opts` (Task 18's review, item 8).
    pub fn add_options(&self) -> AddOptions {
        AddOptions {
            provider: self.provider(),
            position: None,
            alias: None,
            yes: false,
        }
    }

    /// The `AddTokenOptions` every plain `add_token` call in these tests starts from.
    pub fn add_token_options(&self, token: &str) -> AddTokenOptions {
        AddTokenOptions {
            provider: self.provider(),
            token: token.into(),
            position: None,
            email: None,
            alias: None,
            yes: false,
        }
    }

    /// Stores an API-key account (§10.2) without touching the live login.
    pub fn add_api_key(&self, key: &str) -> AccountId {
        self.engine
            .add_token(self.add_token_options(key))
            .unwrap()
            .account
            .id
    }

    /// Logs a fresh account in and captures it (§10.1). Tasks 19-21 each need only the
    /// resulting id, so this is the one place that repeats `fx.login` + `add_live`.
    pub fn add(&self, email: &str, rt: &str) -> AccountId {
        self.login(email, rt);
        self.engine.add_live(self.add_options()).unwrap().account.id
    }

    /// A pending replacement whose vault write landed but whose metadata never did — the
    /// crash-recovery scenario `finish_replacement`/`rollback_replacement` exist for.
    /// `identity_json` is the raw `oauthAccount`-shaped object the replacement claims to be
    /// (Task 18's review, item 8: shared by every test that primes this scenario, instead of
    /// each one repeating the same four calls).
    pub fn begin_replacement(
        &self,
        id: &AccountId,
        new_bytes: &[u8],
        identity_json: &Value,
        kind: &str,
    ) {
        let identity = self.cc.parse_identity(identity_json).unwrap();
        let identity_key = format!(
            "{}\n{}",
            identity.email.as_deref().unwrap_or(&identity.label),
            identity.org_uuid
        );
        self.kc.put(SERVICE, id.as_str(), new_bytes);
        let meta = LoginMeta {
            identity_key: &identity_key,
            identity: &identity,
            kind,
            login_expires_at: None,
        };
        self.engine
            .store()
            .unwrap()
            .begin_replacement(id, self.cc.fingerprint(new_bytes).unwrap().as_str(), &meta)
            .unwrap();
    }

    /// Quarantines an account row directly, bound to `fp` (§7.4).
    pub fn quarantine(&self, id: &AccountId, reason: &str, fp: &str) {
        self.engine
            .store()
            .unwrap()
            .set_quarantine(id, reason, fp, 1)
            .unwrap();
    }

    /// The provider's active account and its activation epoch (§12.5).
    pub fn activation(&self) -> Option<Activation> {
        self.engine
            .store()
            .unwrap()
            .activation(&self.provider())
            .unwrap()
    }

    /// Whether the live store is stale-marked for `id` (§12.5).
    pub fn live_store_stale(&self, id: &AccountId) -> bool {
        let store = self.engine.store().unwrap();
        let row = store.account(id).unwrap().unwrap();
        store.live_store_stale(&row).unwrap()
    }

    /// A second engine over this fixture's provider and clock, as another tagteam process.
    fn engine_over(&self, env: Env, vault: Vault, oracle: Arc<dyn Oracle>) -> Engine {
        Engine::new(EngineConfig {
            env,
            registry: ProviderRegistry::new().with(self.cc.clone()),
            vault,
            oracle,
            clock: self.clock.clone(),
            http: self.http.clone(),
            default_provider: ProviderId::new(CLAUDE_CODE),
            settings: Settings::default(),
        })
    }

    fn keychain_vault(&self) -> Vault {
        Vault::new(Box::new(KeychainVault::new(self.kc.clone())))
    }

    /// An engine over the same Keychain, oracle and clock, but a different Env.
    pub fn engine_with_env(&self, env: Env) -> Engine {
        self.engine_over(env, self.keychain_vault(), self.oracle.clone())
    }

    /// An engine over the same Keychain, clock and Env, but a caller-supplied oracle — for
    /// exercising the oracle-call race between the pre-lock read and the locks (Task 18's
    /// review, item 2).
    pub fn engine_with_oracle(&self, oracle: Arc<dyn Oracle>) -> Engine {
        self.engine_over(self.env.clone(), self.keychain_vault(), oracle)
    }

    /// An engine over the same Env, Keychain, oracle and clock whose vault runs `on_read`
    /// before every read (see `ProbeVault`).
    pub fn engine_with_vault_probe(
        &self,
        on_read: impl Fn(&str) + Send + Sync + 'static,
    ) -> Engine {
        let vault = Vault::new(Box::new(ProbeVault {
            inner: KeychainVault::new(self.kc.clone()),
            on_read: Box::new(on_read),
        }));
        self.engine_over(self.env.clone(), vault, self.oracle.clone())
    }

    /// An engine over the same Env, Keychain, oracle, clock and HTTP port, with a
    /// caller-supplied vault: for making the vault itself misbehave.
    pub fn engine_with_vault(&self, vault: Vault) -> Engine {
        self.engine_over(self.env.clone(), vault, self.oracle.clone())
    }
}

/// A path's kind, for the snapshot comparison. A symlink records its `read_link` target
/// rather than following it: the walk never reads or descends through a link.
#[derive(Clone, PartialEq, Eq)]
enum EntryKind {
    File(Vec<u8>),
    Dir,
    Symlink(PathBuf),
}

/// A lossy UTF-8 rendering of a file's bytes, so a violation message shows readable JSON
/// instead of a raw byte-array dump.
impl std::fmt::Debug for EntryKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EntryKind::File(bytes) => f
                .debug_tuple("File")
                .field(&String::from_utf8_lossy(bytes))
                .finish(),
            EntryKind::Dir => write!(f, "Dir"),
            EntryKind::Symlink(target) => f.debug_tuple("Symlink").field(target).finish(),
        }
    }
}

/// One path's kind, permission bits, and content (for a file) — everything the invariant
/// compares. Two snapshots' entries at the same path are equal only if all three match, so a
/// mode change, a kind swap (a symlink replaced by a regular file, say), or a new directory
/// are all differences, not just a changed file's bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    mode: u32,
    kind: EntryKind,
}

/// Every file, directory and symlink under HOME except tagteam's own data dir, plus every
/// Keychain item except the vault's.
pub struct HomeSnapshot {
    files: BTreeMap<PathBuf, Entry>,
    items: BTreeMap<(String, String), Vec<u8>>,
}

/// Records every entry under `dir` except under `skip`, without ever following a symlink into
/// its target. Any I/O failure here is a bug in the fixture or the walk, not a state to
/// tolerate silently: it panics rather than treating a path as absent or empty.
///
/// A bare ancestor directory of `skip` (`~/.local`, say, above `~/.local/share/tagteam`) is
/// walked through — so a sibling of the excluded subtree is still found — and IS recorded like
/// any other entry. Its lazy first creation (a side effect of creating the excluded subtree) is
/// tolerated by the comparison, not by omitting it here: once it exists, a later change to it
/// (a mode change, say) must still be caught, which excluding it from the walk entirely could
/// never do.
fn walk(dir: &Path, skip: &Path, out: &mut BTreeMap<PathBuf, Entry>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.starts_with(skip) {
            continue;
        }
        let meta = fs::symlink_metadata(&path).unwrap();
        let mode = meta.permissions().mode() & 0o7777;
        let is_plain_dir = meta.is_dir() && !meta.file_type().is_symlink();
        if is_plain_dir {
            walk(&path, skip, out);
        }
        let kind = if meta.file_type().is_symlink() {
            EntryKind::Symlink(fs::read_link(&path).unwrap())
        } else if is_plain_dir {
            EntryKind::Dir
        } else {
            EntryKind::File(fs::read(&path).unwrap())
        };
        out.insert(path, Entry { mode, kind });
    }
}

/// §3: `customApiKeyResponses.approved` may only grow by appending; nothing else in that
/// object may change.
fn check_api_key_responses(
    before: Option<&Vec<u8>>,
    after: Option<&Vec<u8>>,
    step: &str,
    path: &Path,
) {
    let get = |d: Option<&Vec<u8>>| {
        d.and_then(|d| get_top_level(d, "customApiKeyResponses").unwrap())
            .unwrap_or_else(|| json!({}))
    };
    let (mut b, mut a) = (get(before), get(after));
    let approved = |v: &mut Value| {
        v.as_object_mut()
            .and_then(|o| o.remove("approved"))
            .and_then(|x| x.as_array().cloned())
            .unwrap_or_default()
    };
    let (b_list, a_list) = (approved(&mut b), approved(&mut a));
    assert!(
        a_list.starts_with(&b_list),
        "{step}: customApiKeyResponses.approved lost or reordered entries in {}",
        path.display()
    );
    assert_eq!(
        b,
        a,
        "{step}: customApiKeyResponses changed beyond appending to approved in {}",
        path.display()
    );
}

fn shared_keys(bytes: Option<&Vec<u8>>, keys: &[&str]) -> Map<String, Value> {
    let v: Value = bytes
        .and_then(|b| serde_json::from_slice(b).ok())
        .unwrap_or(Value::Null);
    v.as_object()
        .map(|o| {
            o.iter()
                .filter(|(k, _)| keys.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// The bytes of `path`'s entry, for a path declared in the identity surface. `None` when it
/// does not exist yet (a credential file created on first use). Panics if it exists as
/// something other than a plain file: a declared surface path swapping kind is itself a
/// violation this comparison must not silently wave through.
fn surface_file_bytes<'e>(
    entry: Option<&'e Entry>,
    step: &str,
    path: &Path,
) -> Option<&'e Vec<u8>> {
    match entry {
        None => None,
        Some(Entry {
            kind: EntryKind::File(bytes),
            ..
        }) => Some(bytes),
        Some(_) => panic!(
            "{step}: {} is declared in the identity surface but is no longer a plain file",
            path.display()
        ),
    }
}

/// Collapses `.`/`..` components lexically (no filesystem access): a relative symlink target
/// joined to its link's parent directory needs this before it can match one of the snapshot's
/// own keys, which are always built without `..` segments.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The real path a declared surface path's writes actually land at: the pre-mutation
/// snapshot's `read_link` target when the surface path is itself a symlink (§9.5's
/// write-through) — a relative target resolved against its link's own directory, and a chain of
/// links followed up to 40 hops, matching `tagteam_provider::atomic::resolve_target`'s own limit
/// — or the path itself when it is not a symlink. Resolving from `before` — never `after` —
/// means the literal surface path is then left to the default byte-for-byte rule below, so a
/// link that gets replaced, or repointed, is still caught: it is no longer a surface path once
/// resolved away from, so any change to it at all is a violation.
fn resolve(snapshot: &HomeSnapshot, path: &Path) -> PathBuf {
    let mut current = path.to_path_buf();
    for _ in 0..40 {
        let Some(Entry {
            kind: EntryKind::Symlink(target),
            ..
        }) = snapshot.files.get(&current)
        else {
            return current;
        };
        let next = if target.is_absolute() {
            target.clone()
        } else {
            current.parent().unwrap_or(Path::new("/")).join(target)
        };
        current = normalize(&next);
    }
    current
}

impl Fx {
    pub fn snapshot(&self) -> HomeSnapshot {
        let mut files = BTreeMap::new();
        walk(&self.env.home, &self.env.data_dir(), &mut files);
        let items = self
            .kc
            .items()
            .into_iter()
            .filter(|((svc, _), _)| svc != SERVICE)
            .collect();
        HomeSnapshot { files, items }
    }

    /// §15.3: every byte, mode, and kind outside the identity surface is identical; inside it,
    /// only the declared keys moved, and the machine-shared credential keys kept their values.
    /// A declared path's rules apply to its resolved target, not its literal name, so a
    /// symlinked surface file is checked correctly while the link itself is held to the same
    /// byte-for-byte rule as everything else.
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
        let data_dir = self.env.data_dir();
        let json_keys: BTreeMap<PathBuf, Vec<String>> = surface
            .json_keys
            .iter()
            .map(|(p, keys)| (resolve(before, p), keys.clone()))
            .collect();
        let cred_files: BTreeSet<PathBuf> = surface
            .credential_files
            .iter()
            .map(|p| resolve(before, p))
            .collect();
        let paths: BTreeSet<&PathBuf> = before.files.keys().chain(after.files.keys()).collect();
        for path in paths {
            let (b, a) = (before.files.get(path), after.files.get(path));
            if b.is_none() && data_dir.starts_with(path) {
                // A bare ancestor of tagteam's own data dir, created lazily just now: tolerated
                // only on its first appearance. Once it exists in `before` too, it falls through
                // to the rules below like any other path, so a later change to it is still caught.
                continue;
            }
            if let Some(keys) = json_keys.get(path) {
                let (bb, ab) = (
                    surface_file_bytes(b, step, path),
                    surface_file_bytes(a, step, path),
                );
                if let (Some(bm), Some(am)) = (b.map(|e| e.mode), a.map(|e| e.mode)) {
                    assert_eq!(bm, am, "{step}: {} changed mode", path.display());
                }
                if keys.iter().any(|k| k == "customApiKeyResponses") {
                    check_api_key_responses(bb, ab, step, path);
                }
                let strip = |doc: Option<&Vec<u8>>| {
                    doc.map(|d| {
                        keys.iter()
                            .fold(d.clone(), |acc, k| remove_top_level(&acc, k).unwrap())
                    })
                };
                assert_eq!(
                    strip(bb),
                    strip(ab),
                    "{step}: {} changed outside {keys:?}",
                    path.display()
                );
            } else if cred_files.contains(path) {
                let (bb, ab) = (
                    surface_file_bytes(b, step, path),
                    surface_file_bytes(a, step, path),
                );
                if let (Some(bm), Some(am)) = (b.map(|e| e.mode), a.map(|e| e.mode)) {
                    // A credential file is forced to 0600 on every write (never preserved,
                    // never chmod'ed to anything else), so a mode change is only ever
                    // allowed when it lands exactly there.
                    assert!(
                        bm == am || am == 0o600,
                        "{step}: {} changed mode from {bm:o} to {am:o}",
                        path.display()
                    );
                }
                assert_eq!(
                    shared_keys(bb, &surface.machine_shared_keys),
                    shared_keys(ab, &surface.machine_shared_keys),
                    "{step}: machine-shared keys changed in {}",
                    path.display()
                );
            } else {
                assert_eq!(b, a, "{step}: {} changed", path.display());
            }
        }
        let owned: BTreeSet<(String, String)> = surface.owned_items.iter().cloned().collect();
        let creds: BTreeSet<(String, String)> = surface.credential_items.iter().cloned().collect();
        let keys: BTreeSet<&(String, String)> =
            before.items.keys().chain(after.items.keys()).collect();
        for key in keys {
            let (b, a) = (before.items.get(key), after.items.get(key));
            if owned.contains(key) {
                continue;
            }
            if creds.contains(key) {
                assert_eq!(
                    shared_keys(b, &surface.machine_shared_keys),
                    shared_keys(a, &surface.machine_shared_keys),
                    "{step}: machine-shared keys changed in Keychain item {key:?}"
                );
            } else {
                assert_eq!(b, a, "{step}: Keychain item {key:?} changed");
            }
        }
    }
}

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
            settings: Settings::default(),
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
                auto: None,
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

/// How many files `rescue/` holds; 0 when it does not exist.
pub fn rescue_files(fx: &Fx) -> usize {
    fs::read_dir(fx.env.data_dir().join("rescue")).map_or(0, |d| d.count())
}

/// The refresh token inside `id`'s `.prev` vault generation, if there is one.
pub fn prev_refresh_token(fx: &Fx, id: &AccountId) -> Option<String> {
    let v: Value = serde_json::from_slice(&fx.kc.get(SERVICE, &format!("{id}.prev"))?).ok()?;
    v["claudeAiOauth"]["refreshToken"]
        .as_str()
        .map(str::to_owned)
}

/// Usage collection (§8), shared by `collect.rs`, `collect_active.rs` and `switch.rs`.
impl Fx {
    /// Queues a reply from the usage endpoint (§8.1): `status` with a JSON `body`.
    pub fn script_usage(&self, status: u16, body: Value) {
        self.http
            .push_json(Method::Get, &Self::endpoints().usage, status, body);
    }

    /// Queues a 429 whose `Retry-After` header is `retry_after`, verbatim (§8.1, §8.5).
    pub fn script_usage_429(&self, retry_after: &str) {
        let mut reply = tagteam_provider::HttpResponse::json_body(
            429,
            &json!({"type": "error", "error": {"type": "rate_limit_error", "message": "Rate limited"}}),
        );
        reply
            .headers
            .push(("retry-after".to_owned(), retry_after.to_owned()));
        self.http
            .push(Method::Get, &Self::endpoints().usage, Ok(reply));
    }

    /// `list`'s on-demand collection of `ids` through the fixture's engine (§8.3).
    pub fn collect(&self, ids: &[&AccountId]) -> tagteam_engine::collect::CollectReport {
        self.engine
            .collect_usage(tagteam_engine::collect::CollectMode::OnDemand {
                accounts: ids.iter().map(|id| (*id).clone()).collect(),
            })
            .unwrap()
    }

    /// `id`'s `usage_state` row, if it has one.
    pub fn usage_state(&self, id: &AccountId) -> Option<tagteam_engine::store::UsageStateRow> {
        self.engine.store().unwrap().usage_state(id).unwrap()
    }

    /// An engine over this fixture's Env, Keychain, oracle and clock whose requests go to
    /// `http` rather than to the fixture's scripted port.
    pub fn engine_with_http(&self, http: Arc<dyn tagteam_provider::Http>) -> Engine {
        self.engine_over_http(http, Settings::default())
    }

    /// An engine over this fixture's Env, Keychain, oracle, clock and scripted port with
    /// `settings`, as the CLI builds one after reading `config.toml`.
    pub fn engine_with_settings(&self, settings: Settings) -> Engine {
        self.engine_over_http(self.http.clone(), settings)
    }

    fn engine_over_http(
        &self,
        http: Arc<dyn tagteam_provider::Http>,
        settings: Settings,
    ) -> Engine {
        Engine::new(EngineConfig {
            env: self.env.clone(),
            registry: ProviderRegistry::new().with(self.cc.clone()),
            vault: self.keychain_vault(),
            oracle: self.oracle.clone(),
            clock: self.clock.clone(),
            http,
            default_provider: ProviderId::new(CLAUDE_CODE),
            settings,
        })
    }
}

/// The usage body recorded from the live endpoint (Appendix A.5).
pub fn usage_fixture() -> Value {
    let recorded: Value = serde_json::from_str(include_str!(
        "../../../tagteam-cc/tests/fixtures/endpoints/usage-200.json"
    ))
    .unwrap();
    recorded["body"].clone()
}

/// The access tokens Claude Code's usage requests carried, in the order they were sent.
pub fn usage_bearers(fx: &Fx) -> Vec<String> {
    let usage = Fx::endpoints().usage;
    fx.http
        .requests()
        .iter()
        .filter(|r| r.method == Method::Get && r.url == usage)
        .filter_map(|r| {
            r.headers
                .iter()
                .find(|(name, _)| name == "authorization")
                .and_then(|(_, value)| value.strip_prefix("Bearer "))
                .map(str::to_owned)
        })
        .collect()
}

/// When each slot the hourly budget counts was reserved, in epoch seconds, oldest first (§8.6).
pub fn slot_times(fx: &Fx) -> Vec<i64> {
    let conn = rusqlite::Connection::open(fx.env.data_dir().join("tagteam.db")).unwrap();
    let mut rows = conn
        .prepare("SELECT at FROM usage_requests ORDER BY at")
        .unwrap();
    rows.query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// How many slots the hourly budget counts: every request sent, and any reserved but unsent.
pub fn usage_requests(fx: &Fx) -> usize {
    slot_times(fx).len()
}

/// The access-token fingerprint `rejected_fp` holds for `secret` (§8.1).
pub fn access_fp(fx: &Fx, secret: &[u8]) -> String {
    fx.cc
        .access_fingerprint(secret)
        .unwrap()
        .as_str()
        .to_owned()
}

/// A usage collection that recorded `kind` as its failure.
pub fn failed(kind: &str) -> tagteam_engine::collect::Collected {
    tagteam_engine::collect::Collected::Failed { kind: kind.into() }
}

/// The usage endpoint refusing the token.
pub fn refused() -> Value {
    json!({"type": "error", "error": {"type": "authentication_error", "message": "Invalid bearer token"}})
}

/// The method of every request the fixture's scripted port has seen, in order.
pub fn methods(fx: &Fx) -> Vec<Method> {
    fx.http.requests().iter().map(|r| r.method).collect()
}

/// A `tracing` writer that appends to a shared buffer.
#[derive(Clone, Default)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
    type Writer = LogBuffer;

    fn make_writer(&'a self) -> LogBuffer {
        self.clone()
    }
}

/// Runs `f` with every `tracing` event this thread emits captured, and returns `f`'s result
/// with the captured lines: one per event, level first (`ERROR`, `WARN`, ...), then the
/// message and its fields, without colour or timestamps.
pub fn capture_logs<T>(f: impl FnOnce() -> T) -> (T, Vec<String>) {
    let buffer = LogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(buffer.clone())
        .with_ansi(false)
        .without_time()
        .with_target(false)
        .with_max_level(tracing::Level::TRACE)
        .finish();
    let result = tracing::subscriber::with_default(subscriber, f);
    let text = String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap();
    (result, text.lines().map(str::to_owned).collect())
}

/// The thread `cc_holds_storage_write_from` starts; it returns the instant just before CC let go
/// of its storage-write lock.
#[cfg(feature = "test-hooks")]
pub type CcWrite = Arc<Mutex<Option<std::thread::JoinHandle<std::time::Instant>>>>;

/// Claude Code holding its storage-write lock (§9.1) from the engine's hook `point` on: it takes
/// the lock there; then, on another thread, it sets the engine's cancel token to SIGINT after
/// `ctrl_c` when given, applies `cc_write` to the live OAuth item 300 ms in, and lets go.
#[cfg(feature = "test-hooks")]
pub fn cc_holds_storage_write_from(
    fx: &Fx,
    point: &'static str,
    ctrl_c: Option<Duration>,
    cc_write: impl Fn(&mut Value) + Send + Sync + 'static,
) -> CcWrite {
    let (svc, _) = fx.live_item(ItemKind::OAuth);
    writer_holds_storage_write_from(fx, point, ctrl_c, &svc, cc_write)
}

/// `cc_holds_storage_write_from` for the Keychain item `svc`, which need not be the one Claude
/// Code reads first: what another writer holding the storage-write lock does to it.
#[cfg(feature = "test-hooks")]
pub fn writer_holds_storage_write_from(
    fx: &Fx,
    point: &'static str,
    ctrl_c: Option<Duration>,
    svc: &str,
    cc_write: impl Fn(&mut Value) + Send + Sync + 'static,
) -> CcWrite {
    let lock = fx.paths().storage_write_lock;
    let (kc, svc, acct) = (fx.kc.clone(), svc.to_owned(), keychain_account(&fx.env));
    let cancel = fx.engine.cancel().clone();
    let cc_write = Arc::new(cc_write);
    let cc: CcWrite = Arc::new(Mutex::new(None));
    let started = cc.clone();
    fx.engine.on_point(
        point,
        Box::new(move || {
            fs::create_dir(&lock).unwrap();
            let (lock, kc, svc, acct) = (lock.clone(), kc.clone(), svc.clone(), acct.clone());
            let (cancel, cc_write) = (cancel.clone(), cc_write.clone());
            *started.lock().unwrap() = Some(std::thread::spawn(move || {
                let begun = std::time::Instant::now();
                if let Some(after) = ctrl_c {
                    std::thread::sleep(after);
                    cancel.request(libc::SIGINT);
                }
                std::thread::sleep(Duration::from_millis(300).saturating_sub(begun.elapsed()));
                let mut live: Value =
                    serde_json::from_slice(&kc.get(&svc, &acct).unwrap()).unwrap();
                cc_write(&mut live);
                kc.put(&svc, &acct, live.to_string().as_bytes());
                let at = std::time::Instant::now();
                fs::remove_dir(&lock).unwrap();
                at
            }));
        }),
    );
    cc
}

/// The instant CC let go of its storage-write lock (`cc_holds_storage_write_from`).
#[cfg(feature = "test-hooks")]
pub fn cc_released(cc: &CcWrite) -> std::time::Instant {
    cc.lock()
        .unwrap()
        .take()
        .expect("the hook point was reached")
        .join()
        .unwrap()
}

/// CC's dead-token marking (Appendix A.3): both tokens empty and `expiresAt` 0, a write CC
/// makes without its credential locks. It is no conflict (§9.1).
pub fn cc_marks_dead(live: &mut Value) {
    live["claudeAiOauth"]["accessToken"] = json!("");
    live["claudeAiOauth"]["refreshToken"] = json!("");
    live["claudeAiOauth"]["expiresAt"] = json!(0);
}

/// A marking together with another account-scoped change, a new `trustedDeviceToken`: not a
/// marking alone, so a write that finds it aborts (§9.1).
pub fn cc_marks_dead_and_more(live: &mut Value) {
    cc_marks_dead(live);
    live["trustedDeviceToken"] = json!("cc-device");
}

/// An auto-switch event sink that keeps every event it is given, in order (§11.4).
#[derive(Default)]
pub struct Recorded(Mutex<Vec<AutoEvent>>);

impl EventSink for Recorded {
    fn emit(&self, e: &AutoEvent) {
        self.0.lock().unwrap().push(e.clone());
    }
}

impl Recorded {
    /// The events emitted since the last call, oldest first.
    pub fn take(&self) -> Vec<AutoEvent> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

/// A usage window of `kind` at `pct` that resets at `resets_at`, labelled as Claude Code
/// labels its windows (`scoped:Fable` is `Fable`).
pub fn usage_window(key: &str, kind: WindowKind, pct: f64, resets_at: i64) -> Window {
    Window {
        key: key.into(),
        label: key.strip_prefix("scoped:").unwrap_or(key).into(),
        kind,
        pct,
        resets_at: Some(resets_at),
        period_s: match kind {
            WindowKind::Short => Some(18_000),
            WindowKind::Spend => None,
            _ => Some(604_800),
        },
        detail: None,
    }
}

/// Records `windows` as `id`'s reading taken at `at` (epoch seconds), its next poll planned at
/// `next_poll_at`, through `engine`'s store as the collector records one: reserve (§8.3 phase
/// 1, which counts a slot of the hourly budget and leaves a 90 s lease), then record (phase 3).
pub fn record_reading(
    engine: &Engine,
    id: &AccountId,
    windows: &[Window],
    at: i64,
    next_poll_at: i64,
) {
    let store = engine.store().unwrap();
    let row = store.account(id).unwrap().unwrap();
    let r = match store
        .reserve_usage(
            &row,
            at * 1000,
            Eligibility::Scheduled,
            &PollBudget::STANDARD,
        )
        .unwrap()
    {
        Reserve::Reserved(r) => r,
        other => panic!("not reserved at {at}: {other:?}"),
    };
    let plan = PollPlan {
        interval_s: next_poll_at - at,
        next_poll_at,
    };
    assert!(store.record_usage(&r, windows, at, &plan, 180).unwrap());
}
