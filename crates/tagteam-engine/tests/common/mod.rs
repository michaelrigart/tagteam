#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Map, Value, json};
use tagteam_cc::live::{LiveStore, Platform};
use tagteam_cc::{CcPaths, ClaudeCode, ItemKind, keychain_account, keychain_service};
use tagteam_core::{AccountId, CLAUDE_CODE, ProviderId};
use tagteam_engine::lifecycle::{AddOptions, AddTokenOptions};
use tagteam_engine::oracle::Oracle;
use tagteam_engine::registry::ProviderRegistry;
use tagteam_engine::store::LoginMeta;
use tagteam_engine::switch::{SwitchOutcome, SwitchRequest, SwitchTarget};
use tagteam_engine::vault::{FileVault, KeychainVault, SERVICE, Vault, VaultBackend, VaultError};
use tagteam_engine::{Engine, EngineConfig, EngineError};
use tagteam_provider::splice::{get_top_level, remove_top_level, replace_top_level};
use tagteam_provider::{
    Credential, Env, FakeClock, FakeKeychain, Identity, MutationGuard, Provider, Read,
};

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

/// Whether tagteam's mutation lock is free right now; takes and drops it if so.
pub fn mutation_lock_free(env: &Env) -> bool {
    MutationGuard::acquire(env, Duration::ZERO).is_ok()
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
            default_provider: ProviderId::new(CLAUDE_CODE),
        });
        Fx {
            dir,
            env,
            platform,
            kc,
            oracle,
            clock,
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
        }
    }

    pub fn switch_to(&self, id: &AccountId, force: bool) -> Result<SwitchOutcome, EngineError> {
        self.engine.switch(self.switch_request(id, force))
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

    /// Directly quarantines an account row. There is no public writer for this yet (a later
    /// task adds one); a second connection to the same on-disk store, mirroring
    /// `tests/store.rs`, is the only way a fixture can prime this state today.
    pub fn quarantine(&self, id: &AccountId, reason: &str, fp: &str) {
        self.engine.store().unwrap(); // ensures the db file exists and is migrated
        let path = self.env.data_dir().join("tagteam.db");
        rusqlite::Connection::open(path)
            .unwrap()
            .execute(
                "UPDATE accounts SET quarantine_reason = ?2, quarantine_fp = ?3, quarantine_at = 1 WHERE id = ?1",
                rusqlite::params![id.as_str(), reason, fp],
            )
            .unwrap();
    }

    /// A second engine over this fixture's provider and clock, as another tagteam process.
    fn engine_over(&self, env: Env, vault: Vault, oracle: Arc<dyn Oracle>) -> Engine {
        Engine::new(EngineConfig {
            env,
            registry: ProviderRegistry::new().with(self.cc.clone()),
            vault,
            oracle,
            clock: self.clock.clone(),
            default_provider: ProviderId::new(CLAUDE_CODE),
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
}

/// Every file under HOME except tagteam's own data dir, plus every Keychain item except the
/// vault's.
pub struct HomeSnapshot {
    files: BTreeMap<PathBuf, Vec<u8>>,
    items: BTreeMap<(String, String), Vec<u8>>,
}

fn walk(dir: &Path, skip: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let path = e.path();
        if path.starts_with(skip) {
            continue;
        }
        let meta = fs::symlink_metadata(&path).unwrap();
        if meta.is_dir() {
            walk(&path, skip, out);
        } else {
            out.insert(path.clone(), fs::read(&path).unwrap_or_default());
        }
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

impl Fx {
    pub fn snapshot(&self) -> HomeSnapshot {
        let mut files = BTreeMap::new();
        walk(&self.env.home, &self.env.data_dir(), &mut files);
        let items = self
            .kc
            .items()
            .into_iter()
            .filter(|((svc, _), _)| svc != "tagteam")
            .collect();
        HomeSnapshot { files, items }
    }

    /// §15.3: every byte outside the identity surface is identical; inside it, only the
    /// declared keys moved, and the machine-shared credential keys kept their values.
    pub fn assert_only_surface_changed(
        &self,
        before: &HomeSnapshot,
        after: &HomeSnapshot,
        step: &str,
    ) {
        let surface = self.cc.identity_surface(&self.env);
        let json_keys: BTreeMap<PathBuf, Vec<String>> = surface.json_keys.iter().cloned().collect();
        let cred_files: BTreeSet<PathBuf> = surface.credential_files.iter().cloned().collect();
        let paths: BTreeSet<&PathBuf> = before.files.keys().chain(after.files.keys()).collect();
        for path in paths {
            let (b, a) = (before.files.get(path), after.files.get(path));
            if let Some(keys) = json_keys.get(path) {
                if keys.iter().any(|k| k == "customApiKeyResponses") {
                    check_api_key_responses(b, a, step, path);
                }
                let strip = |doc: Option<&Vec<u8>>| {
                    doc.map(|d| {
                        keys.iter()
                            .fold(d.clone(), |acc, k| remove_top_level(&acc, k).unwrap())
                    })
                };
                assert_eq!(
                    strip(b),
                    strip(a),
                    "{step}: {} changed outside {keys:?}",
                    path.display()
                );
            } else if cred_files.contains(path) {
                assert_eq!(
                    shared_keys(b, &surface.machine_shared_keys),
                    shared_keys(a, &surface.machine_shared_keys),
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
