//! Claude Code's session facts (§4.5 "Parallel sessions", §12): the share policy, the outer
//! home, the profile spelling, and the profile credential's read and deletion under the
//! hashed Keychain name for a recorded spelling (Appendix A.2).

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use sha2::{Digest, Sha256};
use tagteam_cc::live::{LiveStore, Platform};
use tagteam_cc::{CcPaths, ClaudeCode, keychain_account};
use tagteam_core::AccountId;
use tagteam_provider::{
    EntryKind, Env, FakeKeychain, Keychain, KeychainError, LockError, LockState, MustShare,
    MutationGuard, Provenance, Provider, ProviderError, Read, canonical_profile_path,
    entry_matches, profile_path,
};

struct Fx {
    _d: tempfile::TempDir,
    env: Env,
    kc: Arc<FakeKeychain>,
    cc: ClaudeCode,
}

fn fx_on(platform: Platform) -> Fx {
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    fs::create_dir_all(env.home.join(".claude")).unwrap();
    let kc = Arc::new(FakeKeychain::new());
    let cc = ClaudeCode::with_store(
        LiveStore::new(kc.clone(), platform).with_retry_delay(Duration::ZERO),
    );
    Fx { _d: d, env, kc, cc }
}

fn fx() -> Fx {
    fx_on(Platform::MacOs)
}

/// `"<prefix>-" + hex(sha256(spelling))[..8]` (Appendix A.2), computed here independently of
/// the crate's naming code. `spelling` is already NFC, as `profile_spelling` makes it.
fn hashed(prefix: &str, spelling: &str) -> String {
    format!(
        "{prefix}-{}",
        &hex::encode(Sha256::digest(spelling.as_bytes()).as_slice())[..8]
    )
}

/// A profile directory under the fixture's data dir, and the spelling it is exported as.
fn profile(f: &Fx, id: &str) -> (PathBuf, String) {
    let dir = profile_path(&f.env, &AccountId::from_string(id));
    fs::create_dir_all(&dir).unwrap();
    let spelling =
        f.cc.profile_spelling(&canonical_profile_path(&dir).unwrap());
    (dir, spelling)
}

fn with_vars(env: &Env, config: Option<&str>, secure: Option<&str>) -> Env {
    let mut e = env.clone();
    e.claude_config_dir = config.map(OsString::from);
    e.claude_securestorage_config_dir = secure.map(OsString::from);
    e
}

const ENTRY: &[u8] = br#"{"claudeAiOauth":{"refreshToken":"rt-profile"}}"#;

#[test]
fn claude_code_s_session_facts() {
    let f = fx();
    assert_eq!(f.cc.launch_command(), "claude");
    assert_eq!(f.cc.session_dir_var(), Some("CLAUDE_CONFIG_DIR"));
    assert_eq!(f.cc.session_dir(&f.env), None);
    assert_eq!(
        f.cc.session_dir(&with_vars(&f.env, Some(""), None)),
        None,
        "an empty value is unset (Appendix A.1)"
    );
    assert_eq!(
        f.cc.session_dir(&with_vars(&f.env, Some("/p/0192"), Some(""))),
        Some(PathBuf::from("/p/0192"))
    );
    assert_eq!(
        f.cc.session_records_dir(Path::new("/p/0192")),
        Path::new("/p/0192/sessions")
    );
}

#[test]
fn the_share_policy_is_section_12_2_s_tables_from_the_resolved_home() {
    let f = fx();
    let policy = f.cc.share_policy(&f.env);
    assert_eq!(policy.source, f.env.home.join(".claude"));
    let custom = with_vars(&f.env, Some("/custom/home"), None);
    assert_eq!(
        f.cc.share_policy(&custom).source,
        CcPaths::resolve(&custom).config_home
    );
    assert_eq!(
        policy.must_share,
        vec![
            MustShare {
                name: "projects",
                kind: EntryKind::Dir
            },
            MustShare {
                name: "history.jsonl",
                kind: EntryKind::File
            },
        ]
    );
    assert_eq!(
        policy.shared,
        [
            "CLAUDE.md",
            "settings.json",
            "keybindings.json",
            "agents",
            "commands",
            "skills",
            "plugins",
            "hooks",
            "output-styles",
            "themes",
            "rules",
            "workflows",
            "file-history",
            "paste-cache",
            "shell-snapshots",
            "session-env"
        ]
    );
    let private = |name: &str| policy.private.iter().any(|p| entry_matches(p, name));
    // Every row of §12.2's private table, spelled as CC 2.1.286 writes it.
    for name in [
        ".credentials.json",
        ".claude.json",
        ".claude-custom-oauth.json",
        ".claude-local-oauth.json",
        ".claude-staging-oauth.json",
        ".config.json",
        "sessions",
        "ide",
        "jobs",
        "daemon",
        "daemon.json",
        "daemon.lock",
        "daemon.log",
        "daemon.status.json",
        "daemon.scheduled.status.json",
        "daemon-auth-cooldown",
        "daemon-auth-status.json",
        "backups",
        "cache",
        "mcp-needs-auth-cache.json",
        "stats-cache.json",
        "policy-limits.json",
        "policy-limits.json.signature",
        "policy-limits.json.stamp",
        "remote-settings.json",
        "remote-settings-consent.json",
        "remote-settings-helper-consent",
        ".session_ingress_token",
        "hfi-auth.json",
        "state",
        "seed-admin",
        "bridge-spawn",
        "chrome",
        "debug",
        "feedback",
        "routines",
        "settings.local.json",
        ".last-cleanup",
        ".cc-writes",
        ".device-keys.json",
        ".oauth_refresh.lock",
        ".oauth_refresh.lock.owner",
        ".storage-write",
        ".tagteam-profile.json",
        ".tagteam-launch",
    ] {
        assert!(private(name), "{name} is private");
    }
    for name in policy
        .shared
        .iter()
        .chain(policy.must_share.iter().map(|m| &m.name))
    {
        assert!(!private(name), "{name} is both shared and private");
    }
}

#[test]
fn the_outer_home_round_trips_undefined_set_and_defined_but_empty() {
    let f = fx();
    // In a run shell, CLAUDE_CONFIG_DIR names the profile and the secure-storage dir is scrubbed.
    // The CLI captures the variable into `vars` too (Decision 5).
    let mut inside = with_vars(&f.env, Some("/data/sessions/0192"), None);
    inside
        .vars
        .insert("CLAUDE_CONFIG_DIR".into(), "/data/sessions/0192".into());
    for (config, secure) in [
        (None, None),
        (Some("/custom/home"), None),
        (None, Some("")),
        (Some("/custom/home"), Some("")),
        (Some(""), Some("/secure")),
    ] {
        let outer_env = with_vars(&f.env, config, secure);
        let outer = f.cc.outer_home(&outer_env);
        assert_eq!(
            outer,
            json!({"CLAUDE_CONFIG_DIR": config, "CLAUDE_SECURESTORAGE_CONFIG_DIR": secure}),
            "{config:?}, {secure:?}"
        );
        let restored = f.cc.apply_outer_home(&inside, &outer).unwrap();
        assert_eq!(
            (
                restored.claude_config_dir.clone(),
                restored.claude_securestorage_config_dir.clone()
            ),
            (
                outer_env.claude_config_dir.clone(),
                outer_env.claude_securestorage_config_dir.clone()
            ),
            "{config:?}, {secure:?}"
        );
        assert_eq!(
            restored.var("CLAUDE_CONFIG_DIR"),
            config.map(OsStr::new),
            "{config:?}, {secure:?}: the captured variable follows, undefined included"
        );
        assert_eq!(
            restored.var("CLAUDE_SECURESTORAGE_CONFIG_DIR"),
            None,
            "a variable never captured is not added"
        );
        assert_eq!(restored.home, inside.home, "nothing else moves");
    }
}

#[test]
fn an_outer_record_that_is_not_claude_code_s_is_refused_without_quoting_it() {
    let f = fx();
    for outer in [
        json!("SENTINEL"),
        json!({}),
        json!({"CLAUDE_CONFIG_DIR": null}),
        json!({"CLAUDE_CONFIG_DIR": "SENTINEL", "CLAUDE_SECURESTORAGE_CONFIG_DIR": 7}),
        json!({"FAKEAGENT_HOME": "SENTINEL"}),
    ] {
        match f.cc.apply_outer_home(&f.env, &outer) {
            Err(ProviderError::Invalid(msg)) => assert!(!msg.contains("SENTINEL"), "{msg}"),
            other => panic!("{outer}: {other:?}"),
        }
    }
}

#[test]
fn the_spelling_is_the_nfc_of_the_canonical_path() {
    let f = fx();
    assert_eq!(
        f.cc.profile_spelling(Path::new("/p/cafe\u{301}")),
        "/p/caf\u{e9}"
    );
    let (dir, spelling) = profile(&f, "0192");
    assert_eq!(
        Path::new(&spelling),
        fs::canonicalize(&dir).unwrap(),
        "absolute, resolved, no trailing slash"
    );
}

#[test]
fn the_profile_credential_is_read_from_the_hashed_item_of_its_spelling() {
    let f = fx();
    let (dir, spelling) = profile(&f, "0192");
    let acct = keychain_account(&f.env);
    let item = hashed("Claude Code-credentials", &spelling);
    assert!(
        matches!(
            f.cc.read_profile_credential(&f.env, &dir, &spelling),
            Read::Absent
        ),
        "a new profile has none"
    );
    f.kc.put(&item, &acct, ENTRY);
    let c =
        f.cc.read_profile_credential(&f.env, &dir, &spelling)
            .present()
            .unwrap();
    assert_eq!((c.bytes(), c.provenance()), (ENTRY, Provenance::Fresh));
    // §12.5: the profile env drops the outer secure-storage dir, so it plays no part.
    let outer = with_vars(&f.env, Some("/elsewhere"), Some(""));
    assert_eq!(
        f.cc.read_profile_credential(&outer, &dir, &spelling)
            .present()
            .unwrap()
            .bytes(),
        ENTRY
    );
    // Another spelling of the same directory names another item (Appendix A.2).
    assert!(matches!(
        f.cc.read_profile_credential(&f.env, &dir, &format!("{spelling}/")),
        Read::Absent
    ));
    // Then `<profile>/.credentials.json`: it covers an unreadable item only as a degraded read.
    f.kc.set_unreadable(&item, &acct, true);
    assert!(matches!(
        f.cc.read_profile_credential(&f.env, &dir, &spelling),
        Read::Unreadable(_)
    ));
    fs::write(dir.join(".credentials.json"), b"file").unwrap();
    let c =
        f.cc.read_profile_credential(&f.env, &dir, &spelling)
            .present()
            .unwrap();
    assert_eq!(
        (c.bytes(), c.provenance()),
        (&b"file"[..], Provenance::Degraded)
    );
    f.kc.set_unreadable(&item, &acct, false);
    f.kc.delete(&item, &acct).unwrap();
    let c =
        f.cc.read_profile_credential(&f.env, &dir, &spelling)
            .present()
            .unwrap();
    assert_eq!(
        (c.bytes(), c.provenance()),
        (&b"file"[..], Provenance::Fresh)
    );
}

#[test]
fn on_linux_the_profile_credential_is_its_file_alone() {
    let f = fx_on(Platform::Linux);
    let (dir, spelling) = profile(&f, "0192");
    f.kc.put(
        &hashed("Claude Code-credentials", &spelling),
        &keychain_account(&f.env),
        ENTRY,
    );
    assert!(matches!(
        f.cc.read_profile_credential(&f.env, &dir, &spelling),
        Read::Absent
    ));
    fs::write(dir.join(".credentials.json"), b"file").unwrap();
    assert_eq!(
        f.cc.read_profile_credential(&f.env, &dir, &spelling)
            .present()
            .unwrap()
            .bytes(),
        b"file"
    );
    // Decision 19: the file is where the profile is, whatever spelling the marker records.
    assert_eq!(
        f.cc.read_profile_credential(&f.env, &dir, "/moved-from/sessions/0192")
            .present()
            .unwrap()
            .bytes(),
        b"file"
    );
}

#[test]
fn a_moved_profile_is_read_from_its_old_spelling_s_item_and_from_its_files_where_it_is() {
    // Decision 19, §12.2: the data directory moved, so the marker's recorded spelling names a
    // path that is gone. The profile's hashed item is still named from it, and its files are
    // where the profile is now.
    let f = fx();
    let (dir, current) = profile(&f, "0192");
    let old = f.env.home.join("moved-from/sessions/0192");
    let old = old.to_str().unwrap();
    let acct = keychain_account(&f.env);
    let item = hashed("Claude Code-credentials", old);
    f.kc.put(&item, &acct, ENTRY);
    f.kc.put(
        &hashed("Claude Code-credentials", &current),
        &acct,
        b"the current spelling's item",
    );
    fs::write(dir.join(".credentials.json"), b"file").unwrap();
    fs::write(
        dir.join(".claude.json"),
        json!({"oauthAccount": {"emailAddress": "p@x.co"}}).to_string(),
    )
    .unwrap();

    let c =
        f.cc.read_profile_credential(&f.env, &dir, old)
            .present()
            .unwrap();
    assert_eq!(
        (c.bytes(), c.provenance()),
        (ENTRY, Provenance::Fresh),
        "the item the recorded spelling names, never one derived again"
    );
    f.kc.delete(&item, &acct).unwrap();
    assert_eq!(
        f.cc.read_profile_credential(&f.env, &dir, old)
            .present()
            .unwrap()
            .bytes(),
        b"file",
        "then the file where the profile is"
    );
    assert!(
        matches!(
            f.cc.read_profile_credential(&f.env, Path::new(old), old),
            Read::Absent
        ),
        "under the old path there is nothing"
    );
    let id = f.cc.profile_identity(&f.env, &dir).present().unwrap();
    assert_eq!(id.email.as_deref(), Some("p@x.co"));
    assert!(matches!(
        f.cc.profile_identity(&f.env, Path::new(old)),
        Read::Absent
    ));
}

#[test]
fn the_profile_identity_is_the_profile_s_own_oauth_account() {
    let f = fx();
    let (dir, _) = profile(&f, "0192");
    fs::write(
        f.env.home.join(".claude.json"),
        json!({"oauthAccount": {"emailAddress": "default@x.co"}}).to_string(),
    )
    .unwrap();
    assert!(matches!(f.cc.profile_identity(&f.env, &dir), Read::Absent));
    fs::write(
        dir.join(".claude.json"),
        json!({"oauthAccount": {"emailAddress": "p@x.co", "organizationUuid": "org-1"}})
            .to_string(),
    )
    .unwrap();
    let id = f.cc.profile_identity(&f.env, &dir).present().unwrap();
    assert_eq!(
        (id.email.as_deref(), id.org_uuid.as_str()),
        (Some("p@x.co"), "org-1")
    );
    fs::write(dir.join(".claude.json"), b"{\"oauthAccount\": {").unwrap();
    assert!(matches!(
        f.cc.profile_identity(&f.env, &dir),
        Read::Unreadable(_)
    ));
}

#[test]
fn deleting_a_profile_credential_removes_and_verifies_both_items_of_its_spelling_only() {
    let f = fx();
    let (dir, spelling) = profile(&f, "0192");
    let acct = keychain_account(&f.env);
    let oauth = hashed("Claude Code-credentials", &spelling);
    let managed = hashed("Claude Code", &spelling);
    let older = hashed("Claude Code-credentials", "/old/data/tagteam/sessions/0192");
    f.kc.put(&oauth, &acct, ENTRY);
    f.kc.put(&managed, &acct, b"sk-ant-api03-profile");
    f.kc.put(&older, &acct, b"older spelling");
    f.kc.put("Claude Code-credentials", &acct, b"default home");
    fs::write(dir.join(".credentials.json"), b"file").unwrap();
    f.cc.delete_profile_credential(&f.env, &dir, &spelling)
        .unwrap();
    assert_eq!(
        (f.kc.get(&oauth, &acct), f.kc.get(&managed, &acct)),
        (None, None)
    );
    assert_eq!(f.kc.get(&older, &acct).unwrap(), b"older spelling");
    assert_eq!(
        f.kc.get("Claude Code-credentials", &acct).unwrap(),
        b"default home"
    );
    assert!(
        dir.join(".credentials.json").exists(),
        "the file goes with the directory"
    );
    f.cc.delete_profile_credential(&f.env, &dir, &spelling)
        .expect("absent items are already deleted");
    // An item that will not go is reported, never assumed gone.
    f.kc.put(&oauth, &acct, ENTRY);
    f.kc.set_fail_delete(&oauth, true);
    match f.cc.delete_profile_credential(&f.env, &dir, &spelling) {
        Err(ProviderError::ShadowingItem(svc)) => assert_eq!(svc, oauth),
        other => panic!("{other:?}"),
    }
    assert!(
        !dir.join(".storage-write").exists(),
        "the lock is released on failure too"
    );
}

#[test]
fn deleting_a_profile_credential_on_linux_touches_nothing() {
    let f = fx_on(Platform::Linux);
    let (dir, spelling) = profile(&f, "0192");
    let acct = keychain_account(&f.env);
    let oauth = hashed("Claude Code-credentials", &spelling);
    f.kc.put(&oauth, &acct, ENTRY);
    fs::write(dir.join(".credentials.json"), b"file").unwrap();
    f.cc.delete_profile_credential(&f.env, &dir, &spelling)
        .unwrap();
    assert_eq!(f.kc.get(&oauth, &acct).unwrap(), ENTRY);
    assert_eq!(fs::read(dir.join(".credentials.json")).unwrap(), b"file");
}

/// A Keychain that records, at each delete, whether each of `locks` is held, as CC's locks are
/// while it writes (`tests/live_store.rs`' `LockProbeKeychain`).
struct LockProbeKeychain {
    inner: Arc<FakeKeychain>,
    locks: Vec<PathBuf>,
    deletes: Mutex<Vec<(String, Vec<bool>)>>,
}

impl LockProbeKeychain {
    fn over(inner: &Arc<FakeKeychain>, locks: &[PathBuf]) -> Arc<Self> {
        Arc::new(LockProbeKeychain {
            inner: inner.clone(),
            locks: locks.to_vec(),
            deletes: Mutex::new(Vec::new()),
        })
    }
}

/// The locks Claude Code takes for the profile in `dir` (§9.1): with `CLAUDE_CONFIG_DIR` naming
/// it and secure storage undefined, its credential locks and its storage-write lock are anchored
/// there.
fn profile_lock_paths(f: &Fx, dir: &Path) -> CcPaths {
    CcPaths::resolve(&with_vars(&f.env, dir.to_str(), None))
}

impl Keychain for LockProbeKeychain {
    fn find(&self, s: &str, a: &str) -> Read<Vec<u8>> {
        self.inner.find(s, a)
    }
    fn exists(&self, s: &str, a: &str) -> Read<()> {
        self.inner.exists(s, a)
    }
    fn upsert(&self, s: &str, a: &str, d: &[u8]) -> Result<(), KeychainError> {
        self.inner.upsert(s, a, d)
    }
    fn delete(&self, s: &str, a: &str) -> Result<(), KeychainError> {
        let held = self.locks.iter().map(|l| l.is_dir()).collect();
        self.deletes.lock().unwrap().push((s.to_owned(), held));
        self.inner.delete(s, a)
    }
    fn lock_state(&self) -> LockState {
        self.inner.lock_state()
    }
    fn unlock(&self) -> bool {
        self.inner.unlock()
    }
}

#[test]
fn deleting_a_profile_credential_holds_the_storage_write_lock_at_the_profile_s_actual_directory() {
    // §9.1: tagteam takes CC's storage-write lock for every delete of a credential entry, and a
    // per-config-dir daemon may still write it. Decision 19: the lock lives where the profile is
    // now, not under the recorded spelling, whose directory is gone after a data-directory move.
    let f = fx();
    let (dir, _) = profile(&f, "0192");
    let gone = f.env.home.join("moved-from/sessions/0192");
    let spelling = gone.to_str().unwrap();
    let lock = dir.join(".storage-write");
    let probe = LockProbeKeychain::over(&f.kc, &[lock.clone()]);
    let cc = ClaudeCode::with_store(
        LiveStore::new(probe.clone(), Platform::MacOs).with_retry_delay(Duration::ZERO),
    );
    let acct = keychain_account(&f.env);
    let oauth = hashed("Claude Code-credentials", spelling);
    let managed = hashed("Claude Code", spelling);
    f.kc.put(&oauth, &acct, ENTRY);
    f.kc.put(&managed, &acct, b"sk-ant-api03-profile");

    cc.delete_profile_credential(&f.env, &dir, spelling)
        .unwrap();

    assert_eq!(
        *probe.deletes.lock().unwrap(),
        [(oauth.clone(), vec![true]), (managed.clone(), vec![true])],
        "each delete runs under the lock"
    );
    assert!(!lock.exists(), "released when the delete returns");
    assert!(!gone.exists(), "nothing is created under the old spelling");
    assert_eq!(
        (f.kc.get(&oauth, &acct), f.kc.get(&managed, &acct)),
        (None, None)
    );
}

#[test]
fn deleting_a_profile_credential_holds_the_profile_s_own_credential_locks_and_releases_them() {
    // §9.1: the storage-write lock is taken only while the credential locks are held, for a
    // profile the profile's own, and §4.3 orders them: the credential locks first, the
    // storage-write lock last, around each delete.
    let f = fx();
    let (dir, spelling) = profile(&f, "0192");
    let paths = profile_lock_paths(&f, &dir);
    let locks = [
        paths.refresh_lock.clone(),
        paths.legacy_lock(),
        paths.storage_write_lock.clone(),
    ];
    assert_eq!(locks[0], dir.join(".oauth_refresh.lock"));
    assert_eq!(locks[2], dir.join(".storage-write"));
    let probe = LockProbeKeychain::over(&f.kc, &locks);
    let cc = ClaudeCode::with_store(
        LiveStore::new(probe.clone(), Platform::MacOs).with_retry_delay(Duration::ZERO),
    );
    let acct = keychain_account(&f.env);
    let oauth = hashed("Claude Code-credentials", &spelling);
    let managed = hashed("Claude Code", &spelling);
    f.kc.put(&oauth, &acct, ENTRY);
    f.kc.put(&managed, &acct, b"sk-ant-api03-profile");

    cc.delete_profile_credential(&f.env, &dir, &spelling)
        .unwrap();

    assert_eq!(
        *probe.deletes.lock().unwrap(),
        [
            (oauth.clone(), vec![true; 3]),
            (managed.clone(), vec![true; 3])
        ],
        "each delete holds the refresh, legacy and storage-write locks"
    );
    for lock in &locks {
        assert!(!lock.exists(), "{} is released", lock.display());
    }
    assert_eq!(
        (f.kc.get(&oauth, &acct), f.kc.get(&managed, &acct)),
        (None, None)
    );
}

#[test]
fn a_set_token_ends_a_profile_credential_delete_before_it_takes_or_deletes_anything() {
    // §14.1: the credential-lock wait is a cancellation point; a set token makes no attempt.
    let f = fx();
    let (dir, spelling) = profile(&f, "0192");
    let paths = profile_lock_paths(&f, &dir);
    let acct = keychain_account(&f.env);
    let oauth = hashed("Claude Code-credentials", &spelling);
    f.kc.put(&oauth, &acct, ENTRY);
    f.env.cancel.request(libc::SIGTERM);

    match f.cc.delete_profile_credential(&f.env, &dir, &spelling) {
        Err(ProviderError::Lock(LockError::Interrupted { path, signal })) => {
            assert_eq!((path, signal), (paths.refresh_lock.clone(), libc::SIGTERM))
        }
        other => panic!("expected an interrupted wait, got {other:?}"),
    }
    assert_eq!(f.kc.get(&oauth, &acct).unwrap(), ENTRY, "nothing deleted");
    for lock in [
        paths.refresh_lock.clone(),
        paths.legacy_lock(),
        paths.storage_write_lock.clone(),
    ] {
        assert!(!lock.exists(), "{} was never taken", lock.display());
    }
}

/// §9.1: a session in the profile is refreshing its token, so the delete waits for the
/// profile's credential locks, then times out, deleting nothing.
#[cfg(feature = "test-hooks")]
#[test]
fn a_held_profile_credential_lock_makes_the_delete_wait_then_time_out() {
    let f = fx();
    let (dir, spelling) = profile(&f, "0192");
    let paths = profile_lock_paths(&f, &dir);
    let cc = ClaudeCode::with_store(
        LiveStore::new(f.kc.clone(), Platform::MacOs).with_retry_delay(Duration::ZERO),
    )
    .with_lock_timeout(Duration::from_millis(300));
    let acct = keychain_account(&f.env);
    let oauth = hashed("Claude Code-credentials", &spelling);
    f.kc.put(&oauth, &acct, ENTRY);
    fs::create_dir(&paths.refresh_lock).unwrap(); // the session's agent holds it, freshly

    match cc.delete_profile_credential(&f.env, &dir, &spelling) {
        Err(ProviderError::Lock(LockError::Timeout(path))) => assert_eq!(path, paths.refresh_lock),
        other => panic!("expected a timeout, got {other:?}"),
    }
    assert_eq!(f.kc.get(&oauth, &acct).unwrap(), ENTRY, "nothing deleted");
    assert!(
        paths.refresh_lock.is_dir(),
        "the other holder's lock is left alone"
    );
    assert!(
        !paths.storage_write_lock.exists(),
        "the leaf was never reached"
    );
}

/// Deleting the profile's items with `dir` as `place` makes: nothing, a file, or a dangling link.
fn delete_where_the_profile_is_not_a_directory(place: impl Fn(&Path)) {
    let f = fx();
    let (dir, spelling) = profile(&f, "0192");
    fs::remove_dir(&dir).unwrap();
    place(&dir);
    let placed = fs::symlink_metadata(&dir).ok().map(|m| m.file_type());
    let legacy = profile_lock_paths(&f, &dir).legacy_lock();
    let acct = keychain_account(&f.env);
    let oauth = hashed("Claude Code-credentials", &spelling);
    let managed = hashed("Claude Code", &spelling);
    f.kc.put(&oauth, &acct, ENTRY);
    f.kc.put(&managed, &acct, b"sk-ant-api03-profile");
    f.cc.delete_profile_credential(&f.env, &dir, &spelling)
        .expect("no lock is needed, and none may block the removal");
    assert_eq!(
        (f.kc.get(&oauth, &acct), f.kc.get(&managed, &acct)),
        (None, None),
        "both items are deleted and verified gone"
    );
    assert!(!dir.join(".storage-write").exists());
    assert!(!dir.join(".oauth_refresh.lock").exists() && !legacy.exists());
    assert_eq!(
        fs::symlink_metadata(&dir).ok().map(|m| m.file_type()),
        placed,
        "the profile path is left as it was"
    );
}

#[test]
fn a_missing_profile_directory_still_has_its_items_deleted() {
    delete_where_the_profile_is_not_a_directory(|_| {});
}

#[test]
fn a_regular_file_at_the_profile_path_still_has_its_items_deleted() {
    delete_where_the_profile_is_not_a_directory(|dir| fs::write(dir, b"stray").unwrap());
}

#[test]
fn a_dangling_symlink_at_the_profile_path_still_has_its_items_deleted() {
    delete_where_the_profile_is_not_a_directory(|dir| {
        std::os::unix::fs::symlink(dir.with_file_name("nowhere"), dir).unwrap();
    });
}

#[test]
fn a_symlink_to_a_directory_at_the_profile_path_takes_no_lock_either() {
    let f = fx();
    let (dir, spelling) = profile(&f, "0192");
    let real = f.env.home.join("real-profile");
    fs::rename(&dir, &real).unwrap();
    std::os::unix::fs::symlink(&real, &dir).unwrap();
    // A held lock behind the link would time this delete out if it took the lock: the
    // storage-write lock, or the credential locks it is taken under.
    fs::create_dir(real.join(".storage-write")).unwrap();
    fs::create_dir(real.join(".oauth_refresh.lock")).unwrap();
    let cc = ClaudeCode::with_store(
        LiveStore::new(f.kc.clone(), Platform::MacOs)
            .with_retry_delay(Duration::ZERO)
            .with_storage_write_timeout(Duration::from_millis(300)),
    );
    let acct = keychain_account(&f.env);
    let oauth = hashed("Claude Code-credentials", &spelling);
    f.kc.put(&oauth, &acct, ENTRY);
    cc.delete_profile_credential(&f.env, &dir, &spelling)
        .unwrap();
    assert_eq!(f.kc.get(&oauth, &acct), None);
    assert!(
        real.join(".storage-write").is_dir() && real.join(".oauth_refresh.lock").is_dir(),
        "the other holder's locks are untouched"
    );
}

#[test]
fn a_held_storage_write_lock_makes_a_profile_credential_delete_wait_then_time_out() {
    let f = fx();
    let (dir, spelling) = profile(&f, "0192");
    let cc = ClaudeCode::with_store(
        LiveStore::new(f.kc.clone(), Platform::MacOs)
            .with_retry_delay(Duration::ZERO)
            .with_storage_write_timeout(Duration::from_millis(300)),
    );
    let acct = keychain_account(&f.env);
    let oauth = hashed("Claude Code-credentials", &spelling);
    f.kc.put(&oauth, &acct, ENTRY);
    let lock = dir.join(".storage-write");
    fs::create_dir(&lock).unwrap(); // a daemon holds it, freshly

    match cc.delete_profile_credential(&f.env, &dir, &spelling) {
        Err(ProviderError::Lock(LockError::Timeout(path))) => assert_eq!(path, lock),
        other => panic!("{other:?}"),
    }
    assert_eq!(f.kc.get(&oauth, &acct).unwrap(), ENTRY, "nothing deleted");
    assert!(lock.is_dir(), "the other holder's lock is left alone");
}

#[test]
fn claude_code_invoked_a_process_that_has_claudecode_1_or_a_config_dir() {
    let f = fx();
    let with_var = |name: &str, v: &str| {
        let mut e = f.env.clone();
        e.vars.insert(name.into(), v.into());
        e
    };
    assert!(!f.cc.invoked_by(&f.env));
    assert!(f.cc.invoked_by(&with_var("CLAUDECODE", "1")));
    assert!(!f.cc.invoked_by(&with_var("CLAUDECODE", "0")));
    assert!(!f.cc.invoked_by(&with_var("CLAUDECODE", "")));
    assert!(f.cc.invoked_by(&with_vars(&f.env, Some("/p"), None)));
    assert!(!f.cc.invoked_by(&with_vars(&f.env, Some(""), None)));
}

/// `path`'s permission bits.
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// Holds a `mkdir` lock at `path` for `for_ms`, as another process would, then lets it go.
fn hold(path: &Path, for_ms: u64) -> std::thread::JoinHandle<()> {
    fs::create_dir(path).unwrap();
    let path = path.to_path_buf();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(for_ms));
        fs::remove_dir(&path).unwrap();
    })
}

mod seed_and_merge_back {
    //! §12.4: the seed of a profile's `.claude.json` from the default file, and the three-way
    //! merge of its `projects` and `mcpServers` back into it (Review Focus 4).

    use super::*;
    use std::thread;
    use std::time::Instant;

    use serde_json::Value;
    use tagteam_provider::splice::{get_top_level, remove_top_level, replace_top_level};
    use tagteam_provider::{Cancel, Identity, LockError, MergeReport};

    /// The default home's `~/.claude.json` as CC writes it: keys before, between and after the
    /// two subtrees a merge-back may touch, and a number serde would re-render if anything ever
    /// re-serialized the file.
    const DEFAULT_JSON: &str = r#"{
  "numStartups": 3,
  "projects": {
    "/work/app": {
      "allowedTools": [],
      "hasTrustDialogAccepted": false
    },
    "/work/lib": {
      "allowedTools": [
        "Bash"
      ]
    }
  },
  "userID": "default-user",
  "mcpServers": {
    "local": {
      "command": "srv"
    },
    "remote": {
      "url": "https://mcp.example"
    }
  },
  "theme": "light",
  "someFutureKey": {
    "n": 1e400
  }
}
"#;

    fn account(f: &Fx) -> Identity {
        f.cc.parse_identity(
            &json!({"emailAddress": "p@x.co", "organizationUuid": "org-1", "accountUuid": "acct-1"}),
        )
        .unwrap()
    }

    fn default_config(f: &Fx) -> PathBuf {
        f.env.home.join(".claude.json")
    }

    fn json_at(path: &Path) -> Value {
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
    }

    fn top(doc: &[u8], key: &str) -> Option<Value> {
        get_top_level(doc, key).unwrap()
    }

    /// `doc` without these top-level keys: the bytes that must not move (§3, §9.5).
    fn strip(doc: &[u8], keys: &[&str]) -> Vec<u8> {
        keys.iter()
            .fold(doc.to_vec(), |d, k| remove_top_level(&d, k).unwrap())
    }

    /// Replaces one top-level value of the file at `path`, as a CC session rewriting it does.
    fn edit(path: &Path, key: &str, value: Value) {
        let doc = fs::read(path).unwrap();
        fs::write(path, replace_top_level(&doc, key, &value).unwrap()).unwrap();
    }

    /// A profile seeded over `DEFAULT_JSON`. Returns its directory.
    fn seeded(f: &Fx) -> PathBuf {
        fs::write(default_config(f), DEFAULT_JSON).unwrap();
        let (dir, _) = profile(f, "0192");
        f.cc.seed_profile(&f.env, &dir, &account(f)).unwrap();
        dir
    }

    #[test]
    fn a_profile_with_no_file_is_seeded_from_nothing() {
        let f = fx();
        fs::write(default_config(&f), DEFAULT_JSON).unwrap();
        let (dir, _) = profile(&f, "0192");
        assert!(!f.cc.has_baseline(&dir));

        f.cc.seed_profile(&f.env, &dir, &account(&f)).unwrap();

        let config = dir.join(".claude.json");
        let v = json_at(&config);
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "projects",
                "mcpServers",
                "oauthAccount",
                "hasCompletedOnboarding",
                "theme"
            ]
        );
        let default = DEFAULT_JSON.as_bytes();
        assert_eq!(Some(v["projects"].clone()), top(default, "projects"));
        assert_eq!(Some(v["mcpServers"].clone()), top(default, "mcpServers"));
        assert_eq!(v["oauthAccount"], account(&f).raw);
        assert_eq!(v["hasCompletedOnboarding"], json!(true));
        assert_eq!(v["theme"], json!("light"), "the default file's theme");
        assert_eq!(mode(&config), 0o600);
        assert!(f.cc.has_baseline(&dir));
        let baseline = dir.join(".tagteam-baseline.json");
        assert_eq!(mode(&baseline), 0o600);
        assert_eq!(
            json_at(&baseline),
            json!({
                "format": "tagteam-baseline", "version": 1,
                "projects": v["projects"], "mcpServers": v["mcpServers"]
            })
        );
        assert_eq!(
            fs::read(default_config(&f)).unwrap(),
            default,
            "a seed never writes the default file"
        );
    }

    #[test]
    fn the_seed_overwrites_the_profile_s_own_subtrees_and_keeps_its_other_keys_and_theme() {
        let f = fx();
        fs::write(default_config(&f), DEFAULT_JSON).unwrap();
        let (dir, _) = profile(&f, "0192");
        let config = dir.join(".claude.json");
        let own = r#"{
  "userID": "profile-user",
  "projects": {
    "/stale": {
      "allowedTools": []
    }
  },
  "theme": "dark-daltonized",
  "mcpServers": {},
  "oauthAccount": {
    "emailAddress": "old@x.co"
  },
  "machineID": "m-1"
}
"#;
        fs::write(&config, own).unwrap();
        fs::set_permissions(&config, std::os::unix::fs::PermissionsExt::from_mode(0o640)).unwrap();

        f.cc.seed_profile(&f.env, &dir, &account(&f)).unwrap();

        let after = fs::read(&config).unwrap();
        let default = DEFAULT_JSON.as_bytes();
        assert_eq!(top(&after, "projects"), top(default, "projects"));
        assert_eq!(top(&after, "mcpServers"), top(default, "mcpServers"));
        assert_eq!(top(&after, "oauthAccount"), Some(account(&f).raw));
        assert_eq!(top(&after, "hasCompletedOnboarding"), Some(json!(true)));
        assert_eq!(
            top(&after, "theme"),
            Some(json!("dark-daltonized")),
            "a profile's own theme is kept"
        );
        let spliced = [
            "projects",
            "mcpServers",
            "oauthAccount",
            "hasCompletedOnboarding",
        ];
        assert_eq!(
            strip(&after, &spliced),
            strip(own.as_bytes(), &spliced),
            "every other byte, userID and machineID included"
        );
        assert_eq!(mode(&config), 0o640, "CC's file keeps its mode (§9.5)");
    }

    #[test]
    fn a_seed_with_no_theme_anywhere_sets_dark_and_copies_the_default_s_absence() {
        let f = fx();
        fs::write(default_config(&f), "{\n  \"userID\": \"u\"\n}\n").unwrap();
        let (dir, _) = profile(&f, "0192");
        fs::write(
            dir.join(".claude.json"),
            "{\n  \"projects\": {\n    \"/stale\": {}\n  },\n  \"mcpServers\": {}\n}\n",
        )
        .unwrap();

        f.cc.seed_profile(&f.env, &dir, &account(&f)).unwrap();

        let v = json_at(&dir.join(".claude.json"));
        assert_eq!(
            v.get("projects"),
            None,
            "the default has none, so the profile keeps none"
        );
        assert_eq!(v.get("mcpServers"), None);
        assert_eq!(v["theme"], json!("dark"));
        assert_eq!(
            json_at(&dir.join(".tagteam-baseline.json")),
            json!({"format": "tagteam-baseline", "version": 1, "projects": null, "mcpServers": null})
        );
    }

    #[test]
    fn the_seed_waits_for_the_profile_s_config_lock_and_never_takes_the_default_s() {
        let f = fx();
        fs::write(default_config(&f), DEFAULT_JSON).unwrap();
        let (dir, _) = profile(&f, "0192");
        // The default home's config lock stays held: a seed that took it would time out.
        fs::create_dir(f.env.home.join(".claude.json.lock")).unwrap();
        let cc_writing = hold(&dir.join(".claude.json.lock"), 300);
        let start = Instant::now();

        f.cc.seed_profile(&f.env, &dir, &account(&f)).unwrap();

        assert!(
            start.elapsed() >= Duration::from_millis(300),
            "the seed waited for the profile's own lock"
        );
        cc_writing.join().unwrap();
        assert!(!dir.join(".claude.json.lock").exists(), "and released it");
        assert!(f.env.home.join(".claude.json.lock").is_dir());
    }

    #[test]
    fn a_profile_config_that_is_a_link_refuses_the_seed_and_leaves_the_default_file_as_it_was() {
        // The profile's `.claude.json` is its own (`CC_PRIVATE`). The write would follow a
        // hand-made link to the default file and put this account's `oauthAccount` there. A link
        // that resolves to nothing is refused too, before anything appears where it points.
        let f = fx();
        fs::write(default_config(&f), DEFAULT_JSON).unwrap();
        let (dir, _) = profile(&f, "0192");
        let config = dir.join(".claude.json");
        let nowhere = f.env.home.join("elsewhere.json");
        for target in [default_config(&f), nowhere.clone()] {
            let _ = fs::remove_file(&config);
            std::os::unix::fs::symlink(&target, &config).unwrap();

            let err = f.cc.seed_profile(&f.env, &dir, &account(&f)).unwrap_err();

            assert!(
                err.to_string()
                    .contains(&format!("{} is a link", config.display())),
                "{err}"
            );
            assert_eq!(fs::read_link(&config).unwrap(), target, "it stays");
            assert!(!f.cc.has_baseline(&dir));
        }
        assert_eq!(
            fs::read(default_config(&f)).unwrap(),
            DEFAULT_JSON.as_bytes(),
            "byte for byte"
        );
        assert!(!nowhere.exists(), "nothing was written through the link");
    }

    #[test]
    fn a_torn_file_on_either_side_stops_the_seed_and_writes_nothing() {
        let f = fx();
        let (dir, _) = profile(&f, "0192");
        let config = dir.join(".claude.json");
        fs::write(default_config(&f), DEFAULT_JSON).unwrap();
        fs::write(&config, b"{\"userID\": ").unwrap();
        match f.cc.seed_profile(&f.env, &dir, &account(&f)) {
            Err(ProviderError::ConfigUnsplicable { path, remedy }) => {
                assert_eq!(path, config);
                assert!(
                    !remedy.contains("backups"),
                    "a profile's file has no backups: {remedy}"
                );
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(fs::read(&config).unwrap(), b"{\"userID\": ");
        assert!(!f.cc.has_baseline(&dir));

        fs::remove_file(&config).unwrap();
        fs::write(default_config(&f), b"[1]").unwrap();
        assert!(matches!(
            f.cc.seed_profile(&f.env, &dir, &account(&f)),
            Err(ProviderError::ConfigUnsplicable { path, .. }) if path == default_config(&f)
        ));
        assert!(!config.exists());
        assert!(!f.cc.has_baseline(&dir));
    }

    /// Review Focus 4: default-home sessions edit `~/.claude.json` while the profile's session
    /// runs, and the profile's session edits its own file.
    #[test]
    fn a_merge_back_keeps_the_default_where_both_changed_and_applies_the_rest() {
        let f = fx();
        let dir = seeded(&f);
        let profile_config = dir.join(".claude.json");
        // The default home: the same key changed, a project of its own, and a key outside
        // both subtrees.
        edit(
            &default_config(&f),
            "projects",
            json!({
                "/work/app": {"allowedTools": ["Read"], "hasTrustDialogAccepted": true},
                "/work/lib": {"allowedTools": ["Bash"]},
                "/work/default-new": {"allowedTools": []}
            }),
        );
        edit(&default_config(&f), "numStartups", json!(4));
        // The profile: that key changed differently, another key, a new project, and an MCP
        // server removed.
        edit(
            &profile_config,
            "projects",
            json!({
                "/work/app": {"allowedTools": ["Edit"], "hasTrustDialogAccepted": false},
                "/work/lib": {"allowedTools": ["Bash", "Edit"]},
                "/work/profile-new": {"allowedTools": [], "hasTrustDialogAccepted": true}
            }),
        );
        edit(
            &profile_config,
            "mcpServers",
            json!({"local": {"command": "srv"}}),
        );
        let before = fs::read(default_config(&f)).unwrap();
        let profile_before = fs::read(&profile_config).unwrap();

        let report = f.cc.merge_back(&f.env, &dir, &Cancel::new()).unwrap();

        assert_eq!(
            report,
            MergeReport {
                applied: 4,
                conflicts: vec![r#"projects["/work/app"].allowedTools"#.to_owned()],
            }
        );
        let after = fs::read(default_config(&f)).unwrap();
        let projects = top(&after, "projects").unwrap();
        assert_eq!(
            projects,
            json!({
                "/work/app": {"allowedTools": ["Read"], "hasTrustDialogAccepted": true},
                "/work/lib": {"allowedTools": ["Bash", "Edit"]},
                "/work/default-new": {"allowedTools": []},
                "/work/profile-new": {"allowedTools": [], "hasTrustDialogAccepted": true}
            })
        );
        let order: Vec<&str> = projects
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            order,
            [
                "/work/app",
                "/work/lib",
                "/work/default-new",
                "/work/profile-new"
            ]
        );
        assert_eq!(
            top(&after, "mcpServers"),
            Some(json!({"local": {"command": "srv"}}))
        );
        assert_eq!(
            strip(&after, &["projects", "mcpServers"]),
            strip(&before, &["projects", "mcpServers"]),
            "every other byte of ~/.claude.json, 1e400 and the default's numStartups included"
        );
        assert_eq!(
            fs::read(&profile_config).unwrap(),
            profile_before,
            "the profile is left as it is"
        );
        assert!(
            !f.cc.has_baseline(&dir),
            "a merge-back that ran leaves no baseline"
        );
    }

    #[test]
    fn a_merge_back_with_nothing_to_merge_writes_nothing() {
        let f = fx();
        let dir = seeded(&f);
        edit(&default_config(&f), "numStartups", json!(9));
        let before = fs::read(default_config(&f)).unwrap();

        let report = f.cc.merge_back(&f.env, &dir, &Cancel::new()).unwrap();

        assert_eq!(report, MergeReport::default());
        assert_eq!(
            fs::read(default_config(&f)).unwrap(),
            before,
            "not one byte"
        );
        assert!(!f.cc.has_baseline(&dir));
        assert_eq!(
            f.cc.merge_back(&f.env, &dir, &Cancel::new()).unwrap(),
            MergeReport::default(),
            "with no baseline left, a second merge-back does nothing"
        );
    }

    #[test]
    fn a_torn_default_file_fails_the_merge_back_and_keeps_the_profile_and_its_baseline() {
        let f = fx();
        let dir = seeded(&f);
        let profile_config = dir.join(".claude.json");
        edit(&profile_config, "mcpServers", json!({}));
        let profile_before = fs::read(&profile_config).unwrap();
        let baseline_before = fs::read(dir.join(".tagteam-baseline.json")).unwrap();
        fs::write(default_config(&f), b"{\"projects\": {").unwrap();

        match f.cc.merge_back(&f.env, &dir, &Cancel::new()) {
            Err(ProviderError::ConfigUnsplicable { path, remedy }) => {
                assert_eq!(path, default_config(&f));
                assert!(remedy.contains("~/.claude/backups/"), "{remedy}");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(fs::read(default_config(&f)).unwrap(), b"{\"projects\": {");
        assert_eq!(fs::read(&profile_config).unwrap(), profile_before);
        assert_eq!(
            fs::read(dir.join(".tagteam-baseline.json")).unwrap(),
            baseline_before
        );
        assert!(f.cc.has_baseline(&dir));
        assert!(
            !f.env.home.join(".claude.json.lock").exists(),
            "the default's lock is released"
        );
    }

    #[test]
    fn a_merge_back_waits_for_the_default_s_config_lock_alone() {
        let f = fx();
        let dir = seeded(&f);
        edit(&dir.join(".claude.json"), "mcpServers", json!({}));
        // Locks a merge-back never takes, held throughout: the profile's own config and
        // credential locks, and the default home's credential lock.
        for held in [
            dir.join(".claude.json.lock"),
            dir.join(".oauth_refresh.lock"),
            f.env.home.join(".claude/.oauth_refresh.lock"),
        ] {
            fs::create_dir(held).unwrap();
        }
        let cc_writing = hold(&f.env.home.join(".claude.json.lock"), 300);
        let start = Instant::now();

        let report = f.cc.merge_back(&f.env, &dir, &Cancel::new()).unwrap();

        assert!(start.elapsed() >= Duration::from_millis(300));
        cc_writing.join().unwrap();
        assert_eq!(report.applied, 2, "both servers removed");
        assert_eq!(
            top(&fs::read(default_config(&f)).unwrap(), "mcpServers"),
            Some(json!({}))
        );
    }

    #[test]
    fn a_signal_ends_the_merge_back_s_wait_and_keeps_the_baseline() {
        let f = fx();
        let dir = seeded(&f);
        edit(&dir.join(".claude.json"), "mcpServers", json!({}));
        fs::create_dir(f.env.home.join(".claude.json.lock")).unwrap();
        let cancel = Cancel::new();
        let signal = {
            let cancel = cancel.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(200));
                cancel.request(15);
            })
        };

        let result = f.cc.merge_back(&f.env, &dir, &cancel);

        signal.join().unwrap();
        assert!(
            matches!(
                result,
                Err(ProviderError::Lock(LockError::Interrupted {
                    signal: 15,
                    ..
                }))
            ),
            "{result:?}"
        );
        assert!(f.cc.has_baseline(&dir));
        assert_eq!(
            fs::read(default_config(&f)).unwrap(),
            DEFAULT_JSON.as_bytes()
        );
    }

    #[test]
    fn a_default_file_that_is_gone_is_created_with_the_merged_subtree_alone() {
        let f = fx();
        let dir = seeded(&f);
        edit(
            &dir.join(".claude.json"),
            "mcpServers",
            json!({
                "local": {"command": "srv"}, "remote": {"url": "https://mcp.example"},
                "added": {"command": "new"}
            }),
        );
        fs::remove_file(default_config(&f)).unwrap();

        let report = f.cc.merge_back(&f.env, &dir, &Cancel::new()).unwrap();

        assert_eq!(report.applied, 1);
        assert_eq!(
            json_at(&default_config(&f)),
            json!({"mcpServers": {"added": {"command": "new"}}})
        );
        assert_eq!(mode(&default_config(&f)), 0o600);
    }

    #[test]
    fn a_profile_whose_file_is_gone_has_nothing_to_merge_back() {
        let f = fx();
        let dir = seeded(&f);
        fs::remove_file(dir.join(".claude.json")).unwrap();

        assert_eq!(
            f.cc.merge_back(&f.env, &dir, &Cancel::new()).unwrap(),
            MergeReport::default()
        );
        assert_eq!(
            fs::read(default_config(&f)).unwrap(),
            DEFAULT_JSON.as_bytes()
        );
        assert!(!f.cc.has_baseline(&dir), "so the next launch can seed");
    }

    #[test]
    fn a_baseline_that_is_not_one_fails_the_merge_back_and_stays() {
        let f = fx();
        let dir = seeded(&f);
        let baseline = dir.join(".tagteam-baseline.json");
        for bytes in [&b"{\"format\": \"tagteam-profile\"}"[..], b"{", b"[]"] {
            fs::write(&baseline, bytes).unwrap();
            assert!(matches!(
                f.cc.merge_back(&f.env, &dir, &Cancel::new()),
                Err(ProviderError::Invalid(_))
            ));
            assert_eq!(fs::read(&baseline).unwrap(), bytes);
            assert!(f.cc.has_baseline(&dir));
        }
        // M4a's own-file rule (T5-a): a link that resolves to nothing, or crosses a file, is
        // unreadable, never absent. The merge-back fails and the link stays.
        let target = dir.join("nowhere/baseline.json");
        for link_to in [target.clone(), dir.join(".claude.json/baseline.json")] {
            fs::remove_file(&baseline).unwrap();
            std::os::unix::fs::symlink(&link_to, &baseline).unwrap();
            assert!(f.cc.has_baseline(&dir));
            assert!(matches!(
                f.cc.merge_back(&f.env, &dir, &Cancel::new()),
                Err(ProviderError::Unreadable(_))
            ));
            assert_eq!(fs::read_link(&baseline).unwrap(), link_to, "it stays");
            assert!(f.cc.has_baseline(&dir));
        }
        assert!(!target.exists(), "nothing was written through the link");
        assert_eq!(
            fs::read(default_config(&f)).unwrap(),
            DEFAULT_JSON.as_bytes()
        );
    }
}

mod profile_credential {
    //! §12.3 step 4: the bootstrap's credential, composed from the vault and the profile's own
    //! credential, written to `<profile>/.credentials.json` alone.

    use super::*;
    use std::sync::Mutex;
    use std::time::Instant;

    use serde_json::Value;
    use tagteam_provider::{KeychainError, LockState, MutationGuard};

    /// The vault's current generation: account-scoped keys, an unknown sibling among them
    /// (Appendix A.4), and machine-shared keys from another home that a profile must never get.
    fn vault() -> Vec<u8> {
        json!({
            "claudeAiOauth": {"accessToken": "at-v", "refreshToken": "rt-v", "expiresAt": 9},
            "trustedDeviceToken": "device-v",
            "designOauth": {"t": "v"},
            "mcpOAuth": {"srv": {"token": "stale-from-another-home"}},
            "pluginSecrets": {"p": "stale"}
        })
        .to_string()
        .into_bytes()
    }

    /// The vault's account-scoped keys alone.
    fn account_keys() -> Value {
        json!({
            "claudeAiOauth": {"accessToken": "at-v", "refreshToken": "rt-v", "expiresAt": 9},
            "trustedDeviceToken": "device-v",
            "designOauth": {"t": "v"}
        })
    }

    /// The profile's own credential: an older generation, and its own MCP token.
    fn profile_cred(mcp: &str) -> Vec<u8> {
        json!({
            "claudeAiOauth": {"accessToken": "at-p", "refreshToken": "rt-p"},
            "trustedDeviceToken": "device-p",
            "mcpOAuth": {"srv": {"token": mcp}}
        })
        .to_string()
        .into_bytes()
    }

    fn parsed(b: &[u8]) -> Value {
        serde_json::from_slice(b).unwrap()
    }

    fn guard(f: &Fx) -> MutationGuard {
        MutationGuard::acquire(&f.env, Duration::from_secs(1)).unwrap()
    }

    /// Every Keychain call, and whether each watched lock directory was held at that moment.
    struct CallLog {
        inner: Arc<FakeKeychain>,
        watch: Mutex<Vec<PathBuf>>,
        calls: Mutex<Vec<(&'static str, String, Vec<bool>)>>,
    }

    impl CallLog {
        fn record(&self, op: &'static str, svc: &str) {
            let held = self
                .watch
                .lock()
                .unwrap()
                .iter()
                .map(|p| p.is_dir())
                .collect();
            self.calls.lock().unwrap().push((op, svc.to_owned(), held));
        }
    }

    impl Keychain for CallLog {
        fn find(&self, s: &str, a: &str) -> Read<Vec<u8>> {
            self.record("find", s);
            self.inner.find(s, a)
        }
        fn exists(&self, s: &str, a: &str) -> Read<()> {
            self.record("exists", s);
            self.inner.exists(s, a)
        }
        fn upsert(&self, s: &str, a: &str, d: &[u8]) -> Result<(), KeychainError> {
            self.record("upsert", s);
            self.inner.upsert(s, a, d)
        }
        fn delete(&self, s: &str, a: &str) -> Result<(), KeychainError> {
            self.record("delete", s);
            self.inner.delete(s, a)
        }
        fn lock_state(&self) -> LockState {
            self.inner.lock_state()
        }
        fn unlock(&self) -> bool {
            self.inner.unlock()
        }
    }

    /// A fixture whose Claude Code logs every Keychain call it makes.
    fn logged(platform: Platform) -> (Fx, Arc<CallLog>) {
        let f = fx_on(platform);
        let log = Arc::new(CallLog {
            inner: f.kc.clone(),
            watch: Mutex::new(vec![]),
            calls: Mutex::new(vec![]),
        });
        let cc = ClaudeCode::with_store(
            LiveStore::new(log.clone(), platform).with_retry_delay(Duration::ZERO),
        );
        (Fx { cc, ..f }, log)
    }

    #[test]
    fn a_new_profile_gets_the_vault_s_account_keys_and_no_machine_shared_ones() {
        let f = fx();
        let out = f.cc.compose_profile_credential(&vault(), None).unwrap();
        assert_eq!(
            parsed(&out),
            account_keys(),
            "the vault's stale MCP keys never reach it"
        );
    }

    #[test]
    fn an_existing_profile_keeps_its_own_machine_shared_keys_and_their_absence() {
        let f = fx();
        let own = profile_cred("profile-mcp");
        let out = parsed(
            &f.cc
                .compose_profile_credential(&vault(), Some(&own))
                .unwrap(),
        );
        let mut want = account_keys();
        want["mcpOAuth"] = json!({"srv": {"token": "profile-mcp"}});
        assert_eq!(
            out, want,
            "the profile's MCP token, and no pluginSecrets, which the profile does not hold"
        );
    }

    #[test]
    fn a_credential_that_is_not_a_json_object_is_refused() {
        let f = fx();
        let v = vault();
        for (vault, profile) in [
            (&b"not json"[..], None),
            (&v[..], Some(&b"[1]"[..])),
            (&v[..], Some(&b""[..])),
        ] {
            assert!(matches!(
                f.cc.compose_profile_credential(vault, profile),
                Err(ProviderError::Invalid(_))
            ));
        }
    }

    #[test]
    fn the_write_takes_the_profile_s_locks_never_the_default_s_and_only_reads_the_keychain() {
        let (f, log) = logged(Platform::MacOs);
        let (dir, spelling) = profile(&f, "0192");
        let acct = keychain_account(&f.env);
        let item = hashed("Claude Code-credentials", &spelling);
        let managed = hashed("Claude Code", &spelling);
        // The profile's own item holds its credential: CC reads it first (§12.3 step 2).
        f.kc.put(&item, &acct, &profile_cred("item-mcp"));
        let canonical = PathBuf::from(&spelling);
        let mut legacy = canonical.clone().into_os_string();
        legacy.push(".lock");
        let default = CcPaths::resolve(&f.env);
        *log.watch.lock().unwrap() = vec![
            canonical.join(".oauth_refresh.lock"),
            PathBuf::from(legacy),
            canonical.join(".storage-write"),
            default.refresh_lock.clone(),
            default.legacy_lock(),
        ];
        let bytes =
            f.cc.compose_profile_credential(&vault(), Some(&profile_cred("item-mcp")))
                .unwrap();

        f.cc.write_profile_credential(&f.env, &spelling, &guard(&f), &bytes)
            .unwrap();

        let file = dir.join(".credentials.json");
        assert_eq!(parsed(&fs::read(&file).unwrap()), parsed(&bytes));
        assert_eq!(mode(&file), 0o600);
        assert_eq!(
            f.kc.get(&item, &acct).unwrap(),
            profile_cred("item-mcp"),
            "the profile's item is left for step 5 to delete"
        );
        assert_eq!(f.kc.items().len(), 1, "no item was created");
        let calls = log.calls.lock().unwrap().clone();
        let (last, earlier) = calls.split_last().expect("the entry was read");
        for (op, svc, held) in &calls {
            assert_eq!(*op, "find", "the Keychain is only ever read: {calls:?}");
            assert!(
                svc == &item || svc == &managed,
                "only the profile's items: {svc}"
            );
            assert_eq!(
                held[..2],
                [true, true],
                "under the profile's credential locks: {svc}"
            );
            assert_eq!(held[3..], [false, false], "never the default home's: {svc}");
        }
        assert_eq!(
            (last.1.as_str(), last.2[2]),
            (item.as_str(), true),
            "the last read is the re-read under the storage-write lock"
        );
        assert!(
            earlier.iter().all(|c| !c.2[2]),
            "which is held for the write alone"
        );
        for lock in log.watch.lock().unwrap().iter() {
            assert!(!lock.exists(), "{} is released", lock.display());
        }
    }

    #[test]
    fn on_linux_the_write_is_the_file_alone_at_0600() {
        let (f, log) = logged(Platform::Linux);
        let (dir, spelling) = profile(&f, "0192");
        let file = dir.join(".credentials.json");
        fs::write(&file, profile_cred("old")).unwrap();
        fs::set_permissions(&file, std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
        let bytes =
            f.cc.compose_profile_credential(&vault(), Some(&profile_cred("old")))
                .unwrap();

        f.cc.write_profile_credential(&f.env, &spelling, &guard(&f), &bytes)
            .unwrap();

        assert_eq!(parsed(&fs::read(&file).unwrap()), parsed(&bytes));
        assert_eq!(mode(&file), 0o600, "a secret file is 0600, whatever it was");
        assert!(
            log.calls.lock().unwrap().is_empty(),
            "no Keychain call at all"
        );
    }

    #[test]
    fn a_credential_file_that_is_a_link_is_refused_and_its_target_left_alone() {
        // §12.3 step 4: the atomic write would follow the link and put the vault's account keys
        // wherever it points, such as the default home's credential file.
        for platform in [Platform::MacOs, Platform::Linux] {
            let (f, log) = logged(platform);
            let (dir, spelling) = profile(&f, "0192");
            let elsewhere = f.env.home.join(".claude/.credentials.json");
            fs::write(&elsewhere, profile_cred("theirs")).unwrap();
            let file = dir.join(".credentials.json");
            std::os::unix::fs::symlink(&elsewhere, &file).unwrap();
            let bytes = f.cc.compose_profile_credential(&vault(), None).unwrap();

            let err =
                f.cc.write_profile_credential(&f.env, &spelling, &guard(&f), &bytes)
                    .unwrap_err();

            let named = Path::new(&spelling).join(".credentials.json");
            assert!(
                err.to_string()
                    .contains(&format!("{} is a link", named.display())),
                "{platform:?}: {err}"
            );
            assert_eq!(
                fs::read(&elsewhere).unwrap(),
                profile_cred("theirs"),
                "{platform:?}"
            );
            assert_eq!(fs::read_link(&file).unwrap(), elsewhere, "{platform:?}");
            assert!(
                log.calls.lock().unwrap().is_empty(),
                "{platform:?}: refused before any Keychain call"
            );
        }
    }

    #[test]
    fn an_mcp_token_cc_wrote_since_the_composition_is_kept() {
        let f = fx_on(Platform::Linux);
        let (dir, spelling) = profile(&f, "0192");
        let file = dir.join(".credentials.json");
        fs::write(&file, profile_cred("mcp-1")).unwrap();
        let bytes =
            f.cc.compose_profile_credential(&vault(), Some(&profile_cred("mcp-1")))
                .unwrap();
        // CC refreshes an MCP token in the profile between the read and the write (§9.1).
        fs::write(&file, profile_cred("mcp-2")).unwrap();

        f.cc.write_profile_credential(&f.env, &spelling, &guard(&f), &bytes)
            .unwrap();

        let out = parsed(&fs::read(&file).unwrap());
        assert_eq!(out["mcpOAuth"], json!({"srv": {"token": "mcp-2"}}));
        assert_eq!(out["claudeAiOauth"]["refreshToken"], json!("rt-v"));
    }

    #[test]
    fn the_write_waits_for_cc_s_storage_write_lock_in_the_profile() {
        let f = fx();
        let (dir, spelling) = profile(&f, "0192");
        let bytes = f.cc.compose_profile_credential(&vault(), None).unwrap();
        let cc_writing = hold(&dir.join(".storage-write"), 300);
        let start = Instant::now();

        f.cc.write_profile_credential(&f.env, &spelling, &guard(&f), &bytes)
            .unwrap();

        assert!(start.elapsed() >= Duration::from_millis(300));
        cc_writing.join().unwrap();
        assert_eq!(
            parsed(&fs::read(dir.join(".credentials.json")).unwrap()),
            account_keys()
        );
    }

    #[test]
    fn a_held_default_home_lock_never_delays_the_write() {
        let f = fx();
        let (_dir, spelling) = profile(&f, "0192");
        let default = CcPaths::resolve(&f.env);
        for lock in [
            default.refresh_lock.clone(),
            default.legacy_lock(),
            default.config_lock.clone(),
            default.storage_write_lock.clone(),
        ] {
            fs::create_dir(lock).unwrap();
        }
        let bytes = f.cc.compose_profile_credential(&vault(), None).unwrap();
        let start = Instant::now();

        f.cc.write_profile_credential(&f.env, &spelling, &guard(&f), &bytes)
            .unwrap();

        assert!(
            start.elapsed() < Duration::from_secs(2),
            "{:?}",
            start.elapsed()
        );
    }

    #[test]
    fn an_entry_absent_at_the_read_and_under_the_lock_keeps_the_composed_machine_shared_keys() {
        // Decision 22: Claude Code wrote nothing there, so there is nothing to rebase from. After
        // a move, the keys came from the old spelling's item, the only copy left.
        for platform in [Platform::MacOs, Platform::Linux] {
            let f = fx_on(platform);
            let (dir, spelling) = profile(&f, "0192");
            let bytes =
                f.cc.compose_profile_credential(&vault(), Some(&profile_cred("old-item-mcp")))
                    .unwrap();

            f.cc.write_profile_credential(&f.env, &spelling, &guard(&f), &bytes)
                .unwrap();

            let out = parsed(&fs::read(dir.join(".credentials.json")).unwrap());
            assert_eq!(
                out["mcpOAuth"],
                json!({"srv": {"token": "old-item-mcp"}}),
                "{platform:?}"
            );
            assert_eq!(
                out["claudeAiOauth"]["refreshToken"],
                json!("rt-v"),
                "{platform:?}"
            );
        }
    }

    #[test]
    fn an_entry_present_under_the_lock_still_rebases_its_machine_shared_keys_and_their_absence() {
        // §9.1: the keys the entry holds now win over the composed ones, absence included.
        let f = fx_on(Platform::Linux);
        let (dir, spelling) = profile(&f, "0192");
        let file = dir.join(".credentials.json");
        fs::write(&file, account_keys().to_string()).unwrap();
        let bytes =
            f.cc.compose_profile_credential(&vault(), Some(&profile_cred("composed-mcp")))
                .unwrap();

        f.cc.write_profile_credential(&f.env, &spelling, &guard(&f), &bytes)
            .unwrap();

        let out = parsed(&fs::read(&file).unwrap());
        assert_eq!(
            out.get("mcpOAuth"),
            None,
            "the entry holds none, so none are written"
        );
        assert_eq!(out, account_keys());
    }
}

mod validation {
    //! §12.3 step 8 and §12.5 "Environment": the session environment, and `claude auth status`
    //! read into a `Validity`.

    use super::*;
    use std::sync::Mutex;

    use serde_json::Value;
    use tagteam_provider::process::{Captured, ProcessSpawner, ScriptedSpawner, SpawnSpec};
    use tagteam_provider::{Cancel, Identity, SessionEnv, Validity};

    /// §12.5's list, verbatim, so a change to `CC_SCRUB` cannot hide behind itself.
    const SCRUBBED: [&str; 18] = [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_REFRESH_TOKEN",
        "CLAUDE_CODE_OAUTH_SCOPES",
        "CLAUDE_CODE_OAUTH_CLIENT_ID",
        "CLAUDE_CODE_ACCOUNT_UUID",
        "CLAUDE_CODE_USER_EMAIL",
        "CLAUDE_CODE_ORGANIZATION_UUID",
        "ANTHROPIC_PROFILE",
        "ANTHROPIC_CONFIG_DIR",
        "ANTHROPIC_FEDERATION_RULE_ID",
        "ANTHROPIC_IDENTITY_TOKEN",
        "ANTHROPIC_IDENTITY_TOKEN_FILE",
        "CLAUDE_CODE_CUSTOM_OAUTH_URL",
        "USE_LOCAL_OAUTH",
        "USE_STAGING_OAUTH",
        "CLAUDE_SECURESTORAGE_CONFIG_DIR",
    ];

    const EMAIL: &str = "probe1@example.com";
    const ORG: &str = "00000000-0000-4000-8000-000000000002";
    /// The launch command `plan_run` resolved (Decision 20); the scripted spawner never runs it.
    const CLAUDE: &str = "/opt/claude/bin/claude";

    fn account(f: &Fx) -> Identity {
        f.cc.parse_identity(&json!({"emailAddress": EMAIL, "organizationUuid": ORG}))
            .unwrap()
    }

    /// The recorded fields of `claude auth status --json` for a claude.ai login (Appendix
    /// A.7), pointed at `spelling`, with the exit code the fixture records.
    fn logged_in(spelling: &str) -> (i32, Value) {
        let fixture: Value =
            serde_json::from_str(include_str!("fixtures/auth-status/claude-ai.json")).unwrap();
        let mut out = fixture["stdout"].clone();
        out["configDirectory"] = json!(spelling);
        out["projectsDirectory"] = json!(format!("{spelling}/projects"));
        (fixture["rc"].as_i64().unwrap() as i32, out)
    }

    /// A reply logged in another way, or not at all, in the same config dir.
    fn reply_by(spelling: &str, logged_in: bool, method: &str) -> Value {
        json!({
            "loggedIn": logged_in, "authMethod": method, "apiProvider": "firstParty",
            "analyticsDisabled": false, "projectsDirectory": format!("{spelling}/projects"),
            "configDirectory": spelling
        })
    }

    fn exited(code: i32, stdout: &Value) -> Captured {
        Captured::Exited {
            code: Some(code),
            signal: None,
            stdout: serde_json::to_vec_pretty(stdout).unwrap(),
            stderr: vec![],
        }
    }

    /// One login check of `spelling` for `account`, with `reply` scripted.
    fn validate(f: &Fx, spelling: &str, reply: Captured) -> Validity {
        let spawner = ScriptedSpawner::new();
        spawner.push(reply);
        f.cc.validate_profile(
            &f.env,
            spelling,
            Path::new("/work/app"),
            Path::new(CLAUDE),
            &account(f),
            &spawner,
            &Cancel::new(),
        )
    }

    #[test]
    fn the_session_environment_sets_the_spelling_and_scrubs_section_12_5_s_list() {
        let f = fx();
        let env = f.cc.session_env("/data/tagteam/sessions/0192");
        assert_eq!(
            env.set,
            [(
                OsString::from("CLAUDE_CONFIG_DIR"),
                OsString::from("/data/tagteam/sessions/0192")
            )]
        );
        assert_eq!(env.remove[..SCRUBBED.len()], SCRUBBED.map(OsString::from));
        for extra in &env.remove[SCRUBBED.len()..] {
            let name = extra.to_string_lossy();
            assert!(
                name.starts_with("CLAUDE_CODE_") && name.ends_with("_FILE_DESCRIPTOR"),
                "{name}"
            );
            assert!(
                std::env::var_os(extra).is_some(),
                "only a descriptor this process holds: {name}"
            );
        }
    }

    #[test]
    fn the_login_check_runs_auth_status_in_the_session_environment_and_directory() {
        let f = fx();
        let (_dir, spelling) = profile(&f, "0192");
        let spawner = ScriptedSpawner::new();
        let (rc, reply) = logged_in(&spelling);
        spawner.push(exited(rc, &reply));
        let cwd = f.env.home.join("work/app");

        let got = f.cc.validate_profile(
            &f.env,
            &spelling,
            &cwd,
            Path::new(CLAUDE),
            &account(&f),
            &spawner,
            &Cancel::new(),
        );

        assert_eq!(got, Validity::Valid);
        let specs = spawner.specs();
        assert_eq!(specs.len(), 1);
        let spec = &specs[0];
        assert_eq!(
            spec.program,
            PathBuf::from(CLAUDE),
            "the launch command plan_run resolved, never `claude` by name"
        );
        assert_eq!(spec.args, ["auth", "status", "--json"].map(OsString::from));
        let SessionEnv { set, remove } = f.cc.session_env(&spelling);
        assert_eq!(
            (&spec.set, &spec.remove),
            (&set, &remove),
            "exactly the session's environment"
        );
        assert_eq!(spec.cwd.as_deref(), Some(cwd.as_path()));
    }

    #[test]
    fn the_login_check_is_given_ten_seconds() {
        #[derive(Default)]
        struct Timed(Mutex<Vec<Duration>>);
        impl ProcessSpawner for Timed {
            fn run_captured(&self, _: &SpawnSpec, timeout: Duration, _: &Cancel) -> Captured {
                self.0.lock().unwrap().push(timeout);
                Captured::TimedOut
            }
        }
        let f = fx();
        let (_dir, spelling) = profile(&f, "0192");
        let timed = Timed::default();

        let got = f.cc.validate_profile(
            &f.env,
            &spelling,
            Path::new("/"),
            Path::new(CLAUDE),
            &account(&f),
            &timed,
            &Cancel::new(),
        );

        assert!(
            matches!(&got, Validity::Unknown(why) if why.contains("10 s")),
            "a timeout is unknown, naming it: {got:?}"
        );
        assert_eq!(*timed.0.lock().unwrap(), [Duration::from_secs(10)]);
    }

    #[test]
    fn valid_needs_claude_ai_this_spelling_this_email_and_this_org_when_both_name_one() {
        let f = fx();
        let (_dir, spelling) = profile(&f, "0192");
        let (rc, reply) = logged_in(&spelling);
        assert_eq!(validate(&f, &spelling, exited(rc, &reply)), Validity::Valid);
        let mut no_org = reply.clone();
        no_org.as_object_mut().unwrap().shift_remove("orgId");
        assert_eq!(
            validate(&f, &spelling, exited(0, &no_org)),
            Validity::Valid,
            "an org is compared only when both name one"
        );
        let personal =
            f.cc.parse_identity(&json!({"emailAddress": EMAIL, "organizationUuid": null}))
                .unwrap();
        let spawner = ScriptedSpawner::new();
        spawner.push(exited(0, &reply));
        assert_eq!(
            f.cc.validate_profile(
                &f.env,
                &spelling,
                Path::new("/"),
                Path::new(CLAUDE),
                &personal,
                &spawner,
                &Cancel::new()
            ),
            Validity::Valid
        );
    }

    #[test]
    fn invalid_is_logged_out_or_another_account_or_organization() {
        let f = fx();
        let (_dir, spelling) = profile(&f, "0192");
        assert_eq!(
            validate(
                &f,
                &spelling,
                exited(1, &reply_by(&spelling, false, "none"))
            ),
            Validity::Invalid("not logged in".into())
        );
        let (_, mut other) = logged_in(&spelling);
        other["email"] = json!("someone-else@example.com");
        assert_eq!(
            validate(&f, &spelling, exited(0, &other)),
            Validity::Invalid("logged in to claude.ai as another account".into())
        );
        let (_, mut other_org) = logged_in(&spelling);
        other_org["orgId"] = json!("00000000-0000-4000-8000-000000000099");
        assert_eq!(
            validate(&f, &spelling, exited(0, &other_org)),
            Validity::Invalid("logged in to claude.ai in another organization".into())
        );
        for got in [
            validate(&f, &spelling, exited(0, &other)),
            validate(&f, &spelling, exited(0, &other_org)),
        ] {
            assert!(
                !format!("{got:?}").contains("example.com"),
                "no email: {got:?}"
            );
        }
    }

    #[test]
    fn the_table_s_edges() {
        let f = fx();
        let (_dir, spelling) = profile(&f, "0192");
        assert_eq!(
            validate(&f, &spelling, exited(0, &reply_by(&spelling, true, "none"))),
            Validity::Invalid("not logged in".into()),
            "logged in by no method"
        );
        assert_eq!(
            validate(
                &f,
                &spelling,
                exited(1, &reply_by(&spelling, false, "claude.ai"))
            ),
            Validity::Invalid("not logged in".into())
        );
        for method in ["api_key_helper", "api_key", "oauth_token", "third_party"] {
            let got = validate(
                &f,
                &spelling,
                exited(1, &reply_by(&spelling, false, method)),
            );
            assert_eq!(
                got,
                Validity::Unknown("inconsistent login state".into()),
                "{method}: not logged in, yet by another method, confirms nothing and must not delete the profile"
            );
        }
        let (_, mut empty_org) = logged_in(&spelling);
        empty_org["orgId"] = json!("");
        assert_eq!(
            validate(&f, &spelling, exited(0, &empty_org)),
            Validity::Valid,
            "an empty orgId is no org"
        );
    }

    #[test]
    fn overridden_names_the_method_and_its_source() {
        let f = fx();
        let (_dir, spelling) = profile(&f, "0192");
        for (method, source) in [
            ("api_key_helper", Some("apiKeyHelper")),
            ("api_key", Some("ANTHROPIC_API_KEY")),
            ("oauth_token", None),
            ("third_party", None),
        ] {
            let mut reply = reply_by(&spelling, true, method);
            if let Some(s) = source {
                reply["apiKeySource"] = json!(s);
            }
            assert_eq!(
                validate(&f, &spelling, exited(0, &reply)),
                Validity::Overridden {
                    method: method.into(),
                    source: source.map(str::to_owned)
                },
                "{method}"
            );
        }
    }

    #[test]
    fn drifted_is_another_config_directory_whatever_else_the_reply_says() {
        let f = fx();
        let (_dir, spelling) = profile(&f, "0192");
        let (_, mut moved) = logged_in(&spelling);
        moved["configDirectory"] = json!(format!("{spelling}/"));
        assert_eq!(
            validate(&f, &spelling, exited(0, &moved)),
            Validity::Drifted {
                reported: format!("{spelling}/")
            }
        );
        let elsewhere =
            json!({"loggedIn": false, "authMethod": "none", "configDirectory": "/u/.claude"});
        assert_eq!(
            validate(&f, &spelling, exited(1, &elsewhere)),
            Validity::Drifted {
                reported: "/u/.claude".into()
            },
            "never invalid, which would delete the profile"
        );
    }

    #[test]
    fn unknown_is_a_reply_that_does_not_parse_or_cannot_confirm_the_login() {
        let f = fx();
        let (_dir, spelling) = profile(&f, "0192");
        for stdout in [
            &b""[..],
            b"Logged in as probe1@example.com\n",
            b"[1]",
            br#"{"loggedIn": true}"#,
            br#"{"loggedIn": "yes", "authMethod": "claude.ai", "configDirectory": "/p"}"#,
        ] {
            let got = validate(
                &f,
                &spelling,
                Captured::Exited {
                    code: Some(0),
                    signal: None,
                    stdout: stdout.to_vec(),
                    stderr: vec![],
                },
            );
            assert!(matches!(got, Validity::Unknown(_)), "{got:?}");
        }
        let killed = validate(
            &f,
            &spelling,
            Captured::Exited {
                code: None,
                signal: Some(9),
                stdout: vec![],
                stderr: vec![],
            },
        );
        assert!(
            matches!(&killed, Validity::Unknown(why) if why.contains("signal 9")),
            "{killed:?}"
        );
        let (_, mut no_email) = logged_in(&spelling);
        no_email.as_object_mut().unwrap().shift_remove("email");
        assert!(
            matches!(
                validate(&f, &spelling, exited(0, &no_email)),
                Validity::Unknown(_)
            ),
            "an email that is not there confirms nothing, and must not delete the profile"
        );
        let (_, reply) = logged_in(&spelling);
        assert!(
            matches!(
                validate(&f, &spelling, exited(1, &reply)),
                Validity::Unknown(_)
            ),
            "a login that did not exit 0"
        );
    }

    #[test]
    fn unreachable_is_a_spawn_failure() {
        let f = fx();
        let (_dir, spelling) = profile(&f, "0192");
        assert_eq!(
            validate(
                &f,
                &spelling,
                Captured::SpawnFailed("No such file or directory (os error 2)".into())
            ),
            Validity::Unreachable("No such file or directory (os error 2)".into())
        );
    }

    #[test]
    fn an_interrupted_check_is_unknown_interrupted_and_a_set_token_spawns_nothing() {
        let f = fx();
        let (_dir, spelling) = profile(&f, "0192");
        assert_eq!(
            validate(&f, &spelling, Captured::Interrupted(2)),
            Validity::Unknown("interrupted".into())
        );
        let spawner = ScriptedSpawner::new();
        let cancel = Cancel::new();
        cancel.request(15);
        assert_eq!(
            f.cc.validate_profile(
                &f.env,
                &spelling,
                Path::new("/"),
                Path::new(CLAUDE),
                &account(&f),
                &spawner,
                &cancel
            ),
            Validity::Unknown("interrupted".into())
        );
        assert!(spawner.specs().is_empty(), "nothing was spawned");
    }
}

#[test]
fn a_settled_profile_read_waits_out_the_profile_s_refresh_lock_then_releases_it() {
    // §13.3: a refresh the session has in flight completes first, and the read is what it
    // wrote. The locks are the profile's own, named from its spelling.
    let f = fx_on(Platform::Linux);
    let (dir, spelling) = profile(&f, "0193");
    fs::write(
        dir.join(".credentials.json"),
        br#"{"claudeAiOauth":{"refreshToken":"rt-consumed"}}"#,
    )
    .unwrap();
    let lock = Path::new(&spelling).join(".oauth_refresh.lock");
    fs::create_dir(&lock).unwrap();
    let (file, held) = (dir.join(".credentials.json"), lock.clone());
    let session = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        fs::write(&file, ENTRY).unwrap();
        fs::remove_dir(&held).unwrap();
    });
    let guard = MutationGuard::acquire(&f.env, Duration::from_millis(100)).unwrap();
    let started = std::time::Instant::now();

    let read =
        f.cc.read_profile_credential_settled(&f.env, &dir, &spelling, &guard)
            .unwrap();
    session.join().unwrap();

    assert!(started.elapsed() >= Duration::from_millis(300));
    let c = read.present().unwrap();
    assert_eq!((c.bytes(), c.provenance()), (ENTRY, Provenance::Fresh));
    assert!(!lock.exists(), "released on return");
    let mut legacy = fs::canonicalize(&dir).unwrap().into_os_string();
    legacy.push(".lock");
    assert!(!Path::new(&legacy).exists());
}
