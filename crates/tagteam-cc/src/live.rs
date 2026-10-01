use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use serde_json::{Map, Value, json};
use tagteam_provider::atomic::{
    ensure_private_dir, remove_target, write_atomic_private_with, write_atomic_with,
};
use tagteam_provider::{
    BeforeFallback, Credential, DoomedEntry, Env, Keychain, ProviderError, Read, SecretStore,
};

use crate::config::{self, read_bytes};
use crate::naming::{ItemKind, keychain_account, keychain_service, read_services};
use crate::paths::CcPaths;
use crate::provider::CONFIG_REMEDY;
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

/// How much of one auth axis a change destroys (§9.4 step 7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Extent {
    /// Left alone.
    None,
    /// Written: the primary item, and the file or `primaryApiKey` behind it. The other items a
    /// reader tries go only if the Keychain refuses the write and it falls back (Appendix
    /// A.3), or always once the credential entry is pinned to the file.
    Written,
    /// Cleared: every item a reader tries, and the file or `primaryApiKey`.
    Cleared,
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
    /// Appendix A.3: a credential-entry write of the current operation fell back to the file,
    /// so the operation's later writes go there too. Cleared when the operation's credential
    /// locks are released (`ClaudeCode::lock_credentials`) and by `restore`.
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

/// `primaryApiKey` in the global config. An empty one names no key, so it reads as absent.
fn read_primary_api_key(paths: &CcPaths) -> Read<Vec<u8>> {
    match config::get_key(&paths.global_config, "primaryApiKey") {
        Read::Present(Some(Value::String(k))) if !k.is_empty() => Read::Present(k.into_bytes()),
        Read::Present(_) | Read::Absent => Read::Absent,
        Read::Unreadable(e) => Read::Unreadable(e),
    }
}

/// Removes what the path resolves to; a symlink itself is never deleted (§9.5).
fn remove_if_present(path: &Path) -> Result<(), ProviderError> {
    Ok(remove_target(path)?)
}

pub(crate) const UNPARSABLE_ENTRY: &str = "a credential entry is not a JSON object";

/// `write_managed_key` refuses rather than silently reinitialising either of these: a
/// `customApiKeyResponses` that exists but is not an object, or an `approved` that exists
/// but is not an array (for example `{"approved": null}`). Fixed message, no bytes.
pub(crate) const MALFORMED_CUSTOM_API_KEY_RESPONSES: &str =
    "customApiKeyResponses is not a JSON object";
pub(crate) const MALFORMED_APPROVED: &str = "customApiKeyResponses.approved is not a JSON array";

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
/// mutation, inside the write for a restored value or explicitly before a removal.
/// `private` forces mode 0600 regardless of the file's current mode (the credentials
/// file, a secret); otherwise the file's own mode is preserved (`global_config`, which
/// tagteam does not own — §9.5).
fn restore_file(
    path: &Path,
    value: &Option<Vec<u8>>,
    fence: Fence<'_>,
    private: bool,
) -> Result<(), ProviderError> {
    match value {
        Some(v) if private => write_atomic_private_with(path, v, 0o600, fence),
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

    /// Ends the file-mode pin: the next credential-entry write tries the Keychain again
    /// (Appendix A.3).
    pub(crate) fn unpin_file_mode(&self) {
        self.file_mode_pinned.store(false, Ordering::SeqCst);
    }

    fn mac(&self) -> bool {
        self.platform == Platform::MacOs
    }

    /// `read`, retried twice `retry_delay` apart while it is unreadable (Appendix A.3: active
    /// reads retry the Keychain).
    fn retrying<T>(&self, read: impl Fn() -> Read<T>) -> Read<T> {
        let mut r = read();
        for _ in 0..2 {
            if !matches!(r, Read::Unreadable(_)) {
                break;
            }
            thread::sleep(self.retry_delay);
            r = read();
        }
        r
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
        match self.retrying(|| self.find_first(&services, &acct)) {
            Read::Present(b) => Read::Present(Credential::fresh(b)),
            Read::Absent => read_bytes(&paths.credentials_file).map(Credential::fresh),
            Read::Unreadable(e) => match read_bytes(&paths.credentials_file) {
                Read::Present(b) => Read::Present(Credential::degraded(b)),
                _ => Read::Unreadable(e),
            },
        }
    }

    /// The Keychain item first; then `primaryApiKey` in the config. An empty item stays
    /// `Present("")`, since a Keychain timeout can look like that. An empty `primaryApiKey` is
    /// what a successful file read really found, and it names no key, so it reads as absent;
    /// both write paths remove it anyway.
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
        read_primary_api_key(paths)
    }

    /// Every entry holding secrets that a change of these extents destroys, read now (§9.4
    /// step 7): on each axis the Keychain items a reader tries, in reader order, then the
    /// plaintext entry behind them (`.credentials.json`, `primaryApiKey`). An absent entry is
    /// listed as `Absent`.
    pub fn doomed(
        &self,
        env: &Env,
        paths: &CcPaths,
        entry: Extent,
        managed: Extent,
    ) -> Vec<DoomedEntry> {
        let mut out = Vec::new();
        let axes = [
            (ItemKind::OAuth, entry, self.file_mode_pinned()),
            (ItemKind::ManagedKey, managed, false),
        ];
        for (kind, extent, pinned) in axes {
            if extent == Extent::None {
                continue;
            }
            if self.mac() {
                let acct = keychain_account(env);
                for (i, svc) in read_services(env, kind).iter().enumerate() {
                    out.push(DoomedEntry {
                        bytes: self.retrying(|| self.keychain.find(svc, &acct)),
                        on_fallback: extent == Extent::Written && i > 0 && !pinned,
                    });
                }
            }
            let plain = match kind {
                ItemKind::OAuth => read_bytes(&paths.credentials_file),
                ItemKind::ManagedKey => read_primary_api_key(paths),
            };
            out.push(DoomedEntry {
                bytes: plain,
                on_fallback: false,
            });
        }
        out
    }

    /// Hands `before_fallback` the current bytes of every item a reader tries for `kind`, before
    /// a fallback deletes them all.
    fn report_items(
        &self,
        env: &Env,
        kind: ItemKind,
        before_fallback: BeforeFallback<'_>,
    ) -> Result<(), ProviderError> {
        let acct = keychain_account(env);
        for svc in read_services(env, kind) {
            if let Some(bytes) = present_or_err(self.keychain.find(&svc, &acct))? {
                before_fallback(&bytes)?;
            }
        }
        Ok(())
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
        write_atomic_private_with(&paths.credentials_file, bytes, 0o600, fence)
    }

    /// Appendix A.3 write, including the verified file fallback, which first reports every
    /// item it will delete to `before_fallback`. A fallback pins file mode, so every later
    /// write of the same operation goes straight to the file. Returns where this write put the
    /// credential: a file mirrored for hot reload does not make it a file store.
    pub fn write_credential_entry(
        &self,
        env: &Env,
        paths: &CcPaths,
        bytes: &[u8],
        fence: Fence<'_>,
        before_fallback: BeforeFallback<'_>,
    ) -> Result<SecretStore, ProviderError> {
        if !self.mac() {
            self.write_file(paths, bytes, fence)?;
            return Ok(SecretStore::File(paths.credentials_file.clone()));
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
                        write_atomic_private_with(&paths.credentials_file, bytes, 0o600, fence)?;
                    }
                    return Ok(SecretStore::Keychain);
                }
                Err(e) => tracing::warn!(
                    "keychain write failed, falling back to the credentials file: {e}"
                ),
            }
        }
        self.report_items(env, ItemKind::OAuth, before_fallback)?;
        self.write_file(paths, bytes, fence)?;
        self.remove_items(env, ItemKind::OAuth, fence)?;
        self.file_mode_pinned.store(true, Ordering::SeqCst);
        Ok(SecretStore::Fallback(paths.credentials_file.clone()))
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
                Some(k) => write_atomic_private_with(&paths.credentials_file, &k, 0o600, fence)?,
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
    /// instead, and every managed-key item is removed and verified gone, after being reported
    /// to `before_fallback`: CC reads the Keychain first, so a stale item would stay the
    /// effective key. Returns where this write put the key; the credential entry's file pin
    /// plays no part in it.
    pub fn write_managed_key(
        &self,
        env: &Env,
        paths: &CcPaths,
        key: &[u8],
        fence: Fence<'_>,
        before_fallback: BeforeFallback<'_>,
    ) -> Result<SecretStore, ProviderError> {
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
            Read::Present(None) | Read::Absent => Map::new(),
            Read::Present(Some(_)) => {
                return Err(ProviderError::Invalid(
                    MALFORMED_CUSTOM_API_KEY_RESPONSES.into(),
                ));
            }
            Read::Unreadable(_) => {
                return Err(ProviderError::ConfigUnsplicable {
                    path: paths.global_config.clone(),
                    remedy: CONFIG_REMEDY,
                });
            }
        };
        let appended = match responses.entry("approved").or_insert_with(|| json!([])) {
            Value::Array(list) => {
                if list.iter().any(|v| v.as_str() == Some(tail.as_str())) {
                    false
                } else {
                    list.push(Value::String(tail));
                    true
                }
            }
            _ => return Err(ProviderError::Invalid(MALFORMED_APPROVED.into())),
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
                    return Ok(SecretStore::Keychain);
                }
                Err(e) => {
                    tracing::warn!("keychain write failed, storing primaryApiKey instead: {e}")
                }
            }
            self.report_items(env, ItemKind::ManagedKey, before_fallback)?;
        }
        config::splice_key(
            &paths.global_config,
            "primaryApiKey",
            Some(&Value::String(key_str)),
            fence,
        )?;
        if !self.mac() {
            return Ok(SecretStore::File(paths.global_config.clone()));
        }
        self.remove_items(env, ItemKind::ManagedKey, fence)?;
        Ok(SecretStore::Fallback(paths.global_config.clone()))
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
        // A rollback clears the pin (Appendix A.3): the items put back below are what CC reads
        // first, so a later write of this operation that stayed on the file would sit behind
        // them. Trying the Keychain first is always safe; a refusal falls back again.
        self.unpin_file_mode();
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
            if let Err(e) = restore_file(&paths.global_config, &snap.global_config, fence, false) {
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
            if let Err(e) =
                restore_file(&paths.credentials_file, &snap.credentials_file, fence, true)
            {
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
