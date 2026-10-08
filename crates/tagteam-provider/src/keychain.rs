use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::read::{Read, ReadError};

/// The default keychain's lock state, from `show-keychain-info` (Appendix A.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockState {
    Unlocked,
    /// rc 36: an SSH session's login keychain stays locked until it is unlocked.
    Locked,
    /// Any other rc, or a timeout. Callers proceed; the tri-state reads refuse safely.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeychainError {
    pub rc: Option<i32>,
    pub detail: String,
}

impl fmt::Display for KeychainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.rc {
            Some(rc) => write!(f, "keychain operation failed (rc {rc}): {}", self.detail),
            None => write!(f, "keychain operation failed: {}", self.detail),
        }
    }
}

impl std::error::Error for KeychainError {}

/// Generic-password items keyed by (service, account). Appendix A.3 semantics.
pub trait Keychain: Send + Sync {
    fn find(&self, service: &str, account: &str) -> Read<Vec<u8>>;
    /// Attributes only: never prompts and never returns the secret.
    fn exists(&self, service: &str, account: &str) -> Read<()>;
    /// Adds or updates in place (`-U`), then verifies by reading back.
    fn upsert(&self, service: &str, account: &str, data: &[u8]) -> Result<(), KeychainError>;
    /// Deleting an absent item succeeds.
    fn delete(&self, service: &str, account: &str) -> Result<(), KeychainError>;
    /// The default keychain's lock state. Never prompts.
    fn lock_state(&self) -> LockState;
    /// Asks macOS to unlock the default keychain with the terminal attached, so it prompts for
    /// the password itself; tagteam never sees, stores or passes it. True on exit 0. Only the
    /// CLI calls this, and only on a terminal.
    fn unlock(&self) -> bool;
    /// Appendix A.3's delete by service (`purge --keychain-orphans`, §10.5): deletes every item
    /// of `service`, whatever its account, then verifies that none is left. Returns how many it
    /// deleted. The default refuses: only a keychain that can enumerate a service implements it.
    fn delete_service(&self, service: &str) -> Result<u32, KeychainError> {
        Err(KeychainError {
            rc: None,
            detail: format!("this keychain cannot delete the items of {service:?} by service"),
        })
    }
    /// Appendix A.3's probe of a whole service, attributes only: `Present(true)` while any
    /// item of `service` exists, whatever its account. It never prompts and never reads a
    /// secret, so it answers on a locked keychain too. The default cannot tell.
    fn service_has_items(&self, service: &str) -> Read<bool> {
        Read::Unreadable(ReadError::new(
            "keychain",
            format!("this keychain cannot probe the service {service:?}"),
        ))
    }
}

impl<K: Keychain + ?Sized> Keychain for Arc<K> {
    fn find(&self, s: &str, a: &str) -> Read<Vec<u8>> {
        (**self).find(s, a)
    }
    fn exists(&self, s: &str, a: &str) -> Read<()> {
        (**self).exists(s, a)
    }
    fn upsert(&self, s: &str, a: &str, d: &[u8]) -> Result<(), KeychainError> {
        (**self).upsert(s, a, d)
    }
    fn delete(&self, s: &str, a: &str) -> Result<(), KeychainError> {
        (**self).delete(s, a)
    }
    fn lock_state(&self) -> LockState {
        (**self).lock_state()
    }
    fn unlock(&self) -> bool {
        (**self).unlock()
    }
    fn delete_service(&self, s: &str) -> Result<u32, KeychainError> {
        (**self).delete_service(s)
    }
    fn service_has_items(&self, s: &str) -> Read<bool> {
        (**self).service_has_items(s)
    }
}

fn locked_read<T>() -> Read<T> {
    Read::Unreadable(ReadError::new("keychain", "rc 36: the keychain is locked"))
}

pub(crate) fn locked_err() -> KeychainError {
    KeychainError {
        rc: Some(36),
        detail: "the keychain is locked".into(),
    }
}

type Key = (String, String);

/// In-memory Keychain for tests, with failure injection.
#[derive(Default)]
pub struct FakeKeychain {
    items: Mutex<BTreeMap<Key, Vec<u8>>>,
    locked: AtomicBool,
    unreadable: Mutex<BTreeSet<Key>>,
    fail_write: Mutex<BTreeSet<String>>,
    fail_delete: Mutex<BTreeSet<String>>,
    panic_delete: Mutex<BTreeSet<String>>,
    refuse_unlock: AtomicBool,
    unlock_attempts: AtomicUsize,
}

fn key(s: &str, a: &str) -> Key {
    (s.to_owned(), a.to_owned())
}

fn toggle(set: &Mutex<BTreeSet<String>>, s: &str, on: bool) {
    let mut g = set.lock().unwrap();
    if on {
        g.insert(s.to_owned());
    } else {
        g.remove(s);
    }
}

impl FakeKeychain {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn put(&self, s: &str, a: &str, data: &[u8]) {
        self.items.lock().unwrap().insert(key(s, a), data.to_vec());
    }
    pub fn get(&self, s: &str, a: &str) -> Option<Vec<u8>> {
        self.items.lock().unwrap().get(&key(s, a)).cloned()
    }
    pub fn items(&self) -> BTreeMap<Key, Vec<u8>> {
        self.items.lock().unwrap().clone()
    }
    pub fn set_locked(&self, on: bool) {
        self.locked.store(on, Ordering::SeqCst);
    }
    pub fn set_unreadable(&self, s: &str, a: &str, on: bool) {
        let mut g = self.unreadable.lock().unwrap();
        if on {
            g.insert(key(s, a));
        } else {
            g.remove(&key(s, a));
        }
    }
    pub fn set_fail_write(&self, s: &str, on: bool) {
        toggle(&self.fail_write, s, on);
    }
    pub fn set_fail_delete(&self, s: &str, on: bool) {
        toggle(&self.fail_delete, s, on);
    }
    /// Panics inside `delete` for this service, to test rollback during unwinding.
    pub fn set_panic_on_delete(&self, s: &str, on: bool) {
        toggle(&self.panic_delete, s, on);
    }
    /// Makes `unlock` fail, as a wrong password or a dismissed prompt does.
    pub fn set_refuse_unlock(&self, on: bool) {
        self.refuse_unlock.store(on, Ordering::SeqCst);
    }
    pub fn unlock_attempts(&self) -> usize {
        self.unlock_attempts.load(Ordering::SeqCst)
    }
    fn blocked(&self, s: &str, a: &str) -> bool {
        self.locked.load(Ordering::SeqCst) || self.unreadable.lock().unwrap().contains(&key(s, a))
    }
}

impl Keychain for FakeKeychain {
    fn find(&self, s: &str, a: &str) -> Read<Vec<u8>> {
        if self.blocked(s, a) {
            return locked_read();
        }
        match self.get(s, a) {
            Some(v) => Read::Present(v),
            None => Read::Absent,
        }
    }
    /// Attributes only, as `security find-generic-password` without `-w` or `-g` reads them:
    /// rc 0 when the item is there and rc 44 when it is not, locked or not (L342). Only an
    /// item marked unreadable fails, which is how a test injects a failing `exists`.
    fn exists(&self, s: &str, a: &str) -> Read<()> {
        if self.unreadable.lock().unwrap().contains(&key(s, a)) {
            return Read::Unreadable(ReadError::new(
                "keychain",
                "rc 1: injected failure reading the item's attributes",
            ));
        }
        match self.get(s, a) {
            Some(_) => Read::Present(()),
            None => Read::Absent,
        }
    }
    fn upsert(&self, s: &str, a: &str, d: &[u8]) -> Result<(), KeychainError> {
        if self.locked.load(Ordering::SeqCst) {
            return Err(locked_err());
        }
        if self.fail_write.lock().unwrap().contains(s) {
            return Err(KeychainError {
                rc: Some(25),
                detail: "injected write failure".into(),
            });
        }
        self.put(s, a, d);
        Ok(())
    }
    fn delete(&self, s: &str, a: &str) -> Result<(), KeychainError> {
        if self.panic_delete.lock().unwrap().contains(s) {
            panic!("injected panic deleting {s}");
        }
        if self.locked.load(Ordering::SeqCst) {
            return Err(locked_err());
        }
        if self.fail_delete.lock().unwrap().contains(s) {
            return Err(KeychainError {
                rc: Some(25),
                detail: "injected delete failure".into(),
            });
        }
        self.items.lock().unwrap().remove(&key(s, a));
        Ok(())
    }
    fn lock_state(&self) -> LockState {
        if self.locked.load(Ordering::SeqCst) {
            LockState::Locked
        } else {
            LockState::Unlocked
        }
    }
    fn unlock(&self) -> bool {
        self.unlock_attempts.fetch_add(1, Ordering::SeqCst);
        if self.refuse_unlock.load(Ordering::SeqCst) {
            return false;
        }
        self.set_locked(false);
        true
    }
    /// A locked keychain and an injected delete failure fail it, as `delete` does.
    fn delete_service(&self, s: &str) -> Result<u32, KeychainError> {
        if self.locked.load(Ordering::SeqCst) {
            return Err(locked_err());
        }
        if self.fail_delete.lock().unwrap().contains(s) {
            return Err(KeychainError {
                rc: Some(25),
                detail: "injected delete failure".into(),
            });
        }
        let mut items = self.items.lock().unwrap();
        let before = items.len();
        items.retain(|(svc, _), _| svc != s);
        Ok(u32::try_from(before - items.len()).expect("a test keychain holds few items"))
    }
    /// Attributes only, locked or not, like `exists` (L342). An item of the service marked
    /// unreadable fails it, as it fails `exists`.
    fn service_has_items(&self, s: &str) -> Read<bool> {
        if self
            .unreadable
            .lock()
            .unwrap()
            .iter()
            .any(|(svc, _)| svc == s)
        {
            return Read::Unreadable(ReadError::new(
                "keychain",
                "rc 1: injected failure reading the service's attributes",
            ));
        }
        Read::Present(self.items.lock().unwrap().keys().any(|(svc, _)| svc == s))
    }
}

/// A directory-backed fake, so tests can drive the real binary across processes.
#[cfg(feature = "file-keychain")]
pub struct FileKeychain {
    dir: std::path::PathBuf,
}

#[cfg(feature = "file-keychain")]
impl FileKeychain {
    pub fn new(dir: impl Into<std::path::PathBuf>) -> Self {
        Self { dir: dir.into() }
    }
    fn path(&self, s: &str, a: &str) -> std::path::PathBuf {
        self.dir
            .join(format!("{}.{}", hex::encode(s), hex::encode(a)))
    }
    fn locked(&self) -> bool {
        self.dir.join("LOCKED").exists()
    }
    /// The files of `service`'s items: every name `path` gives one, whatever the account.
    fn service_files(&self, s: &str) -> std::io::Result<Vec<std::path::PathBuf>> {
        let prefix = format!("{}.", hex::encode(s));
        let listing = match std::fs::read_dir(&self.dir) {
            Ok(l) => l,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut files = Vec::new();
        for entry in listing {
            let entry = entry?;
            if entry.file_name().to_string_lossy().starts_with(&prefix) {
                files.push(entry.path());
            }
        }
        Ok(files)
    }
}

#[cfg(feature = "file-keychain")]
impl Keychain for FileKeychain {
    fn find(&self, s: &str, a: &str) -> Read<Vec<u8>> {
        if self.locked() {
            return locked_read();
        }
        match std::fs::read(self.path(s, a)) {
            Ok(v) => Read::Present(v),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Read::Absent,
            Err(e) => Read::Unreadable(ReadError::new("keychain", e.to_string())),
        }
    }
    /// Attributes only, locked or not, like `FakeKeychain::exists` (L342).
    fn exists(&self, s: &str, a: &str) -> Read<()> {
        match std::fs::metadata(self.path(s, a)) {
            Ok(_) => Read::Present(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Read::Absent,
            Err(e) => Read::Unreadable(ReadError::new("keychain", e.to_string())),
        }
    }
    fn upsert(&self, s: &str, a: &str, d: &[u8]) -> Result<(), KeychainError> {
        if self.locked() {
            return Err(locked_err());
        }
        crate::atomic::ensure_private_dir(&self.dir)
            .and_then(|()| crate::atomic::write_atomic(&self.path(s, a), d, 0o600))
            .map_err(|e| KeychainError {
                rc: None,
                detail: e.to_string(),
            })
    }
    fn delete(&self, s: &str, a: &str) -> Result<(), KeychainError> {
        if self.locked() {
            return Err(locked_err());
        }
        match std::fs::remove_file(self.path(s, a)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(KeychainError {
                rc: None,
                detail: e.to_string(),
            }),
        }
    }
    fn lock_state(&self) -> LockState {
        if self.locked() {
            LockState::Locked
        } else {
            LockState::Unlocked
        }
    }
    /// No terminal can unlock a directory; binary tests reach this only without one.
    fn unlock(&self) -> bool {
        false
    }
    fn delete_service(&self, s: &str) -> Result<u32, KeychainError> {
        if self.locked() {
            return Err(locked_err());
        }
        let failed = |e: std::io::Error| KeychainError {
            rc: None,
            detail: e.to_string(),
        };
        let mut deleted = 0;
        for path in self.service_files(s).map_err(failed)? {
            match std::fs::remove_file(&path) {
                Ok(()) => deleted += 1,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(failed(e)),
            }
        }
        Ok(deleted)
    }
    /// Attributes only, locked or not, like `exists` (L342).
    fn service_has_items(&self, s: &str) -> Read<bool> {
        match self.service_files(s) {
            Ok(files) => Read::Present(!files.is_empty()),
            Err(e) => Read::Unreadable(ReadError::new("keychain", e.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_keychain_models_absent_locked_and_failures() {
        let k = FakeKeychain::new();
        assert!(matches!(k.find("s", "a"), Read::Absent));
        k.upsert("s", "a", b"v").unwrap();
        assert_eq!(k.find("s", "a").present().unwrap(), b"v");
        assert!(k.exists("s", "a").is_present());
        k.set_unreadable("s", "a", true);
        assert!(matches!(k.find("s", "a"), Read::Unreadable(_)));
        k.set_unreadable("s", "a", false);
        k.set_fail_write("s", true);
        assert!(k.upsert("s", "a", b"w").is_err());
        k.set_fail_delete("s", true);
        assert!(k.delete("s", "a").is_err());
        k.set_locked(true);
        // L342: `exists` reads attributes only, which `security` answers without an unlock.
        assert!(k.exists("s", "a").is_present());
        assert!(matches!(k.find("s", "a"), Read::Unreadable(_)));
        k.set_locked(false);
        k.set_fail_delete("s", false);
        k.delete("s", "a").unwrap();
        k.delete("s", "a").unwrap();
        assert!(matches!(k.find("s", "a"), Read::Absent));
    }

    #[test]
    fn fake_exists_answers_present_or_absent_even_when_locked() {
        // L342: real `security find-generic-password` (no -w, no -g) gives rc 0 or rc 44
        // whether or not the keychain is locked; engine tests must follow reality.
        let k = FakeKeychain::new();
        k.put("s", "a", b"v");
        k.set_locked(true);
        assert!(k.exists("s", "a").is_present());
        assert!(matches!(k.exists("s", "missing"), Read::Absent));
        assert!(
            matches!(k.find("s", "a"), Read::Unreadable(_)),
            "the secret still needs it"
        );
        // An item marked unreadable is still the way a test injects a failing `exists`.
        k.set_locked(false);
        k.set_unreadable("s", "a", true);
        assert!(matches!(k.exists("s", "a"), Read::Unreadable(_)));
    }

    #[test]
    fn the_fake_deletes_and_probes_a_whole_service_through_an_arc_too() {
        // Appendix A.3: every item of the service, whatever its account, and nothing else.
        let k = Arc::new(FakeKeychain::new());
        k.put("s", "a", b"1");
        k.put("s", "a.prev", b"2");
        k.put("s", "b", b"3");
        k.put("t", "a", b"4");
        let kc: &dyn Keychain = &k;
        assert!(matches!(kc.service_has_items("s"), Read::Present(true)));
        k.set_locked(true);
        assert!(
            matches!(kc.service_has_items("s"), Read::Present(true)),
            "attributes need no unlock (L342)"
        );
        assert_eq!(kc.delete_service("s").unwrap_err().rc, Some(36));
        k.set_locked(false);
        k.set_fail_delete("s", true);
        assert!(kc.delete_service("s").is_err());
        k.set_fail_delete("s", false);
        assert_eq!(kc.delete_service("s").unwrap(), 3);
        assert!(matches!(kc.service_has_items("s"), Read::Present(false)));
        assert_eq!(
            k.get("t", "a").as_deref(),
            Some(&b"4"[..]),
            "another service"
        );
        assert_eq!(kc.delete_service("s").unwrap(), 0, "absent is done");
        k.set_unreadable("t", "a", true);
        assert!(matches!(kc.service_has_items("t"), Read::Unreadable(_)));
    }

    #[test]
    fn a_keychain_that_cannot_enumerate_a_service_refuses_both() {
        // The defaults fail closed: nothing is deleted and nothing is taken for absent.
        struct Bare;
        impl Keychain for Bare {
            fn find(&self, _: &str, _: &str) -> Read<Vec<u8>> {
                Read::Absent
            }
            fn exists(&self, _: &str, _: &str) -> Read<()> {
                Read::Absent
            }
            fn upsert(&self, _: &str, _: &str, _: &[u8]) -> Result<(), KeychainError> {
                Ok(())
            }
            fn delete(&self, _: &str, _: &str) -> Result<(), KeychainError> {
                Ok(())
            }
            fn lock_state(&self) -> LockState {
                LockState::Unlocked
            }
            fn unlock(&self) -> bool {
                true
            }
        }
        assert!(Bare.delete_service("s").is_err());
        assert!(matches!(Bare.service_has_items("s"), Read::Unreadable(_)));
    }

    #[test]
    fn fake_keychain_models_the_lock_check_and_unlock() {
        let k = FakeKeychain::new();
        assert_eq!(k.lock_state(), LockState::Unlocked);
        k.set_locked(true);
        assert_eq!(k.lock_state(), LockState::Locked);
        k.set_refuse_unlock(true); // a wrong password, or a dismissed prompt
        assert!(!k.unlock());
        assert_eq!(k.lock_state(), LockState::Locked);
        k.set_refuse_unlock(false);
        assert!(k.unlock());
        assert_eq!(k.lock_state(), LockState::Unlocked);
        assert_eq!(k.unlock_attempts(), 2);
    }

    #[cfg(feature = "file-keychain")]
    #[test]
    fn file_keychain_persists_across_instances() {
        let d = tempfile::tempdir().unwrap();
        FileKeychain::new(d.path())
            .upsert("Claude Code-credentials", "me", b"{}")
            .unwrap();
        let k = FileKeychain::new(d.path());
        assert_eq!(
            k.find("Claude Code-credentials", "me").present().unwrap(),
            b"{}"
        );
        assert_eq!(k.lock_state(), LockState::Unlocked);
        std::fs::write(d.path().join("LOCKED"), "").unwrap();
        assert!(matches!(
            k.find("Claude Code-credentials", "me"),
            Read::Unreadable(_)
        ));
        assert!(k.upsert("x", "y", b"z").is_err());
        assert_eq!(k.lock_state(), LockState::Locked);
        assert!(!k.unlock());
        // L342: attributes need no unlock.
        assert!(
            k.exists("Claude Code-credentials", "me").is_present(),
            "present while locked"
        );
        assert!(matches!(k.exists("nope", "me"), Read::Absent));
    }

    #[cfg(feature = "file-keychain")]
    #[test]
    fn file_keychain_deletes_and_probes_a_whole_service() {
        let d = tempfile::tempdir().unwrap();
        let k = FileKeychain::new(d.path().join("kc"));
        assert!(
            matches!(k.service_has_items("s"), Read::Present(false)),
            "no directory"
        );
        assert_eq!(k.delete_service("s").unwrap(), 0);
        for (svc, acct) in [("s", "a"), ("s", "a.prev"), ("s", "b"), ("st", "a")] {
            k.upsert(svc, acct, b"v").unwrap();
        }
        assert!(matches!(k.service_has_items("s"), Read::Present(true)));
        std::fs::write(d.path().join("kc/LOCKED"), "").unwrap();
        assert!(
            matches!(k.service_has_items("s"), Read::Present(true)),
            "L342"
        );
        assert_eq!(k.delete_service("s").unwrap_err().rc, Some(36));
        std::fs::remove_file(d.path().join("kc/LOCKED")).unwrap();
        assert_eq!(k.delete_service("s").unwrap(), 3);
        assert!(matches!(k.service_has_items("s"), Read::Present(false)));
        assert!(
            k.exists("st", "a").is_present(),
            "a service whose name starts like it is another service"
        );
    }
}
