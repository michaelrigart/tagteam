use std::fs;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tagteam_cc::live::{LiveStore, Platform};
use tagteam_cc::{CcPaths, ItemKind, config, keychain_account, keychain_service, read_services};
use tagteam_provider::{
    Env, FakeKeychain, LiveLockSet, LiveLocks, LockError, MutationGuard, Provenance, ProviderError,
    Read, Undo,
};

struct Fx {
    _dir: tempfile::TempDir,
    env: Env,
    paths: CcPaths,
    kc: Arc<FakeKeychain>,
}

fn fx() -> Fx {
    fx_with(|_| {})
}

fn fx_with(adjust: impl FnOnce(&mut Env)) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let mut env = Env::for_test(dir.path());
    adjust(&mut env);
    fs::create_dir_all(env.home.join(".claude")).unwrap();
    let paths = CcPaths::resolve(&env);
    Fx {
        _dir: dir,
        env,
        paths,
        kc: Arc::new(FakeKeychain::new()),
    }
}

fn store(f: &Fx, p: Platform) -> LiveStore {
    LiveStore::new(f.kc.clone(), p).with_retry_delay(Duration::ZERO)
}

/// A fence that always passes: these tests hold no CC locks.
fn open() -> Result<(), ProviderError> {
    Ok(())
}

/// A lock set that is always owned, for undo calls in these tests.
struct Held;

impl LiveLockSet for Held {
    fn check_owned(&self) -> Result<(), LockError> {
        Ok(())
    }
}

fn oauth_svc(f: &Fx) -> (String, String) {
    (
        keychain_service(&f.env, ItemKind::OAuth),
        keychain_account(&f.env),
    )
}

fn json_of(b: &[u8]) -> Value {
    serde_json::from_slice(b).unwrap()
}

#[test]
fn mac_reads_the_keychain_first_then_the_file() {
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    assert!(matches!(s.read_credential(&f.env, &f.paths), Read::Absent));
    fs::write(&f.paths.credentials_file, "file").unwrap();
    let c = s.read_credential(&f.env, &f.paths).present().unwrap();
    assert_eq!(
        (c.bytes(), c.provenance()),
        (&b"file"[..], Provenance::Fresh)
    );
    f.kc.put(&svc, &acct, b"kc");
    assert_eq!(
        s.read_credential(&f.env, &f.paths)
            .present()
            .unwrap()
            .bytes(),
        b"kc"
    );
}

#[test]
fn a_failed_keychain_read_covered_by_the_file_is_degraded() {
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, b"kc");
    f.kc.set_unreadable(&svc, &acct, true);
    assert!(matches!(
        s.read_credential(&f.env, &f.paths),
        Read::Unreadable(_)
    ));
    fs::write(&f.paths.credentials_file, "file").unwrap();
    assert_eq!(
        s.read_credential(&f.env, &f.paths)
            .present()
            .unwrap()
            .provenance(),
        Provenance::Degraded
    );
}

#[test]
fn an_unreadable_primary_item_is_never_skipped_for_a_fallback() {
    let f = fx_with(|e| e.claude_config_dir = Some(e.home.join(".claude").into_os_string()));
    let s = store(&f, Platform::MacOs);
    let acct = keychain_account(&f.env);
    for kind in [ItemKind::OAuth, ItemKind::ManagedKey] {
        let services = read_services(&f.env, kind);
        f.kc.put(&services[0], &acct, b"newer");
        f.kc.set_unreadable(&services[0], &acct, true);
        f.kc.put(&services[1], &acct, b"older");
    }
    assert!(matches!(
        s.read_credential(&f.env, &f.paths),
        Read::Unreadable(_)
    ));
    assert!(matches!(
        s.read_managed_key(&f.env, &f.paths),
        Read::Unreadable(_)
    ));
    fs::write(&f.paths.credentials_file, "file").unwrap();
    assert_eq!(
        s.read_credential(&f.env, &f.paths)
            .present()
            .unwrap()
            .provenance(),
        Provenance::Degraded
    );
}

#[test]
fn a_lock_lost_before_publication_publishes_nothing() {
    let f = fx();
    fs::write(&f.paths.global_config, "{\"a\": 1}").unwrap();
    let lost = || Err(ProviderError::Lock(LockError::Compromised("x".into())));
    assert!(
        config::splice_key(
            &f.paths.global_config,
            "oauthAccount",
            Some(&json!({})),
            &lost
        )
        .is_err()
    );
    assert_eq!(fs::read(&f.paths.global_config).unwrap(), b"{\"a\": 1}");
}

#[test]
fn linux_reads_and_writes_only_the_file() {
    let f = fx();
    let s = store(&f, Platform::Linux);
    s.write_credential_entry(&f.env, &f.paths, b"{\"a\":1}", &open)
        .unwrap();
    assert_eq!(fs::read(&f.paths.credentials_file).unwrap(), b"{\"a\":1}");
    assert!(f.kc.items().is_empty());
    assert_eq!(
        s.read_credential(&f.env, &f.paths)
            .present()
            .unwrap()
            .bytes(),
        b"{\"a\":1}"
    );
}

#[test]
fn a_keychain_write_bumps_an_existing_file_but_never_creates_one() {
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    s.write_credential_entry(&f.env, &f.paths, b"v1", &open)
        .unwrap();
    assert_eq!(f.kc.get(&svc, &acct).unwrap(), b"v1");
    assert!(!f.paths.credentials_file.exists());
    fs::write(&f.paths.credentials_file, "old").unwrap();
    s.write_credential_entry(&f.env, &f.paths, b"v2", &open)
        .unwrap();
    assert_eq!(fs::read(&f.paths.credentials_file).unwrap(), b"v2");
}

#[test]
fn a_failed_fence_stops_every_write() {
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let lost = || Err(ProviderError::Lock(LockError::Compromised("x".into())));
    assert!(
        s.write_credential_entry(&f.env, &f.paths, b"v1", &lost)
            .is_err()
    );
    assert!(
        s.write_managed_key(&f.env, &f.paths, b"sk-ant-api03-zzzz", &lost)
            .is_err()
    );
    assert!(f.kc.items().is_empty());
    assert!(!f.paths.credentials_file.exists() && !f.paths.global_config.exists());
}

#[test]
fn file_fallback_requires_the_shadowing_item_to_be_gone() {
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, b"old");
    f.kc.set_fail_write(&svc, true);
    f.kc.set_fail_delete(&svc, true);
    assert!(
        s.write_credential_entry(&f.env, &f.paths, b"new", &open)
            .is_err()
    );
    f.kc.set_fail_delete(&svc, false);
    s.write_credential_entry(&f.env, &f.paths, b"new", &open)
        .unwrap();
    assert_eq!(fs::read(&f.paths.credentials_file).unwrap(), b"new");
    assert!(f.kc.get(&svc, &acct).is_none());
    assert!(s.file_mode_pinned());
    // Pinned: the next write goes straight to the file, even with the Keychain healthy again.
    f.kc.set_fail_write(&svc, false);
    s.write_credential_entry(&f.env, &f.paths, b"newer", &open)
        .unwrap();
    assert!(f.kc.get(&svc, &acct).is_none());
}

#[test]
fn clearing_oauth_keeps_machine_shared_keys() {
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    let env_json = json!({"claudeAiOauth": {"refreshToken": "r"}, "trustedDeviceToken": "d", "mcpOAuth": {"m": 1}});
    f.kc.put(&svc, &acct, env_json.to_string().as_bytes());
    fs::write(&f.paths.credentials_file, env_json.to_string()).unwrap();
    s.clear_credential_account_keys(&f.env, &f.paths, &open)
        .unwrap();
    assert_eq!(
        json_of(&f.kc.get(&svc, &acct).unwrap()),
        json!({"mcpOAuth": {"m": 1}})
    );
    assert_eq!(
        json_of(&fs::read(&f.paths.credentials_file).unwrap()),
        json!({"mcpOAuth": {"m": 1}})
    );
    // An entry with nothing machine-shared is deleted outright.
    f.kc.put(&svc, &acct, br#"{"claudeAiOauth":{}}"#);
    fs::write(&f.paths.credentials_file, r#"{"claudeAiOauth":{}}"#).unwrap();
    s.clear_credential_account_keys(&f.env, &f.paths, &open)
        .unwrap();
    assert!(f.kc.get(&svc, &acct).is_none());
    assert!(!f.paths.credentials_file.exists());
}

#[test]
fn every_fallback_item_is_cleared_snapshotted_and_restored() {
    // An explicit CLAUDE_CONFIG_DIR=~/.claude reads the suffixed item, then the plain one.
    let f = fx_with(|e| e.claude_config_dir = Some(e.home.join(".claude").into_os_string()));
    let s = store(&f, Platform::MacOs);
    let acct = keychain_account(&f.env);
    let services = read_services(&f.env, ItemKind::OAuth);
    assert_eq!(services.len(), 2);
    f.kc.put(
        &services[1],
        &acct,
        br#"{"claudeAiOauth":{"refreshToken":"plain"}}"#,
    );
    let snap = s.snapshot(&f.env, &f.paths).unwrap();
    s.clear_credential_account_keys(&f.env, &f.paths, &open)
        .unwrap();
    assert!(
        f.kc.get(&services[1], &acct).is_none(),
        "the fallback item must not stay active"
    );
    s.restore(&f.env, &f.paths, &snap, &open).unwrap();
    assert!(f.kc.get(&services[1], &acct).is_some());
}

#[test]
fn managed_keys_record_approval_and_never_leave_a_shadowing_item() {
    let f = fx();
    let s = store(&f, Platform::MacOs);
    fs::write(&f.paths.global_config, "{\n  \"userID\": \"u\"\n}\n").unwrap();
    let key = b"sk-ant-api03-0123456789abcdefghijKLMNOPQRST";
    let tail = "abcdefghijKLMNOPQRST";
    s.write_managed_key(&f.env, &f.paths, key, &open).unwrap();
    let managed = (
        keychain_service(&f.env, ItemKind::ManagedKey),
        keychain_account(&f.env),
    );
    assert_eq!(f.kc.get(&managed.0, &managed.1).unwrap(), key);
    let approved = config::get_key(&f.paths.global_config, "customApiKeyResponses")
        .present()
        .unwrap()
        .unwrap();
    assert_eq!(approved["approved"], json!([tail]));
    s.write_managed_key(&f.env, &f.paths, key, &open).unwrap(); // idempotent approval
    let approved = config::get_key(&f.paths.global_config, "customApiKeyResponses")
        .present()
        .unwrap()
        .unwrap();
    assert_eq!(approved["approved"], json!([tail]));
    assert_eq!(s.read_managed_key(&f.env, &f.paths).present().unwrap(), key);

    // The Keychain update fails: the key lands in primaryApiKey and the stale item is removed,
    // so the key CC will actually use is the new one.
    f.kc.set_fail_write(&managed.0, true);
    let other = b"sk-ant-api03-other-key-000000000000";
    s.write_managed_key(&f.env, &f.paths, other, &open).unwrap();
    assert!(f.kc.get(&managed.0, &managed.1).is_none());
    assert_eq!(
        s.read_managed_key(&f.env, &f.paths).present().unwrap(),
        other
    );

    // And when the stale item cannot be removed, the write fails instead of lying.
    f.kc.set_fail_write(&managed.0, false);
    s.write_managed_key(&f.env, &f.paths, key, &open).unwrap();
    f.kc.set_fail_write(&managed.0, true);
    f.kc.set_fail_delete(&managed.0, true);
    assert!(matches!(
        s.write_managed_key(&f.env, &f.paths, other, &open),
        Err(ProviderError::ShadowingItem(_))
    ));
    f.kc.set_fail_write(&managed.0, false);
    f.kc.set_fail_delete(&managed.0, false);

    s.clear_managed_key(&f.env, &f.paths, &open).unwrap();
    assert!(f.kc.get(&managed.0, &managed.1).is_none());
    assert_eq!(
        config::get_key(&f.paths.global_config, "primaryApiKey")
            .present()
            .unwrap(),
        None
    );
    // `approved` is append-only and survives.
    assert!(
        config::get_key(&f.paths.global_config, "customApiKeyResponses")
            .present()
            .unwrap()
            .is_some()
    );
}

#[test]
fn snapshot_and_restore_are_byte_exact() {
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, b"orig");
    fs::write(&f.paths.global_config, "{\"a\": 1}").unwrap();
    let snap = s.snapshot(&f.env, &f.paths).unwrap();
    s.write_credential_entry(&f.env, &f.paths, b"new", &open)
        .unwrap();
    s.write_managed_key(
        &f.env,
        &f.paths,
        b"sk-ant-api03-zzzzzzzzzzzzzzzzzzzz",
        &open,
    )
    .unwrap();
    s.restore(&f.env, &f.paths, &snap, &open).unwrap();
    assert_eq!(f.kc.get(&svc, &acct).unwrap(), b"orig");
    assert!(
        f.kc.get(&keychain_service(&f.env, ItemKind::ManagedKey), &acct)
            .is_none()
    );
    assert!(!f.paths.credentials_file.exists());
    assert_eq!(fs::read(&f.paths.global_config).unwrap(), b"{\"a\": 1}");
}

#[test]
fn an_unreadable_entry_cannot_be_snapshotted() {
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, b"x");
    f.kc.set_unreadable(&svc, &acct, true);
    assert!(s.snapshot(&f.env, &f.paths).is_err());
}

#[test]
fn live_identity_reads_oauth_account_and_refuses_torn_files() {
    let f = fx();
    assert!(matches!(config::live_identity(&f.paths), Read::Absent));
    fs::write(&f.paths.global_config, r#"{"userID": "u"}"#).unwrap();
    assert!(matches!(config::live_identity(&f.paths), Read::Absent));
    fs::write(
        &f.paths.global_config,
        r#"{"oauthAccount": {"emailAddress": "a@b.co"}}"#,
    )
    .unwrap();
    assert_eq!(
        config::live_identity(&f.paths).present().unwrap().label,
        "a@b.co"
    );
    fs::write(
        &f.paths.global_config,
        r#"{"oauthAccount": {"emailAddress": "a@b"#,
    )
    .unwrap();
    assert!(matches!(
        config::live_identity(&f.paths),
        Read::Unreadable(_)
    ));
}

fn mode_of(p: &std::path::Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(p).unwrap().permissions().mode() & 0o7777
}

#[test]
fn splicing_a_symlinked_config_writes_through_the_link() {
    // Review Focus 5.
    use std::os::unix::fs::PermissionsExt;
    let f = fx();
    let real = f.env.home.join("dotfiles/claude.json");
    fs::create_dir_all(real.parent().unwrap()).unwrap();
    fs::write(&real, "{\n  \"userID\": \"u\"\n}\n").unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o640)).unwrap();
    std::os::unix::fs::symlink(&real, &f.paths.global_config).unwrap();
    let undo = config::splice_key(
        &f.paths.global_config,
        "oauthAccount",
        Some(&json!({"emailAddress": "a@b.co"})),
        &open,
    )
    .unwrap();
    assert!(
        fs::symlink_metadata(&f.paths.global_config)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(fs::read_to_string(&real).unwrap().contains("a@b.co"));
    assert_eq!(
        mode_of(&real),
        0o640,
        "the target's mode must be preserved across the splice"
    );
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let held = LiveLocks::new(&g, Box::new(Held));
    Box::new(undo).undo(&held).unwrap();
    assert_eq!(
        fs::read_to_string(&real).unwrap(),
        "{\n  \"userID\": \"u\"\n}\n"
    );
    assert_eq!(
        mode_of(&real),
        0o640,
        "the target's mode must be preserved across the undo"
    );
}

#[test]
fn rolling_back_through_dangling_links_keeps_the_links() {
    let f = fx();
    let s = store(&f, Platform::Linux);
    let dots = f.env.home.join("dotfiles");
    fs::create_dir_all(&dots).unwrap();
    std::os::unix::fs::symlink(dots.join("claude.json"), &f.paths.global_config).unwrap();
    std::os::unix::fs::symlink(dots.join("credentials.json"), &f.paths.credentials_file).unwrap();
    let snap = s.snapshot(&f.env, &f.paths).unwrap(); // both targets absent
    let undo = config::splice_key(
        &f.paths.global_config,
        "oauthAccount",
        Some(&json!({})),
        &open,
    )
    .unwrap();
    s.write_credential_entry(&f.env, &f.paths, b"{}", &open)
        .unwrap();
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    Box::new(undo)
        .undo(&LiveLocks::new(&g, Box::new(Held)))
        .unwrap();
    s.restore(&f.env, &f.paths, &snap, &open).unwrap();
    for link in [&f.paths.global_config, &f.paths.credentials_file] {
        assert!(
            fs::symlink_metadata(link).unwrap().file_type().is_symlink(),
            "{}",
            link.display()
        );
    }
    assert!(!dots.join("claude.json").exists() && !dots.join("credentials.json").exists());
}

#[test]
fn splicing_a_torn_config_is_refused_and_writes_nothing() {
    let f = fx();
    fs::write(&f.paths.global_config, "{\"a\": ").unwrap();
    match config::splice_key(
        &f.paths.global_config,
        "oauthAccount",
        Some(&json!({})),
        &open,
    ) {
        Err(e) => assert!(e.to_string().contains("backups"), "{e}"),
        Ok(_) => panic!("a torn config must not be spliced"),
    }
    assert_eq!(fs::read(&f.paths.global_config).unwrap(), b"{\"a\": ");
}

#[test]
fn splicing_a_missing_config_creates_it_with_only_that_key() {
    let f = fx();
    config::splice_key(
        &f.paths.global_config,
        "oauthAccount",
        Some(&json!({"emailAddress": "a@b.co"})),
        &open,
    )
    .unwrap();
    let doc: Value = serde_json::from_slice(&fs::read(&f.paths.global_config).unwrap()).unwrap();
    assert_eq!(doc, json!({"oauthAccount": {"emailAddress": "a@b.co"}}));
}
