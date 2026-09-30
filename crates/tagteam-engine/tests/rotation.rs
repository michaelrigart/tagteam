mod common;

use std::fs;
use std::sync::{Arc, Mutex};

use common::{Fx, vault_fp};
use tagteam_cc::{ItemKind, keychain_account, keychain_service};
use tagteam_core::AccountId;
use tagteam_engine::EngineError;
use tagteam_engine::switch::{SwitchOutcome, SwitchReason};
use tagteam_engine::vault::SERVICE;
use tagteam_provider::Keychain;

fn rotate(fx: &Fx) -> Result<SwitchOutcome, EngineError> {
    fx.engine.switch(fx.rotation_request(false))
}

/// `claude /logout`: no `oauthAccount` and no credential. The store is not told.
fn log_out(fx: &Fx) {
    fs::write(fx.paths().global_config, common::CLAUDE_JSON).unwrap();
    fx.kc
        .delete(
            &keychain_service(&fx.env, ItemKind::OAuth),
            &keychain_account(&fx.env),
        )
        .unwrap();
}

#[test]
fn a_quarantined_account_is_skipped() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.add("c@x.co", "rt-c"); // live: the walk would wrap to a
    fx.quarantine(&a, "invalid_grant", &vault_fp(&fx, &a));
    assert_eq!(rotate(&fx).unwrap().to.unwrap().id, b);
}

#[test]
fn a_quarantined_account_does_not_count_toward_two() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b"); // live
    fx.quarantine(&a, "invalid_grant", &vault_fp(&fx, &a));
    let out = rotate(&fx).unwrap();
    assert_eq!(
        (out.switched, out.reason),
        (false, SwitchReason::OnlyOneAccount)
    );
}

#[test]
fn an_account_whose_vault_holds_nothing_is_skipped() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.add("c@x.co", "rt-c"); // live
    fx.kc.delete(SERVICE, a.as_str()).unwrap();
    assert_eq!(rotate(&fx).unwrap().to.unwrap().id, b);
}

#[test]
fn a_walk_that_finds_no_credential_is_only_one_account() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b"); // live
    fx.kc.delete(SERVICE, a.as_str()).unwrap();
    let out = rotate(&fx).unwrap();
    assert_eq!(
        (out.switched, out.reason),
        (false, SwitchReason::OnlyOneAccount)
    );
}

#[test]
fn an_unreadable_vault_before_the_pick_names_the_account() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    fx.add("c@x.co", "rt-c"); // live: the walk tries a first
    fx.kc.set_unreadable(SERVICE, a.as_str(), true);
    let err = rotate(&fx).unwrap_err();
    assert!(
        matches!(&err, EngineError::UnreadableAccount { position: 1, label, .. } if label == "a@x.co"),
        "{err}"
    );
    assert_eq!(err.kind(), "unreadable");
    let msg = err.to_string();
    assert!(
        msg.contains("a@x.co") && msg.contains("position 1"),
        "{msg}"
    );
    assert_eq!(
        fx.live_email().as_deref(),
        Some("c@x.co"),
        "nothing switched"
    );
}

#[test]
fn an_unreadable_vault_after_the_pick_is_never_read() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.add("c@x.co", "rt-c"); // live: the walk picks a before it reaches b
    fx.kc.set_unreadable(SERVICE, b.as_str(), true);
    assert_eq!(rotate(&fx).unwrap().to.unwrap().id, a);
}

#[test]
fn a_bare_switch_reads_only_the_vaults_it_switches_between() {
    // M-8: M1's rotation read every account's vault, twice. The walk reads only as far as its
    // pick, and the transaction only the outgoing account and the target.
    let fx = Fx::new();
    let ids: Vec<AccountId> = (0..10)
        .map(|n| fx.add(&format!("a{n}@x.co"), &format!("rt-{n}")))
        .collect();
    // live: a9, at position 10; the walk wraps to a0, at position 1.
    let reads = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen = reads.clone();
    let engine = fx.engine_with_vault_probe(move |key| seen.lock().unwrap().push(key.to_owned()));
    let out = engine.switch(fx.rotation_request(false)).unwrap();
    assert_eq!(out.to.unwrap().id, ids[0]);
    let reads = reads.lock().unwrap();
    for id in &ids[1..9] {
        assert!(
            !reads.iter().any(|key| key.starts_with(id.as_str())),
            "{id} was read: {reads:?}"
        );
    }
}

#[test]
fn with_no_live_login_a_quarantined_active_account_is_skipped() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live and the store's active account
    fx.quarantine(&b, "invalid_grant", &vault_fp(&fx, &b));
    log_out(&fx);
    assert_eq!(rotate(&fx).unwrap().to.unwrap().id, a);
}

#[test]
fn an_account_the_walk_skipped_is_not_read_again_under_the_locks() {
    // §9.3: under the locks only the chosen account is read again. a's vault is empty, so the
    // walk reads it once while planning and passes over it; re-deriving the rotation under the
    // locks decides from the store alone and does not read it a second time.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.add("c@x.co", "rt-c"); // live
    fx.kc.delete(SERVICE, a.as_str()).unwrap();
    let reads = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen = reads.clone();
    let engine = fx.engine_with_vault_probe(move |key| seen.lock().unwrap().push(key.to_owned()));
    let out = engine.switch(fx.rotation_request(false)).unwrap();
    assert_eq!(out.to.unwrap().id, b);
    let reads = reads.lock().unwrap();
    let of_a = reads
        .iter()
        .filter(|key| key.starts_with(a.as_str()))
        .count();
    assert_eq!(of_a, 1, "{reads:?}");
}
