mod common;

use std::sync::Arc;

use tagteam_core::{AccountId, Fingerprint};
use tagteam_engine::account_lock::AccountLock;
use tagteam_engine::vault::{FileVault, KeychainVault, Vault};
use tagteam_provider::{Env, FakeKeychain, Read};

fn fp(b: &[u8]) -> Option<Fingerprint> {
    // Test lineage: everything up to the first '#' is the generation.
    let s = std::str::from_utf8(b).ok()?;
    Some(Fingerprint::of_secret(s.split('#').next()?.as_bytes()))
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
    for v in backends(&env, Arc::new(FakeKeychain::new())) {
        let id = AccountId::from_string("0192-a");
        let lock = AccountLock::acquire(&env, &id, AccountLock::WAIT).unwrap();
        assert!(matches!(v.read(&id), Read::Absent));
        v.store(&lock, b"gen1#a", &fp).unwrap();
        v.store(&lock, b"gen1#b", &fp).unwrap(); // same generation: no .prev
        assert!(matches!(v.read_prev(&id), Read::Absent));
        v.store(&lock, b"gen2", &fp).unwrap();
        assert_eq!(v.read_prev(&id).present().unwrap(), b"gen1#b");
        assert_eq!(v.read(&id).present().unwrap(), b"gen2");
        v.delete(&lock).unwrap();
        assert!(matches!(v.read(&id), Read::Absent));
        assert!(matches!(v.read_prev(&id), Read::Absent));
    }
}

#[test]
fn a_locked_keychain_aborts_the_delete_and_keeps_the_items() {
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    let kc = Arc::new(FakeKeychain::new());
    let v = Vault::new(Box::new(KeychainVault::new(kc.clone())));
    let id = AccountId::from_string("x");
    let lock = AccountLock::acquire(&env, &id, AccountLock::WAIT).unwrap();
    v.store(&lock, b"g", &fp).unwrap();
    kc.set_locked(true);
    assert!(v.delete(&lock).is_err());
    kc.set_locked(false);
    assert!(v.read(&id).is_present());
}

#[test]
fn file_vault_entries_are_private() {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    let v = Vault::new(Box::new(FileVault::new(env.data_dir().join("vault"))));
    let id = AccountId::from_string("y");
    let lock = AccountLock::acquire(&env, &id, AccountLock::WAIT).unwrap();
    v.store(&lock, b"g", &fp).unwrap();
    let meta = std::fs::metadata(env.data_dir().join("vault/y.json")).unwrap();
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
}
