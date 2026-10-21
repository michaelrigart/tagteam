mod common;

use std::error::Error;
use std::sync::Arc;

use serde_json::json;
use tagteam_core::AccountId;
use tagteam_engine::account_lock::AccountLock;
use tagteam_engine::vault::{
    FileVault, KeychainVault, Leftovers, SERVICE, Vault, VaultBackend, VaultCause, VaultError,
};
use tagteam_fake::FakeAgent;
use tagteam_provider::{Env, FakeKeychain, KeychainError, Read};

/// A FakeAgent credential of generation `renew`, told apart within it by `token`. It has no
/// machine-shared key, so its vault form is itself.
fn cred(renew: &str, token: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({"fa": {"token": token, "renew": renew}})).unwrap()
}

fn backends(env: &Env, kc: Arc<FakeKeychain>) -> Vec<Vault> {
    vec![
        Vault::new(Box::new(KeychainVault::new(kc))),
        Vault::new(Box::new(FileVault::new(env.data_dir().join("vault")))),
    ]
}

#[test]
fn prev_moves_only_when_the_generation_changes() {
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    let agent = FakeAgent::new();
    for v in backends(&env, Arc::new(FakeKeychain::new())) {
        let id = AccountId::from_string("0192-a");
        let lock = AccountLock::acquire(&env, &id, AccountLock::WAIT).unwrap();
        assert!(matches!(v.read(&id), Read::Absent));
        v.store(&lock, &cred("gen1", "a"), &agent).unwrap();
        v.store(&lock, &cred("gen1", "b"), &agent).unwrap(); // same generation: no .prev
        assert!(matches!(v.read_prev(&id), Read::Absent));
        v.store(&lock, &cred("gen2", "t"), &agent).unwrap();
        assert_eq!(v.read_prev(&id).present().unwrap(), cred("gen1", "b"));
        assert_eq!(v.read(&id).present().unwrap(), cred("gen2", "t"));
        v.delete(&lock).unwrap();
        assert!(matches!(v.read(&id), Read::Absent));
        assert!(matches!(v.read_prev(&id), Read::Absent));
    }
}

#[test]
fn a_locked_keychain_aborts_the_delete_and_keeps_the_items() {
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    let agent = FakeAgent::new();
    let kc = Arc::new(FakeKeychain::new());
    let v = Vault::new(Box::new(KeychainVault::new(kc.clone())));
    let id = AccountId::from_string("x");
    let lock = AccountLock::acquire(&env, &id, AccountLock::WAIT).unwrap();
    v.store(&lock, &cred("g", "t"), &agent).unwrap();
    kc.set_locked(true);
    assert!(v.delete(&lock).is_err());
    kc.set_locked(false);
    assert!(v.read(&id).is_present());
}

#[test]
fn store_with_an_unreadable_current_generation_refuses_and_writes_nothing() {
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    let agent = FakeAgent::new();
    let kc = Arc::new(FakeKeychain::new());
    let v = Vault::new(Box::new(KeychainVault::new(kc.clone())));
    let id = AccountId::from_string("z");
    let lock = AccountLock::acquire(&env, &id, AccountLock::WAIT).unwrap();
    v.store(&lock, &cred("gen1", "t"), &agent).unwrap();
    kc.set_unreadable(SERVICE, id.as_str(), true);
    assert!(matches!(
        v.store(&lock, &cred("gen2", "t"), &agent),
        Err(VaultError::Unreadable(_))
    ));
    kc.set_unreadable(SERVICE, id.as_str(), false);
    // Nothing was written: the current generation is untouched, and no `.prev` was created.
    assert_eq!(v.read(&id).present().unwrap(), cred("gen1", "t"));
    assert!(matches!(v.read_prev(&id), Read::Absent));
}

#[test]
fn a_failed_delete_leaves_the_current_generation_readable() {
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    let agent = FakeAgent::new();
    let kc = Arc::new(FakeKeychain::new());
    let v = Vault::new(Box::new(KeychainVault::new(kc.clone())));
    let id = AccountId::from_string("w");
    let lock = AccountLock::acquire(&env, &id, AccountLock::WAIT).unwrap();
    v.store(&lock, &cred("gen1", "t"), &agent).unwrap();
    v.store(&lock, &cred("gen2", "t"), &agent).unwrap(); // gen1 moves to `.prev`, gen2 is current
    kc.set_fail_delete(SERVICE, true);
    assert!(v.delete(&lock).is_err());
    kc.set_fail_delete(SERVICE, false);
    assert_eq!(v.read(&id).present().unwrap(), cred("gen2", "t"));
}

#[test]
fn file_vault_entries_are_private() {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    let agent = FakeAgent::new();
    let v = Vault::new(Box::new(FileVault::new(env.data_dir().join("vault"))));
    let id = AccountId::from_string("y");
    let lock = AccountLock::acquire(&env, &id, AccountLock::WAIT).unwrap();
    v.store(&lock, &cred("g", "t"), &agent).unwrap();
    let meta = std::fs::metadata(env.data_dir().join("vault/y.json")).unwrap();
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
}

#[test]
fn file_vault_forces_0600_even_over_a_widened_existing_mode() {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    let agent = FakeAgent::new();
    let v = Vault::new(Box::new(FileVault::new(env.data_dir().join("vault"))));
    let id = AccountId::from_string("q");
    let lock = AccountLock::acquire(&env, &id, AccountLock::WAIT).unwrap();
    v.store(&lock, &cred("gen1", "t"), &agent).unwrap();
    let path = env.data_dir().join("vault/q.json");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    v.store(&lock, &cred("gen2", "t"), &agent).unwrap();
    let meta = std::fs::metadata(&path).unwrap();
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
}

#[test]
fn the_keychain_sweep_deletes_another_store_s_items_only_when_asked() {
    // §10.5: every data directory on a Mac shares the `tagteam` service.
    let kc = Arc::new(FakeKeychain::new());
    let v = Vault::new(Box::new(KeychainVault::new(kc.clone())));
    assert_eq!(v.sweep(false).unwrap(), Leftovers::None);
    kc.put(SERVICE, "another-stores-account", b"theirs");
    kc.put("Claude Code-credentials", "me", b"live");
    assert_eq!(v.sweep(false).unwrap(), Leftovers::Shared);
    assert!(kc.get(SERVICE, "another-stores-account").is_some(), "kept");
    kc.set_unreadable(SERVICE, "another-stores-account", true);
    assert!(matches!(v.sweep(false).unwrap(), Leftovers::Unknown(_)));
    kc.set_unreadable(SERVICE, "another-stores-account", false);
    kc.set_locked(true);
    assert!(v.sweep(true).is_err(), "a locked keychain deletes nothing");
    kc.set_locked(false);
    assert_eq!(v.sweep(true).unwrap(), Leftovers::None);
    assert!(kc.get(SERVICE, "another-stores-account").is_none());
    assert!(
        kc.get("Claude Code-credentials", "me").is_some(),
        "another service"
    );
}

#[test]
fn the_file_vault_sweep_deletes_its_directory_whatever_it_is_asked() {
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    let agent = FakeAgent::new();
    let dir = env.data_dir().join("vault");
    let v = Vault::new(Box::new(FileVault::new(dir.clone())));
    assert_eq!(v.sweep(false).unwrap(), Leftovers::None, "none there");
    let id = AccountId::from_string("z");
    let lock = AccountLock::acquire(&env, &id, AccountLock::WAIT).unwrap();
    v.store(&lock, &cred("g", "t"), &agent).unwrap();
    std::fs::write(dir.join("an-orphan.json"), "{}").unwrap();
    assert_eq!(v.sweep(false).unwrap(), Leftovers::None);
    assert!(!dir.exists());
}

#[test]
fn a_keychain_vault_names_its_keychain_and_a_file_vault_its_directory() {
    // §13.6: doctor's lock check and by-service probe, or its listing of `vault/`.
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    let kc = Arc::new(FakeKeychain::new());
    let [keychain, files] = <[Vault; 2]>::try_from(backends(&env, kc)).ok().unwrap();
    assert!(keychain.keychain().is_some() && keychain.dir().is_none());
    assert_eq!(files.dir(), Some(env.data_dir().join("vault").as_path()));
    assert!(files.keychain().is_none());
}

#[test]
fn store_writes_and_verifies_the_vault_form() {
    // H5.3. Were the read-back compared with the bytes given, this store would fail `Verify`.
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    let agent = FakeAgent::new();
    for v in backends(&env, Arc::new(FakeKeychain::new())) {
        let id = AccountId::from_string("f");
        let lock = AccountLock::acquire(&env, &id, AccountLock::WAIT).unwrap();
        let full =
            serde_json::to_vec(&tagteam_fake::credential_json("t", Some("gen1"), None)).unwrap();
        v.store(&lock, &full, &agent).unwrap();
        assert_eq!(v.read(&id).present().unwrap(), cred("gen1", "t"));
    }
}

#[test]
fn an_item_stored_before_this_branch_is_read_as_it_is_and_leaves_for_prev_in_vault_form() {
    // H5.3: nothing migrates an old item; it keeps its bytes until its next store.
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    let kc = Arc::new(FakeKeychain::new());
    let v = Vault::new(Box::new(KeychainVault::new(kc.clone())));
    let agent = FakeAgent::new();
    let id = AccountId::from_string("old");
    let lock = AccountLock::acquire(&env, &id, AccountLock::WAIT).unwrap();
    let old = serde_json::to_vec(&tagteam_fake::credential_json("t", Some("gen1"), None)).unwrap();
    kc.put(SERVICE, id.as_str(), &old);
    assert_eq!(v.read(&id).present().unwrap(), old);
    v.store(&lock, &cred("gen2", "t"), &agent).unwrap();
    assert_eq!(v.read_prev(&id).present().unwrap(), cred("gen1", "t"));
}

/// Review refactor §8.5: the Keychain's failure stays the source of a vault write or delete, so
/// a caller can match it; the vault's message is what it always was.
#[test]
fn a_keychain_failure_stays_the_source_of_a_vault_write_or_delete() {
    let kc = Arc::new(FakeKeychain::new());
    let vault = KeychainVault::new(kc.clone());

    kc.set_fail_write(SERVICE, true);
    let e = vault.write("k", b"x").unwrap_err();
    assert!(matches!(
        e,
        VaultError::Write(VaultCause::Keychain(KeychainError { rc: Some(25), .. }))
    ));
    assert_eq!(
        e.to_string(),
        "vault write failed: keychain operation failed (rc 25): injected write failure"
    );
    assert_eq!(
        e.source().unwrap().to_string(),
        "keychain operation failed (rc 25): injected write failure"
    );

    kc.set_fail_delete(SERVICE, true);
    let e = vault.delete("k").unwrap_err();
    assert!(matches!(
        e,
        VaultError::Delete(VaultCause::Keychain(KeychainError { rc: Some(25), .. }))
    ));
    assert_eq!(
        e.to_string(),
        "vault delete failed: keychain operation failed (rc 25): injected delete failure"
    );
    assert_eq!(
        e.source().unwrap().to_string(),
        "keychain operation failed (rc 25): injected delete failure"
    );
}
