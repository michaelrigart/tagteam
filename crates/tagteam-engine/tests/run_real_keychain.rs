//! §15.2 "Provenance" with the real `security` driver (macOS): a session's exit, an inactive
//! vault refresh, a relaunch, and the capture check at its exit end with the vault's generation
//! effective in the profile, and never capture the consumed one. Every item lives in a
//! throwaway keychain file, never the login keychain.
#![cfg(all(target_os = "macos", feature = "real_keychain"))]

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use common::{CLAUDE_JSON, FixedOracle, claude_bin, splice_oauth_account};
use serde_json::{Value, json};
use tagteam_cc::endpoints::Endpoints;
use tagteam_cc::live::{LiveStore, Platform};
use tagteam_cc::{CcPaths, ClaudeCode, ItemKind, keychain_account, keychain_service};
use tagteam_core::{AccountId, CLAUDE_CODE, ProviderId};
use tagteam_engine::launch::LaunchEnd;
use tagteam_engine::lifecycle::AddOptions;
use tagteam_engine::refresh::GateOutcome;
use tagteam_engine::registry::ProviderRegistry;
use tagteam_engine::settings::Settings;
use tagteam_engine::store::AccountRow;
use tagteam_engine::vault::{KeychainVault, SERVICE, Vault};
use tagteam_engine::{Engine, EngineConfig};
use tagteam_provider::http::Method;
use tagteam_provider::liveness::FakeProcessProbe;
use tagteam_provider::process::{Captured, ScriptedSpawner};
use tagteam_provider::profile::RunShell;
use tagteam_provider::security::{ProcessRunner, SecurityCli};
use tagteam_provider::{Env, FakeClock, Keychain, Provider, Read, ScriptedHttp};

/// A keychain file made for this test and deleted with it.
struct TempKeychain {
    path: PathBuf,
    _dir: tempfile::TempDir,
}

impl TempKeychain {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.keychain");
        for args in [
            ["create-keychain", "-p", "pw", path.to_str().unwrap()],
            ["unlock-keychain", "-p", "pw", path.to_str().unwrap()],
        ] {
            assert!(
                Command::new("/usr/bin/security")
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        Self { path, _dir: dir }
    }
}

impl Drop for TempKeychain {
    fn drop(&mut self) {
        let _ = Command::new("/usr/bin/security")
            .args(["delete-keychain", self.path.to_str().unwrap()])
            .status();
    }
}

/// A credential of `rt`'s lineage, with a machine-shared MCP token when `mcp` is given.
fn credential(rt: &str, mcp: Option<&str>) -> Vec<u8> {
    let mut v = json!({
        "claudeAiOauth": {
            "accessToken": format!("at-{rt}"),
            "refreshToken": rt,
            "expiresAt": 1_790_003_600_000i64,
            "refreshTokenExpiresAt": 1_797_000_000_000i64,
            "scopes": ["user:inference", "user:profile"],
            "subscriptionType": "max"
        }
    });
    if let Some(token) = mcp {
        v["mcpOAuth"] = json!({"srv": {"token": token}});
    }
    v.to_string().into_bytes()
}

fn refresh_token(bytes: &[u8]) -> String {
    let v: Value = serde_json::from_slice(bytes).unwrap();
    v["claudeAiOauth"]["refreshToken"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// One engine whose Claude Code provider and vault both drive the real `security` against a
/// throwaway keychain; the network and the `claude` spawn are scripted.
struct Real {
    env: Env,
    kc: Arc<dyn Keychain>,
    cc: Arc<ClaudeCode>,
    http: Arc<ScriptedHttp>,
    spawner: Arc<ScriptedSpawner>,
    engine: Engine,
    home: tempfile::TempDir,
    _keychain: TempKeychain,
}

impl Real {
    fn new() -> Self {
        let keychain = TempKeychain::new();
        let home = tempfile::tempdir().unwrap();
        let env = Env::for_test(home.path());
        let claude = env.home.join(".claude");
        fs::create_dir_all(claude.join("projects")).unwrap();
        fs::write(claude.join("history.jsonl"), "").unwrap();
        fs::write(CcPaths::resolve(&env).global_config, CLAUDE_JSON).unwrap();
        let kc: Arc<dyn Keychain> = Arc::new(SecurityCli::with_runner(
            Box::new(ProcessRunner),
            Some(keychain.path.clone()),
        ));
        let cc = Arc::new(ClaudeCode::with_store(
            LiveStore::new(kc.clone(), Platform::MacOs).with_retry_delay(Duration::ZERO),
        ));
        let http = Arc::new(ScriptedHttp::new());
        let spawner = Arc::new(ScriptedSpawner::new());
        let engine = Engine::new(EngineConfig {
            env: env.clone(),
            registry: ProviderRegistry::new().with(cc.clone()),
            vault: Vault::new(Box::new(KeychainVault::new(kc.clone()))),
            oracle: Arc::new(FixedOracle::default()),
            clock: Arc::new(FakeClock::new(1_790_000_000_000)),
            http: http.clone(),
            default_provider: ProviderId::new(CLAUDE_CODE),
            settings: Settings::default(),
            process: Arc::new(FakeProcessProbe::new()),
            run_shell: RunShell::Outside,
            spawner: spawner.clone(),
        });
        Real {
            env,
            kc,
            cc,
            http,
            spawner,
            engine,
            home,
            _keychain: keychain,
        }
    }

    /// What `claude /login` as `email` leaves in the default home, then `tagteam add`.
    fn add(&self, email: &str, rt: &str) -> AccountId {
        splice_oauth_account(
            &CcPaths::resolve(&self.env).global_config,
            &json!({"emailAddress": email, "organizationUuid": "", "organizationName": null,
                    "accountUuid": format!("uuid-{email}")}),
        );
        self.kc
            .upsert(
                &keychain_service(&self.env, ItemKind::OAuth),
                &keychain_account(&self.env),
                &credential(rt, Some("mcp-default")),
            )
            .unwrap();
        self.engine
            .add_live(AddOptions {
                provider: ProviderId::new(CLAUDE_CODE),
                position: None,
                alias: None,
                yes: false,
            })
            .unwrap()
            .account
            .id
    }

    fn row(&self, id: &AccountId) -> AccountRow {
        self.engine.store().unwrap().account(id).unwrap().unwrap()
    }

    fn vault_bytes(&self, id: &AccountId) -> Vec<u8> {
        self.kc.find(SERVICE, id.as_str()).present().unwrap()
    }

    /// The hashed item Claude Code names from `spelling` (Appendix A.2).
    fn profile_item(&self, spelling: &str) -> (String, String) {
        let mut env = self.env.clone();
        env.claude_config_dir = Some(spelling.into());
        env.claude_securestorage_config_dir = None;
        (
            keychain_service(&env, ItemKind::OAuth),
            keychain_account(&env),
        )
    }

    /// §12.3's `valid` reply for `email` in `spelling`.
    fn valid(&self, spelling: &Path, email: &str) -> Captured {
        Captured::Exited {
            code: Some(0),
            signal: None,
            stdout: json!({"loggedIn": true, "authMethod": "claude.ai",
                           "configDirectory": spelling.to_str().unwrap(), "email": email})
            .to_string()
            .into_bytes(),
            stderr: vec![],
        }
    }

    /// A 200 token reply rotating to `rt`.
    fn script_refresh(&self, rt: &str) {
        self.http.push_json(
            Method::Post,
            &Endpoints::production().token,
            200,
            json!({"token_type": "Bearer", "access_token": format!("at-{rt}"),
                   "refresh_token": rt, "expires_in": 28800,
                   "scope": "user:inference user:profile"}),
        );
    }
}

#[test]
#[ignore = "real Keychain: run with --features real_keychain -- --ignored, outside the sandbox"]
fn a_relaunch_after_an_inactive_refresh_runs_on_the_vault_s_generation_and_never_captures_the_consumed_one()
 {
    let r = Real::new();
    let a = r.add("a@x.co", "rt-a");
    r.add("b@x.co", "rt-b");
    let cwd = r.home.path().join("work");
    fs::create_dir_all(&cwd).unwrap();
    let spelling = fs::canonicalize(r.env.data_dir())
        .unwrap()
        .join("sessions")
        .join(a.as_str());

    // 1. The first session's bootstrap writes the file and leaves no item (§12.3 steps 4–5).
    r.spawner.push(r.valid(&spelling, "a@x.co"));
    let launched = r.engine.launch(&r.row(&a), claude_bin(), &cwd).unwrap();
    let profile = launched.profile.clone();
    let (svc, acct) = r.profile_item(&launched.spelling);
    assert!(matches!(r.kc.exists(&svc, &acct), Read::Absent));
    // 2. Claude Code's first credential write in the session rotates it and moves the file
    //    into its hashed item (Appendix A.3), with an MCP token of the profile's own.
    r.kc.upsert(&svc, &acct, &credential("rt-a1", Some("mcp-profile")))
        .unwrap();
    fs::remove_file(profile.join(".credentials.json")).unwrap();
    // 3. Its exit captures the rotation (§12.5).
    r.engine.finish_run(launched, LaunchEnd::Exited(0));
    assert_eq!(refresh_token(&r.vault_bytes(&a)), "rt-a1");

    // 4. A refresh of the inactive account consumes rt-a1 (§7.3).
    r.script_refresh("rt-a2");
    let snapshot = r.vault_bytes(&a);
    assert!(matches!(
        r.engine
            .refresh_stored(r.cc.as_ref(), &a, &snapshot)
            .unwrap(),
        GateOutcome::Refreshed(_)
    ));
    assert_eq!(refresh_token(&r.vault_bytes(&a)), "rt-a2");

    // The item holding the consumed generation is still there, so step 5's deletion is its own.
    assert!(matches!(r.kc.exists(&svc, &acct), Read::Present(())));

    // 5. The relaunch finds the vault moved on (§12.5's table: P = S). It bootstraps again and
    //    deletes the item holding the consumed generation, verified gone by the real probe.
    r.spawner.push(r.valid(&spelling, "a@x.co"));
    let launched = r.engine.launch(&r.row(&a), claude_bin(), &cwd).unwrap();
    assert!(matches!(r.kc.exists(&svc, &acct), Read::Absent));
    let effective =
        r.cc.read_profile_credential(&r.env, &launched.profile, &launched.spelling)
            .present()
            .expect("the profile's credential");
    let v: Value = serde_json::from_slice(effective.bytes()).unwrap();
    assert_eq!(
        v["claudeAiOauth"]["refreshToken"],
        json!("rt-a2"),
        "the vault's generation is what Claude Code reads"
    );
    assert_eq!(
        v["mcpOAuth"]["srv"]["token"],
        json!("mcp-profile"),
        "with the profile's own MCP token (§12.3 step 4)"
    );

    // 6. Its exit finds nothing to capture: the consumed rt-a1 never comes back.
    r.engine.finish_run(launched, LaunchEnd::Exited(0));
    assert_eq!(refresh_token(&r.vault_bytes(&a)), "rt-a2");
}
