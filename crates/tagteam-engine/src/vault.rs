use std::fs;
use std::io;
use std::path::PathBuf;
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

pub trait VaultBackend: Send + Sync {
    fn read(&self, key: &str) -> Read<Vec<u8>>;
    fn write(&self, key: &str, bytes: &[u8]) -> Result<(), VaultError>;
    fn delete(&self, key: &str) -> Result<(), VaultError>;
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
    /// fingerprint changes; the write is verified by reading back.
    pub fn store(
        &self,
        lock: &AccountLock,
        bytes: &[u8],
        fingerprint: &dyn Fn(&[u8]) -> Option<Fingerprint>,
    ) -> Result<(), VaultError> {
        let id = lock.id();
        match self.backend.read(id.as_str()) {
            Read::Present(old) => {
                if fingerprint(&old) != fingerprint(bytes) {
                    self.backend.write(&prev_key(id), &old)?;
                }
            }
            Read::Absent => {}
            Read::Unreadable(e) => return Err(VaultError::Unreadable(e)),
        }
        self.backend.write(id.as_str(), bytes)?;
        match self.backend.read(id.as_str()) {
            Read::Present(v) if v == bytes => Ok(()),
            _ => Err(VaultError::Verify),
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
