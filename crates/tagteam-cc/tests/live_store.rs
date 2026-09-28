use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use tagteam_cc::live::{LiveStore, Platform};
use tagteam_cc::{CcPaths, ItemKind, config, keychain_account, keychain_service, read_services};
use tagteam_provider::{
    Env, FakeKeychain, Keychain, KeychainError, LiveLockSet, LiveLocks, LockError, LockState,
    MutationGuard, Provenance, ProviderError, Read, Undo,
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

/// A lock set that always reports the lock lost, for undo calls that must refuse.
struct Lost;

impl LiveLockSet for Lost {
    fn check_owned(&self) -> Result<(), LockError> {
        Err(LockError::Compromised("x".into()))
    }
}

/// A fence that passes exactly `n` times, then fails like a lost lock — for pinning down
/// exactly which mutation a fence protects.
struct CountingFence {
    remaining: AtomicUsize,
}

impl CountingFence {
    fn new(n: usize) -> Self {
        Self {
            remaining: AtomicUsize::new(n),
        }
    }

    fn check(&self) -> Result<(), ProviderError> {
        let passed = self
            .remaining
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| v.checked_sub(1))
            .is_ok();
        if passed {
            Ok(())
        } else {
            Err(ProviderError::Lock(LockError::Compromised(
                "counting fence exhausted".into(),
            )))
        }
    }
}

/// Wraps a `FakeKeychain` to count `upsert` calls, so a test can prove an entry that
/// already matches its snapshot was never rewritten.
struct CountingKeychain {
    inner: Arc<FakeKeychain>,
    upserts: AtomicUsize,
}

impl CountingKeychain {
    fn new(inner: Arc<FakeKeychain>) -> Self {
        Self {
            inner,
            upserts: AtomicUsize::new(0),
        }
    }

    fn upsert_count(&self) -> usize {
        self.upserts.load(Ordering::SeqCst)
    }
}

impl Keychain for CountingKeychain {
    fn find(&self, s: &str, a: &str) -> Read<Vec<u8>> {
        self.inner.find(s, a)
    }
    fn exists(&self, s: &str, a: &str) -> Read<()> {
        self.inner.exists(s, a)
    }
    fn upsert(&self, s: &str, a: &str, d: &[u8]) -> Result<(), KeychainError> {
        self.upserts.fetch_add(1, Ordering::SeqCst);
        self.inner.upsert(s, a, d)
    }
    fn delete(&self, s: &str, a: &str) -> Result<(), KeychainError> {
        self.inner.delete(s, a)
    }
    fn lock_state(&self) -> LockState {
        self.inner.lock_state()
    }
    fn unlock(&self) -> bool {
        self.inner.unlock()
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

// --- Fix round 1 -----------------------------------------------------------------

#[test]
fn a_successful_keychain_write_clears_a_stale_primary_api_key() {
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let managed = (
        keychain_service(&f.env, ItemKind::ManagedKey),
        keychain_account(&f.env),
    );
    // Activate key A while the Keychain is down: it lands in `primaryApiKey`.
    f.kc.set_fail_write(&managed.0, true);
    let a = "sk-ant-api03-aaaaaaaaaaaaaaaaaaaa";
    s.write_managed_key(&f.env, &f.paths, a.as_bytes(), &open)
        .unwrap();
    assert_eq!(
        config::get_key(&f.paths.global_config, "primaryApiKey")
            .present()
            .unwrap()
            .unwrap(),
        json!(a)
    );
    // Activate B with a healthy Keychain: the item holds B, and the stale plaintext A
    // must not still be live in `~/.claude.json`.
    f.kc.set_fail_write(&managed.0, false);
    let b = "sk-ant-api03-bbbbbbbbbbbbbbbbbbbb";
    s.write_managed_key(&f.env, &f.paths, b.as_bytes(), &open)
        .unwrap();
    assert_eq!(f.kc.get(&managed.0, &managed.1).unwrap(), b.as_bytes());
    assert_eq!(
        config::get_key(&f.paths.global_config, "primaryApiKey")
            .present()
            .unwrap(),
        None
    );
}

#[test]
fn restore_continues_past_a_non_lock_failure_and_names_every_unrestored_entry() {
    let f = fx_with(|e| e.claude_config_dir = Some(e.home.join(".claude").into_os_string()));
    let counting = Arc::new(CountingKeychain::new(f.kc.clone()));
    let s = LiveStore::new(counting.clone(), Platform::MacOs).with_retry_delay(Duration::ZERO);
    let acct = keychain_account(&f.env);
    let services = read_services(&f.env, ItemKind::OAuth);
    assert_eq!(services.len(), 2);
    // Only the plain fallback item is present; the primary is absent from the start.
    f.kc.put(
        &services[1],
        &acct,
        br#"{"claudeAiOauth":{"refreshToken":"plain"}}"#,
    );
    let snap = s.snapshot(&f.env, &f.paths).unwrap();

    // Both upserts fail, so the write falls back to the file, and `remove_items` deletes
    // both fallback items, including the plain one.
    f.kc.set_fail_write(&services[0], true);
    f.kc.set_fail_write(&services[1], true);
    s.write_credential_entry(&f.env, &f.paths, b"new", &open)
        .unwrap();
    assert!(f.kc.get(&services[0], &acct).is_none());
    assert!(f.kc.get(&services[1], &acct).is_none());

    // Restore: the plain item's upsert still fails.
    let upserts_before = counting.upsert_count();
    match s.restore(&f.env, &f.paths, &snap, &open) {
        Err(ProviderError::Incomplete { failed }) => {
            assert_eq!(failed, vec![services[1].clone()]);
        }
        other => panic!("expected Incomplete naming {}, got {other:?}", services[1]),
    }
    // The files were still restored despite the Keychain failure.
    assert!(!f.paths.credentials_file.exists());
    // The primary item was never touched: it already matched its (absent) snapshot, and
    // so did both managed-key items, so only the plain item's upsert was attempted.
    assert_eq!(
        counting.upsert_count(),
        upserts_before + 1,
        "an entry that already matched its snapshot must not be rewritten"
    );
}

#[test]
fn config_undo_checks_lock_ownership_before_restoring_or_removing() {
    let f = fx();
    let g = MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap();
    let lost = LiveLocks::new(&g, Box::new(Lost));

    // Restore-bytes branch: an existing file was spliced, so undo must put its bytes back.
    fs::write(&f.paths.global_config, "{\"a\": 1}").unwrap();
    let undo = config::splice_key(&f.paths.global_config, "k", Some(&json!(1)), &open).unwrap();
    let spliced = fs::read(&f.paths.global_config).unwrap();
    assert!(matches!(
        Box::new(undo).undo(&lost),
        Err(ProviderError::Lock(_))
    ));
    assert_eq!(
        fs::read(&f.paths.global_config).unwrap(),
        spliced,
        "undo must write nothing once the lock is lost"
    );

    // Remove-created-file branch: the splice created the file from nothing.
    let created = f.paths.config_home.join("created.json");
    let undo2 = config::splice_key(&created, "k", Some(&json!(1)), &open).unwrap();
    assert!(created.exists());
    assert!(matches!(
        Box::new(undo2).undo(&lost),
        Err(ProviderError::Lock(_))
    ));
    assert!(
        created.exists(),
        "undo must not remove the file once the lock is lost"
    );
}

#[test]
fn a_counting_fence_stops_the_hot_reload_rewrite() {
    let f = fx();
    let s = store(&f, Platform::MacOs);
    fs::write(&f.paths.credentials_file, "old").unwrap();
    let cf = CountingFence::new(1);
    let fence = || cf.check();
    assert!(matches!(
        s.write_credential_entry(&f.env, &f.paths, b"new", &fence),
        Err(ProviderError::Lock(_))
    ));
    assert_eq!(fs::read(&f.paths.credentials_file).unwrap(), b"old");
    let (svc, acct) = oauth_svc(&f);
    assert_eq!(
        f.kc.get(&svc, &acct).unwrap(),
        b"new",
        "the keychain write itself happened before the fence tripped"
    );
}

#[test]
fn a_counting_fence_stops_the_deletes_in_remove_items() {
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, b"old");
    f.kc.set_fail_write(&svc, true);
    let cf = CountingFence::new(3);
    let fence = || cf.check();
    assert!(matches!(
        s.write_credential_entry(&f.env, &f.paths, b"new", &fence),
        Err(ProviderError::Lock(_))
    ));
    // The file fallback already landed; the shadowing Keychain item was never deleted
    // because the fence tripped first.
    assert_eq!(fs::read(&f.paths.credentials_file).unwrap(), b"new");
    assert_eq!(f.kc.get(&svc, &acct).unwrap(), b"old");
    assert!(!s.file_mode_pinned());
}

#[test]
fn restore_is_fenced_for_both_files_and_items_and_aborts_immediately_on_fence_failure() {
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, b"orig-item");
    fs::write(&f.paths.credentials_file, "orig-file").unwrap();
    fs::write(&f.paths.global_config, "{\"a\": 1}").unwrap();
    let snap = s.snapshot(&f.env, &f.paths).unwrap();

    // Change everything, so every entry needs restoring: the credential write bumps both
    // the Keychain item and the pre-existing file, and the config is spliced directly.
    s.write_credential_entry(&f.env, &f.paths, b"changed", &open)
        .unwrap();
    config::splice_key(&f.paths.global_config, "b", Some(&json!(2)), &open).unwrap();

    // Two passes restore both files; the third, for the Keychain item, fails.
    let cf = CountingFence::new(2);
    let fence = || cf.check();
    assert!(matches!(
        s.restore(&f.env, &f.paths, &snap, &fence),
        Err(ProviderError::Lock(_))
    ));
    assert_eq!(fs::read(&f.paths.global_config).unwrap(), b"{\"a\": 1}");
    assert_eq!(fs::read(&f.paths.credentials_file).unwrap(), b"orig-file");
    assert_eq!(
        f.kc.get(&svc, &acct).unwrap(),
        b"changed",
        "the item restore never ran once the fence tripped"
    );
}

#[test]
fn a_counting_fence_stops_the_file_branch_of_clearing_account_keys() {
    let f = fx();
    let s = store(&f, Platform::MacOs);
    // No machine-shared keys survive, so the file branch takes the delete path.
    fs::write(
        &f.paths.credentials_file,
        r#"{"claudeAiOauth":{"refreshToken":"r"}}"#,
    )
    .unwrap();
    let cf = CountingFence::new(0);
    let fence = || cf.check();
    assert!(matches!(
        s.clear_credential_account_keys(&f.env, &f.paths, &fence),
        Err(ProviderError::Lock(_))
    ));
    assert!(
        f.paths.credentials_file.exists(),
        "the file must not be removed once the fence trips"
    );
}

#[test]
fn read_managed_key_is_unreadable_when_the_keychain_item_is_unreadable_even_with_a_primary_api_key_present()
 {
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let svc = keychain_service(&f.env, ItemKind::ManagedKey);
    let acct = keychain_account(&f.env);
    f.kc.put(&svc, &acct, b"sk-ant-api03-keychain");
    f.kc.set_unreadable(&svc, &acct, true);
    fs::write(
        &f.paths.global_config,
        r#"{"primaryApiKey": "sk-ant-api03-file"}"#,
    )
    .unwrap();
    assert!(matches!(
        s.read_managed_key(&f.env, &f.paths),
        Read::Unreadable(_)
    ));
}

#[test]
fn file_fallback_verifies_every_fallback_item_is_gone_including_the_plain_one() {
    let f = fx_with(|e| e.claude_config_dir = Some(e.home.join(".claude").into_os_string()));
    let s = store(&f, Platform::MacOs);
    let acct = keychain_account(&f.env);
    let services = read_services(&f.env, ItemKind::OAuth);
    assert_eq!(services.len(), 2);
    f.kc.set_fail_write(&services[0], true);
    f.kc.set_fail_write(&services[1], true);
    // The plain fallback item's delete is made to fail, so it survives the attempt and
    // the existence check must catch it.
    f.kc.put(&services[1], &acct, b"stale");
    f.kc.set_fail_delete(&services[1], true);
    match s.write_credential_entry(&f.env, &f.paths, b"new", &open) {
        Err(ProviderError::ShadowingItem(name)) => assert_eq!(name, services[1]),
        other => panic!("expected ShadowingItem({}), got {other:?}", services[1]),
    }
    f.kc.set_fail_delete(&services[1], false);
    s.write_credential_entry(&f.env, &f.paths, b"new", &open)
        .unwrap();
    assert!(f.kc.get(&services[0], &acct).is_none());
    assert!(f.kc.get(&services[1], &acct).is_none());
}

#[test]
fn a_failed_fallback_never_pins_the_file_mode() {
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
    assert!(!s.file_mode_pinned());
}

#[test]
fn clearing_account_keys_refuses_an_unparsable_keychain_entry() {
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, b"not json");
    assert!(matches!(
        s.clear_credential_account_keys(&f.env, &f.paths, &open),
        Err(ProviderError::Invalid(_))
    ));
    assert!(
        f.kc.get(&svc, &acct).is_some(),
        "an unparsable entry must not be deleted"
    );
}

#[test]
fn clearing_account_keys_refuses_an_unparsable_credentials_file() {
    let f = fx();
    let s = store(&f, Platform::MacOs);
    fs::write(&f.paths.credentials_file, b"not json").unwrap();
    assert!(matches!(
        s.clear_credential_account_keys(&f.env, &f.paths, &open),
        Err(ProviderError::Invalid(_))
    ));
    assert!(
        f.paths.credentials_file.exists(),
        "an unparsable file must not be deleted"
    );
}
