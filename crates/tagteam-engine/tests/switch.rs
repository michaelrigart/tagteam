mod common;

use std::fs;

use common::Fx;
use serde_json::json;
use tagteam_cc::{ItemKind, keychain_account, keychain_service};
use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::EngineError;
use tagteam_engine::lifecycle::AddTokenOptions;
use tagteam_engine::store::NewAccount;
use tagteam_engine::switch::{SwitchOutcome, SwitchReason, SwitchRequest, SwitchTarget};
use tagteam_provider::{Keychain, Provider};

fn switch(fx: &Fx, target: SwitchTarget, force: bool) -> Result<SwitchOutcome, EngineError> {
    fx.engine.switch(SwitchRequest {
        provider: fx.provider(),
        target,
        force,
        source: "cli",
    })
}

fn to(id: &AccountId) -> SwitchTarget {
    SwitchTarget::Account(id.clone())
}

const API_KEY: &str = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz";

fn add_api_key(fx: &Fx) -> AccountId {
    fx.engine
        .add_token(fx.add_token_options(API_KEY))
        .unwrap()
        .account
        .id
}

fn displaced_files(fx: &Fx) -> usize {
    fs::read_dir(fx.env.data_dir().join("displaced"))
        .map(|d| d.count())
        .unwrap_or(0)
}

#[test]
fn rotation_activates_the_next_account_and_keeps_machine_shared_keys() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live now: b
    let out = switch(&fx, SwitchTarget::Rotation, false).unwrap();
    assert_eq!(
        (out.switched, out.reason, out.strategy),
        (true, SwitchReason::Switched, "rotation")
    );
    assert_eq!(out.to.unwrap().id, a);
    assert_eq!(out.from.unwrap().id, b);
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
    assert_eq!(
        fx.live_credential().unwrap()["mcpOAuth"],
        json!({"srv": {"token": "machine-shared"}})
    );
    let store = fx.engine.store().unwrap();
    assert_eq!(store.active(&fx.provider()).unwrap(), Some(a));
    assert!(store.journal(&fx.provider()).unwrap().is_none());
    assert_eq!(store.events().unwrap().last().unwrap().kind, "switch");
    // CC's lock directories are gone again.
    assert!(!fx.paths().refresh_lock.exists() && !fx.paths().config_lock.exists());
}

#[test]
fn a_rotated_outgoing_credential_is_captured_before_switching() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.rotate_live("rt-b2"); // CC refreshed b in place
    switch(&fx, to(&a), false).unwrap();
    assert_eq!(fx.vault_refresh_token(&b).as_deref(), Some("rt-b2"));
    let prev = fx.kc.get("tagteam", &format!("{b}.prev")).unwrap();
    assert!(String::from_utf8(prev).unwrap().contains("rt-b"));
}

#[test]
fn a_wiped_outgoing_credential_never_overwrites_the_vault() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.set_live_credential(br#"{"claudeAiOauth":{"accessToken":"","refreshToken":""}}"#);
    switch(&fx, to(&a), false).unwrap();
    assert_eq!(fx.vault_refresh_token(&b).as_deref(), Some("rt-b"));
}

#[test]
fn a_foreign_outgoing_credential_is_displaced_not_captured() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.rotate_live("someone-elses-token");
    let stranger = fx
        .cc
        .parse_identity(&json!({"emailAddress": "z@x.co", "accountUuid": "uuid-z"}))
        .unwrap();
    fx.oracle.set(Some(stranger));
    let out = switch(&fx, to(&a), false).unwrap();
    assert_eq!(fx.vault_refresh_token(&b).as_deref(), Some("rt-b"));
    assert_eq!(displaced_files(&fx), 1);
    assert!(out.warnings.iter().any(|w| w.contains("displaced")));
}

#[test]
fn an_access_token_only_blob_never_replaces_a_refresh_token() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.set_live_credential(br#"{"claudeAiOauth":{"accessToken":"only-access"}}"#);
    switch(&fx, to(&a), false).unwrap();
    assert_eq!(fx.vault_refresh_token(&b).as_deref(), Some("rt-b"));
    assert_eq!(displaced_files(&fx), 1);
}

#[test]
fn an_unmanaged_live_login_is_a_noop_unless_forced() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.login("stranger@x.co", "rt-s");
    let out = switch(&fx, to(&a), false).unwrap();
    assert_eq!(
        (out.switched, out.reason),
        (false, SwitchReason::UnmanagedAccount)
    );
    assert_eq!(out.unmanaged_email.as_deref(), Some("stranger@x.co"));
    assert_eq!(fx.live_email().as_deref(), Some("stranger@x.co"));
    let out = switch(&fx, to(&a), true).unwrap();
    assert!(out.switched);
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
    assert_eq!(displaced_files(&fx), 1);
}

#[test]
fn a_fresh_machine_activates_the_first_switchable_account() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    // A new machine: no oauthAccount, no credential.
    fs::write(fx.paths().global_config, common::CLAUDE_JSON).unwrap();
    fx.kc
        .delete(
            &keychain_service(&fx.env, ItemKind::OAuth),
            &keychain_account(&fx.env),
        )
        .unwrap();
    fx.engine
        .store()
        .unwrap()
        .set_active(&fx.provider(), None)
        .unwrap();
    let out = switch(&fx, SwitchTarget::Rotation, false).unwrap();
    assert_eq!(out.to.unwrap().id, a);
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
    assert!(
        fx.live_credential().unwrap().get("mcpOAuth").is_none(),
        "no live JSON: no machine-shared keys"
    );
}

#[test]
fn a_replacement_finished_by_the_lock_is_switched_away_from_with_its_new_kind() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    // b was stored as an API key; an `add` over it with b's OAuth login wrote the vault and
    // then died before its metadata landed.
    let b = fx
        .engine
        .add_token(AddTokenOptions {
            email: Some("b@x.co".into()),
            ..fx.add_token_options("sk-ant-api03-b")
        })
        .unwrap()
        .account;
    fx.login("b@x.co", "rt-b");
    let cred = Fx::credential_json("b@x.co", "rt-b")
        .to_string()
        .into_bytes();
    fx.begin_replacement(&b.id, &cred, &Fx::oauth_account("b@x.co"), "oauth");
    fx.rotate_live("rt-b2"); // then CC refreshed b in place
    switch(&fx, to(&a), false).unwrap();
    assert_eq!(
        fx.vault_refresh_token(&b.id).as_deref(),
        Some("rt-b2"),
        "b's newest generation was captured, not lost"
    );
}

#[test]
fn a_switch_with_no_store_creates_nothing() {
    let fx = Fx::new();
    let out = switch(&fx, SwitchTarget::Rotation, false).unwrap();
    assert_eq!(out.reason, SwitchReason::NoValidTarget);
    fx.login("stranger@x.co", "rt-s");
    let out = switch(&fx, SwitchTarget::Rotation, false).unwrap();
    assert_eq!(
        (out.reason, out.unmanaged_email.as_deref()),
        (SwitchReason::UnmanagedAccount, Some("stranger@x.co"))
    );
    assert!(!fx.env.data_dir().exists());
}

#[test]
fn a_resolved_self_switch_activates_the_generation_it_captured() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    switch(&fx, to(&a), false).unwrap();
    fx.rotate_live("rt-a2"); // CC rotated a; the vault still holds rt-a
    fx.oracle.set(Some(
        fx.cc.parse_identity(&Fx::oauth_account("a@x.co")).unwrap(),
    ));
    let out = switch(&fx, to(&a), false).unwrap();
    assert_eq!(out.reason, SwitchReason::Activated);
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"));
    assert_eq!(
        fx.live_refresh_token().as_deref(),
        Some("rt-a2"),
        "never the superseded rt-a"
    );
}

#[test]
fn a_diverged_self_switch_whose_oracle_answer_has_no_uuid_stays_a_noop() {
    // §7.6: an answer without a non-empty uuid of its own attributes nothing, so it cannot
    // justify reconciling a self-switch; comparing it by email would.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.rotate_live("rt-a2");
    fx.oracle.set(Some(
        fx.cc
            .parse_identity(&json!({"emailAddress": "a@x.co", "organizationUuid": ""}))
            .unwrap(),
    ));
    let out = switch(&fx, to(&a), false).unwrap();
    assert_eq!(out.reason, SwitchReason::AlreadyActive);
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
}

#[test]
fn trivial_cases_are_noops() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    assert_eq!(
        switch(&fx, SwitchTarget::Rotation, false).unwrap().reason,
        SwitchReason::OnlyOneAccount
    );
    assert_eq!(
        switch(&fx, to(&a), false).unwrap().reason,
        SwitchReason::AlreadyActive
    );
    let forced = switch(&fx, to(&a), true).unwrap();
    assert_eq!(forced.reason, SwitchReason::Activated);
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
}

#[test]
fn rotation_skips_disabled_accounts_but_direct_targets_may_be_disabled() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.add("c@x.co", "rt-c"); // live: c
    fx.engine.set_disabled(&a, true).unwrap();
    assert_eq!(
        switch(&fx, SwitchTarget::Rotation, false)
            .unwrap()
            .to
            .unwrap()
            .id,
        b
    );
    assert_eq!(switch(&fx, to(&a), false).unwrap().to.unwrap().id, a);
}

#[test]
fn unsafe_live_reads_abort_without_changing_anything() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let (svc, acct) = (
        keychain_service(&fx.env, ItemKind::OAuth),
        keychain_account(&fx.env),
    );
    let before = fx.kc.get(&svc, &acct).unwrap();

    fx.kc.set_unreadable(&svc, &acct, true);
    assert!(matches!(
        switch(&fx, to(&a), false),
        Err(EngineError::Unreadable(_))
    ));
    fs::write(fx.paths().credentials_file, "{}").unwrap();
    assert!(
        matches!(switch(&fx, to(&a), true), Err(EngineError::DegradedRead)),
        "--force never overrides an unreadable entry"
    );
    fx.kc.set_unreadable(&svc, &acct, false);
    fs::remove_file(fx.paths().credentials_file).unwrap();

    fx.set_live_credential(b"");
    assert!(
        matches!(switch(&fx, to(&a), false), Err(EngineError::InvalidInput(m)) if m.contains("empty"))
    );
    fx.set_live_credential(&before);
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
}

#[test]
fn an_empty_managed_key_is_never_captured_over_an_api_key() {
    // §9.4 step 3 on the managed-key axis: a Keychain timeout can look empty, and capturing
    // it would leave the account with nothing to activate.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let k = add_api_key(&fx);
    switch(&fx, to(&k), false).unwrap();
    let managed = keychain_service(&fx.env, ItemKind::ManagedKey);
    fx.kc.put(&managed, &keychain_account(&fx.env), b"");
    assert!(
        matches!(switch(&fx, to(&a), false), Err(EngineError::InvalidInput(m)) if m.contains("empty"))
    );
    assert_eq!(fx.vault_bytes(&k).as_deref(), Some(API_KEY.as_bytes()));
}

#[test]
fn api_key_accounts_move_the_auth_axis_both_ways() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let k = add_api_key(&fx);
    let acct = keychain_account(&fx.env);
    let managed = keychain_service(&fx.env, ItemKind::ManagedKey);
    switch(&fx, to(&k), false).unwrap();
    assert_eq!(fx.kc.get(&managed, &acct).unwrap(), API_KEY.as_bytes());
    assert_eq!(
        fx.live_credential().unwrap(),
        json!({"mcpOAuth": {"srv": {"token": "machine-shared"}}})
    );
    assert_eq!(fx.live_email().as_deref(), Some("api-key-2@token.local"));
    switch(&fx, to(&a), false).unwrap();
    assert!(fx.kc.get(&managed, &acct).is_none());
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
    assert_eq!(
        fx.live_credential().unwrap()["mcpOAuth"],
        json!({"srv": {"token": "machine-shared"}})
    );
}

#[test]
fn the_file_store_reports_itself_for_the_hint() {
    let fx = Fx::with_platform(tagteam_cc::live::Platform::Linux);
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    assert!(switch(&fx, to(&a), false).unwrap().file_store);
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
}

#[test]
fn a_switch_never_crosses_providers() {
    // §9.3: an account of another provider is not a target for this one.
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let other = AccountId::from_string("other-provider-account");
    let identity = fx.cc.parse_identity(&Fx::oauth_account("o@x.co")).unwrap();
    fx.engine
        .store()
        .unwrap()
        .insert_account(&NewAccount {
            id: &other,
            provider: &ProviderId::new("elsewhere"),
            position: 1,
            identity_key: "o@x.co\n",
            identity: &identity,
            kind: "oauth",
            alias: None,
            login_expires_at: None,
            added_at: 1,
        })
        .unwrap();
    assert!(matches!(
        switch(&fx, to(&other), true),
        Err(EngineError::NoSuchAccount(_))
    ));
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
}

#[test]
#[ignore = "waits the full 9 s CC lock timeout; run with --ignored"]
fn cc_holding_its_refresh_lock_blocks_the_switch_and_changes_nothing() {
    // Review Focus 1.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    fs::create_dir(fx.paths().refresh_lock).unwrap();
    let err = switch(&fx, to(&a), false).unwrap_err();
    assert_eq!(err.kind(), "lock-timeout");
    assert!(err.to_string().contains(".oauth_refresh.lock"));
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    assert!(fx.paths().refresh_lock.is_dir(), "CC's lock is left alone");
}
