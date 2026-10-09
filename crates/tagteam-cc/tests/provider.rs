use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tagteam_cc::endpoints::Endpoints;
use tagteam_cc::live::{LiveStore, Platform};
use tagteam_cc::provider::ClaudeCode;
use tagteam_cc::usage;
use tagteam_cc::{CcPaths, ItemKind, keychain_account, keychain_service};
use tagteam_core::Fingerprint;
use tagteam_core::usage::WindowKind;
use tagteam_provider::http::{HttpResponse, Method, ScriptedHttp};
use tagteam_provider::provider::TransientKind;
use tagteam_provider::{
    Capabilities, Credential, Env, FakeKeychain, Keychain, KeychainError, KindTraits, LiveLocks,
    LockError, LockState, MutationGuard, Pace, PollBudget, Provider, ProviderError, Read,
    SecretStore, StoredLogin, UsageResult, Window,
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

/// The unsuffixed items an explicit `CLAUDE_CONFIG_DIR=~/.claude` fell back to before Claude
/// Code 2.1.286. Inert now (Appendix A.2): nothing reads, writes or clears them.
const INERT_OAUTH: &str = "Claude Code-credentials";
const INERT_MANAGED: &str = "Claude Code";

/// `"<prefix>-" + hex(sha256(dir))[..8]`: Appendix A.2's name, computed here independently of
/// `keychain_service`.
fn hashed(prefix: &str, dir: &str) -> String {
    use sha2::{Digest, Sha256};
    format!(
        "{prefix}-{}",
        &hex::encode(Sha256::digest(dir.as_bytes()).as_slice())[..8]
    )
}

/// Every entry a change can destroy, planted with a distinct secret, under an explicit
/// `CLAUDE_CONFIG_DIR=~/.claude`, beside the two inert former fallback items (Appendix A.2).
fn plant_every_entry(f: &Fx) -> Env {
    let mut env = f.env.clone();
    env.claude_config_dir = Some(env.home.join(".claude").into_os_string());
    let paths = CcPaths::resolve(&env);
    let acct = keychain_account(&env);
    let entry = json!({"claudeAiOauth": {"refreshToken": "rt-item"}, "mcpOAuth": {"m": 1}});
    f.kc.put(
        &keychain_service(&env, ItemKind::OAuth),
        &acct,
        entry.to_string().as_bytes(),
    );
    let inert = json!({"claudeAiOauth": {"refreshToken": "rt-inert"}, "mcpOAuth": {"m": 2}});
    f.kc.put(INERT_OAUTH, &acct, inert.to_string().as_bytes());
    let file = json!({"claudeAiOauth": {"refreshToken": "rt-file"}});
    fs::write(&paths.credentials_file, file.to_string()).unwrap();
    f.kc.put(
        &keychain_service(&env, ItemKind::ManagedKey),
        &acct,
        b"sk-ant-api03-item",
    );
    f.kc.put(INERT_MANAGED, &acct, b"sk-ant-api03-inert");
    let config = json!({"primaryApiKey": "sk-ant-api03-plain"});
    fs::write(&paths.global_config, config.to_string()).unwrap();
    env
}

/// Every secret the planted entries hold now, by where it is, the inert items included.
fn secrets_by_place(f: &Fx, env: &Env) -> Vec<(String, Vec<u8>)> {
    let paths = CcPaths::resolve(env);
    let acct = keychain_account(env);
    let mut out: Vec<(String, Vec<u8>)> = [
        keychain_service(env, ItemKind::OAuth),
        keychain_service(env, ItemKind::ManagedKey),
        INERT_OAUTH.to_owned(),
        INERT_MANAGED.to_owned(),
    ]
    .into_iter()
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
    // the file by an earlier refusal in the same operation)
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
        if keychain == 2 {
            // The pin lasts one operation (Appendix A.3), so the write that sets it runs under
            // the same live locks as the change being checked.
            let primary = keychain_service(&env, ItemKind::OAuth);
            f.kc.set_fail_write(&primary, true);
            f.cc.write_credential(
                &env,
                &locks,
                &target(&f, "p@x.co", "rt-p"),
                &mut save_nothing,
            )
            .unwrap();
            f.kc.set_fail_write(&primary, false);
            plant_every_entry(&f);
        }
        check(&f, &env, &locks, change, keychain, api_key);
    }

    fn check(
        f: &Fx,
        env: &Env,
        locks: &LiveLocks<'_>,
        change: LiveChange,
        keychain: u8,
        api_key: &str,
    ) {
        let acct = keychain_account(env);
        let inert_before = (f.kc.get(INERT_OAUTH, &acct), f.kc.get(INERT_MANAGED, &acct));
        let before = secrets_by_place(f, env);
        let doomed = f.cc.doomed(env, locks, change);
        let planned: Vec<Vec<u8>> = doomed
            .iter()
            .filter_map(|d| d.bytes.clone().present())
            .collect();
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
                f.cc.write_credential(env, locks, &login, &mut record)
                    .unwrap();
            }
            LiveChange::ClearOther(kind) => {
                f.cc.clear_other_axis(env, locks, kind).unwrap();
            }
        }
        let after = secrets_by_place(f, env);
        let case = format!("{change:?}, keychain={keychain}");
        for (place, bytes) in &before {
            if !after.contains(&(place.clone(), bytes.clone())) {
                assert!(
                    planned.contains(bytes),
                    "{case}: {place} was destroyed without being named"
                );
            }
        }
        for r in &reported {
            assert!(
                planned.contains(r),
                "{case}: a fallback reported an entry the plan did not name"
            );
        }
        assert_eq!(
            (f.kc.get(INERT_OAUTH, &acct), f.kc.get(INERT_MANAGED, &acct)),
            inert_before,
            "{case}: an inert former fallback item was touched"
        );
        for inert in [&inert_before.0, &inert_before.1].into_iter().flatten() {
            assert!(!planned.contains(inert), "{case}: an inert item was named");
        }
        if !refused && !pinned {
            assert!(reported.is_empty(), "{case}: nothing falls back");
        }
        if pinned {
            assert!(
                !reported.is_empty(),
                "{case}: a pinned write goes to the file and reports the item it deletes"
            );
        }
    }
}

#[test]
fn doomed_reports_an_unreadable_item_and_never_reads_an_inert_one() {
    let f = fx();
    let env = plant_every_entry(&f);
    let acct = keychain_account(&env);
    f.kc.set_unreadable(INERT_OAUTH, &acct, true);
    f.kc.set_unreadable(INERT_MANAGED, &acct, true);
    let g = MutationGuard::acquire(&env, Duration::from_secs(1)).unwrap();
    let locks = f.cc.lock_live(&env, &g).unwrap();
    let change = tagteam_provider::LiveChange::Write("oauth");
    let doomed = f.cc.doomed(&env, &locks, change);
    assert!(
        doomed
            .iter()
            .all(|d| !matches!(d.bytes, Read::Unreadable(_))),
        "an inert item is never read: {doomed:?}"
    );
    f.kc.set_unreadable(&keychain_service(&env, ItemKind::OAuth), &acct, true);
    let doomed = f.cc.doomed(&env, &locks, change);
    assert!(
        doomed
            .iter()
            .any(|d| matches!(d.bytes, Read::Unreadable(_))),
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
fn identity_surface_lists_the_one_item_per_axis_of_the_exported_spelling() {
    // Appendix A.2 (2.1.286): a symlinked config dir is named by the link's spelling alone,
    // never also by its target's.
    let f = fx();
    let target_dir = f.env.home.join("real-profile");
    fs::create_dir_all(&target_dir).unwrap();
    let link = f.env.home.join("link-profile");
    std::os::unix::fs::symlink(&target_dir, &link).unwrap();
    let mut env = f.env.clone();
    env.claude_config_dir = Some(link.clone().into_os_string());

    let s = f.cc.identity_surface(&env);
    let acct = keychain_account(&env);
    let spelling = link.to_str().unwrap();
    assert_eq!(
        s.credential_items,
        vec![(hashed("Claude Code-credentials", spelling), acct.clone())]
    );
    assert_eq!(s.owned_items, vec![(hashed("Claude Code", spelling), acct)]);
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
    assert_eq!(
        s.create_only,
        vec![
            paths.config_home.join("projects"),
            paths.config_home.join("history.jsonl")
        ],
        "§3's create-only row: the must-share entries, in the config home"
    );
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

/// §14.1: a lock wait is a cancellation point, and unwinding releases what is held: the
/// credential locks `lock_config` was given are removed with the interrupted config wait. No
/// clock is read, so a loaded machine cannot fail it: a wait that is not interrupted ends in
/// `Timeout`, and an attempt made before the token was checked takes the free lock.
#[test]
fn an_interrupted_config_wait_releases_the_credential_locks_it_was_given() {
    for held in [true, false] {
        let f = fx();
        let paths = CcPaths::resolve(&f.env);
        if held {
            fs::create_dir(&paths.config_lock).unwrap(); // someone else holds it, freshly
        }
        let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
        let cred =
            f.cc.lock_credentials(&f.env, &g, Duration::from_secs(1))
                .unwrap();
        f.env.cancel.request(libc::SIGTERM);
        match f.cc.lock_config(&f.env, cred, Duration::from_secs(5)) {
            Err(ProviderError::Lock(LockError::Interrupted { path, signal })) => {
                assert_eq!((path, signal), (paths.config_lock.clone(), libc::SIGTERM))
            }
            other => panic!(
                "held {held}: expected an interrupted wait, got {:?}",
                other.err()
            ),
        }
        assert!(
            !paths.refresh_lock.exists() && !paths.legacy_lock().exists(),
            "held {held}: the credential locks are released"
        );
        assert_eq!(
            paths.config_lock.is_dir(),
            held,
            "held {held}: the other holder's lock is left alone, and a free one never taken: \
             the token is checked before the first attempt"
        );
    }
}

/// L356, Review Focus 5: tagteam was suspended past the staleness window, and Claude Code took
/// the refresh lock over with a directory that carries tagteam's last mtime. The write the lock
/// protects is refused, and releasing tagteam's locks leaves CC's directory where it is.
#[test]
fn a_replaced_lock_directory_aborts_the_write_and_is_left_alone() {
    let f = fx();
    let paths = CcPaths::resolve(&f.env);
    let svc = keychain_service(&f.env, ItemKind::OAuth);
    let acct = keychain_account(&f.env);
    let live = br#"{"claudeAiOauth":{"refreshToken":"old"}}"#;
    f.kc.put(&svc, &acct, live);
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let locks = f.cc.lock_live(&f.env, &g).unwrap();
    let ours = fs::metadata(&paths.refresh_lock)
        .unwrap()
        .modified()
        .unwrap();
    fs::remove_dir(&paths.refresh_lock).unwrap();
    fs::create_dir(&paths.refresh_lock).unwrap(); // CC's
    fs::File::open(&paths.refresh_lock)
        .unwrap()
        .set_modified(ours)
        .unwrap();

    let t = target(&f, "new@b.co", "rt-new");
    assert!(matches!(
        f.cc.write_credential(&f.env, &locks, &t, &mut save_nothing),
        Err(ProviderError::Lock(LockError::Compromised(_)))
    ));
    assert_eq!(f.kc.get(&svc, &acct).unwrap(), live, "nothing was written");
    drop(locks);
    assert!(paths.refresh_lock.is_dir(), "CC's directory is left alone");
    assert!(
        !paths.legacy_lock().exists() && !paths.config_lock.exists(),
        "tagteam's own locks are released"
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

#[test]
fn claude_code_describes_its_window_keys_as_it_normalizes_them() {
    // §13.4: `history` asks the provider to describe a window that a stored sample names but
    // the last reading lacks. Each key reads as §8.2's normalization builds it, minus a reading.
    let f = fx();
    let bare = |key: &str, label: &str, kind, period_s| {
        Some(Window {
            key: key.into(),
            label: label.into(),
            kind,
            pct: 0.0,
            resets_at: None,
            period_s,
            detail: None,
        })
    };
    let described = |key: &str| f.cc.describe_window(key);
    assert_eq!(
        described("5h"),
        bare("5h", "5h", WindowKind::Short, Some(18_000))
    );
    assert_eq!(
        described("7d"),
        bare("7d", "7d", WindowKind::Long, Some(604_800))
    );
    assert_eq!(
        described("spend"),
        bare("spend", "spend", WindowKind::Spend, None)
    );
    assert_eq!(
        described("scoped:Fable"),
        bare("scoped:Fable", "Fable", WindowKind::Scoped, None),
        "the key does not say its limit's group, so it has no period"
    );
    for key in ["", "5H", "1h", "scoped:", "seven_day", "Fable", "daily"] {
        assert_eq!(described(key), None, "{key:?}");
    }
    for w in usage::normalize(&usage_body()).unwrap() {
        let d = described(&w.key).unwrap();
        assert_eq!((&d.key, &d.label, d.kind), (&w.key, &w.label, w.kind));
        if w.kind != WindowKind::Scoped {
            assert_eq!(d.period_s, w.period_s, "{}", w.key);
        }
    }
}

#[test]
fn consume_first_ranks_on_the_key_claude_code_normalizes_as_its_long_window() {
    // §4.5: CC's primary long window is "7d". It is the key §8.2's normalization gives the
    // `Long` window, and the key `describe_window` describes as one, so the two cannot drift.
    let f = fx();
    assert_eq!(f.cc.primary_long_window(), Some("7d"));
    let key = f.cc.primary_long_window().unwrap();
    let long: Vec<String> = usage::normalize(&usage_body())
        .unwrap()
        .into_iter()
        .filter(|w| w.kind == WindowKind::Long)
        .map(|w| w.key)
        .collect();
    assert_eq!(long, [key]);
    assert_eq!(
        f.cc.describe_window(key).map(|w| (w.kind, w.period_s)),
        Some((WindowKind::Long, Some(604_800)))
    );
}

/// A live OAuth login in the Keychain, as CC leaves it; returns the item's (service, account).
fn keychain_login(f: &Fx) -> (String, String) {
    let svc = keychain_service(&f.env, ItemKind::OAuth);
    let acct = keychain_account(&f.env);
    f.kc.put(
        &svc,
        &acct,
        json!({"claudeAiOauth": {"refreshToken": "rt-live"}})
            .to_string()
            .as_bytes(),
    );
    (svc, acct)
}

#[test]
fn a_fallback_pins_the_file_for_the_rest_of_its_operation() {
    // Appendix A.3: once a write falls back, every later write under the same live locks goes
    // to the file too, even with the Keychain healthy again.
    let f = fx();
    let paths = CcPaths::resolve(&f.env);
    let (svc, acct) = keychain_login(&f);
    let fell_back = SecretStore::Fallback(paths.credentials_file.clone());
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let locks = f.cc.lock_live(&f.env, &g).unwrap();
    f.kc.set_fail_write(&svc, true);
    let first =
        f.cc.write_credential(
            &f.env,
            &locks,
            &target(&f, "a@b.co", "rt-1"),
            &mut save_nothing,
        )
        .unwrap();
    assert_eq!(first.stored_in, fell_back);
    f.kc.set_fail_write(&svc, false);
    let second =
        f.cc.write_credential(
            &f.env,
            &locks,
            &target(&f, "a@b.co", "rt-2"),
            &mut save_nothing,
        )
        .unwrap();
    assert_eq!(second.stored_in, fell_back);
    assert_eq!(
        f.kc.get(&svc, &acct),
        None,
        "the Keychain item stays deleted"
    );
    let file: Value = serde_json::from_slice(&fs::read(&paths.credentials_file).unwrap()).unwrap();
    assert_eq!(file["claudeAiOauth"]["refreshToken"], json!("rt-2"));
}

#[test]
fn the_next_operation_tries_the_keychain_again() {
    // Appendix A.3 (L396): releasing the live locks ends the operation that fell back, and
    // with it the pin. A long-lived process (`auto`, the daemon) must not stay on the file.
    let f = fx();
    let paths = CcPaths::resolve(&f.env);
    let (svc, acct) = keychain_login(&f);
    {
        let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
        let locks = f.cc.lock_live(&f.env, &g).unwrap();
        f.kc.set_fail_write(&svc, true);
        let written =
            f.cc.write_credential(
                &f.env,
                &locks,
                &target(&f, "a@b.co", "rt-1"),
                &mut save_nothing,
            )
            .unwrap();
        assert_eq!(
            written.stored_in,
            SecretStore::Fallback(paths.credentials_file.clone())
        );
    }
    f.kc.set_fail_write(&svc, false);
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let locks = f.cc.lock_live(&f.env, &g).unwrap();
    let written =
        f.cc.write_credential(
            &f.env,
            &locks,
            &target(&f, "b@b.co", "rt-2"),
            &mut save_nothing,
        )
        .unwrap();
    assert_eq!(written.stored_in, SecretStore::Keychain);
    let item = f.kc.get(&svc, &acct).unwrap();
    assert_eq!(
        oauth_item(&f).unwrap()["claudeAiOauth"]["refreshToken"],
        json!("rt-2")
    );
    assert_eq!(
        fs::read(&paths.credentials_file).unwrap(),
        item,
        "the file the fallback created is rewritten with the item's bytes, for hot reload"
    );
}

#[test]
fn a_rollback_clears_the_pin_within_its_operation() {
    // Appendix A.3: the rollback puts the Keychain item back, so a later write of the same
    // operation must not stay on the file, where the restored item would shadow it.
    let f = fx();
    let paths = CcPaths::resolve(&f.env);
    let (svc, acct) = keychain_login(&f);
    let before = f.kc.get(&svc, &acct).unwrap();
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let locks = f.cc.lock_live(&f.env, &g).unwrap();
    f.kc.set_fail_write(&svc, true);
    let written =
        f.cc.write_credential(
            &f.env,
            &locks,
            &target(&f, "a@b.co", "rt-1"),
            &mut save_nothing,
        )
        .unwrap();
    assert_eq!(
        written.stored_in,
        SecretStore::Fallback(paths.credentials_file.clone())
    );
    f.kc.set_fail_write(&svc, false);
    written.undo.undo(&locks).unwrap();
    assert_eq!(
        f.kc.get(&svc, &acct).unwrap(),
        before,
        "the rollback put the item back"
    );
    assert!(
        !paths.credentials_file.exists(),
        "and removed the fallback's file"
    );
    let again =
        f.cc.write_credential(
            &f.env,
            &locks,
            &target(&f, "a@b.co", "rt-2"),
            &mut save_nothing,
        )
        .unwrap();
    assert_eq!(again.stored_in, SecretStore::Keychain);
    assert_eq!(
        oauth_item(&f).unwrap()["claudeAiOauth"]["refreshToken"],
        json!("rt-2")
    );
    assert!(
        !paths.credentials_file.exists(),
        "a Keychain write never creates the file"
    );
}

/// Wraps a `FakeKeychain` and records, at every `upsert`/`delete`, whether CC's storage-write
/// lock, refresh lock, legacy lock and config lock were each held at that moment (§4.3, §9.1).
struct HeldLocksProbe {
    inner: Arc<FakeKeychain>,
    locks: [PathBuf; 5],
    writes: Mutex<Vec<(String, [bool; 5])>>,
}

impl HeldLocksProbe {
    fn record(&self, s: &str) {
        let held = self.locks.clone().map(|l| l.is_dir());
        self.writes.lock().unwrap().push((s.to_owned(), held));
    }
}

impl Keychain for HeldLocksProbe {
    fn find(&self, s: &str, a: &str) -> Read<Vec<u8>> {
        self.inner.find(s, a)
    }
    fn exists(&self, s: &str, a: &str) -> Read<()> {
        self.inner.exists(s, a)
    }
    fn upsert(&self, s: &str, a: &str, d: &[u8]) -> Result<(), KeychainError> {
        self.record(s);
        self.inner.upsert(s, a, d)
    }
    fn delete(&self, s: &str, a: &str) -> Result<(), KeychainError> {
        self.record(s);
        self.inner.delete(s, a)
    }
    fn lock_state(&self) -> LockState {
        self.inner.lock_state()
    }
    fn unlock(&self) -> bool {
        self.inner.unlock()
    }
}

/// `fx()`, with the provider's Keychain behind a `HeldLocksProbe`.
fn probed_fx() -> (Fx, Arc<HeldLocksProbe>) {
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    fs::create_dir_all(env.home.join(".claude")).unwrap();
    let paths = CcPaths::resolve(&env);
    let kc = Arc::new(FakeKeychain::new());
    let probe = Arc::new(HeldLocksProbe {
        inner: kc.clone(),
        locks: [
            paths.storage_write_lock.clone(),
            paths.storage_write_lock_v2.clone(),
            paths.refresh_lock.clone(),
            paths.legacy_lock(),
            paths.config_lock,
        ],
        writes: Mutex::new(Vec::new()),
    });
    let cc = ClaudeCode::with_store(
        LiveStore::new(probe.clone(), Platform::MacOs).with_retry_delay(Duration::ZERO),
    );
    (Fx { _d: d, env, kc, cc }, probe)
}

#[test]
fn the_storage_write_lock_is_a_leaf_taken_only_around_each_entry_write() {
    // §4.3, §9.1: taken only while CC's live locks are held, never by the lock stages
    // themselves, and released after each credential entry's write, a rollback's included.
    let (f, probe) = probed_fx();
    let paths = CcPaths::resolve(&f.env);
    fs::write(&paths.global_config, "{}").unwrap();
    f.kc.put(
        &keychain_service(&f.env, ItemKind::OAuth),
        &keychain_account(&f.env),
        br#"{"claudeAiOauth":{"refreshToken":"old"},"mcpOAuth":{"m":1}}"#,
    );
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let cred =
        f.cc.lock_credentials(&f.env, &g, Duration::from_secs(1))
            .unwrap();
    assert!(
        !paths.storage_write_lock.exists() && !paths.storage_write_lock_v2.exists(),
        "the credential locks never take it"
    );
    let locks =
        f.cc.lock_config(&f.env, cred, Duration::from_secs(1))
            .unwrap();
    assert!(
        !paths.storage_write_lock.exists() && !paths.storage_write_lock_v2.exists(),
        "nor does the config lock"
    );

    let key = StoredLogin {
        kind: "api_key".into(),
        secret: b"sk-ant-api03-abcdefghijklmnopqrstuvwxyz".to_vec(),
        identity: f.cc.token_identity("api-key-1@token.local"),
    };
    let undo =
        f.cc.write_credential(&f.env, &locks, &key, &mut save_nothing)
            .unwrap()
            .undo;
    assert!(!paths.storage_write_lock.exists() && !paths.storage_write_lock_v2.exists());
    undo.undo(&locks).unwrap();
    assert!(!paths.storage_write_lock.exists() && !paths.storage_write_lock_v2.exists());
    f.cc.write_credential(
        &f.env,
        &locks,
        &target(&f, "new@b.co", "rt-new"),
        &mut save_nothing,
    )
    .unwrap();
    assert!(!paths.storage_write_lock.exists() && !paths.storage_write_lock_v2.exists());
    drop(locks);

    let writes = probe.writes.lock().unwrap().clone();
    assert!(writes.len() >= 4, "{writes:?}");
    assert!(
        writes.iter().all(|(_, held)| *held == [true; 5]),
        "a credential entry written without both storage-write locks or the live locks: {writes:?}"
    );
}

#[test]
fn a_write_that_finds_cc_changed_the_entry_aborts_and_leaves_everything_as_cc_left_it() {
    // §9.1: while tagteam waits for its storage-write lock, CC changes the entry's
    // account-scoped keys by more than a dead-token marking. tagteam's write aborts; having
    // written nothing, it restores nothing either, so the error is the abort itself, not a
    // failed restore.
    let f = fx();
    let paths = CcPaths::resolve(&f.env);
    fs::write(&paths.global_config, "{}").unwrap();
    let svc = keychain_service(&f.env, ItemKind::OAuth);
    let acct = keychain_account(&f.env);
    f.kc.put(
        &svc,
        &acct,
        br#"{"claudeAiOauth":{"refreshToken":"old"},"mcpOAuth":{"m":1}}"#,
    );
    let changed: &[u8] = br#"{"claudeAiOauth":{"accessToken":"","refreshToken":"","expiresAt":0},"trustedDeviceToken":"cc-device","mcpOAuth":{"m":1}}"#;
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let locks = f.cc.lock_live(&f.env, &g).unwrap();
    fs::create_dir(&paths.storage_write_lock).unwrap(); // CC takes it
    let (kc, cc_svc, cc_acct, lock) = (
        f.kc.clone(),
        svc.clone(),
        acct.clone(),
        paths.storage_write_lock.clone(),
    );
    let cc = thread::spawn(move || {
        thread::sleep(Duration::from_millis(300));
        kc.put(&cc_svc, &cc_acct, changed);
        fs::remove_dir(&lock).unwrap();
    });

    let err =
        f.cc.write_credential(
            &f.env,
            &locks,
            &target(&f, "new@b.co", "rt-new"),
            &mut save_nothing,
        )
        .err()
        .unwrap();
    cc.join().unwrap();

    assert!(
        matches!(&err, ProviderError::EntryMoved(name) if *name == svc),
        "{err}"
    );
    assert_eq!(f.kc.get(&svc, &acct).unwrap(), changed, "CC's write stands");
    assert!(
        f.kc.get(&keychain_service(&f.env, ItemKind::ManagedKey), &acct)
            .is_none()
    );
    assert!(!paths.credentials_file.exists());
    assert_eq!(fs::read_to_string(&paths.global_config).unwrap(), "{}");
    assert!(!paths.storage_write_lock.exists());
}

#[test]
fn the_keychain_lock_state_is_the_one_cc_reads_its_stores_from_and_none_off_macos() {
    let kc = Arc::new(FakeKeychain::new());
    let mac = ClaudeCode::new(kc.clone(), Platform::MacOs);
    assert_eq!(mac.keychain_lock_state(), Some(LockState::Unlocked));
    kc.set_locked(true);
    assert_eq!(mac.keychain_lock_state(), Some(LockState::Locked));
    let linux = ClaudeCode::new(kc, Platform::Linux);
    assert_eq!(
        linux.keychain_lock_state(),
        None,
        "CC uses no Keychain off macOS"
    );
}
