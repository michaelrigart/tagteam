use std::fs;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tagteam_cc::endpoints::Endpoints;
use tagteam_cc::live::{LiveStore, Platform};
use tagteam_cc::provider::ClaudeCode;
use tagteam_cc::usage;
use tagteam_cc::{CcPaths, ItemKind, keychain_account, keychain_service, read_services};
use tagteam_core::Fingerprint;
use tagteam_provider::http::{HttpResponse, Method, ScriptedHttp};
use tagteam_provider::provider::TransientKind;
use tagteam_provider::{
    Capabilities, Credential, Env, FakeKeychain, KindTraits, LockError, MutationGuard, Pace,
    PollBudget, Provider, ProviderError, Read, SecretStore, StoredLogin, UsageResult,
};

/// A fallback hook for a test that saves nothing: every entry a fallback reports goes.
fn save_nothing(_: &[u8]) -> Result<(), ProviderError> {
    Ok(())
}

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
    let t = target(&f, "new@b.co", "rt-new");
    let written =
        f.cc.write_credential(&f.env, &locks, &t, &mut save_nothing)
            .unwrap();
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
    let key = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz";
    let t = StoredLogin {
        kind: "api_key".into(),
        secret: key.as_bytes().to_vec(),
        identity: f.cc.token_identity("api-key-2@token.local"),
    };
    let written =
        f.cc.write_credential(&f.env, &locks, &t, &mut save_nothing)
            .unwrap();
    assert_eq!(written.stored_in, SecretStore::Keychain);
    assert_eq!(
        f.kc.get(&keychain_service(&f.env, ItemKind::ManagedKey), &acct)
            .unwrap(),
        key.as_bytes()
    );
    assert_eq!(oauth_item(&f).unwrap(), json!({"pluginSecrets": {"p": 1}}));
    // Back to OAuth: the managed key goes, machine-shared keys stay.
    f.cc.write_credential(
        &f.env,
        &locks,
        &target(&f, "a@b.co", "rt"),
        &mut save_nothing,
    )
    .unwrap();
    assert!(
        f.kc.get(&keychain_service(&f.env, ItemKind::ManagedKey), &acct)
            .is_none()
    );
    assert_eq!(oauth_item(&f).unwrap()["pluginSecrets"], json!({"p": 1}));
}

/// Every entry a change can destroy, planted with a distinct secret, under an explicit
/// `CLAUDE_CONFIG_DIR=~/.claude` so each axis also has a fallback item (Appendix A.2).
fn plant_every_entry(f: &Fx) -> Env {
    let mut env = f.env.clone();
    env.claude_config_dir = Some(env.home.join(".claude").into_os_string());
    let paths = CcPaths::resolve(&env);
    let acct = keychain_account(&env);
    for (i, svc) in read_services(&env, ItemKind::OAuth).iter().enumerate() {
        let entry = json!({"claudeAiOauth": {"refreshToken": format!("rt-item-{i}")}, "mcpOAuth": {"m": 1}});
        f.kc.put(svc, &acct, entry.to_string().as_bytes());
    }
    let file = json!({"claudeAiOauth": {"refreshToken": "rt-file"}});
    fs::write(&paths.credentials_file, file.to_string()).unwrap();
    for (i, svc) in read_services(&env, ItemKind::ManagedKey).iter().enumerate() {
        f.kc.put(svc, &acct, format!("sk-ant-api03-item-{i}").as_bytes());
    }
    let config = json!({"primaryApiKey": "sk-ant-api03-plain"});
    fs::write(&paths.global_config, config.to_string()).unwrap();
    env
}

/// Every secret the planted entries hold now, by where it is.
fn secrets_by_place(f: &Fx, env: &Env) -> Vec<(String, Vec<u8>)> {
    let paths = CcPaths::resolve(env);
    let acct = keychain_account(env);
    let mut out: Vec<(String, Vec<u8>)> = [ItemKind::OAuth, ItemKind::ManagedKey]
        .into_iter()
        .flat_map(|kind| read_services(env, kind))
        .filter_map(|svc| f.kc.get(&svc, &acct).map(|b| (svc, b)))
        .collect();
    if let Ok(b) = fs::read(&paths.credentials_file) {
        out.push(("credentials file".into(), b));
    }
    if let Read::Present(Some(Value::String(k))) =
        tagteam_cc::config::get_key(&paths.global_config, "primaryApiKey")
    {
        out.push(("primaryApiKey".into(), k.into_bytes()));
    }
    out
}

#[test]
fn doomed_names_everything_each_change_destroys() {
    use tagteam_provider::LiveChange;
    let api_key = "sk-ant-api03-target-key-abcdefghijklmn";
    // (change, how the Keychain treats the write: 0 takes it, 1 refuses it, 2 was pinned to
    // the file by an earlier refusal)
    let cases: [(LiveChange, u8); 8] = [
        (LiveChange::Write("oauth"), 0),
        (LiveChange::Write("oauth"), 1),
        (LiveChange::Write("oauth"), 2),
        (LiveChange::Write("api_key"), 0),
        (LiveChange::Write("api_key"), 1),
        (LiveChange::ClearOther("oauth"), 0),
        (LiveChange::ClearOther("api_key"), 0),
        (LiveChange::ClearOther("setup_token"), 0),
    ];
    for (change, keychain) in cases {
        let f = fx();
        let env = plant_every_entry(&f);
        let g = MutationGuard::acquire(&env, Duration::from_secs(1)).unwrap();
        let locks = f.cc.lock_live(&env, &g).unwrap();
        let primary = |kind| keychain_service(&env, kind);
        if keychain == 2 {
            f.kc.set_fail_write(&primary(ItemKind::OAuth), true);
            f.cc.write_credential(
                &env,
                &locks,
                &target(&f, "p@x.co", "rt-p"),
                &mut save_nothing,
            )
            .unwrap();
            f.kc.set_fail_write(&primary(ItemKind::OAuth), false);
            drop(locks);
            drop(g);
            plant_every_entry(&f);
            check(&f, &env, change, keychain, api_key);
            continue;
        }
        drop(locks);
        drop(g);
        check(&f, &env, change, keychain, api_key);
    }

    fn check(f: &Fx, env: &Env, change: LiveChange, keychain: u8, api_key: &str) {
        let g = MutationGuard::acquire(env, Duration::from_secs(1)).unwrap();
        let locks = f.cc.lock_live(env, &g).unwrap();
        let before = secrets_by_place(f, env);
        let doomed = f.cc.doomed(env, &locks, change);
        let present = |fallback: bool| -> Vec<Vec<u8>> {
            doomed
                .iter()
                .filter(|d| d.on_fallback == fallback)
                .filter_map(|d| d.bytes.clone().present())
                .collect()
        };
        let (planned, conditional) = (present(false), present(true));
        let (refused, pinned) = (keychain == 1, keychain == 2);
        let mut reported: Vec<Vec<u8>> = Vec::new();
        match change {
            LiveChange::Write(kind) => {
                let item = if kind == "api_key" {
                    ItemKind::ManagedKey
                } else {
                    ItemKind::OAuth
                };
                f.kc.set_fail_write(&keychain_service(env, item), refused);
                let login = if kind == "api_key" {
                    StoredLogin {
                        kind: kind.into(),
                        secret: api_key.as_bytes().to_vec(),
                        identity: f.cc.token_identity("api-key-9@token.local"),
                    }
                } else {
                    target(f, "t@x.co", "rt-target")
                };
                let mut record = |b: &[u8]| {
                    reported.push(b.to_vec());
                    Ok(())
                };
                f.cc.write_credential(env, &locks, &login, &mut record)
                    .unwrap();
            }
            LiveChange::ClearOther(kind) => {
                f.cc.clear_other_axis(env, &locks, kind).unwrap();
            }
        }
        let after = secrets_by_place(f, env);
        let case = format!("{change:?}, keychain={keychain}");
        for (place, bytes) in &before {
            if !after.contains(&(place.clone(), bytes.clone())) {
                assert!(
                    planned.contains(bytes) || reported.contains(bytes),
                    "{case}: {place} was destroyed without being named"
                );
            }
        }
        for r in &reported {
            assert!(
                conditional.contains(r) || planned.contains(r),
                "{case}: reported an entry the plan did not name"
            );
        }
        for c in &conditional {
            let survived = after.iter().any(|(_, b)| b == c);
            assert_eq!(survived, !refused, "{case}: a conditional entry");
        }
        if !refused && !pinned {
            assert!(reported.is_empty(), "{case}: nothing falls back");
        }
    }
}

#[test]
fn doomed_reports_an_entry_it_cannot_read() {
    let f = fx();
    let env = plant_every_entry(&f);
    let fallback = &read_services(&env, ItemKind::OAuth)[1];
    f.kc.set_unreadable(fallback, &keychain_account(&env), true);
    let g = MutationGuard::acquire(&env, Duration::from_secs(1)).unwrap();
    let locks = f.cc.lock_live(&env, &g).unwrap();
    let doomed =
        f.cc.doomed(&env, &locks, tagteam_provider::LiveChange::Write("oauth"));
    assert!(
        doomed
            .iter()
            .any(|d| d.on_fallback && matches!(d.bytes, Read::Unreadable(_))),
        "{doomed:?}"
    );
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
    assert!(
        f.cc.write_credential(
            &f.env,
            &locks,
            &target(&f, "a@b.co", "rt"),
            &mut save_nothing
        )
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
    assert!(
        f.cc.write_credential(
            &f.env,
            &locks,
            &target(&f, "a@b.co", "rt"),
            &mut save_nothing
        )
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
    let err =
        f.cc.write_credential(
            &f.env,
            &locks,
            &target(&f, "a@b.co", "rt"),
            &mut save_nothing,
        )
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
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        f.cc.write_credential(
            &f.env,
            &locks,
            &target(&f, "a@b.co", "rt"),
            &mut save_nothing,
        )
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
    f.cc.write_credential(
        &f.env,
        &locks,
        &target(&f, "a@b.co", "rt"),
        &mut save_nothing,
    )
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
    let err =
        f.cc.write_credential(
            &f.env,
            &locks,
            &target(&f, "a@b.co", "rt"),
            &mut save_nothing,
        )
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
    assert!(
        f.cc.write_credential(
            &f.env,
            &locks,
            &target(&f, "a@b.co", "rt"),
            &mut save_nothing
        )
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

#[test]
fn claude_code_has_every_capability_and_the_kind_table() {
    let f = fx();
    assert_eq!(
        f.cc.capabilities(),
        Capabilities {
            usage: true,
            refresh: true,
            api_keys: true,
            sessions: true,
            statusline: true,
        }
    );
    for kind in f.cc.credential_kinds() {
        assert_eq!(f.cc.kind_traits(kind), tagteam_cc::shape::kind_traits(kind));
    }
    assert_eq!(
        f.cc.kind_traits("api_key"),
        KindTraits {
            refreshable: false,
            managed_key_axis: true,
            default_email_prefix: Some("api-key"),
            display: Some("api key"),
        }
    );
    assert!(f.cc.kind_traits("oauth").refreshable);
}

#[test]
fn access_token_facts_come_from_the_access_token_not_the_lineage() {
    let f = fx();
    let bytes = json!({"claudeAiOauth": {
        "accessToken": "at-1", "refreshToken": "rt-1", "expiresAt": 1_790_003_600_000i64
    }})
    .to_string()
    .into_bytes();
    assert_eq!(f.cc.access_expires_at(&bytes), Some(1_790_003_600_000));
    assert_eq!(
        f.cc.access_fingerprint(&bytes),
        Some(Fingerprint::of_secret(b"at-1"))
    );
    assert_ne!(f.cc.access_fingerprint(&bytes), f.cc.fingerprint(&bytes));
    // A setup token's lineage is its access token; an API key has no access token.
    let setup = tagteam_cc::shape::setup_token_credential("tok");
    assert_eq!(f.cc.access_fingerprint(&setup), f.cc.fingerprint(&setup));
    assert_eq!(f.cc.access_expires_at(&setup), None);
    assert_eq!(f.cc.access_fingerprint(b"sk-ant-api03-k"), None);
}

#[test]
fn claude_code_s_live_locks_share_the_nine_second_budget_of_section_9_1() {
    assert_eq!(fx().cc.live_lock_budget(), Duration::from_secs(9));
}

#[test]
fn the_stages_are_taken_separately_and_released_config_first() {
    let f = fx();
    let paths = CcPaths::resolve(&f.env);
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let cred =
        f.cc.lock_credentials(&f.env, &g, Duration::from_secs(1))
            .unwrap();
    assert!(paths.refresh_lock.is_dir() && paths.legacy_lock().is_dir());
    assert!(
        !paths.config_lock.exists(),
        "credential locks alone never take it"
    );
    let live =
        f.cc.lock_config(&f.env, cred, Duration::from_secs(1))
            .unwrap();
    assert!(paths.config_lock.is_dir());
    assert!(live.check_owned().is_ok());
    drop(live);
    assert!(
        !paths.refresh_lock.exists()
            && !paths.legacy_lock().exists()
            && !paths.config_lock.exists()
    );
}

#[test]
fn a_held_config_lock_releases_the_credential_locks_it_was_given() {
    let f = fx();
    let paths = CcPaths::resolve(&f.env);
    fs::create_dir(&paths.config_lock).unwrap(); // someone else holds it, freshly
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let cred =
        f.cc.lock_credentials(&f.env, &g, Duration::from_millis(500))
            .unwrap();
    let start = Instant::now();
    assert!(matches!(
        f.cc.lock_config(&f.env, cred, Duration::from_millis(500)),
        Err(ProviderError::Lock(LockError::Timeout(_)))
    ));
    assert!(start.elapsed() < Duration::from_millis(1500));
    assert!(!paths.refresh_lock.exists() && !paths.legacy_lock().exists());
    assert!(
        paths.config_lock.is_dir(),
        "the other holder's lock is left alone"
    );
}

/// §9.1: one budget covers both stages. The legacy lock is held for most of a 2 s budget and a
/// config lock for good, so the credential stage spends about 1.5 s. With one shared budget the
/// config stage gets what remains, and `lock_live` gives up by ~2.5 s. A fresh budget per
/// stage would take at least 3.5 s.
#[cfg(feature = "test-hooks")]
#[test]
fn lock_live_spends_one_budget_across_both_stages() {
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    fs::create_dir_all(env.home.join(".claude")).unwrap();
    let kc = Arc::new(FakeKeychain::new());
    let cc = ClaudeCode::with_store(
        LiveStore::new(kc, Platform::MacOs).with_retry_delay(Duration::ZERO),
    )
    .with_lock_timeout(Duration::from_secs(2));
    let paths = CcPaths::resolve(&env);
    fs::create_dir(&paths.config_lock).unwrap();
    fs::create_dir(paths.legacy_lock()).unwrap();
    let legacy = paths.legacy_lock();
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(1500));
        fs::remove_dir(legacy).unwrap();
    });
    let g = MutationGuard::acquire(&env, Duration::from_secs(1)).unwrap();
    let start = Instant::now();
    assert!(matches!(
        cc.lock_live(&env, &g),
        Err(ProviderError::Lock(LockError::Timeout(_)))
    ));
    let spent = start.elapsed();
    release.join().unwrap();
    assert!(spent < Duration::from_millis(3000), "{spent:?}");
    assert!(!paths.refresh_lock.exists() && !paths.legacy_lock().exists());
}

/// The recorded usage reply's body (`usage-200.json`, Appendix A.5).
fn usage_body() -> Value {
    let v: Value = serde_json::from_str(include_str!("fixtures/endpoints/usage-200.json")).unwrap();
    v["body"].clone()
}

/// An OAuth credential whose access token expired long ago: expiry is the engine's to act on.
fn with_access(at: &str) -> Credential {
    Credential::fresh(
        json!({"claudeAiOauth": {"accessToken": at, "refreshToken": "rt", "expiresAt": 1}})
            .to_string()
            .into_bytes(),
    )
}

#[test]
fn fetch_usage_sends_the_access_token_as_it_is_and_never_refreshes() {
    let f = fx();
    let http = ScriptedHttp::new();
    let e = Endpoints::production();
    http.push_json(Method::Get, &e.usage, 200, usage_body());
    assert_eq!(
        f.cc.fetch_usage(&http, &with_access("at-usage")),
        UsageResult::Windows(usage::normalize(&usage_body()).unwrap())
    );
    let sent = http.requests();
    assert_eq!(sent.len(), 1, "one usage request and no token request");
    assert_eq!(
        (sent[0].method, sent[0].url.as_str()),
        (Method::Get, e.usage.as_str())
    );
    assert_eq!(
        sent[0].headers,
        vec![
            ("authorization".to_owned(), "Bearer at-usage".to_owned()),
            ("anthropic-beta".to_owned(), "oauth-2025-04-20".to_owned()),
        ]
    );
}

#[test]
fn fetch_usage_sends_nothing_without_an_access_token() {
    let f = fx();
    let http = ScriptedHttp::new();
    for bytes in [
        b"sk-ant-api03-key".to_vec(),
        json!({"claudeAiOauth": {"refreshToken": "rt"}})
            .to_string()
            .into_bytes(),
        json!({"claudeAiOauth": {"accessToken": "", "refreshToken": ""}})
            .to_string()
            .into_bytes(),
        b"not json".to_vec(),
    ] {
        assert_eq!(
            f.cc.fetch_usage(&http, &Credential::fresh(bytes)),
            UsageResult::NoAccessToken
        );
    }
    assert!(http.requests().is_empty());
}

#[test]
fn a_setup_token_is_fetched_like_any_access_token() {
    // Decision 11: whether the endpoint accepts a `user:inference`-only token is unrecorded,
    // so it is asked, and a refusal is an ordinary verdict.
    let f = fx();
    let http = ScriptedHttp::new();
    http.push_json(Method::Get, &Endpoints::production().usage, 401, json!({}));
    let setup = Credential::fresh(tagteam_cc::shape::setup_token_credential(
        "sk-ant-oat01-setup",
    ));
    assert_eq!(f.cc.fetch_usage(&http, &setup), UsageResult::Unauthorized);
    assert_eq!(
        http.requests()[0].headers[0],
        (
            "authorization".to_owned(),
            "Bearer sk-ant-oat01-setup".to_owned()
        )
    );
}

#[test]
fn a_rate_limited_fetch_carries_its_retry_after() {
    let f = fx();
    let http = ScriptedHttp::new();
    http.push(
        Method::Get,
        &Endpoints::production().usage,
        Ok(HttpResponse {
            status: 429,
            headers: vec![("retry-after".into(), "90".into())],
            body: vec![],
        }),
    );
    assert_eq!(
        f.cc.fetch_usage(&http, &with_access("at")),
        UsageResult::Failed {
            kind: TransientKind::Http(429),
            retry_after_s: Some(90.0)
        }
    );
}

#[test]
fn claude_code_s_budget_rendering_and_live_identity_source() {
    let f = fx();
    assert_eq!(f.cc.poll_budget(), PollBudget::STANDARD);
    assert_eq!(
        f.cc.live_identity_source(&f.env),
        Some(f.env.home.join(".claude.json"))
    );
    let mut env = f.env.clone();
    let dir = f.env.home.join("cfg");
    env.claude_config_dir = Some(dir.clone().into_os_string());
    assert_eq!(
        f.cc.live_identity_source(&env),
        Some(CcPaths::resolve(&env).global_config),
        "the file live_identity reads"
    );
    assert_eq!(
        f.cc.live_identity_source(&env),
        Some(dir.join(".claude.json"))
    );
    let windows: Vec<_> = usage::normalize(&usage_body())
        .unwrap()
        .into_iter()
        .map(|w| (w, Pace::default()))
        .collect();
    assert_eq!(f.cc.render_usage(&windows)["sevenDay"]["pct"], json!(77.0));
}
