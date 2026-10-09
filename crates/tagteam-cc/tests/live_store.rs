use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Value, json};
use tagteam_cc::live::{LiveStore, Platform};
use tagteam_cc::{CcPaths, ItemKind, config, keychain_account, keychain_service};
use tagteam_provider::{
    Env, FakeKeychain, Keychain, KeychainError, LiveLockSet, LiveLocks, LockError, LockState,
    MutationGuard, Provenance, ProviderError, Read, SecretStore, Undo,
};

/// A fallback hook for a test that saves nothing: every entry a fallback reports goes.
fn save_nothing(_: &[u8]) -> Result<(), ProviderError> {
    Ok(())
}

/// The unsuffixed items an explicit `CLAUDE_CONFIG_DIR=~/.claude` fell back to before Claude
/// Code 2.1.286. Inert now (Appendix A.2): nothing reads, writes or clears them.
const INERT_OAUTH: &str = "Claude Code-credentials";
const INERT_MANAGED: &str = "Claude Code";

/// A fixture with an explicit `CLAUDE_CONFIG_DIR=~/.claude`.
fn explicit_default() -> Fx {
    fx_with(|e| e.claude_config_dir = Some(e.home.join(".claude").into_os_string()))
}

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

/// Wraps a `FakeKeychain` and records every `upsert`/`delete` call, in order, so a test
/// can prove both which entries were (not) touched and in what order they were.
struct RecordingKeychain {
    inner: Arc<FakeKeychain>,
    calls: Mutex<Vec<(String, &'static str)>>,
}

impl RecordingKeychain {
    fn new(inner: Arc<FakeKeychain>) -> Self {
        Self {
            inner,
            calls: Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> Vec<(String, &'static str)> {
        self.calls.lock().unwrap().clone()
    }
}

impl Keychain for RecordingKeychain {
    fn find(&self, s: &str, a: &str) -> Read<Vec<u8>> {
        self.inner.find(s, a)
    }
    fn exists(&self, s: &str, a: &str) -> Read<()> {
        self.inner.exists(s, a)
    }
    fn upsert(&self, s: &str, a: &str, d: &[u8]) -> Result<(), KeychainError> {
        self.calls.lock().unwrap().push((s.to_owned(), "upsert"));
        self.inner.upsert(s, a, d)
    }
    fn delete(&self, s: &str, a: &str) -> Result<(), KeychainError> {
        self.calls.lock().unwrap().push((s.to_owned(), "delete"));
        self.inner.delete(s, a)
    }
    fn lock_state(&self) -> LockState {
        self.inner.lock_state()
    }
    fn unlock(&self) -> bool {
        self.inner.unlock()
    }
}

/// Wraps a `FakeKeychain` and, on every `upsert`/`delete`, records the credentials
/// file's mtime at that moment — so a test can prove the file's own hot-reload bump
/// happens strictly after the Keychain item it reflects, never before or instead of it.
struct MtimeProbeKeychain {
    inner: Arc<FakeKeychain>,
    credentials_file: std::path::PathBuf,
    mtime_at_last_item_write: Mutex<Option<SystemTime>>,
}

impl MtimeProbeKeychain {
    fn new(inner: Arc<FakeKeychain>, credentials_file: std::path::PathBuf) -> Self {
        Self {
            inner,
            credentials_file,
            mtime_at_last_item_write: Mutex::new(None),
        }
    }

    fn mtime_at_last_item_write(&self) -> Option<SystemTime> {
        *self.mtime_at_last_item_write.lock().unwrap()
    }

    fn record(&self) {
        let mtime = fs::metadata(&self.credentials_file)
            .and_then(|m| m.modified())
            .ok();
        *self.mtime_at_last_item_write.lock().unwrap() = mtime;
        // A later bump must land at a strictly later mtime regardless of the
        // filesystem's clock resolution.
        std::thread::sleep(Duration::from_millis(10));
    }
}

impl Keychain for MtimeProbeKeychain {
    fn find(&self, s: &str, a: &str) -> Read<Vec<u8>> {
        self.inner.find(s, a)
    }
    fn exists(&self, s: &str, a: &str) -> Read<()> {
        self.inner.exists(s, a)
    }
    fn upsert(&self, s: &str, a: &str, d: &[u8]) -> Result<(), KeychainError> {
        self.record();
        self.inner.upsert(s, a, d)
    }
    fn delete(&self, s: &str, a: &str) -> Result<(), KeychainError> {
        self.record();
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
fn an_explicit_default_config_dir_reads_only_its_suffixed_item() {
    // Appendix A.2 (2.1.286): `CLAUDE_CONFIG_DIR=~/.claude` names the suffixed items alone. The
    // unsuffixed ones are never read, whatever they hold and whether or not they can be read.
    let f = explicit_default();
    let s = store(&f, Platform::MacOs);
    let acct = keychain_account(&f.env);
    let (oauth, managed) = (
        keychain_service(&f.env, ItemKind::OAuth),
        keychain_service(&f.env, ItemKind::ManagedKey),
    );
    assert_ne!(
        (oauth.as_str(), managed.as_str()),
        (INERT_OAUTH, INERT_MANAGED)
    );
    f.kc.put(INERT_OAUTH, &acct, b"inert");
    f.kc.put(INERT_MANAGED, &acct, b"sk-ant-api03-inert");
    for unreadable in [false, true] {
        f.kc.set_unreadable(INERT_OAUTH, &acct, unreadable);
        f.kc.set_unreadable(INERT_MANAGED, &acct, unreadable);
        assert!(
            matches!(s.read_credential(&f.env, &f.paths), Read::Absent),
            "unreadable={unreadable}"
        );
        assert!(
            matches!(s.read_managed_key(&f.env, &f.paths), Read::Absent),
            "unreadable={unreadable}"
        );
    }
    f.kc.put(&oauth, &acct, b"suffixed");
    f.kc.put(&managed, &acct, b"sk-ant-api03-suffixed");
    assert_eq!(
        s.read_credential(&f.env, &f.paths)
            .present()
            .unwrap()
            .bytes(),
        b"suffixed"
    );
    assert_eq!(
        s.read_managed_key(&f.env, &f.paths).present().unwrap(),
        b"sk-ant-api03-suffixed"
    );
    // The suffixed item is the only authority: unreadable is unreadable, and the file covers
    // it only as a degraded read.
    f.kc.set_unreadable(&oauth, &acct, true);
    f.kc.set_unreadable(&managed, &acct, true);
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
fn a_symlinked_config_dir_is_read_and_cleared_only_under_the_link_s_spelling() {
    // Appendix A.2 (2.1.286): no `hash(readlink target)` fallback.
    let f = fx();
    let real = f.env.home.join("real-profile");
    fs::create_dir_all(&real).unwrap();
    let link = f.env.home.join("link-profile");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let mut by_link = f.env.clone();
    by_link.claude_config_dir = Some(link.into_os_string());
    let mut by_target = f.env.clone();
    by_target.claude_config_dir = Some(real.into_os_string());
    let s = store(&f, Platform::MacOs);
    let acct = keychain_account(&f.env);
    let link_item = keychain_service(&by_link, ItemKind::OAuth);
    let target_item = keychain_service(&by_target, ItemKind::OAuth);
    assert_ne!(link_item, target_item);
    let paths = CcPaths::resolve(&by_link);
    let target_entry = br#"{"claudeAiOauth":{"refreshToken":"target"}}"#;
    f.kc.put(&target_item, &acct, target_entry);
    assert!(
        matches!(s.read_credential(&by_link, &paths), Read::Absent),
        "the target's item is never read through the link"
    );
    f.kc.put(
        &link_item,
        &acct,
        br#"{"claudeAiOauth":{"refreshToken":"link"}}"#,
    );
    assert_eq!(
        s.read_credential(&by_link, &paths)
            .present()
            .unwrap()
            .bytes(),
        br#"{"claudeAiOauth":{"refreshToken":"link"}}"#
    );
    s.clear_credential_account_keys(&by_link, &paths, &open)
        .unwrap();
    assert!(
        f.kc.get(&link_item, &acct).is_none(),
        "cleared under the link's item"
    );
    assert_eq!(
        f.kc.get(&target_item, &acct).unwrap(),
        target_entry,
        "the target's item is left alone"
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
    assert_eq!(
        s.write_credential_entry(&f.env, &f.paths, b"{\"a\":1}", &open, &mut save_nothing)
            .unwrap(),
        SecretStore::File(f.paths.credentials_file.clone())
    );
    assert_eq!(fs::read(&f.paths.credentials_file).unwrap(), b"{\"a\":1}");
    assert_eq!(
        s.write_managed_key(
            &f.env,
            &f.paths,
            b"sk-ant-api03-linux",
            &open,
            &mut save_nothing
        )
        .unwrap(),
        SecretStore::File(f.paths.global_config.clone())
    );
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
    assert_eq!(
        s.write_credential_entry(&f.env, &f.paths, b"v1", &open, &mut save_nothing)
            .unwrap(),
        SecretStore::Keychain
    );
    assert_eq!(f.kc.get(&svc, &acct).unwrap(), b"v1");
    assert!(!f.paths.credentials_file.exists());
    fs::write(&f.paths.credentials_file, "old").unwrap();
    // The file only mirrors the item, for hot reload: the Keychain is still where it went.
    assert_eq!(
        s.write_credential_entry(&f.env, &f.paths, b"v2", &open, &mut save_nothing)
            .unwrap(),
        SecretStore::Keychain
    );
    assert_eq!(fs::read(&f.paths.credentials_file).unwrap(), b"v2");
}

#[test]
fn a_failed_fence_stops_every_write() {
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let lost = || Err(ProviderError::Lock(LockError::Compromised("x".into())));
    assert!(
        s.write_credential_entry(&f.env, &f.paths, b"v1", &lost, &mut save_nothing)
            .is_err()
    );
    assert!(
        s.write_managed_key(
            &f.env,
            &f.paths,
            b"sk-ant-api03-zzzz",
            &lost,
            &mut save_nothing
        )
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
        s.write_credential_entry(&f.env, &f.paths, b"new", &open, &mut save_nothing)
            .is_err()
    );
    f.kc.set_fail_delete(&svc, false);
    let fell_back = SecretStore::Fallback(f.paths.credentials_file.clone());
    assert_eq!(
        s.write_credential_entry(&f.env, &f.paths, b"new", &open, &mut save_nothing)
            .unwrap(),
        fell_back
    );
    assert_eq!(fs::read(&f.paths.credentials_file).unwrap(), b"new");
    assert!(f.kc.get(&svc, &acct).is_none());
    assert!(s.file_mode_pinned());
    // Pinned: the next write goes straight to the file, even with the Keychain healthy again,
    // and says so.
    f.kc.set_fail_write(&svc, false);
    assert_eq!(
        s.write_credential_entry(&f.env, &f.paths, b"newer", &open, &mut save_nothing)
            .unwrap(),
        fell_back
    );
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
fn an_explicit_default_config_dir_clears_snapshots_and_restores_only_its_suffixed_item() {
    let f = explicit_default();
    let s = store(&f, Platform::MacOs);
    let acct = keychain_account(&f.env);
    let oauth = keychain_service(&f.env, ItemKind::OAuth);
    let entry = br#"{"claudeAiOauth":{"refreshToken":"suffixed"},"mcpOAuth":{"m":1}}"#;
    let inert = br#"{"claudeAiOauth":{"refreshToken":"plain"}}"#;
    f.kc.put(&oauth, &acct, entry);
    f.kc.put(INERT_OAUTH, &acct, inert);
    f.kc.set_unreadable(INERT_OAUTH, &acct, true);
    let snap = s
        .snapshot(&f.env, &f.paths)
        .expect("an unreadable inert item never blocks a snapshot");
    f.kc.set_unreadable(INERT_OAUTH, &acct, false);
    s.clear_credential_account_keys(&f.env, &f.paths, &open)
        .unwrap();
    assert_eq!(
        json_of(&f.kc.get(&oauth, &acct).unwrap()),
        json!({"mcpOAuth": {"m": 1}})
    );
    assert_eq!(
        f.kc.get(INERT_OAUTH, &acct).unwrap(),
        inert,
        "the inert item is never cleared"
    );
    f.kc.put(INERT_OAUTH, &acct, b"changed meanwhile");
    s.restore(&f.env, &f.paths, &snap, &open).unwrap();
    assert_eq!(f.kc.get(&oauth, &acct).unwrap(), entry);
    assert_eq!(
        f.kc.get(INERT_OAUTH, &acct).unwrap(),
        b"changed meanwhile",
        "nor snapshotted and restored"
    );
}

#[test]
fn managed_keys_record_approval_and_never_leave_a_shadowing_item() {
    let f = fx();
    let s = store(&f, Platform::MacOs);
    fs::write(&f.paths.global_config, "{\n  \"userID\": \"u\"\n}\n").unwrap();
    let key = b"sk-ant-api03-0123456789abcdefghijKLMNOPQRST";
    let tail = "abcdefghijKLMNOPQRST";
    assert_eq!(
        s.write_managed_key(&f.env, &f.paths, key, &open, &mut save_nothing)
            .unwrap(),
        SecretStore::Keychain
    );
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
    s.write_managed_key(&f.env, &f.paths, key, &open, &mut save_nothing)
        .unwrap(); // idempotent approval
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
    assert_eq!(
        s.write_managed_key(&f.env, &f.paths, other, &open, &mut save_nothing)
            .unwrap(),
        SecretStore::Fallback(f.paths.global_config.clone())
    );
    assert!(f.kc.get(&managed.0, &managed.1).is_none());
    assert_eq!(
        s.read_managed_key(&f.env, &f.paths).present().unwrap(),
        other
    );

    // And when the stale item cannot be removed, the write fails instead of lying.
    f.kc.set_fail_write(&managed.0, false);
    s.write_managed_key(&f.env, &f.paths, key, &open, &mut save_nothing)
        .unwrap();
    f.kc.set_fail_write(&managed.0, true);
    f.kc.set_fail_delete(&managed.0, true);
    assert!(matches!(
        s.write_managed_key(&f.env, &f.paths, other, &open, &mut save_nothing),
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
fn a_managed_key_item_that_will_not_delete_is_reported_as_a_removal_not_a_file_write() {
    // L397: `clear_managed_key` writes no file, so the message must not say one was written.
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let svc = keychain_service(&f.env, ItemKind::ManagedKey);
    let acct = keychain_account(&f.env);
    f.kc.put(&svc, &acct, b"sk-ant-api03-stale");
    f.kc.set_fail_delete(&svc, true);
    let err = s
        .clear_managed_key(&f.env, &f.paths, &open)
        .expect_err("the item cannot be verified gone");
    let ProviderError::ShadowingItem(name) = &err else {
        panic!("expected ShadowingItem, got {err:?}");
    };
    let shown = err.to_string();
    assert!(shown.contains(name.as_str()), "{shown}");
    assert!(shown.contains("could not be verified gone"), "{shown}");
    assert!(
        shown.contains("so Claude Code may still read it"),
        "the consequence, not a write: {shown}"
    );
    assert!(!shown.contains("written to the file"), "{shown}");
    assert!(!shown.contains("what tagteam wrote"), "{shown}");
}

#[test]
fn a_null_approved_refuses_and_leaves_everything_untouched() {
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let config_before = b"{\"customApiKeyResponses\":{\"approved\":null}}\n".to_vec();
    fs::write(&f.paths.global_config, &config_before).unwrap();

    let err = s
        .write_managed_key(
            &f.env,
            &f.paths,
            b"sk-ant-api03-xyz",
            &open,
            &mut save_nothing,
        )
        .unwrap_err();
    assert!(matches!(err, ProviderError::Invalid(_)), "{err}");

    assert!(f.kc.items().is_empty(), "the Keychain must stay untouched");
    assert_eq!(
        config::get_key(&f.paths.global_config, "primaryApiKey")
            .present()
            .unwrap(),
        None
    );
    assert_eq!(
        fs::read(&f.paths.global_config).unwrap(),
        config_before,
        "the config must stay byte-identical"
    );
}

#[test]
fn an_object_approved_refuses_and_leaves_everything_untouched() {
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let config_before = b"{\"customApiKeyResponses\":{\"approved\":{}}}\n".to_vec();
    fs::write(&f.paths.global_config, &config_before).unwrap();

    let err = s
        .write_managed_key(
            &f.env,
            &f.paths,
            b"sk-ant-api03-xyz",
            &open,
            &mut save_nothing,
        )
        .unwrap_err();
    assert!(matches!(err, ProviderError::Invalid(_)), "{err}");

    assert!(f.kc.items().is_empty(), "the Keychain must stay untouched");
    assert_eq!(
        config::get_key(&f.paths.global_config, "primaryApiKey")
            .present()
            .unwrap(),
        None
    );
    assert_eq!(
        fs::read(&f.paths.global_config).unwrap(),
        config_before,
        "the config must stay byte-identical"
    );
}

#[test]
fn a_non_object_custom_api_key_responses_refuses_and_leaves_everything_untouched() {
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let config_before = b"{\"customApiKeyResponses\":[]}\n".to_vec();
    fs::write(&f.paths.global_config, &config_before).unwrap();

    let err = s
        .write_managed_key(
            &f.env,
            &f.paths,
            b"sk-ant-api03-xyz",
            &open,
            &mut save_nothing,
        )
        .unwrap_err();
    assert!(matches!(err, ProviderError::Invalid(_)), "{err}");

    assert!(f.kc.items().is_empty(), "the Keychain must stay untouched");
    assert_eq!(
        fs::read(&f.paths.global_config).unwrap(),
        config_before,
        "the config must stay byte-identical"
    );
}

#[test]
fn a_well_formed_approved_list_still_appends() {
    let f = fx();
    let s = store(&f, Platform::MacOs);
    fs::write(
        &f.paths.global_config,
        b"{\"customApiKeyResponses\":{\"approved\":[\"existing-tail\"]}}\n",
    )
    .unwrap();

    s.write_managed_key(
        &f.env,
        &f.paths,
        b"sk-ant-api03-0123456789abcdefghijKLMNOPQRST",
        &open,
        &mut save_nothing,
    )
    .unwrap();

    let approved = config::get_key(&f.paths.global_config, "customApiKeyResponses")
        .present()
        .unwrap()
        .unwrap();
    assert_eq!(
        approved["approved"],
        json!(["existing-tail", "abcdefghijKLMNOPQRST"])
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
    s.write_credential_entry(&f.env, &f.paths, b"new", &open, &mut save_nothing)
        .unwrap();
    s.write_managed_key(
        &f.env,
        &f.paths,
        b"sk-ant-api03-zzzzzzzzzzzzzzzzzzzz",
        &open,
        &mut save_nothing,
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
    s.write_credential_entry(&f.env, &f.paths, b"{}", &open, &mut save_nothing)
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
    s.write_managed_key(&f.env, &f.paths, a.as_bytes(), &open, &mut save_nothing)
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
    s.write_managed_key(&f.env, &f.paths, b.as_bytes(), &open, &mut save_nothing)
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
fn restore_continues_past_a_non_lock_failure_and_skips_an_already_matching_entry() {
    let f = fx();
    let recording = Arc::new(RecordingKeychain::new(f.kc.clone()));
    let s = LiveStore::new(recording.clone(), Platform::MacOs).with_retry_delay(Duration::ZERO);
    let acct = keychain_account(&f.env);
    let oauth = keychain_service(&f.env, ItemKind::OAuth);
    let managed = keychain_service(&f.env, ItemKind::ManagedKey);

    // This operation's own write clears the OAuth item and the credentials file, with nothing
    // machine-shared in them (it deletes both): a restore puts back only what the operation
    // wrote (§9.1). The OAuth item's upsert fails; the file, restored last, proves the restore
    // does not stop there. The managed-key item was never written and already matches its
    // snapshot, so it must receive no write at all, proving the skip.
    let orig_item = br#"{"claudeAiOauth":{"refreshToken":"orig-item"}}"#;
    let orig_file = br#"{"claudeAiOauth":{"refreshToken":"orig-file"}}"#;
    f.kc.put(&oauth, &acct, orig_item);
    f.kc.put(&managed, &acct, b"unchanged-managed");
    fs::write(&f.paths.credentials_file, orig_file).unwrap();
    let snap = s.snapshot(&f.env, &f.paths).unwrap();

    s.clear_credential_account_keys(&f.env, &f.paths, &open)
        .unwrap();
    assert_eq!(f.kc.get(&oauth, &acct), None);
    assert!(!f.paths.credentials_file.exists());
    f.kc.set_fail_write(&oauth, true);

    match s.restore(&f.env, &f.paths, &snap, &open) {
        Err(ProviderError::Incomplete { failed }) => assert_eq!(failed, vec![oauth.clone()]),
        other => panic!("expected Incomplete naming {oauth}, got {other:?}"),
    }
    assert_eq!(
        f.kc.get(&oauth, &acct),
        None,
        "the failed restore must leave the target value in place, not corrupt it"
    );
    assert_eq!(
        fs::read(&f.paths.credentials_file).unwrap(),
        orig_file,
        "the file must still be restored after the item failed"
    );
    assert_eq!(f.kc.get(&managed, &acct).unwrap(), b"unchanged-managed");

    let calls = recording.calls();
    assert!(
        calls
            .iter()
            .any(|(svc, op)| svc == &oauth && *op == "upsert")
    );
    assert!(
        !calls.iter().any(|(svc, _)| svc == &managed),
        "an entry that already matched its snapshot must receive no write at all: {calls:?}"
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
        s.write_credential_entry(&f.env, &f.paths, b"new", &fence, &mut save_nothing),
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
fn a_counting_fence_stops_the_delete_in_remove_item() {
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, b"old");
    f.kc.set_fail_write(&svc, true);
    let cf = CountingFence::new(3);
    let fence = || cf.check();
    assert!(matches!(
        s.write_credential_entry(&f.env, &f.paths, b"new", &fence, &mut save_nothing),
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
    s.write_credential_entry(&f.env, &f.paths, b"changed", &open, &mut save_nothing)
        .unwrap();
    config::splice_key(&f.paths.global_config, "b", Some(&json!(2)), &open).unwrap();

    // `restore` goes global_config, then the item, then the file last: two passes
    // restore the config and the item; the third, for the credentials file, fails.
    let cf = CountingFence::new(2);
    let fence = || cf.check();
    assert!(matches!(
        s.restore(&f.env, &f.paths, &snap, &fence),
        Err(ProviderError::Lock(_))
    ));
    assert_eq!(fs::read(&f.paths.global_config).unwrap(), b"{\"a\": 1}");
    assert_eq!(
        f.kc.get(&svc, &acct).unwrap(),
        b"orig-item",
        "the item must be restored before the file, and did run here"
    );
    assert_eq!(
        fs::read(&f.paths.credentials_file).unwrap(),
        b"changed",
        "the credentials file restore never ran once the fence tripped"
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
fn an_empty_primary_api_key_reads_as_no_managed_key() {
    // The file read succeeded (a failed one is `Unreadable`), so an empty string there is what
    // the file really holds, and it names no key. An empty Keychain item stays `Present("")`:
    // that is what a Keychain timeout can look like.
    for platform in [Platform::MacOs, Platform::Linux] {
        let f = fx();
        let s = store(&f, platform);
        fs::write(&f.paths.global_config, r#"{"primaryApiKey": ""}"#).unwrap();
        assert!(
            matches!(s.read_managed_key(&f.env, &f.paths), Read::Absent),
            "{platform:?}"
        );
    }
    let f = fx();
    let s = store(&f, Platform::MacOs);
    f.kc.put(
        &keychain_service(&f.env, ItemKind::ManagedKey),
        &keychain_account(&f.env),
        b"",
    );
    assert!(matches!(s.read_managed_key(&f.env, &f.paths), Read::Present(k) if k.is_empty()));
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
fn a_file_fallback_reports_and_deletes_only_the_suffixed_item_of_an_explicit_default_config_dir() {
    let f = explicit_default();
    let s = store(&f, Platform::MacOs);
    let acct = keychain_account(&f.env);
    let oauth = keychain_service(&f.env, ItemKind::OAuth);
    f.kc.put(&oauth, &acct, b"old");
    f.kc.put(INERT_OAUTH, &acct, b"inert");
    f.kc.set_fail_write(&oauth, true);
    let mut reported: Vec<Vec<u8>> = Vec::new();
    let mut record = |b: &[u8]| {
        reported.push(b.to_vec());
        Ok(())
    };
    assert_eq!(
        s.write_credential_entry(&f.env, &f.paths, b"new", &open, &mut record)
            .unwrap(),
        SecretStore::Fallback(f.paths.credentials_file.clone())
    );
    assert_eq!(reported, vec![b"old".to_vec()]);
    assert!(f.kc.get(&oauth, &acct).is_none());
    assert_eq!(f.kc.get(INERT_OAUTH, &acct).unwrap(), b"inert");
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
        s.write_credential_entry(&f.env, &f.paths, b"new", &open, &mut save_nothing)
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

// --- Fix round 2 -----------------------------------------------------------------

#[test]
fn restore_bumps_the_credentials_file_after_the_item_it_reflects() {
    let f = fx();
    fs::write(&f.paths.credentials_file, "orig-file").unwrap();
    let probe = Arc::new(MtimeProbeKeychain::new(
        f.kc.clone(),
        f.paths.credentials_file.clone(),
    ));
    let s = LiveStore::new(probe.clone(), Platform::MacOs).with_retry_delay(Duration::ZERO);
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, b"orig-item");

    let snap = s.snapshot(&f.env, &f.paths).unwrap();
    s.write_credential_entry(&f.env, &f.paths, b"target", &open, &mut save_nothing)
        .unwrap();
    assert_eq!(f.kc.get(&svc, &acct).unwrap(), b"target");
    assert_eq!(fs::read(&f.paths.credentials_file).unwrap(), b"target");

    let mtime_before_restore = fs::metadata(&f.paths.credentials_file)
        .unwrap()
        .modified()
        .unwrap();
    s.restore(&f.env, &f.paths, &snap, &open).unwrap();

    assert_eq!(f.kc.get(&svc, &acct).unwrap(), b"orig-item");
    assert_eq!(fs::read(&f.paths.credentials_file).unwrap(), b"orig-file");
    let mtime_at_item_write = probe.mtime_at_last_item_write().unwrap();
    let mtime_after_restore = fs::metadata(&f.paths.credentials_file)
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(
        mtime_at_item_write, mtime_before_restore,
        "the item must be restored while the file still shows its pre-restore mtime"
    );
    assert!(
        mtime_after_restore > mtime_at_item_write,
        "the file's hot-reload bump must land strictly after the item restore"
    );
}

#[test]
fn restore_rewrites_a_matching_credentials_file_when_only_the_item_differed() {
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, b"orig-item");
    fs::write(&f.paths.credentials_file, "same-file").unwrap();
    let snap = s.snapshot(&f.env, &f.paths).unwrap();
    let mtime_before = fs::metadata(&f.paths.credentials_file)
        .unwrap()
        .modified()
        .unwrap();

    // This operation's write changes only the Keychain item: its fence trips before the
    // hot-reload rewrite, so the file stays byte-identical to the snapshot, and a naive "skip
    // when it already matches" would never bump it.
    std::thread::sleep(Duration::from_millis(10));
    let cf = CountingFence::new(1);
    let fence = || cf.check();
    assert!(
        s.write_credential_entry(&f.env, &f.paths, b"changed-item", &fence, &mut save_nothing)
            .is_err()
    );
    assert_eq!(f.kc.get(&svc, &acct).unwrap(), b"changed-item");

    s.restore(&f.env, &f.paths, &snap, &open).unwrap();

    assert_eq!(f.kc.get(&svc, &acct).unwrap(), b"orig-item");
    assert_eq!(fs::read(&f.paths.credentials_file).unwrap(), b"same-file");
    let mtime_after = fs::metadata(&f.paths.credentials_file)
        .unwrap()
        .modified()
        .unwrap();
    assert!(
        mtime_after > mtime_before,
        "the file must be rewritten (its mtime bumped) even though its bytes already matched"
    );
}

#[test]
fn restore_never_creates_a_credentials_file_the_snapshot_says_was_absent() {
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, b"orig-item");
    // No credentials file at snapshot time.
    let snap = s.snapshot(&f.env, &f.paths).unwrap();
    s.write_credential_entry(&f.env, &f.paths, b"changed-item", &open, &mut save_nothing)
        .unwrap();

    s.restore(&f.env, &f.paths, &snap, &open).unwrap();

    assert_eq!(f.kc.get(&svc, &acct).unwrap(), b"orig-item");
    assert!(
        !f.paths.credentials_file.exists(),
        "a restored item must never conjure a credentials file the snapshot never had"
    );
}

// --- Fix round 3 -----------------------------------------------------------------

#[test]
fn a_linux_credential_write_forces_the_file_to_0600() {
    use std::os::unix::fs::PermissionsExt;
    let f = fx();
    let s = store(&f, Platform::Linux);
    fs::write(&f.paths.credentials_file, "old").unwrap();
    fs::set_permissions(&f.paths.credentials_file, fs::Permissions::from_mode(0o644)).unwrap();

    s.write_credential_entry(&f.env, &f.paths, b"{\"a\":1}", &open, &mut save_nothing)
        .unwrap();

    assert_eq!(mode_of(&f.paths.credentials_file), 0o600);
}

#[test]
fn the_hot_reload_mirror_forces_the_file_to_0600() {
    use std::os::unix::fs::PermissionsExt;
    let f = fx();
    let s = store(&f, Platform::MacOs);
    fs::write(&f.paths.credentials_file, "old").unwrap();
    fs::set_permissions(&f.paths.credentials_file, fs::Permissions::from_mode(0o644)).unwrap();

    assert_eq!(
        s.write_credential_entry(&f.env, &f.paths, b"v2", &open, &mut save_nothing)
            .unwrap(),
        SecretStore::Keychain
    );

    assert_eq!(fs::read(&f.paths.credentials_file).unwrap(), b"v2");
    assert_eq!(mode_of(&f.paths.credentials_file), 0o600);
}

#[test]
fn restore_forces_the_credentials_file_to_0600() {
    use std::os::unix::fs::PermissionsExt;
    let f = fx();
    let s = store(&f, Platform::MacOs);
    fs::write(&f.paths.credentials_file, "orig-file").unwrap();
    let snap = s.snapshot(&f.env, &f.paths).unwrap();
    s.write_credential_entry(&f.env, &f.paths, b"tampered", &open, &mut save_nothing)
        .unwrap();
    fs::set_permissions(&f.paths.credentials_file, fs::Permissions::from_mode(0o644)).unwrap();

    s.restore(&f.env, &f.paths, &snap, &open).unwrap();

    assert_eq!(fs::read(&f.paths.credentials_file).unwrap(), b"orig-file");
    assert_eq!(mode_of(&f.paths.credentials_file), 0o600);
}

#[test]
fn a_symlinked_credentials_file_stays_a_symlink_with_its_target_at_0600() {
    use std::os::unix::fs::PermissionsExt;
    let f = fx();
    let s = store(&f, Platform::Linux);
    let real = f.env.home.join("dotfiles/credentials.json");
    fs::create_dir_all(real.parent().unwrap()).unwrap();
    fs::write(&real, "old").unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o644)).unwrap();
    std::os::unix::fs::symlink(&real, &f.paths.credentials_file).unwrap();

    s.write_credential_entry(&f.env, &f.paths, b"{\"a\":1}", &open, &mut save_nothing)
        .unwrap();

    assert!(
        fs::symlink_metadata(&f.paths.credentials_file)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the credentials file path must stay a symlink"
    );
    assert_eq!(fs::read(&real).unwrap(), b"{\"a\":1}");
    assert_eq!(mode_of(&real), 0o600);
}

// --- CC's storage-write lock (§9.1) ----------------------------------------------

/// Wraps a `FakeKeychain` and records, at every `upsert`/`delete`, the item and whether CC's
/// storage-write lock, both spellings of it (`.storage-write` and `.storage-write.lock`), was
/// held at that moment (§9.1).
struct LockProbeKeychain {
    inner: Arc<FakeKeychain>,
    lock: PathBuf,
    lock_v2: PathBuf,
    writes: Mutex<Vec<(String, bool)>>,
}

impl LockProbeKeychain {
    fn new(inner: Arc<FakeKeychain>, lock: PathBuf, lock_v2: PathBuf) -> Self {
        Self {
            inner,
            lock,
            lock_v2,
            writes: Mutex::new(Vec::new()),
        }
    }

    fn writes(&self) -> Vec<(String, bool)> {
        self.writes.lock().unwrap().clone()
    }

    fn record(&self, s: &str) {
        let held = self.lock.is_dir() && self.lock_v2.is_dir();
        self.writes.lock().unwrap().push((s.to_owned(), held));
    }
}

impl Keychain for LockProbeKeychain {
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

/// A live OAuth entry as CC leaves it: `rt`'s login, and an MCP server's token `mcp`, which is
/// machine-shared (Appendix A.4).
fn cc_login(rt: &str, mcp: &str) -> Vec<u8> {
    json!({
        "claudeAiOauth": {"accessToken": format!("at-{rt}"), "refreshToken": rt},
        "mcpOAuth": {"srv": {"token": mcp}}
    })
    .to_string()
    .into_bytes()
}

/// CC's dead-token marking of `cc_login(_, mcp)` (Appendix A.3): both tokens empty and
/// `expiresAt` 0. CC makes it without the credential locks, and it is no conflict (§9.1).
fn cc_wiped(mcp: &str) -> Vec<u8> {
    json!({
        "claudeAiOauth": {"accessToken": "", "refreshToken": "", "expiresAt": 0},
        "mcpOAuth": {"srv": {"token": mcp}}
    })
    .to_string()
    .into_bytes()
}

/// A marking together with another account-scoped change, a new `trustedDeviceToken`: not a
/// marking alone, so a write that finds it aborts (§9.1).
fn cc_wiped_and_more(mcp: &str) -> Vec<u8> {
    json!({
        "claudeAiOauth": {"accessToken": "", "refreshToken": "", "expiresAt": 0},
        "trustedDeviceToken": "cc-device",
        "mcpOAuth": {"srv": {"token": mcp}}
    })
    .to_string()
    .into_bytes()
}

/// Claude Code holding its storage-write lock (§9.1): it takes the lock now, runs `write` 300 ms
/// later, then lets go. The thread returns the instant just before it let go.
fn cc_writes_under_the_lock(
    lock: &Path,
    write: impl FnOnce() + Send + 'static,
) -> thread::JoinHandle<Instant> {
    fs::create_dir(lock).unwrap();
    let lock = lock.to_path_buf();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(300));
        write();
        let at = Instant::now();
        fs::remove_dir(&lock).unwrap();
        at
    })
}

/// Runs `write` while CC holds the storage-write lock for good: it must wait at least the
/// store's 300 ms and end with a timeout naming the lock.
fn times_out<T: std::fmt::Debug>(
    lock: &Path,
    what: &str,
    write: impl FnOnce() -> Result<T, ProviderError>,
) {
    let start = Instant::now();
    match write() {
        Err(ProviderError::Lock(LockError::Timeout(path))) => assert_eq!(path, lock, "{what}"),
        other => panic!("{what}: expected a timeout on the storage-write lock, got {other:?}"),
    }
    assert!(
        start.elapsed() >= Duration::from_millis(300),
        "{what} gave up without waiting"
    );
}

#[test]
fn every_credential_entry_write_holds_the_storage_write_lock_and_releases_it() {
    // §9.1, B #61: every write and delete of a CC credential entry holds the lock, a restore's
    // included, and releases it when the write returns.
    let f = explicit_default();
    let lock = f.paths.storage_write_lock.clone();
    let lock_v2 = f.paths.storage_write_lock_v2.clone();
    let probe = Arc::new(LockProbeKeychain::new(
        f.kc.clone(),
        lock.clone(),
        lock_v2.clone(),
    ));
    let s = LiveStore::new(probe.clone(), Platform::MacOs).with_retry_delay(Duration::ZERO);
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, &cc_login("rt-0", "m0"));
    fs::write(&f.paths.credentials_file, cc_login("rt-0", "m0")).unwrap();
    fs::write(&f.paths.global_config, "{}").unwrap();
    let snap = s.snapshot(&f.env, &f.paths).unwrap();
    let released = |what: &str| {
        assert!(
            !lock.exists() && !lock_v2.exists(),
            "{what} left the lock behind"
        )
    };

    s.write_credential_entry(
        &f.env,
        &f.paths,
        &cc_login("rt-1", "m0"),
        &open,
        &mut save_nothing,
    )
    .unwrap();
    released("an OAuth write and its hot-reload rewrite");
    f.kc.set_fail_write(&svc, true);
    let mut reported_under_the_lock = Vec::new();
    let mut report = |_: &[u8]| {
        reported_under_the_lock.push(lock.is_dir() && lock_v2.is_dir());
        Ok(())
    };
    s.write_credential_entry(
        &f.env,
        &f.paths,
        &cc_login("rt-2", "m0"),
        &open,
        &mut report,
    )
    .unwrap();
    f.kc.set_fail_write(&svc, false);
    released("a file fallback");
    s.clear_credential_account_keys(&f.env, &f.paths, &open)
        .unwrap();
    released("an OAuth clear");
    s.write_managed_key(
        &f.env,
        &f.paths,
        b"sk-ant-api03-0123456789abcdefghijKLMNOPQRST",
        &open,
        &mut save_nothing,
    )
    .unwrap();
    released("a managed-key write");
    s.clear_managed_key(&f.env, &f.paths, &open).unwrap();
    released("a managed-key delete");
    s.restore(&f.env, &f.paths, &snap, &open).unwrap();
    released("a restore");

    assert_eq!(
        reported_under_the_lock,
        [true],
        "a fallback reports the item it deletes under the lock"
    );
    let writes = probe.writes();
    assert!(writes.len() >= 6, "{writes:?}");
    assert!(
        writes.iter().all(|(_, held)| *held),
        "a Keychain write without the storage-write lock: {writes:?}"
    );
}

/// A fence that records, at each call, whether CC's storage-write lock is held.
fn lock_probe(lock: &Path, seen: &Mutex<Vec<bool>>) -> impl Fn() -> Result<(), ProviderError> {
    let (lock, seen) = (lock.to_path_buf(), seen);
    move || {
        seen.lock().unwrap().push(lock.is_dir());
        Ok(())
    }
}

#[test]
fn on_linux_the_credentials_file_alone_is_written_and_rolled_back_under_the_storage_write_lock() {
    // §9.1, §9.4 step 10 on Linux: `.credentials.json` is the whole entry (no Keychain item, no
    // managed-key axis). The write and the restore each hold the lock around their file
    // writes, release it, and put the file back byte for byte while nothing else wrote it.
    let f = fx();
    let s = store(&f, Platform::Linux);
    let lock = f.paths.storage_write_lock.clone();
    let before = b"{\n  \"claudeAiOauth\": {\"accessToken\": \"at-rt-0\", \"refreshToken\": \"rt-0\"},\n  \"mcpOAuth\": {\"srv\": {\"token\": \"m0\"}}\n}\n";
    fs::write(&f.paths.credentials_file, before).unwrap();
    let snap = s.snapshot(&f.env, &f.paths).unwrap();
    let seen = Mutex::new(Vec::new());
    let probe = lock_probe(&lock, &seen);

    let store_used = s
        .write_credential_entry(
            &f.env,
            &f.paths,
            &cc_login("rt-1", "m0"),
            &probe,
            &mut save_nothing,
        )
        .unwrap();

    assert_eq!(
        store_used,
        SecretStore::File(f.paths.credentials_file.clone())
    );
    assert_eq!(
        fs::read(&f.paths.credentials_file).unwrap(),
        cc_login("rt-1", "m0")
    );
    let writes = std::mem::take(&mut *seen.lock().unwrap());
    assert!(
        !writes.is_empty() && writes.iter().all(|held| *held),
        "{writes:?}"
    );
    assert!(!lock.exists(), "the write released the lock");

    s.restore(&f.env, &f.paths, &snap, &probe).unwrap();

    assert_eq!(
        fs::read(&f.paths.credentials_file).unwrap(),
        before,
        "byte for byte"
    );
    let restores = std::mem::take(&mut *seen.lock().unwrap());
    assert!(
        !restores.is_empty() && restores.iter().all(|held| *held),
        "{restores:?}"
    );
    assert!(!lock.exists(), "the restore released the lock");
    assert!(
        f.kc.items().is_empty(),
        "no Keychain item was ever involved"
    );
}

#[test]
fn on_linux_a_restore_leaves_a_credentials_file_cc_wrote_since_and_names_it() {
    let f = fx();
    let s = store(&f, Platform::Linux);
    fs::write(&f.paths.credentials_file, cc_login("rt-0", "m0")).unwrap();
    let snap = s.snapshot(&f.env, &f.paths).unwrap();
    s.write_credential_entry(
        &f.env,
        &f.paths,
        &cc_login("rt-1", "m0"),
        &open,
        &mut save_nothing,
    )
    .unwrap();
    fs::write(&f.paths.credentials_file, cc_wiped("m0")).unwrap(); // CC's dead-token marking

    match s.restore(&f.env, &f.paths, &snap, &open) {
        Err(ProviderError::Incomplete { failed }) => assert_eq!(
            failed,
            [format!(
                "{} (changed since tagteam wrote it; left as it is)",
                f.paths.credentials_file.display()
            )]
        ),
        other => panic!("expected the file named as left, got {other:?}"),
    }
    assert_eq!(
        fs::read(&f.paths.credentials_file).unwrap(),
        cc_wiped("m0"),
        "CC's write stands"
    );
    assert!(!f.paths.storage_write_lock.exists());
}

#[test]
fn a_held_storage_write_lock_makes_every_entry_write_wait_then_time_out() {
    // §9.1: 9 s in production; this store waits 300 ms. Nothing is written, and CC's lock is
    // left alone.
    let f = fx();
    let s = store(&f, Platform::MacOs).with_storage_write_timeout(Duration::from_millis(300));
    let lock = f.paths.storage_write_lock.clone();
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, &cc_login("rt-0", "m0"));
    fs::write(&f.paths.global_config, "{}").unwrap();
    let snap = s.snapshot(&f.env, &f.paths).unwrap();
    s.write_credential_entry(
        &f.env,
        &f.paths,
        &cc_login("rt-1", "m0"),
        &open,
        &mut save_nothing,
    )
    .unwrap();
    let items = f.kc.items();
    fs::create_dir(&lock).unwrap(); // CC holds it, freshly

    times_out(&lock, "an OAuth write", || {
        s.write_credential_entry(
            &f.env,
            &f.paths,
            &cc_login("rt-2", "m0"),
            &open,
            &mut save_nothing,
        )
    });
    times_out(&lock, "an OAuth clear", || {
        s.clear_credential_account_keys(&f.env, &f.paths, &open)
    });
    times_out(&lock, "a managed-key write", || {
        s.write_managed_key(
            &f.env,
            &f.paths,
            b"sk-ant-api03-0123456789abcdefghijKLMNOPQRST",
            &open,
            &mut save_nothing,
        )
    });
    times_out(&lock, "a managed-key delete", || {
        s.clear_managed_key(&f.env, &f.paths, &open)
    });
    // The managed-key write's approval lands in the config first, as before (§9.4 step 7);
    // a switch's rollback restores it. No credential entry was written.
    assert_eq!(f.kc.items(), items, "no Keychain item was written");
    assert!(!f.paths.credentials_file.exists());

    // A restore waits for each entry this operation wrote, here the OAuth entry only, and
    // names the one it could not restore.
    let start = Instant::now();
    match s.restore(&f.env, &f.paths, &snap, &open) {
        Err(ProviderError::Incomplete { failed }) => {
            assert_eq!(failed.len(), 1, "{failed:?}");
            assert!(
                failed[0].starts_with(&svc) && failed[0].contains(".storage-write"),
                "{failed:?}"
            );
        }
        other => panic!("expected the OAuth entry left unrestored, got {other:?}"),
    }
    assert!(start.elapsed() >= Duration::from_millis(300));
    assert_eq!(f.kc.get(&svc, &acct).unwrap(), cc_login("rt-1", "m0"));
    assert!(lock.is_dir(), "CC's lock is left alone");
}

#[test]
fn a_write_waits_for_cc_and_keeps_the_machine_shared_keys_cc_wrote_meanwhile() {
    // §9.1: the machine-shared keys are taken from the read under the lock, so CC's write made
    // since tagteam's earlier read is never lost (B #61).
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, &cc_login("rt-0", "m0"));
    s.snapshot(&f.env, &f.paths).unwrap(); // tagteam's read under the credential locks
    let (kc, cc_svc, cc_acct) = (f.kc.clone(), svc.clone(), acct.clone());
    let cc = cc_writes_under_the_lock(&f.paths.storage_write_lock, move || {
        kc.put(&cc_svc, &cc_acct, &cc_login("rt-0", "m1")) // CC refreshed its MCP token
    });

    // Composed from the earlier read, so it carries the old MCP token.
    s.write_credential_entry(
        &f.env,
        &f.paths,
        &cc_login("rt-1", "m0"),
        &open,
        &mut save_nothing,
    )
    .unwrap();
    let ended = Instant::now();
    let released = cc.join().unwrap();

    assert!(ended > released, "the write waited for CC's lock");
    assert_eq!(
        json_of(&f.kc.get(&svc, &acct).unwrap()),
        json_of(&cc_login("rt-1", "m1")),
        "the target's login, with the MCP token CC wrote meanwhile"
    );
    assert!(!f.paths.storage_write_lock.exists());
}

#[test]
fn a_write_waits_for_cc_2_1_292_s_storage_write_dot_lock_and_holds_both_spellings() {
    // §9.1: CC 2.1.292 holds `.storage-write.lock`; tagteam waits for it, and while it writes
    // holds `.storage-write` first and `.storage-write.lock` second, then releases both.
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, &cc_login("rt-0", "m0"));
    s.snapshot(&f.env, &f.paths).unwrap();
    let (kc, cc_svc, cc_acct) = (f.kc.clone(), svc.clone(), acct.clone());
    let cc = cc_writes_under_the_lock(&f.paths.storage_write_lock_v2, move || {
        kc.put(&cc_svc, &cc_acct, &cc_login("rt-0", "m1"))
    });
    let seen = Mutex::new(Vec::new());
    let (a, b) = (
        f.paths.storage_write_lock.clone(),
        f.paths.storage_write_lock_v2.clone(),
    );
    let probe = || {
        seen.lock().unwrap().push(a.is_dir() && b.is_dir());
        Ok(())
    };

    s.write_credential_entry(
        &f.env,
        &f.paths,
        &cc_login("rt-1", "m0"),
        &probe,
        &mut save_nothing,
    )
    .unwrap();
    let ended = Instant::now();
    let released = cc.join().unwrap();

    assert!(ended > released, "the write waited for CC's lock");
    assert_eq!(
        json_of(&f.kc.get(&svc, &acct).unwrap()),
        json_of(&cc_login("rt-1", "m1")),
        "the target's login, with the MCP token CC wrote meanwhile"
    );
    let held = seen.lock().unwrap().clone();
    assert!(
        !held.is_empty() && held.iter().all(|h| *h),
        "both held at every write: {held:?}"
    );
    assert!(!a.exists() && !b.exists(), "both released");
}

#[test]
fn a_held_storage_write_dot_lock_makes_an_entry_write_wait_then_time_out() {
    let f = fx();
    let s = store(&f, Platform::MacOs).with_storage_write_timeout(Duration::from_millis(300));
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, &cc_login("rt-0", "m0"));
    fs::write(&f.paths.global_config, "{}").unwrap();
    let items = f.kc.items();
    fs::create_dir(&f.paths.storage_write_lock_v2).unwrap(); // CC 2.1.292 holds it, freshly

    times_out(&f.paths.storage_write_lock_v2, "an OAuth write", || {
        s.write_credential_entry(
            &f.env,
            &f.paths,
            &cc_login("rt-2", "m0"),
            &open,
            &mut save_nothing,
        )
    });

    assert_eq!(f.kc.items(), items, "no Keychain item was written");
    assert!(
        !f.paths.storage_write_lock.exists(),
        "the timeout released the first spelling"
    );
    assert!(
        f.paths.storage_write_lock_v2.is_dir(),
        "CC's lock is left alone"
    );
}

#[test]
fn a_write_after_cc_changed_the_account_scoped_keys_aborts_and_writes_nothing() {
    // §9.1: CC changes the account-scoped keys under the credential locks tagteam holds only by
    // refreshing, and outside them only by its dead-token marking. Any other change found under
    // the storage-write lock aborts the write: here a marking plus a new `trustedDeviceToken`.
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, &cc_login("rt-0", "m0"));
    fs::write(&f.paths.credentials_file, cc_login("rt-0", "m0")).unwrap();
    s.snapshot(&f.env, &f.paths).unwrap();
    let (kc, cc_svc, cc_acct) = (f.kc.clone(), svc.clone(), acct.clone());
    let cc = cc_writes_under_the_lock(&f.paths.storage_write_lock, move || {
        kc.put(&cc_svc, &cc_acct, &cc_wiped_and_more("m0"))
    });

    let written = s.write_credential_entry(
        &f.env,
        &f.paths,
        &cc_login("rt-1", "m0"),
        &open,
        &mut save_nothing,
    );
    cc.join().unwrap();

    match written {
        Err(ProviderError::EntryMoved(name)) => assert_eq!(name, svc),
        other => panic!("expected the write to abort, got {other:?}"),
    }
    // A clear of the same entry aborts the same way.
    assert!(matches!(
        s.clear_credential_account_keys(&f.env, &f.paths, &open),
        Err(ProviderError::EntryMoved(_))
    ));
    assert_eq!(
        f.kc.get(&svc, &acct).unwrap(),
        cc_wiped_and_more("m0"),
        "CC's write stands"
    );
    assert_eq!(
        fs::read(&f.paths.credentials_file).unwrap(),
        cc_login("rt-0", "m0"),
        "nothing was written, the hot-reload file included"
    );
    assert!(!f.paths.storage_write_lock.exists());
}

#[test]
fn a_write_goes_ahead_over_cc_s_dead_token_marking_and_keeps_cc_s_mcp_token() {
    // §9.1: a marking is no conflict. It holds no secret, and the writer holds the generation
    // CC marked or a newer one. The machine-shared keys still come from the read under the
    // lock.
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, &cc_login("rt-0", "m0"));
    s.snapshot(&f.env, &f.paths).unwrap();
    let (kc, cc_svc, cc_acct) = (f.kc.clone(), svc.clone(), acct.clone());
    let cc = cc_writes_under_the_lock(&f.paths.storage_write_lock, move || {
        kc.put(&cc_svc, &cc_acct, &cc_wiped("m1"))
    });

    s.write_credential_entry(
        &f.env,
        &f.paths,
        &cc_login("rt-1", "m0"),
        &open,
        &mut save_nothing,
    )
    .unwrap();
    let ended = Instant::now();
    let released = cc.join().unwrap();

    assert!(ended > released, "the write waited for CC's lock");
    assert_eq!(
        json_of(&f.kc.get(&svc, &acct).unwrap()),
        json_of(&cc_login("rt-1", "m1")),
        "the target's login over CC's marking, with the MCP token CC wrote"
    );
}

#[test]
fn a_restore_leaves_an_entry_changed_since_tagteam_wrote_it_and_names_it() {
    // A restore puts a place back only while it holds exactly what tagteam last wrote there.
    // CC's write since stays as CC wrote it, whatever it changed: account-scoped keys, its
    // dead-token marking alone, or only an MCP token. The restore names the place and still
    // restores everything else, here `~/.claude.json`.
    for cc in [
        cc_wiped_and_more("m0"),
        cc_wiped("m1"),
        cc_login("rt-1", "m1"),
    ] {
        let f = fx();
        let s = store(&f, Platform::MacOs);
        let (svc, acct) = oauth_svc(&f);
        f.kc.put(&svc, &acct, &cc_login("rt-0", "m0"));
        fs::write(&f.paths.global_config, "{\"a\": 1}").unwrap();
        let snap = s.snapshot(&f.env, &f.paths).unwrap();
        s.write_credential_entry(
            &f.env,
            &f.paths,
            &cc_login("rt-1", "m0"),
            &open,
            &mut save_nothing,
        )
        .unwrap();
        config::splice_key(&f.paths.global_config, "b", Some(&json!(2)), &open).unwrap();
        f.kc.put(&svc, &acct, &cc);

        match s.restore(&f.env, &f.paths, &snap, &open) {
            Err(ProviderError::Incomplete { failed }) => assert_eq!(
                failed,
                [format!(
                    "{svc} (changed since tagteam wrote it; left as it is)"
                )]
            ),
            other => panic!("expected the item named as left, got {other:?}"),
        }
        assert_eq!(f.kc.get(&svc, &acct).unwrap(), cc, "CC's write stands");
        assert_eq!(fs::read(&f.paths.global_config).unwrap(), b"{\"a\": 1}");
    }
}

#[test]
fn a_restore_leaves_the_whole_entry_once_cc_wrote_to_an_item_tagteam_created() {
    // Codex rounds 6 and 7: the login is in the file, with no Keychain item. tagteam's write
    // creates the item and mirrors the file; CC then refreshes its MCP token in the item, the
    // only place that holds it. The item stays as CC wrote it, and the file as tagteam wrote
    // it: CC keeps reading the one entry it wrote to.
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    fs::write(&f.paths.credentials_file, cc_login("rt-0", "m0")).unwrap(); // no item
    let snap = s.snapshot(&f.env, &f.paths).unwrap();
    s.write_credential_entry(
        &f.env,
        &f.paths,
        &cc_login("rt-1", "m0"),
        &open,
        &mut save_nothing,
    )
    .unwrap();
    let mirrored = fs::read(&f.paths.credentials_file).unwrap();
    f.kc.put(&svc, &acct, &cc_login("rt-1", "m1")); // CC refreshed its MCP token

    match s.restore(&f.env, &f.paths, &snap, &open) {
        Err(ProviderError::Incomplete { failed }) => assert_eq!(
            failed,
            [format!(
                "{svc} (changed since tagteam wrote it; left as it is)"
            )]
        ),
        other => panic!("expected the item named as left, got {other:?}"),
    }
    assert_eq!(
        f.kc.get(&svc, &acct).unwrap(),
        cc_login("rt-1", "m1"),
        "CC's write stands"
    );
    assert_eq!(fs::read(&f.paths.credentials_file).unwrap(), mirrored);
}

#[test]
fn a_restore_puts_nothing_back_in_front_of_a_place_cc_changed() {
    // tagteam's write fell back: it wrote the file and deleted the item. CC then wrote the file,
    // its own Keychain write failing too. Putting the item back would hide CC's write, since a
    // reader tries the item first: the whole entry stays as it is, and the file is named.
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, &cc_login("rt-0", "m0"));
    let snap = s.snapshot(&f.env, &f.paths).unwrap();
    f.kc.set_fail_write(&svc, true);
    s.write_credential_entry(
        &f.env,
        &f.paths,
        &cc_login("rt-1", "m0"),
        &open,
        &mut save_nothing,
    )
    .unwrap();
    f.kc.set_fail_write(&svc, false);
    fs::write(&f.paths.credentials_file, cc_login("rt-1", "m1")).unwrap(); // CC's write

    match s.restore(&f.env, &f.paths, &snap, &open) {
        Err(ProviderError::Incomplete { failed }) => assert_eq!(
            failed,
            [format!(
                "{} (changed since tagteam wrote it; left as it is)",
                f.paths.credentials_file.display()
            )]
        ),
        other => panic!("expected the file named as left, got {other:?}"),
    }
    assert_eq!(f.kc.get(&svc, &acct), None, "no item hides CC's write");
    assert_eq!(
        fs::read(&f.paths.credentials_file).unwrap(),
        cc_login("rt-1", "m1")
    );
}

#[test]
fn a_restore_leaves_a_place_cc_wrote_between_two_of_tagteam_s_writes() {
    // CC refreshed its MCP token in the item between two of tagteam's writes, and the second
    // carried it on. The item holds exactly what tagteam last wrote, but what it held before
    // tagteam's first write predates CC's token: putting that back would lose the token, so
    // the item is left and named.
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, &cc_login("rt-0", "m0"));
    let snap = s.snapshot(&f.env, &f.paths).unwrap();
    s.write_credential_entry(
        &f.env,
        &f.paths,
        &cc_login("rt-1", "m0"),
        &open,
        &mut save_nothing,
    )
    .unwrap();
    f.kc.put(&svc, &acct, &cc_login("rt-1", "m1")); // CC refreshed its MCP token
    s.write_credential_entry(
        &f.env,
        &f.paths,
        &cc_login("rt-2", "m0"),
        &open,
        &mut save_nothing,
    )
    .unwrap();
    let last = f.kc.get(&svc, &acct).unwrap();

    match s.restore(&f.env, &f.paths, &snap, &open) {
        Err(ProviderError::Incomplete { failed }) => assert_eq!(
            failed,
            [format!(
                "{svc} (changed since tagteam wrote it; left as it is)"
            )]
        ),
        other => panic!("expected the item named as left, got {other:?}"),
    }
    assert_eq!(f.kc.get(&svc, &acct).unwrap(), last);
}

#[test]
fn a_restore_leaves_alone_an_entry_this_operation_never_wrote() {
    // A restore undoes this operation's writes only: CC's change to an entry tagteam did not
    // write is neither overwritten nor a failure.
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    let managed = keychain_service(&f.env, ItemKind::ManagedKey);
    f.kc.put(&svc, &acct, &cc_login("rt-0", "m0"));
    fs::write(&f.paths.global_config, "{}").unwrap();
    let snap = s.snapshot(&f.env, &f.paths).unwrap();
    s.write_managed_key(
        &f.env,
        &f.paths,
        b"sk-ant-api03-0123456789abcdefghijKLMNOPQRST",
        &open,
        &mut save_nothing,
    )
    .unwrap();
    f.kc.put(&svc, &acct, &cc_wiped("m0"));

    s.restore(&f.env, &f.paths, &snap, &open).unwrap();

    assert_eq!(f.kc.get(&managed, &acct), None, "the managed key is undone");
    assert_eq!(f.kc.get(&svc, &acct).unwrap(), cc_wiped("m0"));
}

#[test]
fn a_signal_ends_a_writes_wait_for_the_storage_write_lock_but_never_a_restores() {
    // §9.1, §14.1: the wait is a cancellation point for a write given a token that is set; a
    // restore is a rollback, so it waits under a token nothing sets and runs to completion.
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let lock = f.paths.storage_write_lock.clone();
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, &cc_login("rt-0", "m0"));
    let snap = s.snapshot(&f.env, &f.paths).unwrap();
    s.write_credential_entry(
        &f.env,
        &f.paths,
        &cc_login("rt-1", "m0"),
        &open,
        &mut save_nothing,
    )
    .unwrap();
    fs::create_dir(&lock).unwrap(); // CC holds it
    let cancel = f.env.cancel.clone();
    let ctrl_c = thread::spawn(move || {
        thread::sleep(Duration::from_millis(100));
        cancel.request(libc::SIGINT);
    });

    let start = Instant::now();
    let written = s.write_credential_entry(
        &f.env,
        &f.paths,
        &cc_login("rt-2", "m0"),
        &open,
        &mut save_nothing,
    );
    ctrl_c.join().unwrap();

    match written {
        Err(ProviderError::Lock(LockError::Interrupted { path, signal })) => {
            assert_eq!((path, signal), (lock.clone(), libc::SIGINT))
        }
        other => panic!("expected an interrupted wait, got {other:?}"),
    }
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "not the 9 s budget"
    );
    assert_eq!(f.kc.get(&svc, &acct).unwrap(), cc_login("rt-1", "m0"));
    assert!(lock.is_dir(), "CC's lock is left alone");

    let cc = cc_lets_go_soon(&lock);
    s.restore(&f.env, &f.paths, &snap, &open).unwrap();
    cc.join().unwrap();
    assert_eq!(
        f.kc.get(&svc, &acct).unwrap(),
        cc_login("rt-0", "m0"),
        "the restore waited for CC with the signal set, and ran"
    );
}

/// CC letting go of a storage-write lock it holds, 300 ms from now.
fn cc_lets_go_soon(lock: &Path) -> thread::JoinHandle<()> {
    let lock = lock.to_path_buf();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(300));
        fs::remove_dir(&lock).unwrap();
    })
}

#[test]
fn a_clear_never_reads_or_touches_an_inert_item_another_writer_changed() {
    // Appendix A.2 (2.1.286): the unsuffixed item of an explicit `~/.claude` is no place of the
    // entry, so another writer's change to it is no conflict (§9.1 compares only places), and
    // the clear neither reads nor writes it.
    let f = explicit_default();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, &cc_login("rt-0", "m0"));
    f.kc.put(INERT_OAUTH, &acct, &cc_login("rt-0", "m0"));
    s.snapshot(&f.env, &f.paths).unwrap();
    let (kc, other_acct) = (f.kc.clone(), acct.clone());
    let cc = cc_writes_under_the_lock(&f.paths.storage_write_lock, move || {
        kc.put(INERT_OAUTH, &other_acct, &cc_login("rt-other", "m0"))
    });

    let cleared = s.clear_credential_account_keys(&f.env, &f.paths, &open);
    cc.join().unwrap();

    cleared.unwrap();
    assert_eq!(
        json_of(&f.kc.get(&svc, &acct).unwrap()),
        json!({"mcpOAuth": {"srv": {"token": "m0"}}})
    );
    assert_eq!(
        f.kc.get(INERT_OAUTH, &acct).unwrap(),
        cc_login("rt-other", "m0")
    );
}

#[test]
fn a_clear_goes_ahead_over_cc_s_marking_of_its_item() {
    // §9.1: a marking is no conflict at any place. The clear keeps only what the marked item
    // still holds that is machine-shared.
    let f = explicit_default();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, &cc_login("rt-0", "m0"));
    s.snapshot(&f.env, &f.paths).unwrap();
    let (kc, item, item_acct) = (f.kc.clone(), svc.clone(), acct.clone());
    let cc = cc_writes_under_the_lock(&f.paths.storage_write_lock, move || {
        kc.put(&item, &item_acct, &cc_wiped("m1"))
    });

    s.clear_credential_account_keys(&f.env, &f.paths, &open)
        .unwrap();
    let ended = Instant::now();
    let released = cc.join().unwrap();

    assert!(ended > released, "the clear waited for CC's lock");
    assert_eq!(
        json_of(&f.kc.get(&svc, &acct).unwrap()),
        json!({"mcpOAuth": {"srv": {"token": "m1"}}}),
        "the item keeps only its machine-shared keys, as the marking left them"
    );
}

#[test]
fn a_write_aborts_when_another_writer_changed_the_credentials_file() {
    // The hot-reload rewrite would overwrite the file, so it is checked like the item.
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, &cc_login("rt-0", "m0"));
    fs::write(&f.paths.credentials_file, cc_login("rt-0", "m0")).unwrap();
    s.snapshot(&f.env, &f.paths).unwrap();
    let file = f.paths.credentials_file.clone();
    let cc = cc_writes_under_the_lock(&f.paths.storage_write_lock, move || {
        fs::write(&file, cc_login("rt-other", "m0")).unwrap()
    });

    let written = s.write_credential_entry(
        &f.env,
        &f.paths,
        &cc_login("rt-1", "m0"),
        &open,
        &mut save_nothing,
    );
    cc.join().unwrap();

    match written {
        Err(ProviderError::EntryMoved(name)) => {
            assert_eq!(name, f.paths.credentials_file.display().to_string())
        }
        other => panic!("expected the write to abort, got {other:?}"),
    }
    assert_eq!(f.kc.get(&svc, &acct).unwrap(), cc_login("rt-0", "m0"));
    assert_eq!(
        fs::read(&f.paths.credentials_file).unwrap(),
        cc_login("rt-other", "m0")
    );
}

#[test]
fn a_write_goes_ahead_over_a_marking_of_the_credentials_file() {
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, &cc_login("rt-0", "m0"));
    fs::write(&f.paths.credentials_file, cc_login("rt-0", "m0")).unwrap();
    s.snapshot(&f.env, &f.paths).unwrap();
    let file = f.paths.credentials_file.clone();
    let cc = cc_writes_under_the_lock(&f.paths.storage_write_lock, move || {
        fs::write(&file, cc_wiped("m0")).unwrap()
    });

    s.write_credential_entry(
        &f.env,
        &f.paths,
        &cc_login("rt-1", "m0"),
        &open,
        &mut save_nothing,
    )
    .unwrap();
    let ended = Instant::now();
    let released = cc.join().unwrap();

    assert!(ended > released, "the write waited for CC's lock");
    let item = f.kc.get(&svc, &acct).unwrap();
    assert_eq!(json_of(&item), json_of(&cc_login("rt-1", "m0")));
    assert_eq!(
        fs::read(&f.paths.credentials_file).unwrap(),
        item,
        "the file mirrors the item again"
    );
}

#[test]
fn a_restore_puts_each_place_back_byte_for_byte() {
    // Each place goes back to exactly what it held before tagteam changed it, its own
    // machine-shared keys included: the item and the file hold different MCP tokens, and a
    // clear keeps each one's. Nothing else wrote in between.
    let f = fx();
    let s = store(&f, Platform::MacOs);
    let (svc, acct) = oauth_svc(&f);
    f.kc.put(&svc, &acct, &cc_login("rt-0", "item"));
    fs::write(&f.paths.credentials_file, cc_login("rt-0", "file")).unwrap();
    let snap = s.snapshot(&f.env, &f.paths).unwrap();
    s.clear_credential_account_keys(&f.env, &f.paths, &open)
        .unwrap();

    s.restore(&f.env, &f.paths, &snap, &open).unwrap();

    assert_eq!(f.kc.get(&svc, &acct).unwrap(), cc_login("rt-0", "item"));
    assert_eq!(
        fs::read(&f.paths.credentials_file).unwrap(),
        cc_login("rt-0", "file")
    );
}
