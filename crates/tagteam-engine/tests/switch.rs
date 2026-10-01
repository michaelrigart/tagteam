mod common;

use std::collections::BTreeSet;
use std::fs;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{
    API_KEY, Fx, OTHER_API_KEY, STRAY_API_KEY, capture_logs, mutation_lock_free, usage_fixture,
};
use serde_json::json;
use tagteam_cc::{ItemKind, keychain_account, keychain_service};
use tagteam_core::poll::PollPlan;
use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::EngineError;
use tagteam_engine::lifecycle::AddTokenOptions;
use tagteam_engine::store::NewAccount;
use tagteam_engine::switch::{SwitchOutcome, SwitchReason, SwitchRequest, SwitchTarget};
use tagteam_engine::vault::SERVICE;
use tagteam_provider::{Keychain, Provider, SecretStore};

fn request(fx: &Fx, target: SwitchTarget, force: bool) -> SwitchRequest {
    SwitchRequest {
        provider: fx.provider(),
        target,
        force,
        source: "cli",
    }
}

fn switch(fx: &Fx, target: SwitchTarget, force: bool) -> Result<SwitchOutcome, EngineError> {
    fx.engine.switch(request(fx, target, force))
}

fn to(id: &AccountId) -> SwitchTarget {
    SwitchTarget::Account(id.clone())
}

fn displaced_files(fx: &Fx) -> usize {
    fx.displaced().len()
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

/// A new machine: no `oauthAccount`, no credential, no active account.
fn make_fresh_machine(fx: &Fx) {
    log_out(fx);
    fx.engine
        .store()
        .unwrap()
        .set_active(&fx.provider(), None)
        .unwrap();
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
fn a_live_entry_with_no_token_is_never_captured_over_a_setup_token() {
    // §6.2: an entry holding only machine-shared keys has no generation. Neither side has a
    // refresh token, so the refresh-token rule cannot keep it out; it is left alone like a
    // wiped blob, and the vault keeps its generation.
    let fx = Fx::new();
    let b = fx.add("b@x.co", "rt-b");
    let setup = fx
        .engine
        .add_token(fx.add_token_options("sk-ant-oat01-setup"))
        .unwrap()
        .account
        .id;
    let stored = fx.vault_bytes(&setup);
    switch(&fx, to(&setup), false).unwrap(); // `oauthAccount` names the setup-token account
    fx.set_live_credential(br#"{"mcpOAuth":{"srv":{"token":"machine-shared"}}}"#);
    switch(&fx, to(&b), false).unwrap();
    assert_eq!(fx.vault_bytes(&setup), stored);
    assert_eq!(displaced_files(&fx), 0);
    switch(&fx, to(&setup), false).unwrap();
    assert_eq!(
        fx.live_credential().unwrap()["claudeAiOauth"]["accessToken"],
        "sk-ant-oat01-setup"
    );
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
    make_fresh_machine(&fx);
    let out = switch(&fx, SwitchTarget::Rotation, false).unwrap();
    assert_eq!(out.to.unwrap().id, a);
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
    assert!(
        fx.live_credential().unwrap().get("mcpOAuth").is_none(),
        "no live JSON: no machine-shared keys"
    );
}

#[test]
fn a_bare_switch_rotates_on_from_a_managed_live_login() {
    // §9.3: a managed live account anchors the rotation, even when the store's active account
    // disagrees because the user ran `claude /login` outside tagteam.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let c = fx.add("c@x.co", "rt-c");
    switch(&fx, to(&a), false).unwrap(); // the store's active account: a
    fx.login("b@x.co", "rt-b"); // live: b, while the store still says a
    let out = switch(&fx, SwitchTarget::Rotation, false).unwrap();
    assert_eq!((out.switched, out.reason), (true, SwitchReason::Switched));
    assert_eq!(out.from.map(|r| r.id), Some(b));
    assert_eq!(out.to.map(|r| r.id), Some(c.clone()));
    assert_eq!(fx.live_email().as_deref(), Some("c@x.co"));
    assert_eq!(
        fx.engine.store().unwrap().active(&fx.provider()).unwrap(),
        Some(c)
    );
}

#[test]
fn without_a_managed_live_login_the_rotation_falls_back_to_the_store_s_active_account() {
    // No live login, or an unmanaged one (which only --force may switch away from, §9.2): the
    // store's active account is the anchor, and it is not live, so it is activated.
    for unmanaged in [false, true] {
        let fx = Fx::new();
        fx.add("a@x.co", "rt-a");
        let b = fx.add("b@x.co", "rt-b");
        fx.add("c@x.co", "rt-c");
        switch(&fx, to(&b), false).unwrap(); // the store's active account: b
        if unmanaged {
            fx.login("stranger@x.co", "rt-s");
        } else {
            log_out(&fx);
        }
        let out = switch(&fx, SwitchTarget::Rotation, unmanaged).unwrap();
        assert_eq!(out.to.map(|r| r.id), Some(b), "unmanaged={unmanaged}");
        assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    }
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
fn an_unreadable_vault_is_reported_as_unreadable_never_as_missing() {
    // §4.3: an unreadable vault item is not an absent one. A direct target says so, and a
    // rotation that meets it before its pick neither skips the account nor calls the other one
    // the only switchable account: both name the account (§9.3).
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b"); // live: b
    fx.kc.set_unreadable(SERVICE, a.as_str(), true);
    for target in [SwitchTarget::Rotation, to(&a)] {
        let err = switch(&fx, target.clone(), false).unwrap_err();
        assert!(
            matches!(&err, EngineError::UnreadableAccount { position: 1, label, .. } if label == "a@x.co"),
            "{target:?}: {err}"
        );
        assert!(!err.to_string().contains("no stored credential"), "{err}");
    }
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    fx.kc.set_unreadable(SERVICE, a.as_str(), false);
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
    assert_an_empty_read(switch(&fx, to(&a), false));
    fx.set_live_credential(&before);
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
}

/// §9.4 step 3: a live value that read back empty is reported as unreadable (exit 1, never
/// the usage-error code), and nothing is attempted.
fn assert_an_empty_read(result: Result<SwitchOutcome, EngineError>) {
    let err = result.unwrap_err();
    assert_eq!(err.kind(), "unreadable", "{err}");
    let msg = err.to_string();
    assert!(
        msg.contains("read back empty") && msg.contains("not attempted"),
        "{msg}"
    );
}

/// §9.4 step 3 on the managed-key axis, with or without --force: an empty Keychain item (a
/// Keychain timeout can look empty) aborts, and the item it could not read is neither saved
/// nor cleared.
fn assert_an_empty_managed_key_aborts(fx: &Fx, result: Result<SwitchOutcome, EngineError>) {
    assert_an_empty_read(result);
    assert_eq!(
        fx.managed_key().as_deref(),
        Some(&b""[..]),
        "left untouched"
    );
    assert_eq!(displaced_files(fx), 0);
}

#[test]
fn an_empty_primary_api_key_does_not_block_a_switch() {
    // An empty `primaryApiKey` is what the config file really holds, not a timed-out read: it
    // names no key, and writing OAuth removes it.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let config = fx.paths().global_config;
    common::splice_config_key(&config, "primaryApiKey", &json!(""));
    let out = switch(&fx, to(&a), false).unwrap();
    assert_eq!(out.reason, SwitchReason::Switched);
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
    assert!(matches!(
        tagteam_cc::config::get_key(&config, "primaryApiKey"),
        tagteam_provider::Read::Present(None)
    ));
}

#[test]
fn an_empty_managed_key_is_never_captured_over_an_api_key() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let k = fx.add_api_key(API_KEY);
    switch(&fx, to(&k), false).unwrap();
    fx.put_managed_key(b"");
    assert_an_empty_managed_key_aborts(&fx, switch(&fx, to(&a), false));
    assert_eq!(fx.vault_bytes(&k).as_deref(), Some(API_KEY.as_bytes()));
}

#[test]
fn an_empty_managed_key_aborts_a_forced_switch() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let k = fx.add_api_key(API_KEY);
    switch(&fx, to(&k), false).unwrap();
    fx.put_managed_key(b"");
    assert_an_empty_managed_key_aborts(&fx, switch(&fx, to(&a), true));
    assert_eq!(fx.live_email().as_deref(), Some("api-key-2@token.local"));
}

#[test]
fn an_empty_managed_key_aborts_a_switch_on_a_fresh_machine() {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    make_fresh_machine(&fx);
    fx.put_managed_key(b"");
    assert_an_empty_managed_key_aborts(&fx, switch(&fx, SwitchTarget::Rotation, false));
    assert_eq!(fx.live_email(), None);
}

#[test]
fn a_stray_managed_key_is_displaced_before_an_oauth_switch_clears_it() {
    // §9.4 step 7: an account-scoped secret on the axis being cleared that no vault holds is
    // displaced before it is cleared.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    fx.put_managed_key(STRAY_API_KEY.as_bytes());
    let out = switch(&fx, to(&a), false).unwrap();
    assert_eq!(fx.displaced(), [STRAY_API_KEY.as_bytes()]);
    assert!(out.warnings.iter().any(|w| w.contains("displaced")));
    assert_eq!(
        fx.managed_key(),
        None,
        "then cleared, as writing OAuth does"
    );
}

#[test]
fn a_stray_oauth_login_is_displaced_before_an_api_key_switch_strips_it() {
    // The mirror: account-scoped keys left in the credential entry under an API-key login.
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let k = fx.add_api_key(API_KEY);
    let k2 = fx.add_api_key(OTHER_API_KEY);
    switch(&fx, to(&k), false).unwrap();
    let stray = Fx::credential_json("stray@x.co", "rt-stray")
        .to_string()
        .into_bytes();
    fx.set_live_credential(&stray);
    switch(&fx, to(&k2), false).unwrap();
    assert_eq!(fx.displaced(), [stray]);
    assert_eq!(
        fx.live_credential().unwrap(),
        json!({"mcpOAuth": {"srv": {"token": "machine-shared"}}}),
        "then stripped, as writing an API key does"
    );
}

#[test]
fn a_credentials_file_the_keychain_shadows_is_displaced_before_the_mirror_overwrites_it() {
    // §9.4 "never lose a secret": the Keychain holds the effective credential (b's), and
    // `.credentials.json` a generation no vault holds. Activating a writes the Keychain and
    // mirrors over the file (Appendix A.3's hot reload), so the file's generation is saved first.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let shadowed = Fx::credential_json("b@x.co", "rt-shadowed")
        .to_string()
        .into_bytes();
    let file = fx.paths().credentials_file;
    fs::write(&file, &shadowed).unwrap();
    let out = switch(&fx, to(&a), false).unwrap();
    assert_eq!(fx.displaced(), [shadowed]);
    assert!(out.warnings.iter().any(|w| w.contains("displaced")));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
    let mirrored: serde_json::Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
    assert_eq!(
        Some(mirrored),
        fx.live_credential(),
        "the target is live in both stores"
    );
    // The file now mirrors the effective credential: nothing more to save.
    switch(&fx, to(&b), false).unwrap();
    assert_eq!(displaced_files(&fx), 1);
}

#[test]
fn an_api_key_switch_saves_a_fallback_keychain_item_before_stripping_it() {
    // Appendix A.2: readers also try the unsuffixed item, and activating an API key strips
    // every item a reader tries. A generation there that no vault holds is saved first. An
    // OAuth switch leaves the item alone, so it saves nothing.
    let fx = Fx::with_fallback_items();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let k = fx.add_api_key(API_KEY);
    let stale = Fx::credential_json("old@x.co", "rt-old")
        .to_string()
        .into_bytes();
    fx.put_fallback_item(&stale);
    switch(&fx, to(&a), false).unwrap();
    assert_eq!(displaced_files(&fx), 0, "an OAuth switch");
    switch(&fx, to(&k), false).unwrap();
    assert_eq!(fx.displaced(), [stale]);
    assert_eq!(
        fx.fallback_item(),
        Some(json!({"mcpOAuth": {"srv": {"token": "machine-shared"}}})),
        "then stripped, as writing an API key does"
    );
}

#[test]
fn a_stale_mirror_the_vault_already_holds_is_not_displaced() {
    // The Keychain moved on without the file. CC refreshed it, so the file holds what the
    // capture keeps as `.prev`; or CC wiped it on `invalid_grant`, so the file holds the
    // vault's current generation. Either way the vault already holds it: nothing to save.
    for wiped in [false, true] {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let b = fx.add("b@x.co", "rt-b");
        fs::write(fx.paths().credentials_file, fx.vault_bytes(&b).unwrap()).unwrap();
        if wiped {
            fx.set_live_credential(br#"{"claudeAiOauth":{"accessToken":"","refreshToken":""}}"#);
        } else {
            fx.rotate_live("rt-b2");
        }
        switch(&fx, to(&a), false).unwrap();
        assert_eq!(displaced_files(&fx), 0, "wiped={wiped}");
        assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
    }
}

#[test]
fn a_fallback_item_the_vault_already_holds_is_not_displaced() {
    // A fallback item left from before `CLAUDE_CONFIG_DIR` was set, holding a generation of
    // the outgoing account that CC has since refreshed past: the capture keeps it as `.prev`.
    let fx = Fx::with_fallback_items();
    let a = fx.add("a@x.co", "rt-a");
    let k = fx.add_api_key(API_KEY);
    fx.put_fallback_item(&fx.vault_bytes(&a).unwrap());
    fx.rotate_live("rt-a2");
    switch(&fx, to(&k), false).unwrap();
    assert_eq!(displaced_files(&fx), 0);
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"));
}

#[test]
fn an_unreadable_fallback_keychain_item_aborts_before_anything_is_written() {
    // §9.4 step 3: step 7 may clear the item, and could not tell what it would lose.
    let fx = Fx::with_fallback_items();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    fx.put_fallback_item(b"{}");
    let acct = keychain_account(&fx.env);
    fx.kc.set_unreadable(common::FALLBACK_ITEM, &acct, true);
    let err = switch(&fx, to(&a), false).unwrap_err();
    assert_eq!(err.kind(), "unreadable", "{err}");
    assert!(common::journal(&fx).is_none());
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
}

#[test]
fn an_unreadable_credentials_file_behind_the_keychain_aborts_before_anything_is_written() {
    // §9.4 step 3: the mirror would overwrite a file tagteam could not read.
    use std::os::unix::fs::PermissionsExt;
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let file = fx.paths().credentials_file;
    fs::write(&file, b"{}").unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();
    let err = switch(&fx, to(&a), false).unwrap_err();
    assert_eq!(err.kind(), "unreadable", "{err}");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(fs::read(&file).unwrap(), b"{}");
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    assert!(common::journal(&fx).is_none());
}

#[test]
fn an_other_axis_secret_the_target_holds_is_not_displaced() {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let k = fx.add_api_key(API_KEY);
    fx.put_managed_key(API_KEY.as_bytes()); // k's key, left live under a's OAuth login
    switch(&fx, to(&k), false).unwrap();
    assert_eq!(displaced_files(&fx), 0);
}

#[test]
fn forcing_never_displaces_an_entry_with_only_machine_shared_keys() {
    // Such an entry holds nothing account-scoped, and its machine-shared keys are carried over.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let k = fx.add_api_key(API_KEY);
    switch(&fx, to(&k), false).unwrap(); // the entry now holds only machine-shared keys
    switch(&fx, to(&k), true).unwrap();
    assert_eq!(displaced_files(&fx), 0, "a forced self-switch");
    switch(&fx, to(&a), true).unwrap();
    assert_eq!(
        fx.displaced(),
        [API_KEY.as_bytes()],
        "forcing away from an API-key account saves its key, not the entry"
    );
}

#[test]
fn api_key_accounts_move_the_auth_axis_both_ways() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let k = fx.add_api_key(API_KEY);
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
fn a_switch_reports_the_linux_file_as_its_only_store() {
    let fx = Fx::with_platform(tagteam_cc::live::Platform::Linux);
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    assert_eq!(
        switch(&fx, to(&a), false).unwrap().stored_in,
        Some(SecretStore::File(fx.paths().credentials_file))
    );
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
}

#[test]
fn a_switch_reports_the_fallback_it_took_on_either_axis() {
    // Appendix A.3: an OAuth write the Keychain refuses lands in the credentials file; an API
    // key lands in `primaryApiKey` in the global config.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let key = fx.add_api_key(API_KEY);
    let (oauth_item, _) = fx.live_item(ItemKind::OAuth);
    let (key_item, _) = fx.live_item(ItemKind::ManagedKey);
    assert_eq!(
        switch(&fx, to(&a), false).unwrap().stored_in,
        Some(SecretStore::Keychain)
    );
    fx.kc.set_fail_write(&oauth_item, true);
    assert_eq!(
        switch(&fx, to(&b), false).unwrap().stored_in,
        Some(SecretStore::Fallback(fx.paths().credentials_file))
    );
    fx.kc.set_fail_write(&key_item, true);
    assert_eq!(
        switch(&fx, to(&key), false).unwrap().stored_in,
        Some(SecretStore::Fallback(fx.paths().global_config))
    );
}

#[test]
fn a_file_pin_from_an_earlier_switch_never_mislabels_a_later_api_key_switch() {
    // An OAuth fallback in one switch never mislabels a later API-key switch: the key the
    // Keychain takes was stored in the Keychain, and says so.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let key = fx.add_api_key(API_KEY);
    let (oauth_item, _) = fx.live_item(ItemKind::OAuth);
    fx.kc.set_fail_write(&oauth_item, true);
    assert_eq!(
        switch(&fx, to(&a), false).unwrap().stored_in,
        Some(SecretStore::Fallback(fx.paths().credentials_file))
    );
    fx.kc.set_fail_write(&oauth_item, false);
    assert_eq!(
        switch(&fx, to(&key), false).unwrap().stored_in,
        Some(SecretStore::Keychain)
    );
    assert_eq!(fx.managed_key().as_deref(), Some(API_KEY.as_bytes()));
}

#[test]
fn the_switch_after_a_fallback_tries_the_keychain_again() {
    // Appendix A.3 (L396): file mode lasts only as long as the switch that fell back, so a
    // long-lived process (M3b's `auto`) is not left on the file for good.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live: b, in the Keychain
    let (oauth_item, acct) = fx.live_item(ItemKind::OAuth);
    fx.kc.set_fail_write(&oauth_item, true);
    assert_eq!(
        switch(&fx, to(&a), false).unwrap().stored_in,
        Some(SecretStore::Fallback(fx.paths().credentials_file))
    );
    assert_eq!(
        fx.kc.get(&oauth_item, &acct),
        None,
        "the fallback deleted the item"
    );
    fx.kc.set_fail_write(&oauth_item, false);
    assert_eq!(
        switch(&fx, to(&b), false).unwrap().stored_in,
        Some(SecretStore::Keychain)
    );
    let item = fx.kc.get(&oauth_item, &acct).unwrap();
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-b"));
    // The file the fallback created now mirrors the item, rewritten for CC's hot reload.
    assert_eq!(fs::read(fx.paths().credentials_file).unwrap(), item);
}

#[test]
fn a_switch_that_writes_nothing_reports_no_store() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let outcome = switch(&fx, to(&a), false).unwrap();
    assert_eq!(
        (outcome.reason, outcome.stored_in),
        (SwitchReason::AlreadyActive, None)
    );
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
fn the_rotation_roster_is_read_before_cc_s_locks() {
    // Deciding a rotation reads every account's vault entry; under CC's locks that would
    // lengthen how long CC waits on tagteam. Only the two accounts switched are read there.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.add("c@x.co", "rt-c"); // live and active: the rotation wraps to a
    let refresh_lock = fx.paths().refresh_lock;
    let under_cc = Arc::new(Mutex::new(BTreeSet::new()));
    let seen = under_cc.clone();
    let engine = fx.engine_with_vault_probe(move |key| {
        if refresh_lock.is_dir() {
            seen.lock().unwrap().insert(key.to_owned());
        }
    });
    let out = engine
        .switch(request(&fx, SwitchTarget::Rotation, false))
        .unwrap();
    assert_eq!(out.to.unwrap().id, a);
    let under_cc = under_cc.lock().unwrap();
    assert!(under_cc.contains(a.as_str()), "{under_cc:?}");
    assert!(!under_cc.contains(b.as_str()), "{under_cc:?}");
}

/// Review Focus 1: while CC holds its refresh lock, the switch waits out the lock timeout,
/// fails naming the lock, and changes nothing anywhere.
fn cc_holding_its_refresh_lock_changes_nothing(fx: &Fx) {
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.rotate_live("rt-b2"); // a capture made before CC's locks would show in the vault
    fs::create_dir(fx.paths().refresh_lock).unwrap();
    let keychain = fx.kc.items(); // the live credential, the managed key and the vault
    let config = fs::read(fx.paths().global_config).unwrap();

    let err = switch(fx, to(&a), false).unwrap_err();
    assert_eq!(err.kind(), "lock-timeout");
    assert!(err.to_string().contains(".oauth_refresh.lock"), "{err}");

    assert_eq!(fx.kc.items(), keychain);
    assert_eq!(fs::read(fx.paths().global_config).unwrap(), config);
    let store = fx.engine.store().unwrap();
    assert!(store.journal(&fx.provider()).unwrap().is_none());
    assert_eq!(store.active(&fx.provider()).unwrap(), Some(b));
    assert_eq!(displaced_files(fx), 0);
    assert!(mutation_lock_free(&fx.env), "the mutation lock is released");
    assert!(fx.paths().refresh_lock.is_dir(), "CC's lock is left alone");
}

#[test]
fn cc_holding_its_refresh_lock_blocks_the_switch_for_the_injected_timeout() {
    let start = Instant::now();
    cc_holding_its_refresh_lock_changes_nothing(&Fx::with_lock_timeout(Duration::from_millis(300)));
    assert!(start.elapsed() < Duration::from_secs(5), "not the real 9 s");
}

#[test]
#[ignore = "waits the full 9 s CC lock timeout; run with --ignored"]
fn cc_holding_its_refresh_lock_blocks_the_switch_and_changes_nothing() {
    let start = Instant::now();
    cc_holding_its_refresh_lock_changes_nothing(&Fx::new());
    assert!(
        start.elapsed() >= Duration::from_secs(9),
        "the real timeout"
    );
}

/// When `id`'s next poll is planned, in seconds from now.
fn planned_in(fx: &Fx, id: &AccountId) -> i64 {
    fx.usage_state(id).unwrap().next_poll_at.unwrap() - fx.engine.now_ms() / 1000
}

/// Gives each of `ids` a reading (and so a plan), then forgets the requests that took.
fn read_each(fx: &Fx, ids: &[&AccountId]) {
    for _ in ids {
        fx.script_usage(200, usage_fixture());
    }
    fx.collect(ids);
    for id in ids {
        assert!(fx.usage_state(id).unwrap().fetched_at.is_some());
    }
    fx.http.clear();
}

/// Parks `id`'s plan an hour out, so only a re-plan can bring it back.
fn park_plan(fx: &Fx, id: &AccountId) {
    fx.engine
        .store()
        .unwrap()
        .set_poll_plan(
            id,
            &PollPlan {
                interval_s: 3_600,
                next_poll_at: fx.engine.now_ms() / 1000 + 3_600,
            },
        )
        .unwrap();
}

#[test]
fn a_switch_re_plans_both_accounts_polls_without_fetching() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    read_each(&fx, &[&a, &b]);
    let (a_read, b_read) = (fx.usage_state(&a).unwrap(), fx.usage_state(&b).unwrap());

    let out = switch(&fx, to(&a), false).unwrap();

    assert_eq!(out.reason, SwitchReason::Switched);
    // §9.4: the incoming account is next due 180 s after its reading, with no jitter.
    let incoming = fx.usage_state(&a).unwrap();
    assert_eq!(
        incoming.next_poll_at,
        Some(a_read.fetched_at.unwrap() + 180)
    );
    assert_eq!(incoming.poll_interval_s, Some(180));
    // §8.6's candidate default, jittered ±10%, never under the 180 s floor.
    let outgoing = planned_in(&fx, &b);
    assert!(
        (270..=330).contains(&outgoing),
        "the candidate policy: {outgoing}"
    );
    for (id, before) in [(&a, a_read), (&b, b_read)] {
        let after = fx.usage_state(id).unwrap();
        assert_eq!(
            (&after.last_good, after.fetched_at),
            (&before.last_good, before.fetched_at),
            "the reading stays"
        );
    }
    assert!(fx.http.requests().is_empty(), "a re-plan never fetches");
}

#[test]
fn a_re_plan_keeps_the_last_reading() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    fx.script_usage(200, usage_fixture());
    fx.collect(&[&a]);
    let before = fx.usage_state(&a).unwrap();
    assert!(before.last_good.is_some());
    fx.http.clear();

    switch(&fx, to(&a), false).unwrap();

    let after = fx.usage_state(&a).unwrap();
    assert_eq!(
        (
            &after.last_good,
            after.fetched_at,
            after.consecutive_failures
        ),
        (&before.last_good, before.fetched_at, 0)
    );
    assert_eq!(after.next_poll_at, Some(before.fetched_at.unwrap() + 180));
    assert!(fx.http.requests().is_empty());
}

#[test]
fn an_incoming_account_with_an_old_reading_is_due_at_once_and_fetched_on_demand() {
    // §9.4: `max(now, fetched_at + 180)`: a reading two hours old is overdue, not a minute out.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    read_each(&fx, &[&a]);
    fx.clock.advance_ms(2 * 3_600 * 1_000);

    let out = switch(&fx, to(&a), false).unwrap();

    assert_eq!(out.reason, SwitchReason::Switched);
    let now = fx.engine.now_ms() / 1000;
    assert_eq!(fx.usage_state(&a).unwrap().next_poll_at, Some(now));
    assert!(
        !fx.http.requests().is_empty(),
        "the switch sent its freshen"
    );
    assert!(
        fx.http
            .requests()
            .iter()
            .all(|r| r.url != Fx::endpoints().usage),
        "the switch's own requests (its §7.2 freshen) never touch the usage endpoint"
    );

    fx.http.clear();
    fx.script_usage(200, usage_fixture());
    fx.collect(&[&a]);

    assert_eq!(
        fx.http.requests().len(),
        1,
        "the next on-demand collect fetches it"
    );
}

#[test]
fn an_account_with_no_reading_is_not_re_planned_and_is_fetched_on_demand() {
    // §8.3's on-demand rule treats a plan in force as "not due", so a plan here would keep
    // `list` from reading the account it most needs to read.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    read_each(&fx, &[&b]);
    // Park b's plan far out, so only a re-plan can bring it back to the candidate policy.
    park_plan(&fx, &b);

    let out = switch(&fx, to(&a), false).unwrap();

    assert_eq!(out.reason, SwitchReason::Switched);
    assert_eq!(
        fx.usage_state(&a),
        None,
        "the incoming account has no reading: no plan"
    );
    let outgoing = planned_in(&fx, &b);
    assert!(
        (270..=330).contains(&outgoing),
        "b has one: the candidate policy: {outgoing}"
    );

    fx.script_usage(200, usage_fixture());
    let report = fx.collect(&[&a]);

    assert_eq!(
        fx.http.requests().len(),
        1,
        "the next on-demand collect fetches it"
    );
    assert!(
        fx.usage_state(&a).unwrap().fetched_at.is_some(),
        "{:?}",
        report.outcomes
    );
}

#[test]
fn the_outgoing_account_without_a_reading_is_left_unplanned_too() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    read_each(&fx, &[&a]);
    let read_at = fx.usage_state(&a).unwrap().fetched_at.unwrap();

    switch(&fx, to(&a), false).unwrap();

    assert_eq!(
        fx.usage_state(&a).unwrap().next_poll_at,
        Some(read_at + 180)
    );
    assert_eq!(fx.usage_state(&b), None, "b was never read: no plan");
}

#[test]
fn a_forced_self_switch_plans_only_that_account_as_the_active_one() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    read_each(&fx, &[&b]);
    let read_at = fx.usage_state(&b).unwrap().fetched_at.unwrap();
    // Park b's plan far out, so only a re-plan can bring it back to the active policy.
    park_plan(&fx, &b);

    let out = switch(&fx, to(&b), true).unwrap();

    assert_eq!(out.reason, SwitchReason::Activated);
    assert_eq!(
        fx.usage_state(&b).unwrap().next_poll_at,
        Some(read_at + 180)
    );
    assert_eq!(fx.usage_state(&a), None, "a was not part of this switch");
}

#[test]
fn a_switch_that_activates_nothing_re_plans_nothing() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");

    let out = switch(&fx, to(&b), false).unwrap();

    assert_eq!(out.reason, SwitchReason::AlreadyActive);
    assert_eq!(fx.usage_state(&a), None);
    assert_eq!(fx.usage_state(&b), None);
}

#[test]
fn a_re_plan_that_cannot_be_stored_never_fails_the_switch() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    read_each(&fx, &[&a]);
    let before = fx.usage_state(&a).unwrap().next_poll_at;
    rusqlite::Connection::open(fx.env.data_dir().join("tagteam.db"))
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER no_insert BEFORE INSERT ON usage_state \
               BEGIN SELECT RAISE(ABORT, 'usage_state is read-only'); END;
             CREATE TRIGGER no_update BEFORE UPDATE ON usage_state \
               BEGIN SELECT RAISE(ABORT, 'usage_state is read-only'); END;",
        )
        .unwrap();

    let (out, logs) = capture_logs(|| switch(&fx, to(&a), false).unwrap());

    assert!(out.switched, "{}", out.message);
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
    assert_eq!(
        fx.usage_state(&a).unwrap().next_poll_at,
        before,
        "the plan is as it was"
    );
    // The failed re-plan is logged at ERROR (§14, K3), naming the account by position and id,
    // with the store's error as a field and never the email.
    let errors: Vec<&String> = logs.iter().filter(|l| l.starts_with("ERROR")).collect();
    assert_eq!(errors.len(), 1, "{logs:?}");
    let line = errors[0];
    assert!(
        line.contains("could not re-plan usage polls after the switch")
            && line.contains("position=1")
            && line.contains(&format!("account={a}"))
            && line.contains("error=")
            && line.contains("usage_state is read-only"),
        "{line}"
    );
    assert!(!line.contains("@x.co"), "{line}");
    assert!(
        logs.iter().all(|l| !l.contains("@x.co")),
        "no email in any event: {logs:?}"
    );
}
