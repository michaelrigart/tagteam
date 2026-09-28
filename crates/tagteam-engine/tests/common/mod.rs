#![allow(dead_code)]

use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tagteam_cc::live::{LiveStore, Platform};
use tagteam_cc::{CcPaths, ClaudeCode, ItemKind, keychain_account, keychain_service};
use tagteam_core::{AccountId, CLAUDE_CODE, ProviderId};
use tagteam_engine::lifecycle::{AddOptions, AddTokenOptions};
use tagteam_engine::oracle::Oracle;
use tagteam_engine::registry::ProviderRegistry;
use tagteam_engine::store::LoginMeta;
use tagteam_engine::vault::{FileVault, KeychainVault, SERVICE, Vault, VaultBackend, VaultError};
use tagteam_engine::{Engine, EngineConfig};
use tagteam_provider::splice::replace_top_level;
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

/// Replaces `oauthAccount` in the config at `path`, as CC does on a login. A free function, so
/// a `'static` race callback can call it without borrowing the fixture.
pub fn splice_oauth_account(path: &Path, oauth_account: &Value) {
    let doc = fs::read(path).unwrap();
    fs::write(
        path,
        replace_top_level(&doc, "oauthAccount", oauth_account).unwrap(),
    )
    .unwrap();
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
