//! How many Keychain calls a switch makes, and under which locks (review refactor §8.11).

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::Fx;
use tagteam_cc::live::{LiveStore, Platform};
use tagteam_cc::{CLAUDE_CODE, CcPaths, ClaudeCode, ItemKind};
use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::registry::ProviderRegistry;
use tagteam_engine::settings::Settings;
use tagteam_engine::switch::SwitchReason;
use tagteam_engine::vault::{KeychainVault, SERVICE, Vault};
use tagteam_engine::{Engine, EngineConfig, EventSource};
use tagteam_provider::keychain::{Keychain, LockState, ReadCounts};
use tagteam_provider::profile::RunShell;
use tagteam_provider::{Env, KeychainError, Provider, Read};

/// One call that reached the Keychain: its operation, the item it named and whether Claude
/// Code's refresh lock was held when it was made.
#[derive(Debug, Clone)]
struct Call {
    op: &'static str,
    /// `vault:a`, `vault:b.prev`, ... for tagteam's own items, otherwise Claude Code's service.
    item: String,
    locked: bool,
}

/// A `Keychain` that forwards to the fixture's and logs every call that spawns `security`.
struct Counting {
    inner: Arc<dyn Keychain>,
    env: Env,
    names: Vec<(AccountId, &'static str)>,
    log: Mutex<Vec<Call>>,
}

impl Counting {
    fn record(&self, op: &'static str, service: &str, account: &str) {
        let item = if service == SERVICE {
            let id = account.trim_end_matches(".prev");
            let name = self
                .names
                .iter()
                .find(|(i, _)| i.as_str() == id)
                .map_or("?", |(_, n)| *n);
            let prev = if account.ends_with(".prev") {
                ".prev"
            } else {
                ""
            };
            format!("vault:{name}{prev}")
        } else {
            service.to_owned()
        };
        // The same test as `switch.rs`'s roster test: CC's lock is a directory.
        let locked = CcPaths::resolve(&self.env).refresh_lock.is_dir();
        self.log.lock().unwrap().push(Call { op, item, locked });
    }

    fn calls(&self) -> Vec<Call> {
        self.log.lock().unwrap().clone()
    }
}

impl Keychain for Counting {
    fn find(&self, s: &str, a: &str) -> Read<Vec<u8>> {
        self.record("find", s, a);
        self.inner.find(s, a)
    }
    fn exists(&self, s: &str, a: &str) -> Read<()> {
        self.record("exists", s, a);
        self.inner.exists(s, a)
    }
    fn upsert(&self, s: &str, a: &str, d: &[u8]) -> Result<(), KeychainError> {
        self.record("upsert", s, a);
        self.inner.upsert(s, a, d)
    }
    fn delete(&self, s: &str, a: &str) -> Result<(), KeychainError> {
        self.record("delete", s, a);
        self.inner.delete(s, a)
    }
    fn lock_state(&self) -> LockState {
        self.record("lock_state", "", "");
        self.inner.lock_state()
    }
    fn unlock(&self) -> bool {
        self.record("unlock", "", "");
        self.inner.unlock()
    }
    fn delete_service(&self, service: &str) -> Result<u32, KeychainError> {
        self.record("delete_service", service, "");
        self.inner.delete_service(service)
    }
    fn service_has_items(&self, service: &str) -> Read<bool> {
        self.record("service_has_items", service, "");
        self.inner.service_has_items(service)
    }
    fn read_counts(&self) -> Option<Arc<ReadCounts>> {
        self.inner.read_counts()
    }
}

/// The `find`s of `item` made under CC's lock.
fn locked_reads(calls: &[Call], item: &str) -> usize {
    calls
        .iter()
        .filter(|c| c.locked && c.op == "find" && c.item == item)
        .count()
}

/// The spawns `calls` make, in review refactor §8.11's unit: one per call and two per `upsert`
/// (add, then verify the write). It does not claim to match `SecurityCli` for every operation.
/// `locked == false` counts every call; `locked == true` counts only those made under CC's
/// refresh lock.
fn spawns(calls: &[Call], locked: bool) -> usize {
    calls
        .iter()
        .filter(|c| !locked || c.locked)
        .map(|c| if c.op == "upsert" { 2 } else { 1 })
        .sum()
}

/// Every call as `op item locked`, one per line, for a failure message. Never any bytes.
fn listing(calls: &[Call]) -> String {
    calls
        .iter()
        .map(|c| {
            format!(
                "{} {} {}",
                c.op,
                c.item,
                if c.locked { "locked" } else { "free" }
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// An engine over `fx`'s Env, clock, oracle and ports whose Claude Code and vault both go
/// through a `Counting` keychain. `names` says which letter each account's items log under.
fn counting_engine(fx: &Fx, names: &[(&AccountId, &'static str)]) -> (Engine, Arc<Counting>) {
    let counting = Arc::new(Counting {
        inner: fx.kc.clone(),
        env: fx.env.clone(),
        names: names.iter().map(|(i, n)| ((*i).clone(), *n)).collect(),
        log: Mutex::new(Vec::new()),
    });
    let cc = Arc::new(ClaudeCode::with_store(
        LiveStore::new(counting.clone(), Platform::MacOs).with_retry_delay(Duration::ZERO),
    ));
    let engine = Engine::new(EngineConfig {
        env: fx.env.clone(),
        registry: ProviderRegistry::new().with(cc),
        vault: Vault::new(Box::new(KeychainVault::new(counting.clone()))),
        oracle: fx.oracle.clone(),
        clock: fx.clock.clone(),
        http: fx.http.clone(),
        default_provider: ProviderId::new(CLAUDE_CODE),
        settings: Settings::default(),
        process: fx.process.clone(),
        spawner: fx.spawner.clone(),
        run_shell: RunShell::Outside,
        source: EventSource::Cli,
    });
    (engine, counting)
}

/// Two OAuth accounts, `a` and then `b`, which is live.
fn scenario() -> (Fx, AccountId, AccountId) {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    (fx, a, b)
}

#[test]
fn each_locked_vault_entry_is_read_once_while_nothing_writes_the_vault() {
    let (fx, a, b) = scenario();
    let (engine, counting) = counting_engine(&fx, &[(&a, "a"), (&b, "b")]);
    let out = engine.switch(fx.switch_request(&a, false)).unwrap();
    assert_eq!(out.reason, SwitchReason::Switched);
    let calls = counting.calls();
    for item in ["vault:a", "vault:b", "vault:a.prev", "vault:b.prev"] {
        assert_eq!(locked_reads(&calls, item), 1, "{item}: {calls:?}");
    }
}

#[test]
fn a_capture_under_the_locks_makes_the_next_vault_reads_fresh() {
    let (fx, a, b) = scenario();
    fx.switch_to(&a, false).unwrap();
    fx.rotate_live("rt-a2"); // CC rotated a; the vault still holds rt-a
    fx.oracle.set(Some(
        fx.cc.parse_identity(&Fx::oauth_account("a@x.co")).unwrap(),
    ));
    let (engine, counting) = counting_engine(&fx, &[(&a, "a"), (&b, "b")]);
    let out = engine.switch(fx.switch_request(&a, false)).unwrap();
    assert_eq!(out.reason, SwitchReason::Activated);
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a2"));
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"));
    let calls = counting.calls();
    // `rederive`, `Vault::store`'s read and its verify read, and the activation.
    assert_eq!(locked_reads(&calls, "vault:a"), 4, "{calls:?}");
    // `settle_outgoing` before the capture, and `hold_vault` after it.
    assert_eq!(locked_reads(&calls, "vault:a.prev"), 2, "{calls:?}");
}

// The budget of a manual switch, in spawns (review refactor §8.11). They are pins, not limits: a
// change that saves calls must lower them on purpose, and one that adds calls must raise them
// on purpose.
const SWITCH_N_SPAWNS: usize = 20;
const SWITCH_N_LOCKED_SPAWNS: usize = 15;
const BARE_SWITCH_SPAWNS: usize = 21;
const BARE_SWITCH_LOCKED_SPAWNS: usize = 15;

#[test]
fn a_manual_switch_spends_its_keychain_budget() {
    let (fx, a, b) = scenario();
    let (live_svc, live_acct) = fx.live_item(ItemKind::OAuth);
    let live = fx.kc.get(&live_svc, &live_acct).unwrap();
    let vault_b = fx.vault_bytes(&b).unwrap();
    assert_ne!(
        live, vault_b,
        "the live entry must differ from b's vault bytes"
    );
    assert_eq!(fx.cc.vault_form(&live).as_ref(), vault_b.as_slice());
    assert_eq!(fx.managed_key(), None);
    assert_eq!(common::rescue_files(&fx), 0);

    let (engine, counting) = counting_engine(&fx, &[(&a, "a"), (&b, "b")]);
    let out = engine.switch(fx.switch_request(&a, false)).unwrap();
    assert_eq!(out.reason, SwitchReason::Switched);
    assert!(fx.displaced().is_empty());
    let calls = counting.calls();
    let (total, held) = (spawns(&calls, false), spawns(&calls, true));
    assert_eq!(
        (total, held),
        (SWITCH_N_SPAWNS, SWITCH_N_LOCKED_SPAWNS),
        "switch <a>, spawns in total and under the locks:\n{}",
        listing(&calls)
    );

    let (fx, a, b) = scenario();
    let (engine, counting) = counting_engine(&fx, &[(&a, "a"), (&b, "b")]);
    let out = engine.switch(fx.rotation_request(false)).unwrap();
    assert_eq!(out.reason, SwitchReason::Switched);
    assert_eq!(out.to.map(|r| r.id), Some(a.clone()));
    let calls = counting.calls();
    let (total, held) = (spawns(&calls, false), spawns(&calls, true));
    assert_eq!(
        (total, held),
        (BARE_SWITCH_SPAWNS, BARE_SWITCH_LOCKED_SPAWNS),
        "bare switch, spawns in total and under the locks:\n{}",
        listing(&calls)
    );
}
