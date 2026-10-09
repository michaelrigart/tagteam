mod common;

use std::collections::BTreeSet;
use std::fs;
#[cfg(feature = "test-hooks")]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{
    API_KEY, Fx, OTHER_API_KEY, STRAY_API_KEY, capture_logs, credential, mutation_lock_free,
    token_requests, usage_fixture,
};
use serde_json::json;
use tagteam_cc::{ItemKind, keychain_account, keychain_service};
use tagteam_core::autoswitch::{AutoState, Departure, Trigger};
use tagteam_core::poll::PollPlan;
use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::EngineError;
use tagteam_engine::lifecycle::AddTokenOptions;
use tagteam_engine::store::{Activation, EventRow, NewAccount};
use tagteam_engine::switch::{
    AutoPerform, SwitchOutcome, SwitchReason, SwitchRequest, SwitchTarget,
};
use tagteam_engine::vault::SERVICE;
use tagteam_provider::{Keychain, Provider, SecretStore};

fn request(fx: &Fx, target: SwitchTarget, force: bool) -> SwitchRequest {
    SwitchRequest {
        provider: fx.provider(),
        target,
        force,
        source: "cli",
        auto: None,
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
        .set_active(&fx.provider(), None, None)
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
fn no_switch_reads_or_touches_an_inert_former_fallback_item() {
    // Appendix A.2 (2.1.286): under an explicit CLAUDE_CONFIG_DIR=~/.claude, CC names only the
    // suffixed items. The unsuffixed ones belong to another spelling: an OAuth switch, an
    // API-key switch and the switch back neither save, strip nor delete them.
    let fx = Fx::with_explicit_default_config_dir();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let k = fx.add_api_key(API_KEY);
    let (oauth, managed) = fx.put_inert_items();
    for (step, target) in [("to a", &a), ("to k", &k), ("back to a", &a)] {
        switch(&fx, to(target), false).unwrap();
        assert_eq!(displaced_files(&fx), 0, "{step}");
        assert_eq!(
            fx.inert_items(),
            (Some(oauth.clone()), Some(managed.clone())),
            "{step}"
        );
    }
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
fn an_unreadable_inert_item_never_blocks_a_switch() {
    // It was a fallback that §9.4 step 3 had to read before step 7 could clear it; now no
    // switch reads it at all (Appendix A.2).
    let fx = Fx::with_explicit_default_config_dir();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let k = fx.add_api_key(API_KEY);
    fx.put_inert_items();
    let acct = keychain_account(&fx.env);
    fx.kc.set_unreadable(common::INERT_ITEM, &acct, true);
    fx.kc
        .set_unreadable(common::INERT_MANAGED_ITEM, &acct, true);
    switch(&fx, to(&k), false).unwrap();
    switch(&fx, to(&a), false).unwrap();
    assert!(common::journal(&fx).is_none());
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
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

/// Whether CC's credential locks and the mutation lock can all be taken now: nothing holds them.
fn credential_locks_and_guard_free(fx: &Fx) -> bool {
    let free = fs::create_dir(fx.paths().refresh_lock).is_ok();
    if free {
        fs::remove_dir(fx.paths().refresh_lock).unwrap();
    }
    free && !fx.paths().legacy_lock().exists() && mutation_lock_free(&fx.env)
}

#[test]
fn a_switch_waits_out_cc_s_config_lock_before_taking_any_lock() {
    // §9.1: a config lock CC left behind is waited out holding neither the mutation lock nor
    // CC's credential locks, which CC's own refresh waits on.
    let fx = Fx::with_lock_budgets(Duration::from_millis(300), Duration::from_secs(5));
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b"); // live: b
    fs::create_dir(fx.paths().config_lock).unwrap(); // left behind, fresh

    let out = std::thread::scope(|s| {
        let switching = s.spawn(|| switch(&fx, to(&a), false));
        for _ in 0..6 {
            std::thread::sleep(Duration::from_millis(100));
            assert!(!switching.is_finished(), "the switch did not wait");
            assert!(
                credential_locks_and_guard_free(&fx),
                "the pre-wait holds a lock"
            );
        }
        fs::remove_dir(fx.paths().config_lock).unwrap();
        switching.join().unwrap()
    })
    .unwrap();

    assert_eq!(out.to.unwrap().id, a);
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
}

#[test]
fn a_config_lock_that_outlasts_the_pre_wait_refuses_with_a_lock_timeout_and_changes_nothing() {
    let fx = Fx::with_lock_budgets(Duration::from_millis(300), Duration::from_millis(600));
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fs::create_dir(fx.paths().config_lock).unwrap();
    let config = fs::read(fx.paths().global_config).unwrap();

    let err = switch(&fx, to(&a), false).unwrap_err();

    assert_eq!(err.kind(), "lock-timeout");
    assert!(
        err.to_string().contains("frees itself within about 11 s"),
        "{err}"
    );
    assert_eq!(fs::read(fx.paths().global_config).unwrap(), config);
    assert_eq!(
        fx.engine.store().unwrap().active(&fx.provider()).unwrap(),
        Some(b)
    );
    assert!(credential_locks_and_guard_free(&fx));
    assert!(fx.paths().config_lock.is_dir(), "CC's lock is left alone");
}

/// Plants a hook that has CC's start-up overwrite `oauthAccount` with `email`'s at each of the
/// first `times` reads of §9.1's re-verification, and counts the reads.
#[cfg(feature = "test-hooks")]
fn overwrite_identity_at_reverify(fx: &Fx, email: &'static str, times: usize) -> Arc<AtomicUsize> {
    let reads = Arc::new(AtomicUsize::new(0));
    let seen = reads.clone();
    let config = fx.paths().global_config;
    fx.engine.on_point(
        "identity-reverify",
        Box::new(move || {
            if seen.fetch_add(1, Ordering::SeqCst) < times {
                common::splice_oauth_account(&config, &Fx::oauth_account(email));
            }
        }),
    );
    reads
}

#[test]
#[cfg(feature = "test-hooks")]
fn an_untouched_identity_is_read_once_after_the_commit_and_nothing_more_is_done() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let reads = overwrite_identity_at_reverify(&fx, "x@x.co", 0);

    let out = switch(&fx, to(&a), false).unwrap();

    assert_eq!(reads.load(Ordering::SeqCst), 1);
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
}

#[test]
#[cfg(feature = "test-hooks")]
fn an_identity_overwritten_after_the_commit_is_spliced_again_without_a_warning() {
    // §9.1 (amended): CC's start-up wrote the global config without its lock.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let reads = overwrite_identity_at_reverify(&fx, "b@x.co", 1);

    let out = switch(&fx, to(&a), false).unwrap();

    assert_eq!(reads.load(Ordering::SeqCst), 2, "read, spliced, read again");
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    assert!(out.switched);
    assert!(
        !fx.paths().config_lock.exists(),
        "the splice's lock is released"
    );
}

#[test]
#[cfg(feature = "test-hooks")]
fn an_identity_overwritten_again_warns_and_names_switching_again() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    overwrite_identity_at_reverify(&fx, "b@x.co", 2);

    let (out, logs) = capture_logs(|| switch(&fx, to(&a), false).unwrap());

    assert!(out.switched, "the switch committed");
    assert_eq!(
        out.warnings,
        [
            "a starting Claude Code may have overwritten the account's identity in its config; run `tagteam switch` again to restore it"
        ]
    );
    assert_eq!(
        fx.engine.store().unwrap().active(&fx.provider()).unwrap(),
        Some(a)
    );
    let warned: Vec<_> = logs
        .iter()
        .filter(|l| l.contains("may have overwritten the switched account's identity"))
        .collect();
    assert_eq!(warned.len(), 1, "{logs:?}");
    assert!(
        logs.iter().all(|l| !l.contains("@x.co")),
        "no email in the log: {logs:?}"
    );
}

#[test]
#[cfg(feature = "test-hooks")]
fn an_identity_that_cannot_be_spliced_again_warns_too() {
    // The splice itself fails: the config is torn when it is tried.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let config = fx.paths().global_config;
    fx.engine.on_point(
        "identity-reverify",
        Box::new(move || fs::write(&config, b"{ not json").unwrap()),
    );

    let out = switch(&fx, to(&a), false).unwrap();

    assert!(out.switched);
    assert_eq!(out.warnings.len(), 1, "{:?}", out.warnings);
    assert!(out.warnings[0].contains("tagteam switch"));
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
               BEGIN SELECT RAISE(ABORT, 'usage_state is read-only for bob@example.com'); END;
             CREATE TRIGGER no_update BEFORE UPDATE ON usage_state \
               BEGIN SELECT RAISE(ABORT, 'usage_state is read-only for bob@example.com'); END;",
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
    // with SQLite's result code as the cause (§14.2): the message is the database's own text,
    // here a trigger's, and never logged.
    let errors: Vec<&String> = logs.iter().filter(|l| l.starts_with("ERROR")).collect();
    assert_eq!(errors.len(), 1, "{logs:?}");
    let line = errors[0];
    assert!(
        line.contains("could not re-plan usage polls after the switch")
            && line.contains("position=1")
            && line.contains(&format!("account={a}"))
            && line.contains("code=ConstraintViolation")
            && !line.contains("read-only")
            && !line.contains("bob@example.com"),
        "{line}"
    );
    assert!(!line.contains("@x.co"), "{line}");
    assert!(
        logs.iter().all(|l| !l.contains("@x.co")),
        "no email in any event: {logs:?}"
    );
}

/// The fixture clock's start (`Fx`), in seconds.
const T0: i64 = 1_790_000_000;

/// An automatic switch from `from` to `target` with a 300 s cooldown, as a tick performs it
/// (§11.2 step 11).
fn auto_switch(fx: &Fx, from: &AccountId, target: &AccountId, trigger: Trigger) -> SwitchRequest {
    SwitchRequest {
        provider: fx.provider(),
        target: to(target),
        force: false,
        source: "auto",
        auto: Some(AutoPerform {
            expected_from: from.clone(),
            trigger,
            cooldown_s: 300,
            departure: Departure {
                left_headroom: Some(4.0),
                left_recovery_at: Some(T0 + 9_630),
                left_trigger: trigger,
            },
        }),
    }
}

fn auto_state(fx: &Fx) -> AutoState {
    fx.engine
        .store()
        .unwrap()
        .autoswitch_state(&fx.provider())
        .unwrap()
}

fn switch_events(fx: &Fx) -> Vec<EventRow> {
    let events = fx.engine.store().unwrap().events().unwrap();
    events.into_iter().filter(|e| e.kind == "switch").collect()
}

#[test]
fn the_new_reasons_carry_the_spec_s_tokens() {
    assert_eq!(SwitchReason::LiveChanged.as_str(), "live-changed");
    assert_eq!(SwitchReason::Cooldown.as_str(), "cooldown");
    assert_eq!(SwitchReason::NotCandidate.as_str(), "not-candidate");
}

#[test]
fn an_automatic_switch_records_its_departure_in_the_commit() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    let store = fx.engine.store().unwrap();
    store.set_unhealthy_ticks(&fx.provider(), 2).unwrap();
    let out = fx
        .engine
        .switch(auto_switch(&fx, &b, &a, Trigger::Proactive))
        .unwrap();
    assert_eq!(
        (out.switched, out.reason, out.strategy),
        (true, SwitchReason::Switched, "direct")
    );
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
    assert_eq!(
        auto_state(&fx),
        AutoState {
            last_switch_at: Some(T0),
            last_switch_from: Some(b.clone()),
            last_switch_to: Some(a.clone()),
            left_headroom: Some(4.0),
            left_recovery_at: Some(T0 + 9_630),
            left_trigger: Some(Trigger::Proactive),
            unhealthy_ticks: 0,
        }
    );
    let last = switch_events(&fx).pop().unwrap();
    assert_eq!(
        (
            last.trigger.as_deref(),
            last.source.as_str(),
            last.from_id,
            last.to_id
        ),
        (Some("proactive"), "auto", Some(b), Some(a))
    );
}

#[test]
fn a_manual_switch_records_no_auto_state_and_its_trigger_is_manual() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    assert!(fx.switch_to(&a, false).unwrap().switched);
    assert_eq!(auto_state(&fx), AutoState::default());
    let last = switch_events(&fx).pop().unwrap();
    assert_eq!(
        (last.trigger.as_deref(), last.source.as_str()),
        (Some("manual"), "cli")
    );
}

/// Review Focus 1, the precondition half: a manual `tagteam switch` lands between the tick's
/// decision and its perform, while the automatic switch waits before the mutation lock. The
/// automatic one finds the live account moved and writes nothing; the manual result stands.
#[cfg(feature = "test-hooks")]
#[test]
fn a_manual_switch_between_the_decision_and_the_perform_is_never_overridden() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let c = fx.add("c@x.co", "rt-c"); // live
    let other = fx.engine_with_env(fx.env.clone());
    let manual = fx.switch_request(&b, false);
    fx.engine.on_point(
        "planned",
        Box::new(move || assert!(other.switch(manual.clone()).unwrap().switched)),
    );
    let out = fx
        .engine
        .switch(auto_switch(&fx, &c, &a, Trigger::Proactive))
        .unwrap();
    assert_eq!(
        (out.switched, out.reason, out.reason.as_str()),
        (false, SwitchReason::LiveChanged, "live-changed")
    );
    assert_eq!(out.from.map(|r| r.id), Some(b.clone()));
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-b"));
    let store = fx.engine.store().unwrap();
    assert_eq!(store.active(&fx.provider()).unwrap(), Some(b.clone()));
    let switches = switch_events(&fx);
    assert_eq!(switches.len(), 1, "{switches:?}");
    assert_eq!(
        (switches[0].source.as_str(), switches[0].to_id.clone()),
        ("cli", Some(b))
    );
    assert_eq!(auto_state(&fx), AutoState::default());
}

#[test]
fn an_automatic_switch_never_acts_on_a_live_login_it_did_not_decide_on() {
    // §11.2 step 11: an unmanaged login, or none at all, is a live change too. A direct switch
    // would displace the one and activate over the other.
    let leave: [fn(&Fx); 2] = [log_out, |fx| fx.login("stranger@x.co", "rt-s")];
    for (n, leave) in leave.into_iter().enumerate() {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let b = fx.add("b@x.co", "rt-b");
        leave(&fx);
        let live = fx.live_credential();
        let out = fx
            .engine
            .switch(auto_switch(&fx, &b, &a, Trigger::Failover))
            .unwrap();
        assert_eq!(
            (out.switched, out.reason),
            (false, SwitchReason::LiveChanged),
            "case {n}"
        );
        assert_eq!(fx.live_credential(), live, "case {n}");
        assert!(fx.displaced().is_empty(), "case {n}");
        assert!(switch_events(&fx).is_empty(), "case {n}");
        assert_eq!(auto_state(&fx), AutoState::default(), "case {n}");
    }
}

#[test]
fn the_cooldown_is_judged_again_from_the_state_under_the_lock() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    let first = fx
        .engine
        .switch(auto_switch(&fx, &b, &a, Trigger::Proactive))
        .unwrap();
    assert!(first.switched);
    let recorded = auto_state(&fx);
    fx.clock.advance_ms(60_000);
    for trigger in [Trigger::Proactive, Trigger::ConsumeFirst] {
        let out = fx.engine.switch(auto_switch(&fx, &a, &b, trigger)).unwrap();
        assert_eq!(
            (out.switched, out.reason, out.reason.as_str()),
            (false, SwitchReason::Cooldown, "cooldown"),
            "{trigger:?}"
        );
        assert_eq!(
            out.message,
            "the cooldown after the last automatic switch has 4m left"
        );
    }
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
    assert_eq!(auto_state(&fx), recorded);
    assert_eq!(switch_events(&fx).len(), 1);
    // §11.2 step 6: at-limit and failover bypass it.
    let out = fx
        .engine
        .switch(auto_switch(&fx, &a, &b, Trigger::AtLimit))
        .unwrap();
    assert!(out.switched, "{}", out.message);
    assert_eq!(auto_state(&fx).last_switch_at, Some(T0 + 60));
    // The cooldown ends 300 s after that switch, to the second.
    fx.clock.advance_ms(299_000);
    let out = fx
        .engine
        .switch(auto_switch(&fx, &b, &a, Trigger::Proactive))
        .unwrap();
    assert_eq!(out.reason, SwitchReason::Cooldown);
    assert_eq!(
        out.message,
        "the cooldown after the last automatic switch has <1m left"
    );
    fx.clock.advance_ms(1_000);
    let out = fx
        .engine
        .switch(auto_switch(&fx, &b, &a, Trigger::Proactive))
        .unwrap();
    assert!(out.switched, "{}", out.message);
}

/// What stops `a` being a candidate, in one case below.
type Uncandidate = fn(&Fx, &AccountId);

#[test]
fn a_target_that_is_no_longer_a_candidate_writes_nothing() {
    // §11.2 step 11: a direct switch accepts a disabled or quarantined target (§9.3, §7.2),
    // so an automatic one checks for itself, and the tick moves on to its next target. The
    // quarantined target's token is due: nothing is refreshed for it either.
    let cases: [(&str, Uncandidate, &str); 4] = [
        (
            "disabled",
            |fx, a| drop(fx.engine.set_disabled(a, true).unwrap()),
            "a@x.co (position 1) is no longer a candidate: it is disabled",
        ),
        (
            "quarantined",
            |fx, a| {
                fx.expire_access(a);
                fx.quarantine(a, "invalid_grant", "sha256:sent");
            },
            "a@x.co (position 1) is no longer a candidate: it needs a new login",
        ),
        (
            "vault-less",
            |fx, a| fx.kc.delete(SERVICE, a.as_str()).unwrap(),
            "a@x.co (position 1) is no longer a candidate: it has no stored credential",
        ),
        (
            "removed",
            |fx, a| drop(fx.engine.remove(a).unwrap()),
            "the account to switch to was removed",
        ),
    ];
    for (case, make, message) in cases {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let b = fx.add("b@x.co", "rt-b"); // live
        make(&fx, &a);
        let out = fx
            .engine
            .switch(auto_switch(&fx, &b, &a, Trigger::AtLimit))
            .unwrap();
        assert_eq!(
            (out.switched, out.reason, out.reason.as_str()),
            (false, SwitchReason::NotCandidate, "not-candidate"),
            "{case}"
        );
        assert_eq!(out.message, message, "{case}");
        assert_eq!(fx.live_email().as_deref(), Some("b@x.co"), "{case}");
        assert!(switch_events(&fx).is_empty(), "{case}");
        assert_eq!(token_requests(&fx), 0, "{case}");
    }
}

#[test]
fn an_automatic_switch_to_a_target_a_daemon_or_an_unreadable_lock_holds_says_so() {
    // §11.2 step 11: the reason is the shared ownership text, naming each owner and each file,
    // not "a `tagteam run` session".
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    let dir = fx.make_profile(&a);
    let lock = dir.join("daemon.lock");
    fs::write(&lock, b"{").unwrap();
    let out = fx
        .engine
        .switch(auto_switch(&fx, &b, &a, Trigger::AtLimit))
        .unwrap();
    assert_eq!(out.reason, SwitchReason::NotCandidate);
    assert!(
        out.message
            .contains(&format!("'{}' cannot be read", lock.display()))
            && out.message.contains("delete the lock")
            && !out.message.contains("`tagteam run` session"),
        "{}",
        out.message
    );
    fs::write(
        &lock,
        serde_json::json!({"pid": 4343, "origin": "transient", "procStart": common::LSTART})
            .to_string(),
    )
    .unwrap();
    fx.process.set(
        4343,
        tagteam_provider::liveness::FakeProcess {
            exists: Some(true),
            start_time_s: tagteam_provider::parse_lstart(common::LSTART),
            ..Default::default()
        },
    );
    let out = fx
        .engine
        .switch(auto_switch(&fx, &b, &a, Trigger::AtLimit))
        .unwrap();
    assert_eq!(out.reason, SwitchReason::NotCandidate);
    assert!(
        out.message.contains("background daemon")
            && out.message.contains("claude daemon stop --any"),
        "{}",
        out.message
    );
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
}

#[test]
fn an_automatic_switch_leaves_freshening_to_the_tick() {
    // §11.2 step 10: the tick freshens each target by its own table before it performs; the
    // switch does not freshen again.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    fx.expire_access(&a);
    let out = fx
        .engine
        .switch(auto_switch(&fx, &b, &a, Trigger::AtLimit))
        .unwrap();
    assert!(out.switched, "{}", out.message);
    assert_eq!(token_requests(&fx), 0);
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
}

/// Sets `id`'s `login_epoch` directly, as explicit replacements since it was added would have.
fn set_login_epoch(fx: &Fx, id: &AccountId, epoch: i64) {
    rusqlite::Connection::open(fx.env.data_dir().join("tagteam.db"))
        .unwrap()
        .execute(
            "UPDATE accounts SET login_epoch = ?2 WHERE id = ?1",
            rusqlite::params![id.as_str(), epoch],
        )
        .unwrap();
}

#[test]
fn a_switch_records_the_target_s_login_epoch_as_the_activation_epoch() {
    // §9.4 step 9: the active account and its activation epoch move in the commit.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b"); // live: b
    set_login_epoch(&fx, &a, 3);

    switch(&fx, to(&a), false).unwrap();

    assert_eq!(
        fx.activation(),
        Some(Activation {
            account: a.clone(),
            epoch: Some(3)
        })
    );
    assert!(!fx.live_store_stale(&a));
}

#[cfg(feature = "test-hooks")]
#[test]
fn the_journal_row_carries_the_target_s_login_epoch() {
    // §9.4 step 6: the row names the target's `login_epoch`, which a forward recovery records
    // (§9.6). Read by a second engine while the row exists, between the journal and the commit.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    set_login_epoch(&fx, &a, 3);
    let seen: Arc<Mutex<Option<tagteam_engine::store::JournalRow>>> = Arc::default();
    let other = fx.engine_with_env(fx.env.clone());
    let (slot, provider) = (seen.clone(), fx.provider());
    fx.engine.on_point(
        "after-journal",
        Box::new(move || {
            *slot.lock().unwrap() = other.store().unwrap().journal(&provider).unwrap();
        }),
    );

    switch(&fx, to(&a), false).unwrap();

    let row = seen.lock().unwrap().clone().expect("the row was journaled");
    assert_eq!((row.to_id, row.to_epoch), (a, Some(3)));
}

/// The warning a displaced stale live store leaves, up to the displaced file's name.
const STALE_DISPLACED: &str = "the live credential predates position 2's replacement, so it did not replace the stored one; it was saved as displaced/";

#[test]
fn switching_away_from_a_replaced_live_login_displaces_it_instead_of_capturing() {
    // §9.4 step 4, §12.5, Review Focus 3: b's login was replaced while Claude Code kept the old
    // one and went on refreshing it. Attributed or not, that lineage is displaced.
    for attributed in [false, true] {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let b = fx.add("b@x.co", "rt-b"); // live, position 2
        fx.replace_login(&b, &credential("b@x.co", "rt-b-new"), "oauth");
        fx.rotate_live("rt-b2");
        if attributed {
            let owner = fx.cc.parse_identity(&Fx::oauth_account("b@x.co")).unwrap();
            fx.oracle.set(Some(owner));
        }

        let out = switch(&fx, to(&a), false).unwrap();

        assert_eq!(
            fx.vault_refresh_token(&b).as_deref(),
            Some("rt-b-new"),
            "attributed={attributed}: the replacement stays"
        );
        let displaced = fx.displaced();
        assert_eq!(displaced.len(), 1, "attributed={attributed}");
        assert!(String::from_utf8_lossy(&displaced[0]).contains("rt-b2"));
        assert!(
            out.warnings.iter().any(|w| w.starts_with(STALE_DISPLACED)),
            "{:?}",
            out.warnings
        );
        assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
    }
}

#[test]
fn a_self_switch_over_a_replaced_live_login_activates_the_replacement() {
    // §9.2: a self-switch whose live credential diverged runs a full switch once the oracle
    // attributes it to the account. Its step 4 displaces the old lineage instead of capturing
    // it back, so the replacement is what gets activated, and the mark is cleared.
    let fx = Fx::new();
    fx.add("b@x.co", "rt-b");
    let a = fx.add("a@x.co", "rt-a"); // live, position 2
    fx.replace_login(&a, &credential("a@x.co", "rt-a-new"), "oauth");
    fx.rotate_live("rt-a2");
    let owner = fx.cc.parse_identity(&Fx::oauth_account("a@x.co")).unwrap();
    fx.oracle.set(Some(owner));

    let out = switch(&fx, to(&a), false).unwrap();

    assert_eq!(out.reason, SwitchReason::Activated);
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a-new"));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a-new"));
    assert!(
        out.warnings.iter().any(|w| w.starts_with(STALE_DISPLACED)),
        "{:?}",
        out.warnings
    );
    assert_eq!(
        fx.activation(),
        Some(Activation {
            account: a.clone(),
            epoch: Some(1)
        })
    );
}

#[test]
fn a_forced_switch_to_a_replaced_live_login_clears_the_stale_mark() {
    // §12.5: `tagteam switch <N> --force` re-activates the vault's generation, as a forced
    // self-switch does (§9.2), and its commit records the current epoch. Claude Code's old
    // lineage is displaced, not lost.
    let fx = Fx::new();
    fx.add("b@x.co", "rt-b");
    let a = fx.add("a@x.co", "rt-a");
    fx.replace_login(&a, &credential("a@x.co", "rt-a-new"), "oauth");
    fx.rotate_live("rt-a2");
    assert!(fx.live_store_stale(&a));

    let out = switch(&fx, to(&a), true).unwrap();

    assert!(out.switched);
    assert!(!fx.live_store_stale(&a));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a-new"));
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a-new"));
    let displaced = fx.displaced();
    assert_eq!(displaced.len(), 1);
    assert!(String::from_utf8_lossy(&displaced[0]).contains("rt-a2"));
}
