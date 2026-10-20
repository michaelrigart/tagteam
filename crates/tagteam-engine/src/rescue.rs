//! §6.3 `rescue/`: a received successor, written ahead of its vault write and kept when that
//! write fails (H6), in a 0600 envelope that records which generation it succeeds; and §6.2's
//! rule that pending rescues settle before any activation.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use tagteam_core::{AccountId, Fingerprint};
use tagteam_provider::atomic::{ensure_private_dir, write_atomic_private};
use tagteam_provider::{Provider, Read};

use crate::account_lock::AccountLock;
use crate::engine::Engine;
use crate::error::EngineError;
use crate::store::AccountRow;

const FORMAT: &str = "tagteam-rescue";
const VERSION: i64 = 1;

/// One readable rescue envelope (§6.3). It holds a secret, so it has no `Debug`.
pub(crate) struct RescueEntry {
    pub path: PathBuf,
    /// The generation that was sent: this rescue succeeds it.
    pub predecessor_fp: String,
    pub credential: Vec<u8>,
}

impl RescueEntry {
    /// H6's landed rule: the vault, whose current fingerprint is `vault`, already holds this
    /// rescue's own generation. Claude Code's fingerprint hashes the refresh token, so the
    /// vault then holds a valid generation with the same refresh token, and the rescue can hold
    /// at most an access token that refresh token re-obtains.
    pub(crate) fn landed(&self, p: &dyn Provider, vault: Option<&Fingerprint>) -> bool {
        vault.is_some_and(|v| p.fingerprint(&self.credential).as_ref() == Some(v))
    }
}

pub(crate) enum RescueFile {
    Entry(RescueEntry),
    /// Could not be read or parsed. `detail` never quotes the file's bytes.
    Unreadable {
        path: PathBuf,
        detail: String,
    },
}

/// Why `bytes` is not account `id`'s rescue envelope. Never quotes the bytes.
fn parse(path: &Path, bytes: &[u8], id: &AccountId) -> Result<RescueEntry, String> {
    let v: Value = serde_json::from_slice(bytes).map_err(|_| "it is not JSON".to_owned())?;
    if v["format"].as_str() != Some(FORMAT) || v["version"].as_i64() != Some(VERSION) {
        return Err("it is not a version 1 tagteam rescue envelope".into());
    }
    if v["accountId"].as_str() != Some(id.as_str()) {
        return Err("it names a different account".into());
    }
    if v["loginEpoch"].as_i64().is_none() {
        return Err("it has no loginEpoch".into());
    }
    let predecessor_fp = v["predecessorFp"]
        .as_str()
        .filter(|s| Fingerprint::parse(s).is_some())
        .ok_or_else(|| "its predecessorFp is not a fingerprint".to_owned())?
        .to_owned();
    let credential = v["credential"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "it holds no credential".to_owned())?
        .as_bytes()
        .to_vec();
    Ok(RescueEntry {
        path: path.to_path_buf(),
        predecessor_fp,
        credential,
    })
}

/// §6.3: a `rescue` path that is not a directory, or cannot be listed, so every account's
/// rescues are unknown.
pub(crate) struct RescueUnlisted {
    pub path: PathBuf,
    pub detail: String,
}

impl Engine {
    pub(crate) fn rescue_dir(&self) -> PathBuf {
        self.env.data_dir().join("rescue")
    }

    /// Writes a received successor to `rescue/<id>-<epoch>-<fp12>.json` (§5): a plain 0600 file,
    /// never a Keychain item, created beside its final name and renamed into place (§6.3). The
    /// directory is created 0700 on first use. The credential is stored in `p`'s vault form
    /// (H5.3), what the vault would keep, as a UTF-8 string, which every provider's credential
    /// is. `displaced/` is never written here: it keeps what it saves verbatim. It logs
    /// nothing: its callers say why it was written, ahead of the vault (`Received::new`, H6) or
    /// kept because the vault failed (`Received::keep`).
    pub(crate) fn write_rescue(
        &self,
        p: &dyn Provider,
        id: &AccountId,
        login_epoch: i64,
        predecessor_fp: &str,
        successor: &[u8],
        successor_fp: &Fingerprint,
    ) -> Result<PathBuf, EngineError> {
        let successor = p.vault_form(successor);
        let credential = std::str::from_utf8(&successor).map_err(|_| {
            EngineError::Io(io::Error::other("the refreshed credential is not UTF-8"))
        })?;
        let envelope = json!({
            "format": FORMAT,
            "version": VERSION,
            "accountId": id.as_str(),
            "loginEpoch": login_epoch,
            "predecessorFp": predecessor_fp,
            "credential": credential,
        });
        let dir = self.rescue_dir();
        ensure_private_dir(&dir)?;
        let path = dir.join(format!(
            "{id}-{login_epoch}-{}.json",
            successor_fp.short12()
        ));
        write_atomic_private(
            &path,
            &serde_json::to_vec(&envelope).expect("a Value always serializes"),
            0o600,
        )?;
        Ok(path)
    }

    /// The paths of `id`'s rescue files (named `<id>-…json`), readable or not, in name order.
    /// Never creates `rescue/`. A `rescue` path that cannot be listed is `Err`: it may hide any
    /// account's.
    pub(crate) fn rescue_paths_for(&self, id: &AccountId) -> Result<Vec<PathBuf>, RescueUnlisted> {
        let dir = self.rescue_dir();
        let listing = match fs::read_dir(&dir) {
            Ok(l) => l,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => {
                return Err(RescueUnlisted {
                    path: dir,
                    detail: e.to_string(),
                });
            }
        };
        let prefix = format!("{id}-");
        let mut paths = Vec::new();
        for entry in listing {
            // An entry the listing cannot return may be one of `id`'s: the path cannot be
            // listed, as when it cannot be opened (§14: never skipped).
            let path = match entry {
                Ok(entry) => entry.path(),
                Err(e) => {
                    return Err(RescueUnlisted {
                        path: dir,
                        detail: e.to_string(),
                    });
                }
            };
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(&prefix) && n.ends_with(".json"))
            {
                paths.push(path);
            }
        }
        paths.sort();
        Ok(paths)
    }

    /// Every rescue file for `id` (named `<id>-…json`), in name order. Never creates
    /// `rescue/`. A directory that cannot be listed is itself unreadable: it may hide one.
    pub(crate) fn rescues_for(&self, id: &AccountId) -> Vec<RescueFile> {
        let paths = match self.rescue_paths_for(id) {
            Ok(paths) => paths,
            Err(RescueUnlisted { path, detail }) => {
                return vec![RescueFile::Unreadable { path, detail }];
            }
        };
        paths
            .into_iter()
            .map(|path| match fs::read(&path) {
                Err(e) => RescueFile::Unreadable {
                    path,
                    detail: e.to_string(),
                },
                Ok(bytes) => match parse(&path, &bytes, id) {
                    Ok(entry) => RescueFile::Entry(entry),
                    Err(detail) => RescueFile::Unreadable { path, detail },
                },
            })
            .collect()
    }

    /// Absent is success.
    pub(crate) fn delete_rescue(&self, path: &Path) -> Result<(), EngineError> {
        match fs::remove_file(path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    /// Deletes a rescue the vault already holds (H6's landed rule), and says so at DEBUG.
    pub(crate) fn delete_landed(&self, row: &AccountRow, path: &Path) -> Result<(), EngineError> {
        self.delete_rescue(path)?;
        tracing::debug!(
            position = row.position,
            account = %row.id,
            "deleted a rescue the vault already holds"
        );
        Ok(())
    }

    /// §6.2 "Pending rescues before activation", and §7.3 step 3's adoption. A rescue that
    /// cannot be read might hold the only live successor, so it refuses. Then H6's landed rule:
    /// a rescue whose own generation the vault already holds (its fingerprint is the vault's:
    /// a crash after the vault write, or a reply that kept the refresh token) is deleted and
    /// never adopted, since adopting it could only put an older access token over the vault's;
    /// a failed delete is logged and the rescue is still skipped. Of the rest, a rescue whose
    /// predecessor is the vault's current generation holds the only live successor: it is
    /// written to the vault (verified), and then its file is deleted; a failed adoption
    /// refuses. A rescue whose predecessor is any other generation is superseded and left
    /// alone. The caller holds `lock`.
    pub(crate) fn settle_rescues(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        lock: &AccountLock,
    ) -> Result<(), EngineError> {
        let rescues = self.rescues_for(&row.id);
        if rescues.is_empty() {
            return Ok(());
        }
        let pending = |detail: String| EngineError::RescuePending {
            position: row.position,
            label: row.label.clone(),
            detail,
        };
        if let Some(e) = rescues.iter().find_map(|r| match r {
            RescueFile::Unreadable { path, detail } => {
                Some(format!("{} is unreadable: {detail}", path.display()))
            }
            RescueFile::Entry(_) => None,
        }) {
            return Err(pending(e));
        }
        let current = match self.vault.read(&row.id) {
            Read::Present(b) => p.fingerprint(&b),
            Read::Absent => None,
            Read::Unreadable(e) => return Err(EngineError::Unreadable(e)),
        };
        let mut unlanded = Vec::new();
        for r in rescues {
            let RescueFile::Entry(e) = r else { continue };
            if !e.landed(p, current.as_ref()) {
                unlanded.push(e);
                continue;
            }
            if let Err(err) = self.delete_landed(row, &e.path) {
                tracing::warn!(
                    position = row.position,
                    account = %row.id,
                    kind = err.kind(),
                    "a rescue the vault already holds could not be deleted"
                );
            }
        }
        for e in unlanded {
            if current.as_ref().map(Fingerprint::as_str) != Some(e.predecessor_fp.as_str()) {
                continue;
            }
            self.persist_generation(p, row, lock, &e.credential)
                .map_err(|err| {
                    pending(format!("{} could not be adopted: {err}", e.path.display()))
                })?;
            // Once adopted the vault holds its generation, so a failed delete is harmless: the
            // next pass deletes it by the landed rule, or, once the vault has moved on, finds
            // its predecessor is no longer current and leaves it alone.
            if let Err(err) = self.delete_rescue(&e.path) {
                tracing::warn!(
                    position = row.position,
                    account = %row.id,
                    kind = err.kind(),
                    "an adopted rescue file could not be deleted"
                );
            }
            return Ok(());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::testutil::{T, cred};
    use crate::vault::SERVICE;

    fn mode(p: &Path) -> u32 {
        fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    fn entries(t: &T, id: &AccountId) -> Vec<RescueEntry> {
        t.engine
            .rescues_for(id)
            .into_iter()
            .map(|r| match r {
                RescueFile::Entry(e) => e,
                RescueFile::Unreadable { path, detail } => {
                    panic!("{} unreadable: {detail}", path.display())
                }
            })
            .collect()
    }

    #[test]
    fn an_envelope_round_trips_in_a_private_file() {
        let t = T::new();
        let id = AccountId::from_string("0192-acct");
        let succ = cred("rt-2", 9);
        let succ_fp = t.cc.fingerprint(&succ).unwrap();
        // A real fingerprint: the parser accepts only the canonical `sha256:<64 hex>` form.
        let pred = t.fp(&cred("rt-1", 9));
        let path = t
            .engine
            .write_rescue(t.cc.as_ref(), &id, 3, &pred, &succ, &succ_fp)
            .unwrap();
        assert_eq!(
            path,
            t.env
                .data_dir()
                .join("rescue")
                .join(format!("0192-acct-3-{}.json", succ_fp.short12()))
        );
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(path.parent().unwrap()), 0o700);
        let v: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            v,
            json!({"format": "tagteam-rescue", "version": 1, "accountId": "0192-acct",
                "loginEpoch": 3, "predecessorFp": pred,
                "credential": String::from_utf8(succ.clone()).unwrap()})
        );
        let got = entries(&t, &id);
        assert_eq!(got.len(), 1);
        // The envelope's accountId and loginEpoch (asserted above) are validated on read, not kept.
        assert_eq!(
            (
                &got[0].path,
                got[0].predecessor_fp.as_str(),
                &got[0].credential
            ),
            (&path, pred.as_str(), &succ)
        );
        t.engine.delete_rescue(&path).unwrap();
        assert!(entries(&t, &id).is_empty());
        t.engine.delete_rescue(&path).unwrap(); // already gone: fine
    }

    #[test]
    fn a_rescue_envelope_holds_the_vault_form() {
        let t = T::new();
        let id = AccountId::from_string("acct");
        let mut full: Value = serde_json::from_slice(&cred("rt-2", 9)).unwrap();
        full["mcpOAuth"] = json!({"srv": {"token": "machine-shared"}});
        let full = full.to_string().into_bytes();
        let fp = t.cc.fingerprint(&full).unwrap();
        let path = t
            .engine
            .write_rescue(t.cc.as_ref(), &id, 0, &t.fp(&cred("rt-1", 9)), &full, &fp)
            .unwrap();
        let v: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            v["credential"].as_str().unwrap().as_bytes(),
            cred("rt-2", 9)
        );
    }

    #[test]
    fn looking_never_creates_the_directory() {
        let t = T::new();
        assert!(
            t.engine
                .rescues_for(&AccountId::from_string("x"))
                .is_empty()
        );
        assert!(!t.env.data_dir().join("rescue").exists());
    }

    #[test]
    fn only_this_accounts_files_are_listed_and_damaged_ones_are_unreadable() {
        let t = T::new();
        let id = AccountId::from_string("acct");
        let other = AccountId::from_string("other");
        let succ = cred("rt-2", 9);
        let fp = t.cc.fingerprint(&succ).unwrap();
        let pred = t.fp(&cred("rt-1", 9));
        t.engine
            .write_rescue(t.cc.as_ref(), &other, 0, &pred, &succ, &fp)
            .unwrap();
        let dir = t.env.data_dir().join("rescue");
        fs::write(
            dir.join("acct-0-truncated.json"),
            b"{\"format\":\"tagteam-res",
        )
        .unwrap();
        fs::write(
            dir.join("acct-0-wrongfmt.json"),
            json!({"format": "something-else", "version": 1}).to_string(),
        )
        .unwrap();
        fs::write(
            dir.join("acct-0-mislabelled.json"),
            json!({"format": "tagteam-rescue", "version": 1, "accountId": "other",
                "loginEpoch": 0, "predecessorFp": fp.as_str(), "credential": "x"})
            .to_string(),
        )
        .unwrap();
        let found = t.engine.rescues_for(&id);
        assert_eq!(found.len(), 3);
        for r in &found {
            assert!(matches!(r, RescueFile::Unreadable { .. }));
        }
        assert_eq!(entries(&t, &other).len(), 1);
    }

    #[test]
    fn a_rescue_succeeding_the_vault_generation_is_adopted() {
        let t = T::new();
        let row = t.account("a@x.co", &cred("rt-1", 5));
        t.engine
            .store()
            .unwrap()
            .set_quarantine(&row.id, "invalid_grant", "sha256:old", 1)
            .unwrap();
        let succ = cred("rt-2", 77);
        let pred = t.fp(&cred("rt-1", 5));
        let path = t
            .engine
            .write_rescue(
                t.cc.as_ref(),
                &row.id,
                0,
                &pred,
                &succ,
                &t.cc.fingerprint(&succ).unwrap(),
            )
            .unwrap();
        let lock = t.lock(&row.id);
        t.engine
            .settle_rescues(t.cc.as_ref(), &t.row(&row.id), &lock)
            .unwrap();
        assert_eq!(t.vault_rt(&row.id, false).as_deref(), Some("rt-2"));
        assert_eq!(t.vault_rt(&row.id, true).as_deref(), Some("rt-1"));
        assert!(!path.exists(), "deleted after the verified vault write");
        let after = t.row(&row.id);
        assert_eq!(after.quarantine_reason, None);
        assert_eq!(after.login_expires_at, Some(77));
    }

    #[test]
    fn a_superseded_rescue_is_left_alone() {
        let t = T::new();
        let row = t.account("a@x.co", &cred("rt-3", 5));
        let succ = cred("rt-2", 9);
        let path = t
            .engine
            .write_rescue(
                t.cc.as_ref(),
                &row.id,
                0,
                &t.fp(&cred("rt-1", 5)),
                &succ,
                &t.cc.fingerprint(&succ).unwrap(),
            )
            .unwrap();
        let lock = t.lock(&row.id);
        t.engine.settle_rescues(t.cc.as_ref(), &row, &lock).unwrap();
        assert_eq!(t.vault_rt(&row.id, false).as_deref(), Some("rt-3"));
        assert!(path.exists());
    }

    #[test]
    fn an_unreadable_rescue_blocks_until_it_is_settled() {
        // §6.2: it may hold the successor of the vault's generation.
        let t = T::new();
        let row = t.account("a@x.co", &cred("rt-1", 5));
        let dir = t.env.data_dir().join("rescue");
        fs::create_dir_all(&dir).unwrap();
        let damaged = dir.join(format!("{}-0-abcdef012345.json", row.id));
        fs::write(&damaged, b"not json").unwrap();
        let lock = t.lock(&row.id);
        let err = t
            .engine
            .settle_rescues(t.cc.as_ref(), &row, &lock)
            .unwrap_err();
        match &err {
            EngineError::RescuePending {
                position, detail, ..
            } => {
                assert_eq!(*position, row.position);
                assert!(detail.contains(&damaged.display().to_string()), "{detail}");
            }
            other => panic!("expected RescuePending, got {other:?}"),
        }
        assert_eq!(err.kind(), "rescue-pending");
        assert_eq!(t.vault_rt(&row.id, false).as_deref(), Some("rt-1"));
    }

    fn same_generation(rt: &str) -> Vec<u8> {
        json!({"claudeAiOauth": {"accessToken": "at-other", "refreshToken": rt, "expiresAt": 1,
            "refreshTokenExpiresAt": 5}})
        .to_string()
        .into_bytes()
    }

    /// A rescue of the vault's own generation (H6's landed rule), written as a gate whose
    /// write-ahead outlived its vault write would leave it.
    fn landed_rescue(t: &T, row: &AccountRow, vault: &[u8]) -> PathBuf {
        let same = same_generation("rt-1");
        t.engine
            .write_rescue(
                t.cc.as_ref(),
                &row.id,
                0,
                &t.fp(vault),
                &same,
                &t.cc.fingerprint(&same).unwrap(),
            )
            .unwrap()
    }

    #[test]
    fn a_rescue_the_vault_already_holds_is_deleted_never_adopted() {
        let t = T::new();
        let row = t.account("a@x.co", &cred("rt-1", 5));
        let vault = t.kc.get(SERVICE, row.id.as_str()).unwrap();
        let path = landed_rescue(&t, &row, &vault);
        let lock = t.lock(&row.id);
        t.engine.settle_rescues(t.cc.as_ref(), &row, &lock).unwrap();
        assert!(!path.exists());
        assert_eq!(t.kc.get(SERVICE, row.id.as_str()).unwrap(), vault);
        assert_eq!(t.vault_rt(&row.id, true), None, "no .prev");
    }

    #[test]
    fn a_landed_rescue_that_cannot_be_deleted_is_still_never_adopted() {
        let t = T::new();
        let row = t.account("a@x.co", &cred("rt-1", 5));
        let vault = t.kc.get(SERVICE, row.id.as_str()).unwrap();
        let path = landed_rescue(&t, &row, &vault);
        let dir = path.parent().unwrap();
        fs::set_permissions(dir, fs::Permissions::from_mode(0o500)).unwrap();
        let lock = t.lock(&row.id);
        let settled = t.engine.settle_rescues(t.cc.as_ref(), &row, &lock);
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
        settled.unwrap();
        assert!(path.exists(), "the delete failed");
        assert_eq!(t.kc.get(SERVICE, row.id.as_str()).unwrap(), vault);
        assert_eq!(t.vault_rt(&row.id, true), None, "no .prev");
    }

    #[test]
    fn a_failed_adoption_keeps_the_rescue_and_refuses() {
        let t = T::new();
        let row = t.account("a@x.co", &cred("rt-1", 5));
        let succ = cred("rt-2", 9);
        let path = t
            .engine
            .write_rescue(
                t.cc.as_ref(),
                &row.id,
                0,
                &t.fp(&cred("rt-1", 5)),
                &succ,
                &t.cc.fingerprint(&succ).unwrap(),
            )
            .unwrap();
        t.kc.set_fail_write(SERVICE, true);
        let lock = t.lock(&row.id);
        assert!(matches!(
            t.engine.settle_rescues(t.cc.as_ref(), &row, &lock),
            Err(EngineError::RescuePending { .. })
        ));
        t.kc.set_fail_write(SERVICE, false);
        assert!(
            path.exists(),
            "a rescue is deleted only after a verified vault write"
        );
        assert_eq!(t.vault_rt(&row.id, false).as_deref(), Some("rt-1"));
    }
}
