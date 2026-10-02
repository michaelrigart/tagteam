//! A small engine over Claude Code, a fake Keychain and a fixed clock, for unit tests of the
//! crate-private machinery (rescue, quarantine, the gate). Test-only.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tagteam_cc::ClaudeCode;
use tagteam_cc::live::{LiveStore, Platform};
use tagteam_core::{AccountId, CLAUDE_CODE, ProviderId};
use tagteam_provider::http::NoHttp;
use tagteam_provider::profile::RunShell;
use tagteam_provider::{Env, FakeClock, FakeKeychain, Identity, Provider};

use crate::account_lock::AccountLock;
use crate::engine::{Engine, EngineConfig};
use crate::oracle::NoOracle;
use crate::registry::ProviderRegistry;
use crate::settings::Settings;
use crate::store::{AccountRow, NewAccount};
use crate::vault::{KeychainVault, SERVICE, Vault};

pub(crate) const NOW: i64 = 1_790_000_000_000;

/// A stored OAuth credential of generation `rt`, with a login expiry of `rte`.
pub(crate) fn cred(rt: &str, rte: i64) -> Vec<u8> {
    json!({"claudeAiOauth": {"accessToken": format!("at-{rt}"), "refreshToken": rt,
        "expiresAt": NOW + 3_600_000, "refreshTokenExpiresAt": rte}})
    .to_string()
    .into_bytes()
}

pub(crate) struct T {
    pub _dir: tempfile::TempDir,
    pub env: Env,
    pub kc: Arc<FakeKeychain>,
    pub cc: Arc<ClaudeCode>,
    pub engine: Engine,
}

impl T {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path());
        let kc = Arc::new(FakeKeychain::new());
        let cc = Arc::new(ClaudeCode::with_store(
            LiveStore::new(kc.clone(), Platform::MacOs).with_retry_delay(Duration::ZERO),
        ));
        let engine = Engine::new(EngineConfig {
            env: env.clone(),
            registry: ProviderRegistry::new().with(cc.clone()),
            vault: Vault::new(Box::new(KeychainVault::new(kc.clone()))),
            oracle: Arc::new(NoOracle),
            clock: Arc::new(FakeClock::new(NOW)),
            http: Arc::new(NoHttp),
            default_provider: ProviderId::new(CLAUDE_CODE),
            settings: Settings::default(),
            process: Arc::new(tagteam_provider::liveness::FakeProcessProbe::new()),
            run_shell: RunShell::Outside,
        });
        T {
            _dir: dir,
            env,
            kc,
            cc,
            engine,
        }
    }

    /// Stores an OAuth account at the next position with `bytes` in its vault.
    pub fn account(&self, email: &str, bytes: &[u8]) -> AccountRow {
        let store = self.engine.store().unwrap();
        let provider = ProviderId::new(CLAUDE_CODE);
        let id = AccountId::from_string(format!("id-{email}"));
        let identity = Identity {
            label: email.into(),
            email: Some(email.into()),
            org_uuid: String::new(),
            org_name: None,
            account_uuid: Some(format!("uuid-{email}")),
            raw: json!({"emailAddress": email, "accountUuid": format!("uuid-{email}")}),
        };
        let key = self.cc.identity_key(&identity);
        store
            .insert_account(&NewAccount {
                id: &id,
                provider: &provider,
                position: store.next_position(&provider).unwrap(),
                identity_key: key.as_str(),
                identity: &identity,
                kind: "oauth",
                alias: None,
                login_expires_at: None,
                added_at: 1,
            })
            .unwrap();
        self.kc.put(SERVICE, id.as_str(), bytes);
        self.row(&id)
    }

    pub fn row(&self, id: &AccountId) -> AccountRow {
        self.engine.store().unwrap().account(id).unwrap().unwrap()
    }

    pub fn lock(&self, id: &AccountId) -> AccountLock {
        AccountLock::acquire(&self.env, id, AccountLock::WAIT).unwrap()
    }

    /// The refresh token in the vault's current (`prev` false) or previous generation.
    pub fn vault_rt(&self, id: &AccountId, prev: bool) -> Option<String> {
        let key = if prev {
            format!("{id}.prev")
        } else {
            id.to_string()
        };
        let v: Value = serde_json::from_slice(&self.kc.get(SERVICE, &key)?).ok()?;
        v["claudeAiOauth"]["refreshToken"]
            .as_str()
            .map(str::to_owned)
    }

    pub fn fp(&self, bytes: &[u8]) -> String {
        self.cc.fingerprint(bytes).unwrap().as_str().to_owned()
    }
}
