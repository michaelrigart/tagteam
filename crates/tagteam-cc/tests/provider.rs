use std::fs;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tagteam_cc::live::{LiveStore, Platform};
use tagteam_cc::provider::ClaudeCode;
use tagteam_cc::{CcPaths, ItemKind, keychain_account, keychain_service, read_services};
use tagteam_provider::{
    Env, FakeKeychain, MutationGuard, Provider, Read, SecretStore, StoredLogin,
};

struct Fx {
    _d: tempfile::TempDir,
    env: Env,
    kc: Arc<FakeKeychain>,
    cc: ClaudeCode,
}

fn fx() -> Fx {
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    fs::create_dir_all(env.home.join(".claude")).unwrap();
    let kc = Arc::new(FakeKeychain::new());
    let cc = ClaudeCode::with_store(
        LiveStore::new(kc.clone(), Platform::MacOs).with_retry_delay(Duration::ZERO),
    );
    Fx { _d: d, env, kc, cc }
}

fn oauth_item(f: &Fx) -> Option<Value> {
    let svc = keychain_service(&f.env, ItemKind::OAuth);
    f.kc.get(&svc, &keychain_account(&f.env))
        .map(|b| serde_json::from_slice(&b).unwrap())
}

fn target(f: &Fx, email: &str, rt: &str) -> StoredLogin {
    StoredLogin {
        kind: "oauth".into(),
        secret: json!({"claudeAiOauth": {"accessToken": "at", "refreshToken": rt}})
            .to_string()
            .into_bytes(),
        identity: f
            .cc
            .parse_identity(&json!({"emailAddress": email, "organizationUuid": ""}))
            .unwrap(),
    }
}

#[test]
fn writes_the_composed_credential_and_the_identity_then_undoes_both() {
    let f = fx();
    let paths = CcPaths::resolve(&f.env);
    let before_cfg = "{\n  \"oauthAccount\": {\n    \"emailAddress\": \"old@a.co\"\n  },\n  \"userID\": \"u\"\n}\n";
    fs::write(&paths.global_config, before_cfg).unwrap();
    let svc = keychain_service(&f.env, ItemKind::OAuth);
    let acct = keychain_account(&f.env);
    let live_json = json!({"claudeAiOauth": {"refreshToken": "old"}, "mcpOAuth": {"m": 1}});
    f.kc.put(&svc, &acct, live_json.to_string().as_bytes());

    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let locks = f.cc.lock_live(&f.env, &g).unwrap();
    let live = f.cc.read_live_auth(&f.env);
    let t = target(&f, "new@b.co", "rt-new");
    let written = f.cc.write_credential(&f.env, &locks, &t, &live).unwrap();
    assert_eq!(written.stored_in, SecretStore::Keychain);
    let u1 = written.undo;
    let u2 =
        f.cc.write_identity(&f.env, &locks, Some(&t.identity))
            .unwrap();

    assert_eq!(
        oauth_item(&f).unwrap(),
        json!({"claudeAiOauth": {"accessToken": "at", "refreshToken": "rt-new"}, "mcpOAuth": {"m": 1}})
    );
    assert_eq!(
        f.cc.live_identity(&f.env).present().unwrap().label,
        "new@b.co"
    );
    assert!(
        fs::read_to_string(&paths.global_config)
            .unwrap()
            .contains("\"userID\": \"u\"")
    );

    u2.undo(&locks).unwrap();
    u1.undo(&locks).unwrap();
    assert_eq!(oauth_item(&f).unwrap(), live_json);
    assert_eq!(
        fs::read_to_string(&paths.global_config).unwrap(),
        before_cfg
    );
}

#[test]
fn an_api_key_target_moves_the_auth_axis() {
    let f = fx();
    let paths = CcPaths::resolve(&f.env);
    fs::write(&paths.global_config, "{}").unwrap();
    let svc = keychain_service(&f.env, ItemKind::OAuth);
    let acct = keychain_account(&f.env);
    f.kc.put(
        &svc,
        &acct,
        br#"{"claudeAiOauth":{"refreshToken":"r"},"pluginSecrets":{"p":1}}"#,
    );
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let locks = f.cc.lock_live(&f.env, &g).unwrap();
    let live = f.cc.read_live_auth(&f.env);
    let key = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz";
    let t = StoredLogin {
        kind: "api_key".into(),
        secret: key.as_bytes().to_vec(),
        identity: f.cc.token_identity("api-key-2@token.local"),
    };
    let written = f.cc.write_credential(&f.env, &locks, &t, &live).unwrap();
    assert_eq!(written.stored_in, SecretStore::Keychain);
    assert_eq!(
        f.kc.get(&keychain_service(&f.env, ItemKind::ManagedKey), &acct)
            .unwrap(),
        key.as_bytes()
    );
    assert_eq!(oauth_item(&f).unwrap(), json!({"pluginSecrets": {"p": 1}}));
    // Back to OAuth: the managed key goes, machine-shared keys stay.
    let live = f.cc.read_live_auth(&f.env);
    f.cc.write_credential(&f.env, &locks, &target(&f, "a@b.co", "rt"), &live)
        .unwrap();
    assert!(
        f.kc.get(&keychain_service(&f.env, ItemKind::ManagedKey), &acct)
            .is_none()
    );
    assert_eq!(oauth_item(&f).unwrap()["pluginSecrets"], json!({"p": 1}));
}

#[test]
fn an_unreadable_live_entry_is_never_overwritten() {
    let f = fx();
    let svc = keychain_service(&f.env, ItemKind::OAuth);
    let acct = keychain_account(&f.env);
    f.kc.put(&svc, &acct, b"{}");
    f.kc.set_unreadable(&svc, &acct, true);
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let locks = f.cc.lock_live(&f.env, &g).unwrap();
    let live = f.cc.read_live_auth(&f.env);
    assert!(
        f.cc.write_credential(&f.env, &locks, &target(&f, "a@b.co", "rt"), &live)
            .is_err()
    );
    f.kc.set_unreadable(&svc, &acct, false);
    assert_eq!(f.kc.get(&svc, &acct).unwrap(), b"{}");
}

#[test]
fn a_failed_write_restores_what_it_had_already_changed() {
    let f = fx();
    let paths = CcPaths::resolve(&f.env);
    fs::write(
        &paths.global_config,
        r#"{"primaryApiKey": "sk-ant-api03-old"}"#,
    )
    .unwrap();
    let acct = keychain_account(&f.env);
    let managed = keychain_service(&f.env, ItemKind::ManagedKey);
    f.kc.put(&managed, &acct, b"sk-ant-api03-old");
    f.kc.set_fail_delete(&managed, true); // clearing the managed key will fail after the OAuth write
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let locks = f.cc.lock_live(&f.env, &g).unwrap();
    let live = f.cc.read_live_auth(&f.env);
    assert!(
        f.cc.write_credential(&f.env, &locks, &target(&f, "a@b.co", "rt"), &live)
            .is_err()
    );
    assert!(oauth_item(&f).is_none(), "the OAuth write was rolled back");
    assert!(matches!(
        f.cc.read_live_auth(&f.env).managed_key,
        Read::Present(_)
    ));
}

#[test]
fn a_restore_that_fails_is_reported_not_hidden() {
    let f = fx();
    let paths = CcPaths::resolve(&f.env);
    fs::write(&paths.global_config, "{}").unwrap();
    let acct = keychain_account(&f.env);
    let (oauth, managed) = (
        keychain_service(&f.env, ItemKind::OAuth),
        keychain_service(&f.env, ItemKind::ManagedKey),
    );
    f.kc.put(&managed, &acct, b"sk-ant-api03-old");
    f.kc.set_fail_delete(&managed, true); // clearing the managed key fails after the OAuth write
    f.kc.set_fail_delete(&oauth, true); // and so does restoring the OAuth item's absence
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let locks = f.cc.lock_live(&f.env, &g).unwrap();
    let live = f.cc.read_live_auth(&f.env);
    let err =
        f.cc.write_credential(&f.env, &locks, &target(&f, "a@b.co", "rt"), &live)
            .err()
            .unwrap();
    assert!(
        matches!(err, tagteam_provider::ProviderError::RestoreFailed { .. }),
        "{err}"
    );
}

#[test]
fn a_panic_inside_one_operation_restores_its_first_write() {
    let f = fx();
    let acct = keychain_account(&f.env);
    let (oauth, managed) = (
        keychain_service(&f.env, ItemKind::OAuth),
        keychain_service(&f.env, ItemKind::ManagedKey),
    );
    f.kc.put(&managed, &acct, b"sk-ant-api03-old");
    f.kc.set_panic_on_delete(&managed, true); // panics while clearing the managed key, after the OAuth write
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let locks = f.cc.lock_live(&f.env, &g).unwrap();
    let live = f.cc.read_live_auth(&f.env);
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        f.cc.write_credential(&f.env, &locks, &target(&f, "a@b.co", "rt"), &live)
    }));
    assert!(r.is_err());
    f.kc.set_panic_on_delete(&managed, false);
    assert!(
        f.kc.get(&oauth, &acct).is_none(),
        "the OAuth write was restored during unwinding"
    );
    assert_eq!(f.kc.get(&managed, &acct).unwrap(), b"sk-ant-api03-old");
}

#[test]
fn a_transient_unreadable_caller_read_is_ignored_in_favor_of_a_fresh_one() {
    let f = fx();
    let paths = CcPaths::resolve(&f.env);
    fs::write(&paths.global_config, "{}").unwrap();
    let svc = keychain_service(&f.env, ItemKind::OAuth);
    let acct = keychain_account(&f.env);
    let live_json = json!({"claudeAiOauth": {"refreshToken": "old"}, "mcpOAuth": {"m": 1}});
    f.kc.put(&svc, &acct, live_json.to_string().as_bytes());
    f.kc.set_unreadable(&svc, &acct, true);
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let locks = f.cc.lock_live(&f.env, &g).unwrap();
    let live = f.cc.read_live_auth(&f.env); // the caller's read: unreadable
    assert!(matches!(live.credential, Read::Unreadable(_)));
    f.kc.set_unreadable(&svc, &acct, false); // readable again by the time the write happens
    f.cc.write_credential(&f.env, &locks, &target(&f, "a@b.co", "rt"), &live)
        .unwrap();
    assert_eq!(
        oauth_item(&f).unwrap(),
        json!({"claudeAiOauth": {"accessToken": "at", "refreshToken": "rt"}, "mcpOAuth": {"m": 1}})
    );
}

#[test]
fn garbage_live_bytes_refuse_to_compose_and_write_nothing() {
    let f = fx();
    let paths = CcPaths::resolve(&f.env);
    fs::write(&paths.global_config, "{}").unwrap();
    let svc = keychain_service(&f.env, ItemKind::OAuth);
    let managed = keychain_service(&f.env, ItemKind::ManagedKey);
    let acct = keychain_account(&f.env);
    f.kc.put(&svc, &acct, b"not json at all");
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let locks = f.cc.lock_live(&f.env, &g).unwrap();
    let live = f.cc.read_live_auth(&f.env);
    let err =
        f.cc.write_credential(&f.env, &locks, &target(&f, "a@b.co", "rt"), &live)
            .err()
            .unwrap();
    // The exact fixed message `keep_shared` already uses (`live::UNPARSABLE_ENTRY`), which
    // is `pub(crate)` and so not nameable from this integration test crate; matched here by
    // its known text instead.
    assert!(
        matches!(&err, tagteam_provider::ProviderError::Invalid(msg) if msg == "a credential entry is not a JSON object"),
        "{err}"
    );
    assert_eq!(f.kc.get(&svc, &acct).unwrap(), b"not json at all");
    assert!(f.kc.get(&managed, &acct).is_none());
    assert_eq!(fs::read_to_string(&paths.global_config).unwrap(), "{}");
}

#[test]
fn an_empty_live_entry_refuses_to_compose_and_writes_nothing() {
    let f = fx();
    let paths = CcPaths::resolve(&f.env);
    fs::write(&paths.global_config, "{}").unwrap();
    let svc = keychain_service(&f.env, ItemKind::OAuth);
    let acct = keychain_account(&f.env);
    f.kc.put(&svc, &acct, b"");
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let locks = f.cc.lock_live(&f.env, &g).unwrap();
    let live = f.cc.read_live_auth(&f.env);
    assert!(
        f.cc.write_credential(&f.env, &locks, &target(&f, "a@b.co", "rt"), &live)
            .is_err()
    );
    assert_eq!(f.kc.get(&svc, &acct).unwrap(), b"");
}

#[test]
fn clear_other_axis_toward_oauth_clears_the_managed_key_and_its_undo_restores_it() {
    let f = fx();
    let paths = CcPaths::resolve(&f.env);
    fs::write(
        &paths.global_config,
        r#"{"primaryApiKey": "sk-ant-api03-old", "customApiKeyResponses": {"approved": ["x"]}}"#,
    )
    .unwrap();
    let acct = keychain_account(&f.env);
    let managed = keychain_service(&f.env, ItemKind::ManagedKey);
    f.kc.put(&managed, &acct, b"sk-ant-api03-old");
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let locks = f.cc.lock_live(&f.env, &g).unwrap();

    let undo = f.cc.clear_other_axis(&f.env, &locks, "oauth").unwrap();
    assert!(f.kc.get(&managed, &acct).is_none());
    assert!(
        !fs::read_to_string(&paths.global_config)
            .unwrap()
            .contains("primaryApiKey")
    );

    undo.undo(&locks).unwrap();
    assert_eq!(f.kc.get(&managed, &acct).unwrap(), b"sk-ant-api03-old");
    assert!(
        fs::read_to_string(&paths.global_config)
            .unwrap()
            .contains("sk-ant-api03-old")
    );
}

#[test]
fn clear_other_axis_toward_api_key_keeps_only_machine_shared_keys_and_its_undo_restores_the_entry()
{
    let f = fx();
    let paths = CcPaths::resolve(&f.env);
    fs::write(&paths.global_config, "{}").unwrap();
    let svc = keychain_service(&f.env, ItemKind::OAuth);
    let acct = keychain_account(&f.env);
    let full = json!({"claudeAiOauth": {"refreshToken": "r"}, "mcpOAuth": {"m": 1}});
    f.kc.put(&svc, &acct, full.to_string().as_bytes());
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let locks = f.cc.lock_live(&f.env, &g).unwrap();

    let undo = f.cc.clear_other_axis(&f.env, &locks, "api_key").unwrap();
    assert_eq!(oauth_item(&f).unwrap(), json!({"mcpOAuth": {"m": 1}}));

    undo.undo(&locks).unwrap();
    assert_eq!(oauth_item(&f).unwrap(), full);
}

#[test]
fn identity_surface_lists_every_macos_credential_and_managed_key_service() {
    let f = fx();
    let target_dir = f.env.home.join("real-profile");
    fs::create_dir_all(&target_dir).unwrap();
    let link = f.env.home.join("link-profile");
    std::os::unix::fs::symlink(&target_dir, &link).unwrap();
    let mut env = f.env.clone();
    env.claude_config_dir = Some(link.clone().into_os_string());

    let s = f.cc.identity_surface(&env);
    let acct = keychain_account(&env);
    let expected_oauth: Vec<_> = read_services(&env, ItemKind::OAuth)
        .into_iter()
        .map(|svc| (svc, acct.clone()))
        .collect();
    let expected_managed: Vec<_> = read_services(&env, ItemKind::ManagedKey)
        .into_iter()
        .map(|svc| (svc, acct.clone()))
        .collect();
    assert!(
        expected_oauth.len() > 1,
        "the fixture must actually exercise the symlinked-profile fallback"
    );
    assert_eq!(s.credential_items, expected_oauth);
    assert_eq!(s.owned_items, expected_managed);
}

#[test]
fn identity_surface_has_no_keychain_items_on_linux() {
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    fs::create_dir_all(env.home.join(".claude")).unwrap();
    let cc = ClaudeCode::with_store(LiveStore::new(
        Arc::new(FakeKeychain::new()),
        Platform::Linux,
    ));
    let s = cc.identity_surface(&env);
    assert!(s.credential_items.is_empty());
    assert!(s.owned_items.is_empty());
}

#[test]
fn identity_surface_names_the_section_3_writes() {
    let f = fx();
    let s = f.cc.identity_surface(&f.env);
    let paths = CcPaths::resolve(&f.env);
    assert_eq!(
        s.json_keys,
        vec![(
            paths.global_config,
            vec![
                "oauthAccount".into(),
                "primaryApiKey".into(),
                "customApiKeyResponses".into()
            ]
        )]
    );
    assert_eq!(s.credential_files, vec![paths.credentials_file]);
    assert_eq!(s.machine_shared_keys.len(), 5);
    assert_eq!(
        f.cc.identity_key(&f.cc.token_identity("a@b.co")).as_str(),
        "a@b.co\n"
    );
}
