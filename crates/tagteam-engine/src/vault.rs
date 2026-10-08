use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tagteam_core::{AccountId, Fingerprint};
use tagteam_provider::atomic::{ensure_private_dir, write_atomic_private};
use tagteam_provider::{Keychain, Read, ReadError};

use crate::account_lock::AccountLock;

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("vault write failed: {0}")]
    Write(String),
    #[error("vault delete failed: {0}")]
    Delete(String),
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
            .map_err(|e| VaultError::Write(e.to_string()))
    }
    fn delete(&self, key: &str) -> Result<(), VaultError> {
        self.keychain
            .delete(SERVICE, key)
            .map_err(|e| VaultError::Delete(e.to_string()))
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
            .map_err(|e| VaultError::Write(e.to_string()))
    }
    fn delete(&self, key: &str) -> Result<(), VaultError> {
        match fs::remove_file(self.path(key)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(VaultError::Delete(e.to_string())),
        }
    }
    fn dir(&self) -> Option<&Path> {
        Some(&self.dir)
    }
}

/// The source of truth for credential bytes. It never parses what it stores; lineage comes
/// from the caller's fingerprint function.
pub struct Vault {
    backend: Box<dyn VaultBackend>,
}

fn prev_key(id: &AccountId) -> String {
    format!("{id}.prev")
}

impl Vault {
    pub fn new(backend: Box<dyn VaultBackend>) -> Self {
        Self { backend }
    }

    pub fn read(&self, id: &AccountId) -> Read<Vec<u8>> {
        self.backend.read(id.as_str())
    }

    pub fn read_prev(&self, id: &AccountId) -> Read<Vec<u8>> {
        self.backend.read(&prev_key(id))
    }

    /// Writes under the account lock. The current generation moves to `.prev` only when the
    /// fingerprint changes; the write is verified by reading back. A verified write is logged
    /// at INFO (§14.2, Decision 10): the account's ID, and the generation's fingerprint as its
    /// first 12 hex digits.
    pub fn store(
        &self,
        lock: &AccountLock,
        bytes: &[u8],
        fingerprint: &dyn Fn(&[u8]) -> Option<Fingerprint>,
    ) -> Result<(), VaultError> {
        let id = lock.id();
        let fp = fingerprint(bytes);
        let new_generation = match self.backend.read(id.as_str()) {
            Read::Present(old) => {
                let changed = fingerprint(&old) != fp;
                if changed {
                    self.backend.write(&prev_key(id), &old)?;
                }
                changed
            }
            Read::Absent => true,
            Read::Unreadable(e) => return Err(VaultError::Unreadable(e)),
        };
        self.backend.write(id.as_str(), bytes)?;
        match self.backend.read(id.as_str()) {
            Read::Present(v) if v == bytes => {}
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
                    .map_err(|e| VaultError::Delete(e.to_string()))?;
                return Ok(Leftovers::None);
            }
            return Ok(match keychain.service_has_items(SERVICE) {
                Read::Present(true) => Leftovers::Shared,
                Read::Present(false) | Read::Absent => Leftovers::None,
                Read::Unreadable(e) => Leftovers::Unknown(e.to_string()),
            });
        }
        let Some(dir) = self.dir() else {
            return Err(VaultError::Delete("this vault cannot be swept".into()));
        };
        let removed = match fs::symlink_metadata(dir) {
            Ok(m) if m.is_dir() => fs::remove_dir_all(dir),
            Ok(_) => fs::remove_file(dir),
            Err(e) => Err(e),
        };
        match removed {
            Ok(()) => Ok(Leftovers::None),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Leftovers::None),
            Err(e) => Err(VaultError::Delete(e.to_string())),
        }
    }

    /// Strict: both generations are deleted, errors propagate, and absence is verified. `.prev`
    /// goes first, so a failure partway through leaves the current generation — the one an
    /// account actually needs to keep working — in place rather than gone. A locked Keychain
    /// that still holds an item aborts the delete (§6.2).
    pub fn delete(&self, lock: &AccountLock) -> Result<(), VaultError> {
        let id = lock.id();
        for key in [prev_key(id), id.to_string()] {
            self.backend.delete(&key)?;
            match self.backend.read(&key) {
                Read::Absent => {}
                Read::Present(_) => {
                    return Err(VaultError::Delete(format!("{key} is still present")));
                }
                Read::Unreadable(e) => {
                    return Err(VaultError::Delete(format!("could not verify {key}: {e}")));
                }
            }
        }
        Ok(())
    }
}
