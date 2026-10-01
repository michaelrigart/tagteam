use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::{Map, Value, json};
use tagteam_provider::atomic::{
    ensure_private_dir, remove_target, write_atomic_private_with, write_atomic_with,
};
use tagteam_provider::{
    BeforeFallback, Cancel, Credential, DoomedEntry, Env, Keychain, MkdirLock, ProviderError, Read,
    SecretStore,
};

use crate::config::{self, read_bytes};
use crate::locks;
use crate::naming::{ItemKind, keychain_account, keychain_service, read_services};
use crate::paths::CcPaths;
use crate::provider::CONFIG_REMEDY;
use crate::shape::{MACHINE_SHARED_KEYS, machine_shared_only};

/// Checked immediately before every protected mutation (§9.1). Production passes the live
/// locks' ownership check, so a holder that lost its lock stops before writing anything.
pub type Fence<'a> = &'a dyn Fn() -> Result<(), ProviderError>;

/// One Keychain item's prior value, for `Snapshot`'s item lists.
type ItemSnapshot = (String, Option<Vec<u8>>);

/// The account-scoped part of a credential entry (§9.1, Appendix A.4): every key but the
/// machine-shared ones. Bytes that are not a JSON object, such as an API key, count whole. An
/// absent entry and one holding only machine-shared keys have none. It holds secrets, so it has
/// no `Debug`.
#[derive(Clone, PartialEq)]
enum AccountPart {
    Keys(Map<String, Value>),
    Raw(Vec<u8>),
}

impl AccountPart {
    fn of(bytes: Option<&[u8]>) -> Self {
        let Some(bytes) = bytes else {
            return AccountPart::Keys(Map::new());
        };
        match object(bytes) {
            Some(mut o) => {
                for k in MACHINE_SHARED_KEYS {
                    o.shift_remove(k);
                }
                AccountPart::Keys(o)
            }
            None => AccountPart::Raw(bytes.to_vec()),
        }
    }

    /// This part as Claude Code's dead-token marking leaves it (§9.1, Appendix A.3): in
    /// `claudeAiOauth`, both tokens empty and `expiresAt` 0, every other key as it was. `None`
    /// when there is no `claudeAiOauth` object to mark.
    fn marked(&self) -> Option<AccountPart> {
        let AccountPart::Keys(keys) = self else {
            return None;
        };
        let mut keys = keys.clone();
        let Some(Value::Object(oauth)) = keys.get_mut("claudeAiOauth") else {
            return None;
        };
        oauth.insert("accessToken".into(), json!(""));
        oauth.insert("refreshToken".into(), json!(""));
        oauth.insert("expiresAt".into(), json!(0));
        Some(AccountPart::Keys(keys))
    }
}

/// What one place of a credential entry held each time the current operation looked: what its
/// latest `snapshot` read there (`None`: absent), then every value written there since.
type Values = Vec<Option<Vec<u8>>>;

/// Every place of an entry, named as `Seen` names it, with what it holds now, in reader order.
type Places = Vec<(String, Option<Vec<u8>>)>;

/// What a place of an entry actually holds, as far as the operation knows: what it read there
/// under the storage-write lock, or what a write it saw succeed put there (`None`: absent).
/// `Unknown` after a write that failed where a read under the same hold could not tell. It holds
/// secrets, so it has no `Debug`.
#[derive(Clone, PartialEq)]
enum Held {
    Known(Option<Vec<u8>>),
    Unknown,
}

impl Held {
    fn of(read: Read<Vec<u8>>) -> Self {
        match read {
            Read::Present(b) => Held::Known(Some(b)),
            Read::Absent => Held::Known(None),
            Read::Unreadable(_) => Held::Unknown,
        }
    }
}

/// What the current operation has seen of one credential entry under the credential locks
/// (§9.1), place by place.
#[derive(Default)]
struct Seen {
    /// By place: a Keychain service, or the credentials file's path. `None` until the
    /// operation reads the entry.
    places: Option<BTreeMap<String, Values>>,
    /// What each place actually holds: read under the storage-write lock at the start of each
    /// hold, then updated after each write. Never an attempted value that did not land.
    held: BTreeMap<String, Held>,
    /// What each place the operation's writes changed actually held just before the first of
    /// those changes: what a restore puts back there.
    first: BTreeMap<String, Held>,
    /// The places a restore must leave as they are: changed by another writer since the
    /// operation first changed the entry, or holding something unknown before that change.
    leave: BTreeSet<String>,
}

impl Seen {
    fn values(&self, place: &str) -> Option<&Values> {
        self.places.as_ref()?.get(place)
    }

    /// §9.1: whether `now`, what `place` holds under the storage-write lock, has account-scoped
    /// keys this operation last read or wrote there, or CC's dead-token marking of them. A place
    /// the operation never read has nothing to compare.
    fn known(&self, place: &str, now: Option<&[u8]>) -> bool {
        let Some(values) = self.values(place) else {
            return true;
        };
        let now = AccountPart::of(now);
        values
            .iter()
            .map(|v| AccountPart::of(v.as_deref()))
            .any(|p| p == now || p.marked().as_ref() == Some(&now))
    }
}

/// `Seen` for both credential entries (§9.1): the OAuth entry and the managed-key item.
#[derive(Default)]
struct Ledger {
    oauth: Seen,
    managed: Seen,
}

impl Ledger {
    fn entry(&mut self, kind: ItemKind) -> &mut Seen {
        match kind {
            ItemKind::OAuth => &mut self.oauth,
            ItemKind::ManagedKey => &mut self.managed,
        }
    }
}

/// Why a restore left a place of an entry as it is: another writer changed the entry since this
/// operation wrote it, so putting back what it held before could lose that write.
const CHANGED_SINCE: &str = "changed since tagteam wrote it; left as it is";

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
    /// §9.1: what the current operation read and wrote of each credential entry, so a write
    /// under the storage-write lock can tell Claude Code's changes from its own. Cleared, with
    /// the pin, when the operation's credential locks are released.
    seen: Mutex<Ledger>,
    /// How long one wait for CC's storage-write lock may take (§9.1: 9 s).
    storage_write_timeout: Duration,
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

/// The JSON object `bytes` hold, if they hold one.
fn object(bytes: &[u8]) -> Option<Map<String, Value>> {
    match serde_json::from_slice::<Value>(bytes) {
        Ok(Value::Object(o)) => Some(o),
        _ => None,
    }
}

/// The credentials file as a place of the OAuth entry, named as `restore` and `EntryMoved` name
/// it: its path.
fn file_place(paths: &CcPaths) -> String {
    paths.credentials_file.display().to_string()
}

/// The machine-shared keys an entry holds (Appendix A.4): none for an absent entry or bytes
/// that are not a JSON object.
fn shared_of(bytes: Option<&[u8]>) -> Map<String, Value> {
    bytes
        .and_then(object)
        .map(|o| machine_shared_only(&o))
        .unwrap_or_default()
}

/// `bytes` carrying `shared`, the machine-shared keys the entry holds under the storage-write
/// lock, their absence included (§9.1), so a Claude Code write made since `bytes` were composed
/// is kept. Bytes that already carry exactly those keys, or that are not a JSON object, are
/// returned as they are.
fn rebase(bytes: &[u8], shared: &Map<String, Value>) -> Vec<u8> {
    let Some(mut o) = object(bytes) else {
        return bytes.to_vec();
    };
    if machine_shared_only(&o) == *shared {
        return bytes.to_vec();
    }
    for k in MACHINE_SHARED_KEYS {
        o.shift_remove(k);
    }
    o.extend(shared.clone());
    serde_json::to_vec(&Value::Object(o)).expect("a Value always serializes")
}

/// `fence`, then the storage-write lock's own ownership check: the check that runs immediately
/// before every write the lock protects (§9.1).
fn held<'a>(fence: Fence<'a>, lock: &'a MkdirLock) -> impl Fn() -> Result<(), ProviderError> + 'a {
    move || {
        fence()?;
        Ok(lock.check_owned()?)
    }
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
            seen: Mutex::new(Ledger::default()),
            storage_write_timeout: locks::ACQUIRE_TIMEOUT,
            retry_delay: Duration::from_millis(300),
        }
    }

    pub fn with_retry_delay(mut self, d: Duration) -> Self {
        self.retry_delay = d;
        self
    }

    /// A shorter wait for CC's storage-write lock, so a test of a held lock need not wait 9 s.
    pub fn with_storage_write_timeout(mut self, d: Duration) -> Self {
        self.storage_write_timeout = d;
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

    /// Ends the operation (Appendix A.3, §9.1): the file-mode pin, and what it read and wrote of
    /// each credential entry.
    pub(crate) fn end_operation(&self) {
        self.unpin_file_mode();
        *self.seen.lock().unwrap() = Ledger::default();
    }

    /// CC's storage-write lock, for one entry's write (§9.1), waited for under `cancel`.
    fn storage_write(&self, paths: &CcPaths, cancel: &Cancel) -> Result<MkdirLock, ProviderError> {
        Ok(locks::acquire_storage_write(
            paths,
            self.storage_write_timeout,
            cancel,
        )?)
    }

    /// Every place of `kind`'s entry, read again under the storage-write lock, strictly (Appendix
    /// A.3): the Keychain items a reader tries, in reader order (macOS), then, for the OAuth
    /// entry, the credentials file. An item that exists but cannot be read refuses.
    fn places_now(
        &self,
        env: &Env,
        paths: &CcPaths,
        kind: ItemKind,
    ) -> Result<Places, ProviderError> {
        let mut out = Vec::new();
        if self.mac() {
            let acct = keychain_account(env);
            for svc in read_services(env, kind) {
                let value = present_or_err(self.retrying(|| self.keychain.find(&svc, &acct)))?;
                out.push((svc, value));
            }
        }
        if kind == ItemKind::OAuth {
            let file = present_or_err(read_bytes(&paths.credentials_file))?;
            out.push((file_place(paths), file));
        }
        Ok(out)
    }

    /// §9.1, at every place of the entry: `EntryMoved`, naming the first place whose
    /// account-scoped keys this operation neither last read nor wrote there; only another writer
    /// can have changed them. CC's dead-token marking of what it read or wrote is no conflict:
    /// the write goes ahead over it.
    fn unmoved(
        &self,
        kind: ItemKind,
        places: &[(String, Option<Vec<u8>>)],
    ) -> Result<(), ProviderError> {
        let mut seen = self.seen.lock().unwrap();
        let seen = seen.entry(kind);
        match places
            .iter()
            .find(|(place, now)| !seen.known(place, now.as_deref()))
        {
            Some((place, _)) => Err(ProviderError::EntryMoved(place.clone())),
            None => Ok(()),
        }
    }

    /// Records, before a write, that this operation writes `bytes` (`None`: deletes) at `place`
    /// of `kind`'s entry (§9.1): a later write or restore of the operation may find it there.
    fn intend(&self, kind: ItemKind, place: &str, bytes: Option<&[u8]>) {
        let mut seen = self.seen.lock().unwrap();
        let entry = seen.entry(kind);
        if let Some(places) = &mut entry.places {
            places
                .entry(place.to_owned())
                .or_default()
                .push(bytes.map(<[u8]>::to_vec));
        }
    }

    /// Records, after a write of `bytes` to the Keychain item `svc`, what the item actually
    /// holds: `bytes` when the write `landed`; otherwise what a read under the same hold finds,
    /// unknown if it cannot be read.
    fn settle_item(
        &self,
        env: &Env,
        kind: ItemKind,
        svc: &str,
        bytes: Option<&[u8]>,
        landed: bool,
    ) {
        let after = if landed {
            Held::Known(bytes.map(<[u8]>::to_vec))
        } else {
            Held::of(self.keychain.find(svc, &keychain_account(env)))
        };
        self.settled(kind, svc, after);
    }

    /// `settle_item` for the credentials file.
    fn settle_file(&self, paths: &CcPaths, bytes: Option<&[u8]>, landed: bool) {
        let after = if landed {
            Held::Known(bytes.map(<[u8]>::to_vec))
        } else {
            Held::of(read_bytes(&paths.credentials_file))
        };
        self.settled(ItemKind::OAuth, &file_place(paths), after);
    }

    /// `place` now holds `after` (§9.1). If that is the operation's first change there, what the
    /// place held just before is kept for a restore; when that is unknown, a restore must leave
    /// the place.
    fn settled(&self, kind: ItemKind, place: &str, after: Held) {
        let mut seen = self.seen.lock().unwrap();
        let entry = seen.entry(kind);
        let before = entry
            .held
            .insert(place.to_owned(), after.clone())
            .unwrap_or(Held::Unknown);
        if before != after && !entry.first.contains_key(place) {
            if before == Held::Unknown {
                entry.leave.insert(place.to_owned());
            }
            entry.first.insert(place.to_owned(), before);
        }
    }

    /// What every place holds at the start of a hold of the storage-write lock (§9.1). Once the
    /// operation has changed the entry, a place holding anything but what the operation last
    /// knew there was changed by another writer since, so a restore must leave the entry; a
    /// write whose outcome was unknown and that never landed left its place as it was. Returns
    /// every place a restore must leave.
    fn observe(&self, kind: ItemKind, places: &[(String, Option<Vec<u8>>)]) -> Vec<String> {
        let mut seen = self.seen.lock().unwrap();
        let seen = seen.entry(kind);
        for (place, value) in places {
            let now = Held::Known(value.clone());
            if !seen.first.is_empty() {
                let was = seen.held.get(place).unwrap_or(&Held::Unknown);
                let unchanged =
                    *was == now || (*was == Held::Unknown && seen.first.get(place) == Some(&now));
                if !unchanged {
                    seen.leave.insert(place.clone());
                }
            }
            seen.held.insert(place.clone(), now);
        }
        seen.leave.iter().cloned().collect()
    }

    /// One write of `kind`'s entry under CC's storage-write lock (§9.1). Waits for the lock
    /// under `env.cancel` (§14.1), reads every place of the entry again and refuses with
    /// `EntryMoved` if the account-scoped keys moved at any of them, then runs `write` with the
    /// entry as Claude Code reads it now (its first place that holds anything) and a fence that
    /// also checks the lock is still this holder's. The lock is released on return, so it is
    /// never held across anything but this one entry's write.
    fn under_storage_write<T>(
        &self,
        env: &Env,
        paths: &CcPaths,
        kind: ItemKind,
        fence: Fence<'_>,
        write: impl FnOnce(Option<&[u8]>, Fence<'_>) -> Result<T, ProviderError>,
    ) -> Result<T, ProviderError> {
        let lock = self.storage_write(paths, &env.cancel)?;
        let places = self.places_now(env, paths, kind)?;
        self.observe(kind, &places);
        self.unmoved(kind, &places)?;
        let now = places.iter().find_map(|(_, value)| value.as_deref());
        let fence = held(fence, &lock);
        write(now, &fence)
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
            self.intend(kind, &svc, None);
            fence()?;
            let _ = self.keychain.delete(&svc, &acct);
            let gone = matches!(self.keychain.exists(&svc, &acct), Read::Absent);
            self.settle_item(env, kind, &svc, None, gone);
            if !gone {
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
        self.intend(ItemKind::OAuth, &file_place(paths), Some(bytes));
        fence()?;
        ensure_private_dir(&paths.secure_storage_dir)?;
        let written = write_atomic_private_with(&paths.credentials_file, bytes, 0o600, fence);
        self.settle_file(paths, Some(bytes), written.is_ok());
        written
    }

    /// Appendix A.3 write, including the verified file fallback, which first reports every
    /// item it will delete to `before_fallback`. A fallback pins file mode, so every later
    /// write of the same operation goes straight to the file. Returns where this write put the
    /// credential: a file mirrored for hot reload does not make it a file store.
    ///
    /// The whole write, its hot-reload rewrite or its fallback included, holds CC's
    /// storage-write lock (§9.1). Under it the entry is read again: its account-scoped keys must
    /// still be what this operation last read or wrote (`ProviderError::EntryMoved` otherwise),
    /// and the machine-shared keys written are the ones it holds now, whatever `bytes` carry.
    pub fn write_credential_entry(
        &self,
        env: &Env,
        paths: &CcPaths,
        bytes: &[u8],
        fence: Fence<'_>,
        before_fallback: BeforeFallback<'_>,
    ) -> Result<SecretStore, ProviderError> {
        self.under_storage_write(env, paths, ItemKind::OAuth, fence, |now, fence| {
            let bytes = rebase(bytes, &shared_of(now));
            self.write_entry(env, paths, &bytes, fence, before_fallback)
        })
    }

    /// `write_credential_entry`'s write, under the storage-write lock.
    fn write_entry(
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
            let svc = keychain_service(env, ItemKind::OAuth);
            self.intend(ItemKind::OAuth, &svc, Some(bytes));
            fence()?;
            let upserted = self.keychain.upsert(&svc, &keychain_account(env), bytes);
            self.settle_item(env, ItemKind::OAuth, &svc, Some(bytes), upserted.is_ok());
            match upserted {
                Ok(()) => {
                    if paths.credentials_file.try_exists()? {
                        // Bumps the mtime, so CC reloads (hot reload).
                        self.intend(ItemKind::OAuth, &file_place(paths), Some(bytes));
                        let mirrored =
                            write_atomic_private_with(&paths.credentials_file, bytes, 0o600, fence);
                        self.settle_file(paths, Some(bytes), mirrored.is_ok());
                        mirrored?;
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
    /// reader would try; delete an entry when none remain (§9.4 step 7). It holds CC's
    /// storage-write lock throughout and refuses, writing nothing, if the entry's
    /// account-scoped keys moved (§9.1); what each place keeps is read under the lock.
    pub fn clear_credential_account_keys(
        &self,
        env: &Env,
        paths: &CcPaths,
        fence: Fence<'_>,
    ) -> Result<(), ProviderError> {
        self.under_storage_write(env, paths, ItemKind::OAuth, fence, |_, fence| {
            self.clear_account_keys(env, paths, fence)
        })
    }

    /// `clear_credential_account_keys`' clear, under the storage-write lock.
    fn clear_account_keys(
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
                    self.intend(ItemKind::OAuth, &svc, kept.as_deref());
                    fence()?;
                    let cleared = match &kept {
                        Some(k) => self.keychain.upsert(&svc, &acct, k),
                        None => self.keychain.delete(&svc, &acct),
                    };
                    self.settle_item(env, ItemKind::OAuth, &svc, kept.as_deref(), cleared.is_ok());
                    cleared?;
                }
            }
        }
        if let Some(b) = present_or_err(read_bytes(&paths.credentials_file))? {
            let kept = keep_shared(&b)?;
            self.intend(ItemKind::OAuth, &file_place(paths), kept.as_deref());
            let cleared = match &kept {
                Some(k) => write_atomic_private_with(&paths.credentials_file, k, 0o600, fence),
                None => fence().and_then(|()| remove_if_present(&paths.credentials_file)),
            };
            self.settle_file(paths, kept.as_deref(), cleared.is_ok());
            cleared?;
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
    /// plays no part in it. The managed-key item's write, its fallback included, holds CC's
    /// storage-write lock and refuses if the item moved (§9.1).
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
        let in_primary = |fence: Fence<'_>| {
            config::splice_key(
                &paths.global_config,
                "primaryApiKey",
                Some(&Value::String(key_str.clone())),
                fence,
            )
        };
        if !self.mac() {
            in_primary(fence)?;
            return Ok(SecretStore::File(paths.global_config.clone()));
        }
        let in_keychain =
            self.under_storage_write(env, paths, ItemKind::ManagedKey, fence, |_, fence| {
                let svc = keychain_service(env, ItemKind::ManagedKey);
                let key = Some(key_str.as_bytes());
                self.intend(ItemKind::ManagedKey, &svc, key);
                fence()?;
                let upserted =
                    self.keychain
                        .upsert(&svc, &keychain_account(env), key_str.as_bytes());
                self.settle_item(env, ItemKind::ManagedKey, &svc, key, upserted.is_ok());
                match upserted {
                    Ok(()) => return Ok(true),
                    Err(e) => {
                        tracing::warn!("keychain write failed, storing primaryApiKey instead: {e}")
                    }
                }
                self.report_items(env, ItemKind::ManagedKey, before_fallback)?;
                in_primary(fence)?;
                self.remove_items(env, ItemKind::ManagedKey, fence)?;
                Ok(false)
            })?;
        if !in_keychain {
            return Ok(SecretStore::Fallback(paths.global_config.clone()));
        }
        config::splice_key(&paths.global_config, "primaryApiKey", None, fence)?;
        Ok(SecretStore::Keychain)
    }

    /// Writing OAuth clears the managed key: every managed-key item is deleted (verified) and
    /// `primaryApiKey` is dropped. `approved` is kept (B.10). The deletes hold CC's
    /// storage-write lock and refuse if the item moved (§9.1).
    pub fn clear_managed_key(
        &self,
        env: &Env,
        paths: &CcPaths,
        fence: Fence<'_>,
    ) -> Result<(), ProviderError> {
        if self.mac() {
            self.under_storage_write(env, paths, ItemKind::ManagedKey, fence, |_, fence| {
                self.remove_items(env, ItemKind::ManagedKey, fence)
            })?;
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
        let snap = Snapshot {
            oauth_items: items(ItemKind::OAuth)?,
            managed_items: items(ItemKind::ManagedKey)?,
            credentials_file: present_or_err(read_bytes(&paths.credentials_file))?,
            global_config: present_or_err(read_bytes(&paths.global_config))?,
        };
        // §9.1: this is the operation's latest read of both entries under the credential locks,
        // place by place.
        let places = |items: &[ItemSnapshot]| -> BTreeMap<String, Values> {
            items
                .iter()
                .map(|(svc, v)| (svc.clone(), vec![v.clone()]))
                .collect()
        };
        let mut oauth = places(&snap.oauth_items);
        oauth.insert(file_place(paths), vec![snap.credentials_file.clone()]);
        let mut seen = self.seen.lock().unwrap();
        seen.oauth.places = Some(oauth);
        seen.managed.places = Some(places(&snap.managed_items));
        drop(seen);
        Ok(snap)
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
    ///
    /// Each credential entry, the managed-key item and then the OAuth entry with its file, is
    /// restored under its own hold of CC's storage-write lock (§9.1), and only if this
    /// operation's writes changed it. The lock is waited for under a token nothing sets: a
    /// rollback runs to completion (§14.1). Each place the operation changed is put back, byte
    /// for byte, to what it actually held just before the operation's first change there, read
    /// under the lock; for these entries that, not the snapshot, is what the paragraph above
    /// means. It is put back only while every place of the entry still holds what the operation
    /// last knew there (`observe`). Once another writer changed any place since, by a dead-token
    /// marking or in its machine-shared keys as much as otherwise, or a place's state before the
    /// operation's change is unknown, the whole entry is left as it is and each such place is
    /// named as not restored: putting back any place could overwrite that write, or hide it
    /// behind a place a reader tries first. A place the operation never changed is never
    /// written, but for the credentials file's hot-reload rewrite of its own bytes. A lock that
    /// cannot be taken, or an entry that cannot be read under it, leaves that entry unrestored
    /// too.
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

        // 2. The Keychain items: managed-key, then OAuth; 3. the credentials file last, so a
        // Keychain item's hot-reload bump always lands after that item. Each entry under its
        // own hold of the storage-write lock (§9.1).
        let mut any_item_restored = false;
        let mut start = 0;
        for kind in [ItemKind::ManagedKey, ItemKind::OAuth] {
            let items = match kind {
                ItemKind::ManagedKey => &snap.managed_items,
                ItemKind::OAuth => &snap.oauth_items,
            };
            let at = start;
            start += items.len();
            // What each place this operation changed held just before its first change there.
            let first = self.seen.lock().unwrap().entry(kind).first.clone();
            if first.is_empty() {
                continue;
            }
            // The entry, named by its primary item, else its file (Linux).
            let name = items
                .first()
                .map_or_else(|| credentials_file_name.clone(), |(svc, _)| svc.clone());
            let lock = match self.storage_write(paths, &Cancel::new()) {
                Ok(lock) => lock,
                Err(e) => {
                    failed.push(format!("{name} ({e})"));
                    continue;
                }
            };
            let now = match self.places_now(env, paths, kind) {
                Ok(now) => now,
                Err(e) => {
                    failed.push(format!("{name} ({e})"));
                    continue;
                }
            };
            let changed = self.observe(kind, &now);
            if !changed.is_empty() {
                failed.extend(
                    changed
                        .into_iter()
                        .map(|place| format!("{place} ({CHANGED_SINCE})")),
                );
                continue;
            }
            let fence = held(fence, &lock);
            let mut file_now = None;
            for (i, (place, value)) in now.iter().enumerate() {
                if *place == credentials_file_name {
                    file_now = Some(value.clone());
                    continue;
                }
                let Some(Held::Known(before)) = first.get(place) else {
                    continue;
                };
                if value == before {
                    continue;
                }
                self.intend(kind, place, before.as_deref());
                let restored = self.restore_item(place, &acct, before, &fence);
                self.settle_item(env, kind, place, before.as_deref(), restored.is_ok());
                match restored {
                    Ok(()) => any_item_restored = true,
                    Err(e) => {
                        if matches!(e, ProviderError::Lock(_)) {
                            let mut never_attempted = item_names[at + i..].to_vec();
                            never_attempted.push(credentials_file_name);
                            log_lock_abort(&failed, &never_attempted);
                            return Err(e);
                        }
                        failed.push(place.clone());
                    }
                }
            }
            let Some(file_now) = file_now else {
                continue;
            };
            // The file goes back to what it held before this operation's change. One it never
            // changed is rewritten with its own bytes, if it exists, to bump its mtime after a
            // restored item; it is never created for that.
            let file = match first.get(&credentials_file_name) {
                Some(Held::Known(before)) if *before != file_now => before.clone(),
                _ if any_item_restored && file_now.is_some() => file_now,
                _ => continue,
            };
            self.intend(kind, &credentials_file_name, file.as_deref());
            let restored = restore_file(&paths.credentials_file, &file, &fence, true);
            self.settle_file(paths, file.as_deref(), restored.is_ok());
            if let Err(e) = restored {
                if matches!(e, ProviderError::Lock(_)) {
                    log_lock_abort(&failed, std::slice::from_ref(&credentials_file_name));
                    return Err(e);
                }
                failed.push(credentials_file_name.clone());
            }
        }

        if failed.is_empty() {
            Ok(())
        } else {
            Err(ProviderError::Incomplete { failed })
        }
    }
}
