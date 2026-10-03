//! `displaced/` (§6.3): credentials that were not tagteam's to keep. The writer stashes one
//! before it is overwritten, and `tagteam displaced` lists the entries and purges them.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;
use tagteam_core::{Fingerprint, ProviderId};
use tagteam_provider::Env;
use tagteam_provider::atomic::{ensure_private_dir, write_atomic_private};

use crate::engine::Engine;
use crate::error::EngineError;
use crate::store::{DisplacedRow, Store};

/// One entry of `tagteam displaced` (§6.3): a row, a file, or both, joined by ID.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplacedEntry {
    /// `<epoch s>-<fp12>-<rand6>`, the file's stem.
    pub id: String,
    /// `None` for a file with no row.
    pub provider: Option<ProviderId>,
    /// The row's time. For a file with no row, the second its name carries.
    pub at_ms: i64,
    pub reason: Option<String>,
    /// The row's whole fingerprint. `None` with no row, or for a row that recorded none.
    pub fingerprint: Option<String>,
    /// The identity the displacing code attributed the bytes to, if any: provider-owned JSON.
    pub identity: Option<Value>,
    /// The position of the managed account `identity` names, if any.
    pub account: Option<u32>,
    pub file_present: bool,
    /// Whether a row records the entry.
    pub recorded: bool,
}

/// The listing: the directory the files are in, and every entry, newest first.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplacedList {
    pub dir: PathBuf,
    pub entries: Vec<DisplacedEntry>,
}

/// `^[0-9]{1,19}-[0-9a-f]{12}-[a-z0-9]{6}$` (Decision 12). This is the only form of ID a path is
/// ever built from, so a name that would leave `displaced/` is never an ID.
pub fn is_displaced_id(s: &str) -> bool {
    let mut parts = s.split('-');
    let (Some(epoch), Some(fp12), Some(rand6), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    (1..=19).contains(&epoch.len())
        && epoch.bytes().all(|b| b.is_ascii_digit())
        && fp12.len() == 12
        && fp12
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && rand6.len() == 6
        && rand6
            .bytes()
            .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase())
}

/// A new entry's ID (§5): the epoch second, the fingerprint's first 12 hex digits (twelve zeros
/// when it has none), and six random lowercase letters and digits.
fn new_id(now_ms: i64, fp: Option<&Fingerprint>) -> String {
    let fp12 = fp.map_or("000000000000", |f| f.short12());
    let rand6: String = (0..6)
        .map(|_| fastrand::alphanumeric().to_ascii_lowercase())
        .collect();
    format!("{}-{fp12}-{rand6}", now_ms / 1000)
}

/// When an entry with no row was displaced: the epoch second its ID carries, in ms. A number
/// too large for the clock saturates rather than wrapping.
fn id_at_ms(id: &str) -> i64 {
    id.split('-')
        .next()
        .and_then(|s| s.parse::<i64>().ok())
        .map_or(i64::MAX, |s| s.saturating_mul(1000))
}

fn displaced_dir(env: &Env) -> PathBuf {
    env.data_dir().join("displaced")
}

/// The IDs of `dir`'s entries named `<displaced ID>.json` (Decision 12). Anything else there is
/// ignored, including the atomic writer's temp files. An absent directory has none. One that
/// cannot be listed is an error, never taken for an empty one (§4.3).
fn displaced_files(dir: &Path) -> Result<BTreeSet<String>, EngineError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(e) => return Err(e.into()),
    };
    let mut ids = BTreeSet::new();
    for entry in entries {
        let name = entry?.file_name();
        if let Some(id) = name
            .to_str()
            .and_then(|n| n.strip_suffix(".json"))
            .filter(|stem| is_displaced_id(stem))
        {
            ids.insert(id.to_owned());
        }
    }
    Ok(ids)
}

/// Stashes live credential bytes that are about to be overwritten (§6.3). Forensic and
/// write-only: always a plain 0600 file, never a Keychain item. The file is written first and
/// its row second, so a failed insert leaves a file with no row, never a row that names nothing.
pub(crate) fn displace(
    engine: &Engine,
    provider: &ProviderId,
    bytes: &[u8],
    fp: Option<&Fingerprint>,
    reason: &str,
    identity: Option<&Value>,
) -> Result<String, EngineError> {
    let dir = displaced_dir(engine.env());
    ensure_private_dir(&dir)?;
    let now = engine.now_ms();
    let id = new_id(now, fp);
    write_atomic_private(&dir.join(format!("{id}.json")), bytes, 0o600)?;
    engine.store()?.insert_displaced(&DisplacedRow {
        id: id.clone(),
        provider: provider.clone(),
        at: now,
        reason: reason.to_owned(),
        fingerprint: fp.map(|f| f.as_str().to_owned()).unwrap_or_default(),
        identity: identity.cloned(),
    })?;
    // The entry's own ID, never the identity it was attributed to (§14.2).
    tracing::info!(
        provider = %provider,
        displaced = %id,
        reason,
        "saved a credential to displaced/"
    );
    Ok(id)
}

impl Engine {
    /// `tagteam displaced` (§6.3): every entry, newest first, joining the store's rows with the
    /// directory's files by ID. A file with no row is listed unrecorded, at the time its name
    /// carries; a row with no file, with its file missing. It reads only, under no lock, and
    /// creates nothing when the store or the directory is absent (§5).
    pub fn displaced(&self) -> Result<DisplacedList, EngineError> {
        let dir = displaced_dir(&self.env);
        let mut files = displaced_files(&dir)?;
        let mut entries = Vec::new();
        if let Some(store) = self.existing_store()? {
            for row in store.displaced_rows()? {
                // Only a displaced ID is ever in `files`, so no path is built from a row's ID.
                let file_present = files.remove(&row.id);
                let account =
                    self.displaced_account(&store, &row.provider, row.identity.as_ref())?;
                entries.push(DisplacedEntry {
                    id: row.id,
                    provider: Some(row.provider),
                    at_ms: row.at,
                    reason: Some(row.reason),
                    fingerprint: Some(row.fingerprint).filter(|f| !f.is_empty()),
                    identity: row.identity,
                    account,
                    file_present,
                    recorded: true,
                });
            }
        }
        entries.extend(files.into_iter().map(|id| DisplacedEntry {
            at_ms: id_at_ms(&id),
            id,
            provider: None,
            reason: None,
            fingerprint: None,
            identity: None,
            account: None,
            file_present: true,
            recorded: false,
        }));
        entries.sort_by(|a, b| b.at_ms.cmp(&a.at_ms).then_with(|| b.id.cmp(&a.id)));
        Ok(DisplacedList { dir, entries })
    }

    /// The position of the managed account `identity` names (§6.3), found through the row's
    /// provider by identity key. A key match whose account uuid conflicts is a different account
    /// (§6.1). `None` when the provider is not registered or cannot read the identity.
    fn displaced_account(
        &self,
        store: &Store,
        provider: &ProviderId,
        identity: Option<&Value>,
    ) -> Result<Option<u32>, EngineError> {
        let (Some(raw), Ok(p)) = (identity, self.provider(provider)) else {
            return Ok(None);
        };
        let Ok(identity) = p.parse_identity(raw) else {
            return Ok(None);
        };
        let key = p.identity_key(&identity);
        Ok(store
            .find_by_identity_key(provider, key.as_str())?
            .filter(|row| match (&row.account_uuid, &identity.account_uuid) {
                (Some(stored), Some(named)) => stored == named,
                _ => true,
            })
            .map(|row| row.position))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_displaced_id_is_an_epoch_twelve_hex_digits_and_six_lowercase_characters() {
        // Decision 12: the only names a path is ever built from.
        for good in [
            "1790000000-0123456789ab-a1b2c3",
            "1-000000000000-zzzzzz",
            "9223372036854775807-abcdefabcdef-000000",
        ] {
            assert!(is_displaced_id(good), "{good:?}");
        }
        for bad in [
            "",
            "x",
            "../../tagteam.db",
            "1790000000-0123456789AB-a1b2c3",
            "1790000000-0123456789ab-A1B2C3",
            "-0123456789ab-a1b2c3",
            "12345678901234567890-0123456789ab-a1b2c3",
            "1790000000-0123456789a-a1b2c3",
            "1790000000-0123456789ag-a1b2c3",
            "1790000000-0123456789ab-a1b2c",
            "1790000000-0123456789ab-a1b2c3-x",
            "1790000000-0123456789ab-a1b2c3.json",
            "1790000000-0123456789ab-a1b2c/",
            " 1790000000-0123456789ab-a1b2c3",
            "١٧٩٠-0123456789ab-a1b2c3",
        ] {
            assert!(!is_displaced_id(bad), "{bad:?}");
        }
    }

    #[test]
    fn every_id_the_writer_mints_is_a_displaced_id() {
        let known = Fingerprint::of_secret(b"rt-1");
        for _ in 0..1000 {
            for fp in [Some(&known), None] {
                let id = new_id(1_790_000_000_123, fp);
                assert!(is_displaced_id(&id), "{id}");
                assert!(id.starts_with("1790000000-"), "{id}");
            }
        }
        assert!(new_id(0, None).starts_with("0-000000000000-"));
    }

    #[test]
    fn a_file_with_no_row_is_dated_by_its_name() {
        assert_eq!(
            id_at_ms("1790000250-fedcba987654-bbbbbb"),
            1_790_000_250_000
        );
        assert_eq!(
            id_at_ms("9223372036854775807-abcdefabcdef-000000"),
            i64::MAX,
            "saturates"
        );
        assert_eq!(
            id_at_ms("9999999999999999999-abcdefabcdef-000000"),
            i64::MAX,
            "past i64"
        );
    }
}
