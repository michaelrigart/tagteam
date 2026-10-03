//! §7.6: the profile oracle over HTTP, how often it is asked, and which commands may ask.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use common::{Fx, crash_row, mutation_lock_free};
use serde_json::json;
use tagteam_cc::ClaudeCode;
use tagteam_cc::endpoints::Endpoints;
use tagteam_cc::live::{LiveStore, Platform};
use tagteam_core::{AccountId, CLAUDE_CODE, ProviderId};
use tagteam_engine::account_lock::AccountLock;
use tagteam_engine::net::UreqHttp;
use tagteam_engine::oracle::{CachingOracle, HttpOracle, Oracle};
use tagteam_engine::registry::ProviderRegistry;
use tagteam_engine::settings::Settings;
use tagteam_engine::vault::{KeychainVault, SERVICE, Vault};
use tagteam_engine::{Engine, EngineConfig, EngineError};
use tagteam_provider::http::{Http, Method};
use tagteam_provider::mock_server::{MockReply, MockServer};
use tagteam_provider::profile::RunShell;
use tagteam_provider::{Credential, Identity, Provider};

/// An engine over the fixture whose oracle is the real `HttpOracle`, answered by `fx.http`.
fn http_engine(fx: &Fx) -> Engine {
    fx.engine_with_oracle(Arc::new(HttpOracle::new(fx.http.clone(), fx.clock.clone())))
}

fn profile_asks(fx: &Fx) -> usize {
    fx.http.count(Method::Get, &Fx::endpoints().profile)
}

fn live_bytes(fx: &Fx) -> Vec<u8> {
    fx.live_credential().unwrap().to_string().into_bytes()
}

/// Forgets a stored uuid, as an account added before its uuid was known would have none.
fn forget_uuid(fx: &Fx, id: &AccountId) {
    fx.engine.store().unwrap();
    rusqlite::Connection::open(fx.env.data_dir().join("tagteam.db"))
        .unwrap()
        .execute(
            "UPDATE accounts SET account_uuid = NULL WHERE id = ?1",
            [id.as_str()],
        )
        .unwrap();
}

#[test]
fn http_oracle_resolves_through_the_provider() {
    let fx = Fx::new();
    fx.login("b@x.co", "rt-b");
    fx.script_profile("b@x.co");
    let oracle = HttpOracle::new(fx.http.clone(), fx.clock.clone());
    let owner = oracle
        .resolve(fx.cc.as_ref(), &Credential::fresh(live_bytes(&fx)))
        .unwrap();
    assert_eq!(owner.account_uuid.as_deref(), Some("uuid-b@x.co"));
    assert_eq!(profile_asks(&fx), 1);
}

/// Counts the calls that reach it; answers nothing, as a failed request would.
struct Counting(Arc<AtomicUsize>);

impl Oracle for Counting {
    fn resolve(&self, _p: &dyn Provider, _c: &Credential) -> Option<Identity> {
        self.0.fetch_add(1, Ordering::SeqCst);
        None
    }
}

#[test]
fn a_process_asks_at_most_once_per_credential() {
    // §7.6: keyed by the credential's exact bytes; no answer is remembered too.
    let fx = Fx::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let cache = CachingOracle::new(Counting(calls.clone()));
    let one = Credential::fresh(
        Fx::credential_json("a@x.co", "rt-1")
            .to_string()
            .into_bytes(),
    );
    let two = Credential::fresh(
        Fx::credential_json("a@x.co", "rt-2")
            .to_string()
            .into_bytes(),
    );
    assert!(cache.resolve(fx.cc.as_ref(), &one).is_none());
    assert!(cache.resolve(fx.cc.as_ref(), &one).is_none());
    assert!(cache.resolve(fx.cc.as_ref(), &two).is_none());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn a_new_access_token_under_the_same_refresh_token_is_asked_again() {
    // A skip for one access token must not answer for another: the key is the exact bytes.
    let fx = Fx::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let cache = CachingOracle::new(Counting(calls.clone()));
    let with_access = |at: &str| {
        let mut json = Fx::credential_json("a@x.co", "rt-1");
        json["claudeAiOauth"]["accessToken"] = at.into();
        Credential::fresh(json.to_string().into_bytes())
    };
    let old = with_access("at-old");
    let new = with_access("at-new");
    assert!(cache.resolve(fx.cc.as_ref(), &old).is_none());
    assert!(cache.resolve(fx.cc.as_ref(), &new).is_none());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(
        cache
            .resolve(fx.cc.as_ref(), &with_access("at-new"))
            .is_none()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2, "identical bytes ask once");
}

#[test]
fn an_attributed_rotation_is_captured_and_backfills_the_uuid() {
    // §9.4 step 4 OursRotated, decided by the oracle over HTTP: the vault takes the rotated
    // generation, `.prev` keeps the old one, and the missing uuid is backfilled.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live: b
    forget_uuid(&fx, &b);
    fx.rotate_live("rt-b2");
    fx.script_profile("b@x.co");
    http_engine(&fx)
        .switch(fx.switch_request(&a, false))
        .unwrap();
    assert_eq!(profile_asks(&fx), 1);
    assert_eq!(fx.vault_refresh_token(&b).as_deref(), Some("rt-b2"));
    let prev = fx.kc.get(SERVICE, &format!("{b}.prev")).unwrap();
    assert!(String::from_utf8(prev).unwrap().contains("rt-b"));
    let row = fx.engine.store().unwrap().account(&b).unwrap().unwrap();
    assert_eq!(row.account_uuid.as_deref(), Some("uuid-b@x.co"));
    assert!(fx.displaced().is_empty());
}

#[test]
fn a_rotation_attributed_to_someone_else_is_displaced_and_the_vault_kept() {
    // L301: the outcome, not only the action: the vault still holds rt-b, and the displaced
    // file holds exactly the foreign bytes.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.rotate_live("someone-elses-token");
    let foreign = live_bytes(&fx);
    fx.script_profile("z@x.co");
    let out = http_engine(&fx)
        .switch(fx.switch_request(&a, false))
        .unwrap();
    assert_eq!(fx.vault_refresh_token(&b).as_deref(), Some("rt-b"));
    let displaced = fx.displaced();
    assert_eq!(displaced.len(), 1);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&displaced[0]).unwrap(),
        serde_json::from_slice::<serde_json::Value>(&foreign).unwrap()
    );
    assert!(out.warnings.iter().any(|w| w.contains("displaced")));
}

#[test]
fn an_attributed_blob_without_a_refresh_token_is_displaced_not_captured() {
    // L301 for §6.2: even with a positive answer, a live credential without a refresh token
    // never replaces the vault's complete one. §7.6 does not exempt such a blob from the profile
    // oracle (only an expired token, a setup token and an API key), so the oracle is asked
    // and its positive answer is what the `lacks_refresh_over_complete` rule overrides.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let mut v = fx.live_credential().unwrap();
    v["claudeAiOauth"]["refreshToken"] = json!(null);
    v["claudeAiOauth"]["accessToken"] = json!("only-access");
    fx.set_live_credential(v.to_string().as_bytes());
    fx.script_profile("b@x.co");
    http_engine(&fx)
        .switch(fx.switch_request(&a, false))
        .unwrap();
    assert_eq!(profile_asks(&fx), 1, "the blob is attributed, not skipped");
    assert_eq!(fx.vault_refresh_token(&b).as_deref(), Some("rt-b"));
    assert_eq!(fx.displaced().len(), 1);
}

#[test]
fn an_outgoing_credential_the_vault_holds_is_never_sent_to_the_oracle() {
    // L300 at the engine: `Ours` is decided by bytes first, so no request is made at all.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    http_engine(&fx)
        .switch(fx.switch_request(&a, false))
        .unwrap();
    assert_eq!(profile_asks(&fx), 0);
}

#[test]
fn metadata_commands_recover_without_asking_the_oracle() {
    // §7.6, §9.6: alias, disable, enable and move retry recovery from fingerprints alone.
    // An account-changing command asks.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let row = crash_row(&fx, &a, &b);
    fx.engine.store().unwrap().insert_journal(&row).unwrap();
    fx.rotate_live("rt-unknown"); // neither generation the row names: only the oracle could say
    let engine = http_engine(&fx);
    engine.set_alias(&a, Some("work")).unwrap();
    engine.set_disabled(&a, true).unwrap();
    engine.set_disabled(&a, false).unwrap();
    engine.move_to(&a, 2).unwrap();
    assert_eq!(
        profile_asks(&fx),
        0,
        "no metadata command may reach the network"
    );
    assert!(matches!(
        engine.remove(&a),
        Err(EngineError::InterruptedSwitch(_))
    ));
    assert!(
        profile_asks(&fx) >= 1,
        "remove is account-changing: it asks"
    );
}

#[test]
fn a_signal_before_a_pre_lock_oracle_request_sends_none() {
    // §14.1: the profile request before the locks (a switch's, a recovery's) is a cancellation
    // point: a Ctrl-C that has landed stops the command before it makes the request.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.rotate_live("rt-b2"); // the outgoing live credential is no vault generation: it is asked
    fx.script_profile("b@x.co");
    fx.engine.cancel().request(libc::SIGINT);

    let err = http_engine(&fx)
        .switch(fx.switch_request(&a, false))
        .unwrap_err();

    assert_eq!(err.signal(), Some(libc::SIGINT), "{err}");
    assert_eq!(profile_asks(&fx), 0, "a switch's oracle request");

    // A command that recovers an interrupted switch first asks the oracle about the live
    // credential the row does not name, before the mutation lock.
    let row = crash_row(&fx, &a, &b);
    fx.engine.store().unwrap().insert_journal(&row).unwrap();
    fx.rotate_live("rt-unknown");

    let err = http_engine(&fx).remove(&a).unwrap_err();

    assert_eq!(err.signal(), Some(libc::SIGINT), "{err}");
    assert_eq!(profile_asks(&fx), 0, "a recovery's oracle request");
}

#[test]
fn a_hanging_profile_endpoint_delays_a_switch_by_its_timeout_and_holds_no_lock() {
    // Review Focus 5: a captive portal that never answers. The switch waits out the 5 s
    // profile timeout with no lock held, then captures the rotation as Unresolved, `.prev`
    // keeping the old generation.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.rotate_live("rt-b2");
    let server = MockServer::start();
    server.on("GET", "/api/oauth/profile", MockReply::Hang);
    let cc = Arc::new(
        ClaudeCode::with_store(
            LiveStore::new(fx.kc.clone(), Platform::MacOs).with_retry_delay(Duration::ZERO),
        )
        .with_endpoints(Endpoints::with_base(&server.base_url())),
    );
    let http: Arc<dyn Http> = Arc::new(UreqHttp::direct());
    let engine = Engine::new(EngineConfig {
        env: fx.env.clone(),
        registry: ProviderRegistry::new().with(cc),
        vault: Vault::new(Box::new(KeychainVault::new(fx.kc.clone()))),
        oracle: Arc::new(HttpOracle::new(http.clone(), fx.clock.clone())),
        clock: fx.clock.clone(),
        http,
        default_provider: ProviderId::new(CLAUDE_CODE),
        settings: Settings::default(),
        process: fx.process.clone(),
        spawner: fx.spawner.clone(),
        run_shell: RunShell::Outside,
    });
    let env = fx.env.clone();
    let watched = b.clone();
    let watcher = thread::spawn(move || {
        thread::sleep(Duration::from_secs(2)); // well inside the hang
        (
            mutation_lock_free(&env),
            AccountLock::try_acquire(&env, &watched).unwrap().is_some(),
        )
    });
    let started = Instant::now();
    engine.switch(fx.switch_request(&a, false)).unwrap();
    let took = started.elapsed();
    assert_eq!(
        watcher.join().unwrap(),
        (true, true),
        "no lock is held while the oracle waits (§7.6)"
    );
    assert!(took >= Duration::from_millis(4_500), "{took:?}");
    assert!(took < Duration::from_secs(15), "{took:?}");
    assert_eq!(server.hits("GET", "/api/oauth/profile"), 1);
    assert_eq!(fx.vault_refresh_token(&b).as_deref(), Some("rt-b2"));
    let prev = fx.kc.get(SERVICE, &format!("{b}.prev")).unwrap();
    assert!(String::from_utf8(prev).unwrap().contains("rt-b"));
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
}
