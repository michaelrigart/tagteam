use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use serde_json::{Map, Value, json};
use tagteam_provider::atomic::{ensure_private_dir, remove_target, write_atomic_with};
use tagteam_provider::{Credential, Env, Keychain, ProviderError, Read};

use crate::config::{self, read_bytes};
use crate::naming::{ItemKind, keychain_account, keychain_service, read_services};
use crate::paths::CcPaths;
use crate::shape::machine_shared_only;

/// Checked immediately before every protected mutation (§9.1). Production passes the live
/// locks' ownership check, so a holder that lost its lock stops before writing anything.
pub type Fence<'a> = &'a dyn Fn() -> Result<(), ProviderError>;

/// One Keychain item's prior value, for `Snapshot`'s item lists.
type ItemSnapshot = (String, Option<Vec<u8>>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Linux,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Platform::MacOs
        } else {
            Platform::Linux
        }
    }
}

/// The exact prior state of every entry a switch may write, including every Keychain item a
/// reader tries (Appendix A.2). It holds secrets, so it has no `Debug`.
#[derive(Clone)]
pub struct Snapshot {
    oauth_items: Vec<ItemSnapshot>,
    managed_items: Vec<ItemSnapshot>,
    credentials_file: Option<Vec<u8>>,
    global_config: Option<Vec<u8>>,
}

/// Appendix A.3: reads and writes of the active credential and the managed-key axis.
pub struct LiveStore {
    keychain: Arc<dyn Keychain>,
    platform: Platform,
    file_mode_pinned: AtomicBool,
    retry_delay: Duration,
}

fn present_or_err(r: Read<Vec<u8>>) -> Result<Option<Vec<u8>>, ProviderError> {
    match r {
        Read::Present(v) => Ok(Some(v)),
        Read::Absent => Ok(None),
        Read::Unreadable(e) => Err(ProviderError::Unreadable(e)),
    }
}

/// Removes what the path resolves to; a symlink itself is never deleted (§9.5).
fn remove_if_present(path: &std::path::Path) -> Result<(), ProviderError> {
    Ok(remove_target(path)?)
}

/// Keeps only the machine-shared keys of a credential entry, or `None` when nothing remains.
fn keep_shared(b: &[u8]) -> Option<Vec<u8>> {
    let Ok(Value::Object(o)) = serde_json::from_slice::<Value>(b) else {
        return None;
    };
    let shared = machine_shared_only(&o);
    (!shared.is_empty())
        .then(|| serde_json::to_vec(&Value::Object(shared)).expect("a Value always serializes"))
}

impl LiveStore {
    pub fn new(keychain: Arc<dyn Keychain>, platform: Platform) -> Self {
        Self {
            keychain,
            platform,
            file_mode_pinned: AtomicBool::new(false),
            retry_delay: Duration::from_millis(300),
        }
    }

    pub fn with_retry_delay(mut self, d: Duration) -> Self {
        self.retry_delay = d;
        self
    }

    pub fn platform(&self) -> Platform {
        self.platform
    }

    pub fn file_mode_pinned(&self) -> bool {
        self.file_mode_pinned.load(Ordering::SeqCst)
    }

    fn mac(&self) -> bool {
        self.platform == Platform::MacOs
    }

    /// The first item that answers, in reader order. An unreadable item stops the search: a
    /// later fallback might be superseded by what the unreadable one holds, so it is never
    /// returned as if it were authoritative.
    fn find_first(&self, services: &[String], acct: &str) -> Read<Vec<u8>> {
        for svc in services {
            match self.keychain.find(svc, acct) {
                Read::Absent => continue,
                other => return other,
            }
        }
        Read::Absent
    }

    /// Keychain first, retried twice 300 ms apart; the file covers an absent item. A failed
    /// Keychain read covered by the file is `Degraded` (§4.3).
    pub fn read_credential(&self, env: &Env, paths: &CcPaths) -> Read<Credential> {
        if !self.mac() {
            return read_bytes(&paths.credentials_file).map(Credential::fresh);
        }
        let services = read_services(env, ItemKind::OAuth);
        let acct = keychain_account(env);
        let mut kc = self.find_first(&services, &acct);
        for _ in 0..2 {
            if !matches!(kc, Read::Unreadable(_)) {
                break;
            }
            thread::sleep(self.retry_delay);
            kc = self.find_first(&services, &acct);
        }
        match kc {
            Read::Present(b) => Read::Present(Credential::fresh(b)),
            Read::Absent => read_bytes(&paths.credentials_file).map(Credential::fresh),
            Read::Unreadable(e) => match read_bytes(&paths.credentials_file) {
                Read::Present(b) => Read::Present(Credential::degraded(b)),
                _ => Read::Unreadable(e),
            },
        }
    }

    pub fn read_managed_key(&self, env: &Env, paths: &CcPaths) -> Read<Vec<u8>> {
        if self.mac() {
            match self.find_first(
                &read_services(env, ItemKind::ManagedKey),
                &keychain_account(env),
            ) {
                Read::Present(v) => return Read::Present(v),
                Read::Unreadable(e) => return Read::Unreadable(e),
                Read::Absent => {}
            }
        }
        match config::get_key(&paths.global_config, "primaryApiKey") {
            Read::Present(Some(Value::String(k))) => Read::Present(k.into_bytes()),
            Read::Present(_) | Read::Absent => Read::Absent,
            Read::Unreadable(e) => Read::Unreadable(e),
        }
    }

    /// Deletes every item a reader would try for `kind`, and verifies each one gone.
    fn remove_items(
        &self,
        env: &Env,
        kind: ItemKind,
        fence: Fence<'_>,
    ) -> Result<(), ProviderError> {
        let acct = keychain_account(env);
        for svc in read_services(env, kind) {
            fence()?;
            let _ = self.keychain.delete(&svc, &acct);
            if !matches!(self.keychain.exists(&svc, &acct), Read::Absent) {
                return Err(ProviderError::ShadowingItem(svc));
            }
        }
        Ok(())
    }

    fn write_file(
        &self,
        paths: &CcPaths,
        bytes: &[u8],
        fence: Fence<'_>,
    ) -> Result<(), ProviderError> {
        fence()?;
        ensure_private_dir(&paths.secure_storage_dir)?;
        write_atomic_with(&paths.credentials_file, bytes, 0o600, fence)
    }

    /// Appendix A.3 write, including the verified file fallback.
    pub fn write_credential_entry(
        &self,
        env: &Env,
        paths: &CcPaths,
        bytes: &[u8],
        fence: Fence<'_>,
    ) -> Result<(), ProviderError> {
        if !self.mac() {
            return self.write_file(paths, bytes, fence);
        }
        if !self.file_mode_pinned() {
            fence()?;
            match self.keychain.upsert(
                &keychain_service(env, ItemKind::OAuth),
                &keychain_account(env),
                bytes,
            ) {
                Ok(()) => {
                    if paths.credentials_file.exists() {
                        // Bumps the mtime, so CC reloads (hot reload).
                        write_atomic_with(&paths.credentials_file, bytes, 0o600, fence)?;
                    }
                    return Ok(());
                }
                Err(e) => tracing::warn!(
                    "keychain write failed, falling back to the credentials file: {e}"
                ),
            }
        }
        self.write_file(paths, bytes, fence)?;
        self.remove_items(env, ItemKind::OAuth, fence)?;
        self.file_mode_pinned.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// API-key activation: keep only the machine-shared keys of every credential entry a
    /// reader would try; delete an entry when none remain (§9.4 step 7).
    pub fn clear_credential_account_keys(
        &self,
        env: &Env,
        paths: &CcPaths,
        fence: Fence<'_>,
    ) -> Result<(), ProviderError> {
        if self.mac() {
            let acct = keychain_account(env);
            for svc in read_services(env, ItemKind::OAuth) {
                if let Some(b) = present_or_err(self.keychain.find(&svc, &acct))? {
                    fence()?;
                    match keep_shared(&b) {
                        Some(k) => self.keychain.upsert(&svc, &acct, &k)?,
                        None => self.keychain.delete(&svc, &acct)?,
                    }
                }
            }
        }
        if let Some(b) = present_or_err(read_bytes(&paths.credentials_file))? {
            match keep_shared(&b) {
                Some(k) => write_atomic_with(&paths.credentials_file, &k, 0o600, fence)?,
                None => {
                    fence()?;
                    remove_if_present(&paths.credentials_file)?
                }
            }
        }
        Ok(())
    }

    /// Appends the key's last 20 characters to `customApiKeyResponses.approved`, then stores
    /// the key in the managed-key item. When the Keychain refuses, the key goes to
    /// `primaryApiKey`, and every managed-key item is removed and verified gone: CC reads the
    /// Keychain first, so a stale item would stay the effective key.
    pub fn write_managed_key(
        &self,
        env: &Env,
        paths: &CcPaths,
        key: &[u8],
        fence: Fence<'_>,
    ) -> Result<(), ProviderError> {
        let key_str = String::from_utf8_lossy(key).trim().to_owned();
        let tail: String = key_str
            .chars()
            .rev()
            .take(20)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let mut responses = match config::get_key(&paths.global_config, "customApiKeyResponses") {
            Read::Present(Some(Value::Object(o))) => o,
            Read::Present(_) | Read::Absent => Map::new(),
            Read::Unreadable(_) => {
                return Err(ProviderError::ConfigUnsplicable(
                    paths.global_config.clone(),
                ));
            }
        };
        let appended = match responses.entry("approved").or_insert_with(|| json!([])) {
            Value::Array(list) if !list.iter().any(|v| v.as_str() == Some(tail.as_str())) => {
                list.push(Value::String(tail));
                true
            }
            _ => false,
        };
        if appended {
            config::splice_key(
                &paths.global_config,
                "customApiKeyResponses",
                Some(&Value::Object(responses)),
                fence,
            )?;
        }
        if self.mac() {
            fence()?;
            match self.keychain.upsert(
                &keychain_service(env, ItemKind::ManagedKey),
                &keychain_account(env),
                key_str.as_bytes(),
            ) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    tracing::warn!("keychain write failed, storing primaryApiKey instead: {e}")
                }
            }
        }
        config::splice_key(
            &paths.global_config,
            "primaryApiKey",
            Some(&Value::String(key_str)),
            fence,
        )?;
        if self.mac() {
            self.remove_items(env, ItemKind::ManagedKey, fence)?;
        }
        Ok(())
    }

    /// Writing OAuth clears the managed key: every managed-key item is deleted (verified) and
    /// `primaryApiKey` is dropped. `approved` is kept (B.10).
    pub fn clear_managed_key(
        &self,
        env: &Env,
        paths: &CcPaths,
        fence: Fence<'_>,
    ) -> Result<(), ProviderError> {
        if self.mac() {
            self.remove_items(env, ItemKind::ManagedKey, fence)?;
        }
        config::splice_key(&paths.global_config, "primaryApiKey", None, fence)?;
        Ok(())
    }

    /// Refuses when any entry a switch may overwrite cannot be read.
    pub fn snapshot(&self, env: &Env, paths: &CcPaths) -> Result<Snapshot, ProviderError> {
        let acct = keychain_account(env);
        let items = |kind| -> Result<Vec<ItemSnapshot>, ProviderError> {
            if !self.mac() {
                return Ok(vec![]);
            }
            read_services(env, kind)
                .into_iter()
                .map(|svc| {
                    Ok((
                        svc.clone(),
                        present_or_err(self.keychain.find(&svc, &acct))?,
                    ))
                })
                .collect()
        };
        Ok(Snapshot {
            oauth_items: items(ItemKind::OAuth)?,
            managed_items: items(ItemKind::ManagedKey)?,
            credentials_file: present_or_err(read_bytes(&paths.credentials_file))?,
            global_config: present_or_err(read_bytes(&paths.global_config))?,
        })
    }

    pub fn restore(
        &self,
        env: &Env,
        paths: &CcPaths,
        snap: &Snapshot,
        fence: Fence<'_>,
    ) -> Result<(), ProviderError> {
        let acct = keychain_account(env);
        for (svc, value) in snap.oauth_items.iter().chain(&snap.managed_items) {
            fence()?;
            match value {
                Some(v) => self.keychain.upsert(svc, &acct, v)?,
                None => self.keychain.delete(svc, &acct)?,
            }
        }
        for (path, value) in [
            (&paths.credentials_file, &snap.credentials_file),
            (&paths.global_config, &snap.global_config),
        ] {
            match value {
                Some(v) => write_atomic_with(path, v, 0o600, fence)?,
                None => {
                    fence()?;
                    remove_if_present(path)?
                }
            }
        }
        Ok(())
    }
}
