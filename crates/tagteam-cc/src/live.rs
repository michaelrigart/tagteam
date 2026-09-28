use std::path::Path;
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
fn remove_if_present(path: &Path) -> Result<(), ProviderError> {
    Ok(remove_target(path)?)
}

const UNPARSABLE_ENTRY: &str = "a credential entry is not a JSON object";

/// Keeps only the machine-shared keys of a credential entry; `Ok(None)` means none
/// remain, so the entry may be dropped. An entry that fails to parse is refused rather
/// than silently treated as empty: the spec drops an entry only when no machine-shared
/// key survives it, never because the entry could not be read (§9.4 step 7).
fn keep_shared(b: &[u8]) -> Result<Option<Vec<u8>>, ProviderError> {
    let Ok(Value::Object(o)) = serde_json::from_slice::<Value>(b) else {
        return Err(ProviderError::Invalid(UNPARSABLE_ENTRY.into()));
    };
    let shared = machine_shared_only(&o);
    Ok((!shared.is_empty())
        .then(|| serde_json::to_vec(&Value::Object(shared)).expect("a Value always serializes")))
}

/// Skips restoring an entry already in its snapshotted state. An unreadable current
/// state is never treated as a match: it is restored anyway, rather than risk leaving it
/// wrong because its actual contents were unknown.
fn file_matches(path: &Path, expected: &Option<Vec<u8>>) -> bool {
    match read_bytes(path) {
        Read::Present(b) => expected.as_deref() == Some(b.as_slice()),
        Read::Absent => expected.is_none(),
        Read::Unreadable(_) => false,
    }
}

/// The file half of one `restore` entry: `fence` is checked immediately before the
/// mutation, inside `write_atomic_with` for a restored value or explicitly before a
/// removal.
fn restore_file(
    path: &Path,
    value: &Option<Vec<u8>>,
    fence: Fence<'_>,
) -> Result<(), ProviderError> {
    match value {
        Some(v) => write_atomic_with(path, v, 0o600, fence),
        None => {
            fence()?;
            remove_if_present(path)
        }
    }
}

/// Logs, right before a lock-loss abort, exactly which entries never got restored:
/// those already recorded as failed, and those the abort means will never be
/// attempted. Names only — Keychain services and file paths — never bytes.
fn log_lock_abort(failed: &[String], never_attempted: &[String]) {
    tracing::error!(
        "the lock was lost mid-restore; left unrestored: {failed:?}; never attempted: {never_attempted:?}"
    );
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
                    if paths.credentials_file.try_exists()? {
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
                    let kept = keep_shared(&b)?;
                    fence()?;
                    match kept {
                        Some(k) => self.keychain.upsert(&svc, &acct, &k)?,
                        None => self.keychain.delete(&svc, &acct)?,
                    }
                }
            }
        }
        if let Some(b) = present_or_err(read_bytes(&paths.credentials_file))? {
            let kept = keep_shared(&b)?;
            match kept {
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
    /// the key in the managed-key item, clearing any stale `primaryApiKey` a previous
    /// Keychain failure left behind (otherwise another account's plaintext key would stay
    /// live in `~/.claude.json`). When the Keychain refuses, the key goes to `primaryApiKey`
    /// instead, and every managed-key item is removed and verified gone: CC reads the
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
                Ok(()) => {
                    config::splice_key(&paths.global_config, "primaryApiKey", None, fence)?;
                    return Ok(());
                }
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

    fn item_matches(&self, svc: &str, acct: &str, expected: &Option<Vec<u8>>) -> bool {
        match self.keychain.find(svc, acct) {
            Read::Present(v) => expected.as_deref() == Some(v.as_slice()),
            Read::Absent => expected.is_none(),
            Read::Unreadable(_) => false,
        }
    }

    /// The Keychain half of one `restore` entry: `fence` is checked immediately before
    /// the upsert or delete.
    fn restore_item(
        &self,
        svc: &str,
        acct: &str,
        value: &Option<Vec<u8>>,
        fence: Fence<'_>,
    ) -> Result<(), ProviderError> {
        fence()?;
        match value {
            Some(v) => self.keychain.upsert(svc, acct, v)?,
            None => self.keychain.delete(svc, acct)?,
        }
        Ok(())
    }

    /// Restores every entry a switch may have touched: `global_config` first, then the
    /// managed-key and OAuth Keychain items, then the credentials file last. The file
    /// goes last, and is rewritten even when its bytes already match the snapshot once
    /// any Keychain item was actually restored, so its hot-reload mtime bump (Appendix
    /// A.3, mirroring `write_credential_entry`) always lands after the item it reflects
    /// — restoring the file first, or skipping it because it already matched, would let
    /// CC reload and memoize the target's token before the item behind it was put back.
    /// A file absent from the snapshot is still only removed or left absent, never
    /// created for the sake of a bump. An entry already equal to its snapshot is
    /// otherwise left untouched. A fence or `Lock` failure means the caller's ownership
    /// is gone, so it aborts the whole restore immediately (after logging every entry
    /// left unrestored); any other failure is recorded and every remaining entry is
    /// still attempted, so one flaky Keychain write never strands the rest of the
    /// rollback. When anything was left unrestored, the single error returned names
    /// every one of them — Keychain services and file paths, never bytes.
    pub fn restore(
        &self,
        env: &Env,
        paths: &CcPaths,
        snap: &Snapshot,
        fence: Fence<'_>,
    ) -> Result<(), ProviderError> {
        let acct = keychain_account(env);
        let mut failed: Vec<String> = Vec::new();
        let global_config_name = paths.global_config.display().to_string();
        let credentials_file_name = paths.credentials_file.display().to_string();
        let item_names: Vec<String> = snap
            .managed_items
            .iter()
            .chain(&snap.oauth_items)
            .map(|(svc, _)| svc.clone())
            .collect();

        // 1. `global_config` first: CC's identity and managed-key axis live here.
        if !file_matches(&paths.global_config, &snap.global_config) {
            if let Err(e) = restore_file(&paths.global_config, &snap.global_config, fence) {
                if matches!(e, ProviderError::Lock(_)) {
                    let mut never_attempted = vec![global_config_name];
                    never_attempted.extend(item_names.iter().cloned());
                    never_attempted.push(credentials_file_name);
                    log_lock_abort(&failed, &never_attempted);
                    return Err(e);
                }
                failed.push(global_config_name);
            }
        }

        // 2. The Keychain items: managed-key, then OAuth.
        let mut any_item_restored = false;
        for (i, (svc, value)) in snap
            .managed_items
            .iter()
            .chain(&snap.oauth_items)
            .enumerate()
        {
            if self.item_matches(svc, &acct, value) {
                continue;
            }
            match self.restore_item(svc, &acct, value, fence) {
                Ok(()) => any_item_restored = true,
                Err(e) => {
                    if matches!(e, ProviderError::Lock(_)) {
                        let mut never_attempted = item_names[i..].to_vec();
                        never_attempted.push(credentials_file_name);
                        log_lock_abort(&failed, &never_attempted);
                        return Err(e);
                    }
                    failed.push(svc.clone());
                }
            }
        }

        // 3. The credentials file last, so a Keychain item's hot-reload bump always
        // lands after that item. Never created for a snapshot that had none.
        let mismatched = !file_matches(&paths.credentials_file, &snap.credentials_file);
        let bump_for_reload = any_item_restored
            && snap.credentials_file.is_some()
            && paths.credentials_file.try_exists().unwrap_or(false);
        if mismatched || bump_for_reload {
            if let Err(e) = restore_file(&paths.credentials_file, &snap.credentials_file, fence) {
                if matches!(e, ProviderError::Lock(_)) {
                    log_lock_abort(&failed, std::slice::from_ref(&credentials_file_name));
                    return Err(e);
                }
                failed.push(credentials_file_name);
            }
        }

        if failed.is_empty() {
            Ok(())
        } else {
            Err(ProviderError::Incomplete { failed })
        }
    }
}
