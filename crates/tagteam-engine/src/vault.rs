use std::cell::RefCell;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tagteam_core::AccountId;
use tagteam_provider::atomic::{ensure_private_dir, write_atomic_private};
use tagteam_provider::{Keychain, KeychainError, Provider, Read, ReadError};

use crate::account_lock::AccountLock;

/// What a vault write or delete failed on (review refactor §8.5): the Keychain's own error, the
/// file backend's I/O error, or one of the vault's own checks.
#[derive(Debug, thiserror::Error)]
pub enum VaultCause {
    #[error(transparent)]
    Keychain(#[from] KeychainError),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("{0}")]
    Check(String),
}

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("vault write failed: {0}")]
    Write(#[source] VaultCause),
    #[error("vault delete failed: {0}")]
    Delete(#[source] VaultCause),
    #[error("{0}")]
    Unreadable(ReadError),
    #[error("the vault entry did not read back as written")]
    Verify,
}

/// What a full purge's sweep of the vault left (§10.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Leftovers {
    /// Nothing is left.
    None,
    /// Entries that no account of this store names remain where every tagteam data directory's
    /// entries live (the macOS Keychain's `tagteam` service): they may be another data
    /// directory's, so only `--keychain-orphans` deletes them.
    Shared,
    /// Whether any remain could not be told.
    Unknown(String),
}

pub trait VaultBackend: Send + Sync {
    fn read(&self, key: &str) -> Read<Vec<u8>>;
    fn write(&self, key: &str, bytes: &[u8]) -> Result<(), VaultError>;
    fn delete(&self, key: &str) -> Result<(), VaultError>;
    /// The Keychain behind the backend (macOS): purge's sweep by service (§10.5), and doctor's
    /// lock check and by-service probe (§13.6). `None` for a backend that keeps no Keychain
    /// items.
    fn keychain(&self) -> Option<&dyn Keychain> {
        None
    }
    /// The directory of a file backend (Linux): purge deletes it whole (§10.5), and doctor
    /// lists it (§13.6).
    fn dir(&self) -> Option<&Path> {
        None
    }
}

pub const SERVICE: &str = "tagteam";

/// macOS: generic passwords, service `tagteam`, account `<id>` / `<id>.prev` (§6.2).
pub struct KeychainVault {
    keychain: Arc<dyn Keychain>,
}

impl KeychainVault {
    pub fn new(keychain: Arc<dyn Keychain>) -> Self {
        Self { keychain }
    }
}

impl VaultBackend for KeychainVault {
    fn read(&self, key: &str) -> Read<Vec<u8>> {
        self.keychain.find(SERVICE, key)
    }
    fn write(&self, key: &str, bytes: &[u8]) -> Result<(), VaultError> {
        self.keychain
            .upsert(SERVICE, key, bytes)
            .map_err(|e| VaultError::Write(e.into()))
    }
    fn delete(&self, key: &str) -> Result<(), VaultError> {
        self.keychain
            .delete(SERVICE, key)
            .map_err(|e| VaultError::Delete(e.into()))
    }
    fn keychain(&self) -> Option<&dyn Keychain> {
        Some(self.keychain.as_ref())
    }
}

/// Linux: `<dir>/<id>.json` and `<id>.prev.json`, 0600, directory 0700 (§6.2).
pub struct FileVault {
    dir: PathBuf,
}

impl FileVault {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }
    fn path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.json"))
    }
}

impl VaultBackend for FileVault {
    fn read(&self, key: &str) -> Read<Vec<u8>> {
        match fs::read(self.path(key)) {
            Ok(b) => Read::Present(b),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Read::Absent,
            Err(e) => Read::Unreadable(ReadError::new("vault", e.to_string())),
        }
    }
    fn write(&self, key: &str, bytes: &[u8]) -> Result<(), VaultError> {
        ensure_private_dir(&self.dir)
            .and_then(|()| write_atomic_private(&self.path(key), bytes, 0o600))
            .map_err(|e| VaultError::Write(e.into()))
    }
    fn delete(&self, key: &str) -> Result<(), VaultError> {
        match fs::remove_file(self.path(key)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(VaultError::Delete(e.into())),
        }
    }
    fn dir(&self) -> Option<&Path> {
        Some(&self.dir)
    }
}

/// The source of truth for credential bytes. It never parses what it stores: lineage and the
/// stored form come from the caller's provider (`fingerprint`, and `vault_form`, H5.3).
pub struct Vault {
    backend: Box<dyn VaultBackend>,
    /// Bumped by every `store` and `delete` before it touches the backend, whether or not it
    /// succeeds, because a failed store may already have written `.prev` (review refactor
    /// §8.11). `VaultMemo` keeps a read only while this count is unchanged.
    writes: AtomicU64,
}

fn prev_key(id: &AccountId) -> String {
    format!("{id}.prev")
}

impl Vault {
    pub fn new(backend: Box<dyn VaultBackend>) -> Self {
        Self {
            backend,
            writes: AtomicU64::new(0),
        }
    }

    pub fn read(&self, id: &AccountId) -> Read<Vec<u8>> {
        self.backend.read(id.as_str())
    }

    pub fn read_prev(&self, id: &AccountId) -> Read<Vec<u8>> {
        self.backend.read(&prev_key(id))
    }

    /// Writes `bytes` in `p`'s vault form (H5.3) under the account lock. The current generation
    /// moves to `.prev`, in vault form too, only when the fingerprint changes; the write is
    /// verified by reading back the form written. A verified write is logged at INFO (§14.2,
    /// Decision 10): the account's ID, and the generation's fingerprint as its first 12 hex
    /// digits.
    pub fn store(
        &self,
        lock: &AccountLock,
        bytes: &[u8],
        p: &dyn Provider,
    ) -> Result<(), VaultError> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        let id = lock.id();
        let form = p.vault_form(bytes);
        let fp = p.fingerprint(&form);
        let new_generation = match self.backend.read(id.as_str()) {
            Read::Present(old) => {
                let changed = p.fingerprint(&old) != fp;
                if changed {
                    self.backend.write(&prev_key(id), &p.vault_form(&old))?;
                }
                changed
            }
            Read::Absent => true,
            Read::Unreadable(e) => return Err(VaultError::Unreadable(e)),
        };
        self.backend.write(id.as_str(), &form)?;
        match self.backend.read(id.as_str()) {
            Read::Present(v) if v == *form => {}
            _ => return Err(VaultError::Verify),
        }
        tracing::info!(
            account = %id,
            fp = fp.as_ref().map(|f| tracing::field::display(f.short12())),
            new_generation,
            "stored a credential in the vault"
        );
        Ok(())
    }

    /// The backend's Keychain (macOS), or `None` (§10.5, §13.6).
    pub fn keychain(&self) -> Option<&dyn Keychain> {
        self.backend.keychain()
    }

    /// The backend's directory (Linux), or `None` (§10.5, §13.6).
    pub fn dir(&self) -> Option<&Path> {
        self.backend.dir()
    }

    /// §10.5: a full purge's last word on the vault, once every account's entries are deleted.
    /// `shared_too` is `--keychain-orphans`. Every tagteam data directory on the Mac shares the
    /// `tagteam` service, so items left there are deleted by service only with `shared_too`,
    /// and otherwise found by the attributes-only probe (Appendix A.3). `vault/` is this data
    /// directory's alone, so it goes whole whatever `shared_too` says, and a link there is
    /// removed as a link. A backend with neither refuses, so it never reports a clean sweep.
    pub fn sweep(&self, shared_too: bool) -> Result<Leftovers, VaultError> {
        if let Some(keychain) = self.keychain() {
            if shared_too {
                keychain
                    .delete_service(SERVICE)
                    .map_err(|e| VaultError::Delete(e.into()))?;
                return Ok(Leftovers::None);
            }
            return Ok(match keychain.service_has_items(SERVICE) {
                Read::Present(true) => Leftovers::Shared,
                Read::Present(false) | Read::Absent => Leftovers::None,
                Read::Unreadable(e) => Leftovers::Unknown(e.to_string()),
            });
        }
        let Some(dir) = self.dir() else {
            return Err(VaultError::Delete(VaultCause::Check(
                "this vault cannot be swept".into(),
            )));
        };
        let removed = match fs::symlink_metadata(dir) {
            Ok(m) if m.is_dir() => fs::remove_dir_all(dir),
            Ok(_) => fs::remove_file(dir),
            Err(e) => Err(e),
        };
        match removed {
            Ok(()) => Ok(Leftovers::None),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Leftovers::None),
            Err(e) => Err(VaultError::Delete(e.into())),
        }
    }

    /// Strict: both generations are deleted, errors propagate, and absence is verified. `.prev`
    /// goes first, so a failure partway through leaves the current generation — the one an
    /// account actually needs to keep working — in place rather than gone. A locked Keychain
    /// that still holds an item aborts the delete (§6.2).
    pub fn delete(&self, lock: &AccountLock) -> Result<(), VaultError> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        let id = lock.id();
        for key in [prev_key(id), id.to_string()] {
            self.backend.delete(&key)?;
            match self.backend.read(&key) {
                Read::Absent => {}
                Read::Present(_) => {
                    return Err(VaultError::Delete(VaultCause::Check(format!(
                        "{key} is still present"
                    ))));
                }
                Read::Unreadable(e) => {
                    return Err(VaultError::Delete(VaultCause::Check(format!(
                        "could not verify {key}: {e}"
                    ))));
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Slot {
    Current,
    Prev,
}

struct Kept {
    id: AccountId,
    slot: Slot,
    at: u64,
    read: Read<Vec<u8>>,
}

/// Vault reads under one switch attempt's account locks (review refactor §8.11). Writes need the
/// account lock and bump the vault's count; a read kept at an older count is read again. Never
/// kept: `Unreadable`, or an account outside `locks`. Never shared: the daemon's threads share
/// `Vault` and `Engine`. It holds credentials, so it has no `Debug`.
pub(crate) struct VaultMemo<'a> {
    vault: &'a Vault,
    locks: &'a [AccountLock],
    kept: RefCell<Vec<Kept>>,
}

impl<'a> VaultMemo<'a> {
    pub(crate) fn new(vault: &'a Vault, locks: &'a [AccountLock]) -> Self {
        Self {
            vault,
            locks,
            kept: RefCell::new(Vec::new()),
        }
    }

    pub(crate) fn read(&self, id: &AccountId) -> Read<Vec<u8>> {
        self.through(id, Slot::Current)
    }

    pub(crate) fn read_prev(&self, id: &AccountId) -> Read<Vec<u8>> {
        self.through(id, Slot::Prev)
    }

    fn through(&self, id: &AccountId, slot: Slot) -> Read<Vec<u8>> {
        let fresh = || match slot {
            Slot::Current => self.vault.read(id),
            Slot::Prev => self.vault.read_prev(id),
        };
        if !self.locks.iter().any(|l| l.id() == id) {
            return fresh();
        }
        // Only this thread can write a locked account, under its account lock, so no write of it
        // overlaps this read. A write to another account bumps the count too, and only makes the
        // next read fresh.
        let at = self.vault.writes.load(Ordering::SeqCst);
        let mut kept = self.kept.borrow_mut();
        kept.retain(|k| k.at == at);
        if let Some(k) = kept.iter().find(|k| &k.id == id && k.slot == slot) {
            return k.read.clone();
        }
        let read = fresh();
        if !matches!(read, Read::Unreadable(_)) {
            kept.push(Kept {
                id: id.clone(),
                slot,
                at,
                read: read.clone(),
            });
        }
        read
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use tagteam_cc::ClaudeCode;
    use tagteam_cc::live::Platform;
    use tagteam_provider::{Env, FakeKeychain};

    use super::*;

    /// Backend state the tests reach after the vault has taken the backend.
    #[derive(Default)]
    struct Shared {
        items: Mutex<BTreeMap<String, Vec<u8>>>,
        reads: Mutex<BTreeMap<String, usize>>,
        unreadable: Mutex<BTreeSet<String>>,
        refuse_writes: AtomicBool,
    }

    struct Counting(Arc<Shared>);

    impl VaultBackend for Counting {
        fn read(&self, key: &str) -> Read<Vec<u8>> {
            *self
                .0
                .reads
                .lock()
                .unwrap()
                .entry(key.to_owned())
                .or_default() += 1;
            if self.0.unreadable.lock().unwrap().contains(key) {
                return Read::Unreadable(ReadError::new("vault", "refused"));
            }
            match self.0.items.lock().unwrap().get(key) {
                Some(b) => Read::Present(b.clone()),
                None => Read::Absent,
            }
        }
        fn write(&self, key: &str, bytes: &[u8]) -> Result<(), VaultError> {
            if self.0.refuse_writes.load(Ordering::SeqCst) {
                return Err(VaultError::Write(VaultCause::Check("refused".into())));
            }
            self.0
                .items
                .lock()
                .unwrap()
                .insert(key.to_owned(), bytes.to_vec());
            Ok(())
        }
        fn delete(&self, key: &str) -> Result<(), VaultError> {
            self.0.items.lock().unwrap().remove(key);
            Ok(())
        }
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        env: Env,
        shared: Arc<Shared>,
        vault: Vault,
        provider: ClaudeCode,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let shared = Arc::new(Shared::default());
            Self {
                env: Env::for_test(dir.path()),
                _dir: dir,
                vault: Vault::new(Box::new(Counting(shared.clone()))),
                shared,
                provider: ClaudeCode::new(Arc::new(FakeKeychain::new()), Platform::MacOs),
            }
        }

        fn lock(&self, id: &AccountId) -> AccountLock {
            AccountLock::acquire(&self.env, id, AccountLock::WAIT).unwrap()
        }

        fn reads(&self, key: &str) -> usize {
            self.shared
                .reads
                .lock()
                .unwrap()
                .get(key)
                .copied()
                .unwrap_or(0)
        }

        /// The vault form of a credential whose refresh token is `rt-<n>`.
        fn form(&self, n: u32) -> Vec<u8> {
            self.provider.vault_form(&cred(n)).into_owned()
        }
    }

    fn cred(n: u32) -> Vec<u8> {
        format!(r#"{{"claudeAiOauth":{{"refreshToken":"rt-{n}"}},"mcpOAuth":{{}}}}"#).into_bytes()
    }

    fn id(name: &str) -> AccountId {
        AccountId::from_string(name)
    }

    #[test]
    fn a_memo_reads_a_locked_entry_once_until_the_vault_is_written() {
        let f = Fixture::new();
        let a = id("a");
        let lock = f.lock(&a);
        f.vault.store(&lock, &cred(1), &f.provider).unwrap();
        let locks = [lock];
        let memo = VaultMemo::new(&f.vault, &locks);

        let (cur0, prev0) = (f.reads("a"), f.reads("a.prev"));
        let cur = memo.read(&a);
        let prev = memo.read_prev(&a);
        memo.read(&a);
        memo.read_prev(&a);
        assert_eq!(f.reads("a") - cur0, 1);
        assert_eq!(f.reads("a.prev") - prev0, 1);
        assert_eq!(cur.present(), f.vault.read(&a).present());
        assert_eq!(prev.present(), f.vault.read_prev(&a).present());

        f.vault.store(&locks[0], &cred(2), &f.provider).unwrap();
        let (cur0, prev0) = (f.reads("a"), f.reads("a.prev"));
        assert_eq!(memo.read(&a).present(), Some(f.form(2)));
        assert_eq!(memo.read_prev(&a).present(), Some(f.form(1)));
        memo.read(&a);
        memo.read_prev(&a);
        assert_eq!(f.reads("a") - cur0, 1);
        assert_eq!(f.reads("a.prev") - prev0, 1);

        f.vault.delete(&locks[0]).unwrap();
        let cur0 = f.reads("a");
        assert!(matches!(memo.read(&a), Read::Absent));
        assert!(matches!(memo.read(&a), Read::Absent));
        assert_eq!(f.reads("a") - cur0, 1);
    }

    #[test]
    fn a_failed_write_still_makes_the_next_read_fresh() {
        let f = Fixture::new();
        let a = id("a");
        let locks = [f.lock(&a)];
        let memo = VaultMemo::new(&f.vault, &locks);
        assert!(matches!(memo.read(&a), Read::Absent));

        f.shared.refuse_writes.store(true, Ordering::SeqCst);
        let err = f.vault.store(&locks[0], &cred(1), &f.provider).unwrap_err();
        assert!(matches!(err, VaultError::Write(_)), "{err}");

        let before = f.reads("a");
        assert!(matches!(memo.read(&a), Read::Absent));
        assert_eq!(f.reads("a") - before, 1);
    }

    #[test]
    fn a_memo_keeps_no_unreadable_read_and_no_unlocked_account() {
        let f = Fixture::new();
        let (a, c) = (id("a"), id("c"));
        let locks = [f.lock(&a)];
        f.vault.store(&locks[0], &cred(1), &f.provider).unwrap();
        let memo = VaultMemo::new(&f.vault, &locks);

        f.shared.unreadable.lock().unwrap().insert("a".to_owned());
        let before = f.reads("a");
        assert!(matches!(memo.read(&a), Read::Unreadable(_)));
        assert!(matches!(memo.read(&a), Read::Unreadable(_)));
        assert_eq!(f.reads("a") - before, 2);

        f.shared.unreadable.lock().unwrap().clear();
        assert_eq!(memo.read(&a).present(), Some(f.form(1)));
        assert_eq!(f.reads("a") - before, 3);

        let before = f.reads("c");
        memo.read(&c);
        memo.read(&c);
        assert_eq!(f.reads("c") - before, 2);
    }
}
