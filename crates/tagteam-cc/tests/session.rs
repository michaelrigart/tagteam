//! Claude Code's session facts (§4.5 "Parallel sessions", §12): the share policy, the outer
//! home, the profile spelling, and the profile credential's read and deletion under the
//! hashed Keychain name for a recorded spelling (Appendix A.2).

use std::ffi::OsString;
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
    Provenance, Provider, ProviderError, Read, canonical_profile_path, entry_matches, profile_path,
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
    let inside = with_vars(&f.env, Some("/data/sessions/0192"), None);
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
                restored.claude_config_dir,
                restored.claude_securestorage_config_dir
            ),
            (
                outer_env.claude_config_dir.clone(),
                outer_env.claude_securestorage_config_dir.clone()
            ),
            "{config:?}, {secure:?}"
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

/// A Keychain that records, at each delete, whether `lock` is held, as CC's storage-write lock
/// is while it writes (`tests/live_store.rs`' `LockProbeKeychain`).
struct LockProbeKeychain {
    inner: Arc<FakeKeychain>,
    lock: PathBuf,
    deletes: Mutex<Vec<(String, bool)>>,
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
        self.deletes
            .lock()
            .unwrap()
            .push((s.to_owned(), self.lock.is_dir()));
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
    let probe = Arc::new(LockProbeKeychain {
        inner: f.kc.clone(),
        lock: lock.clone(),
        deletes: Mutex::new(Vec::new()),
    });
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
        [(oauth.clone(), true), (managed.clone(), true)],
        "each delete runs under the lock"
    );
    assert!(!lock.exists(), "released when the delete returns");
    assert!(!gone.exists(), "nothing is created under the old spelling");
    assert_eq!(
        (f.kc.get(&oauth, &acct), f.kc.get(&managed, &acct)),
        (None, None)
    );
}

/// Deleting the profile's items with `dir` as `place` makes: nothing, a file, or a dangling link.
fn delete_where_the_profile_is_not_a_directory(place: impl Fn(&Path)) {
    let f = fx();
    let (dir, spelling) = profile(&f, "0192");
    fs::remove_dir(&dir).unwrap();
    place(&dir);
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
    // A held lock behind the link would time this delete out if it took the lock.
    fs::create_dir(real.join(".storage-write")).unwrap();
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
        real.join(".storage-write").is_dir(),
        "the other holder's lock is untouched"
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
