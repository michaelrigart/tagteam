//! `export` and `import` (§13.3): their output, and the export's file while it is being made.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use tagteam_engine::export::ExportResult;
use tagteam_engine::import::ImportReport;
use tagteam_engine::store::AccountRow;
use tagteam_provider::atomic::sync_parent;

use crate::render;

/// §13.3: whenever the file holds an OAuth account, export says that it hands logins over.
pub(crate) const HAND_OFF: &str = "this export hands its OAuth logins over rather than copying them: whichever machine refreshes one first invalidates every other copy, so use it to move accounts; to use one account on two machines, log in on each";

/// The export's file while it is made (§13.3): a temporary file created 0600 with `O_EXCL`
/// beside the destination before any account is read, so a destination that cannot be written
/// refuses first (Review Focus 5). `publish` renames it into place; dropped unpublished, it is
/// removed.
pub(crate) struct ExportFile {
    temp: PathBuf,
    target: PathBuf,
    file: Option<File>,
    published: bool,
}

impl ExportFile {
    /// Refuses a directory, and a directory it cannot create a file in, before anything is
    /// read; the error names the path.
    pub(crate) fn create(target: &Path) -> io::Result<Self> {
        if fs::metadata(target).is_ok_and(|m| m.is_dir()) {
            return Err(io::Error::new(
                io::ErrorKind::IsADirectory,
                format!(
                    "{} is a directory; name a file to export to",
                    target.display()
                ),
            ));
        }
        let name = target.file_name().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} names no file to export to", target.display()),
            )
        })?;
        let dir = match target.parent() {
            Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
            _ => PathBuf::from("."),
        };
        let temp = dir.join(format!(
            ".{}.tagteam-{}-{:08x}",
            name.to_string_lossy(),
            std::process::id(),
            fastrand::u32(..)
        ));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)
            .map_err(|e| {
                io::Error::new(
                    e.kind(),
                    format!("cannot create the export in {}: {e}", dir.display()),
                )
            })?;
        Ok(Self {
            temp,
            target: target.to_path_buf(),
            file: Some(file),
            published: false,
        })
    }

    /// Writes `bytes`, syncs them, and renames the file into place.
    pub(crate) fn publish(mut self, bytes: &[u8]) -> io::Result<()> {
        let mut file = self.file.take().expect("an export file is published once");
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&self.temp, &self.target)?;
        self.published = true;
        if let Some(dir) = self.target.parent().filter(|d| !d.as_os_str().is_empty()) {
            // As every published write's (§14): silent only where the filesystem does not sync
            // a directory, and never a failure of the export.
            sync_parent(dir, "an export");
        }
        Ok(())
    }
}

impl Drop for ExportFile {
    /// A temporary file that cannot be removed is left beside the destination, holding part of
    /// an export: a contained error, logged at WARN with its cause (§14). The line names the file
    /// by its role, never its path: the user chose the export's name, which may carry an email
    /// or a label (§14.2). One already gone was never left.
    fn drop(&mut self) {
        if self.published {
            return;
        }
        match fs::remove_file(&self.temp) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => tracing::warn!(
                "could not remove an unpublished export's temporary file, left beside the export: {e}"
            ),
            _ => {}
        }
    }
}

/// An account as export's and import's lines name it: `#2 b@x.co`.
fn who(row: &AccountRow) -> String {
    format!("#{} {}", row.position, render::email(row))
}

/// §13.3's warnings, for stderr: the hand-over, the accounts this machine goes on refreshing,
/// and each account a bulk export skipped, with its reason.
pub(crate) fn export_warnings(r: &ExportResult) -> Vec<String> {
    let mut out = Vec::new();
    if r.accounts.iter().any(|e| e.refreshes) {
        out.push(HAND_OFF.to_owned());
    }
    // A kind that never refreshes keeps working in the exported copy (§13.3).
    for e in r.accounts.iter().filter(|e| e.in_use && e.refreshes) {
        out.push(format!(
            "{} is in use here: this machine goes on refreshing it, so its exported copy stops working the next time it does",
            who(&e.row)
        ));
    }
    for s in &r.skipped {
        out.push(format!("{} was not exported: {}", who(&s.row), s.reason));
    }
    out
}

pub(crate) fn export_human(r: &ExportResult, file: Option<&Path>, encrypted: bool) -> String {
    let what = match r.accounts.len() {
        1 => "1 account".to_owned(),
        n => format!("{n} accounts"),
    };
    let how = if encrypted {
        "encrypted"
    } else {
        "unencrypted"
    };
    match file {
        Some(f) => format!("Exported {what} to {}, {how}.\n", f.display()),
        None => format!("Exported {what}, {how}.\n"),
    }
}

/// §13.3's `--json` for `export`.
pub(crate) fn export_json(r: &ExportResult, file: Option<&Path>, encrypted: bool) -> Value {
    let accounts: Vec<Value> = r
        .accounts
        .iter()
        .map(|e| {
            json!({
                "provider": e.row.provider.as_str(),
                "number": e.row.position,
                "email": render::email(&e.row),
                "source": e.source.as_str(),
                "inUse": e.in_use,
            })
        })
        .collect();
    let skipped: Vec<Value> = r
        .skipped
        .iter()
        .map(|s| {
            json!({
                "provider": s.row.provider.as_str(),
                "number": s.row.position,
                "email": render::email(&s.row),
                "reason": s.reason,
            })
        })
        .collect();
    json!({
        "schemaVersion": 1,
        "ok": true,
        "file": file.map(|f| f.display().to_string()),
        "encrypted": encrypted,
        "accounts": accounts,
        "skipped": skipped,
    })
}

/// One line per account of the file, in its order.
pub(crate) fn import_human(r: &ImportReport) -> String {
    if r.accounts.is_empty() {
        return "The file holds no account.\n".to_owned();
    }
    r.accounts
        .iter()
        .map(|a| {
            format!(
                "{:<8} #{} {}: {}\n",
                a.outcome.as_str(),
                a.position,
                a.email,
                a.message
            )
        })
        .collect()
}

/// §13.3's `--json` for `import`.
pub(crate) fn import_json(r: &ImportReport) -> Value {
    let accounts: Vec<Value> = r
        .accounts
        .iter()
        .map(|a| {
            json!({
                "provider": a.provider.as_str(),
                "number": a.position,
                "email": a.email,
                "outcome": a.outcome.as_str(),
                "message": a.message,
            })
        })
        .collect();
    json!({
        "schemaVersion": 1,
        "ok": !r.any_failed(),
        "accounts": accounts,
        "warnings": r.warnings,
    })
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use tagteam_engine::export::{Exported, Source};

    use super::*;

    #[test]
    fn an_export_file_is_private_from_creation_and_appears_only_when_published() {
        let d = tempfile::tempdir().unwrap();
        let target = d.path().join("backup.age");
        let f = ExportFile::create(&target).unwrap();
        let temps: Vec<PathBuf> = fs::read_dir(d.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(temps.len(), 1);
        assert_eq!(
            fs::metadata(&temps[0]).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(!target.exists());
        f.publish(b"sealed").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"sealed");
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(fs::read_dir(d.path()).unwrap().count(), 1, "no temp left");
    }

    #[test]
    fn an_export_file_never_published_leaves_nothing() {
        let d = tempfile::tempdir().unwrap();
        drop(ExportFile::create(&d.path().join("x")).unwrap());
        assert_eq!(fs::read_dir(d.path()).unwrap().count(), 0);
    }

    #[test]
    fn an_unpublished_export_that_cannot_be_removed_is_logged_with_its_cause() {
        // §14: a contained error is logged, never discarded. The temporary file is swapped for
        // a non-empty directory of its name, which no `remove_file` deletes.
        #[derive(Clone, Default)]
        struct Lines(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
        impl io::Write for Lines {
            fn write(&mut self, b: &[u8]) -> io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Lines {
            type Writer = Lines;
            fn make_writer(&'a self) -> Self::Writer {
                self.clone()
            }
        }
        let d = tempfile::tempdir().unwrap();
        // A name the user chose, which carries an email.
        let f = ExportFile::create(&d.path().join("alice@example.com.age")).unwrap();
        let temp = f.temp.clone();
        fs::remove_file(&temp).unwrap();
        fs::create_dir(&temp).unwrap();
        fs::write(temp.join("keep"), "").unwrap();
        let lines = Lines::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(lines.clone())
            .with_ansi(false)
            .without_time()
            .finish();
        tracing::subscriber::with_default(subscriber, || drop(f));
        let text = String::from_utf8(lines.0.lock().unwrap().clone()).unwrap();
        let warnings: Vec<&str> = text.lines().filter(|l| l.contains("WARN")).collect();
        assert_eq!(warnings.len(), 1, "{text}");
        assert!(
            warnings[0].contains("could not remove an unpublished export's temporary file"),
            "{}",
            warnings[0]
        );
        // §14.2: its role and cause, never the path the user chose.
        assert!(!text.contains("alice@example.com"), "{text}");
    }

    #[test]
    fn a_directory_or_a_directory_it_cannot_write_in_is_refused_by_name() {
        let d = tempfile::tempdir().unwrap();
        let err = ExportFile::create(d.path()).err().unwrap();
        assert_eq!(
            err.to_string(),
            format!(
                "{} is a directory; name a file to export to",
                d.path().display()
            )
        );
        let locked = d.path().join("someone-else");
        fs::create_dir(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o500)).unwrap();
        let err = ExportFile::create(&locked.join("x.age")).err().unwrap();
        assert!(
            err.to_string().starts_with(&format!(
                "cannot create the export in {}: ",
                locked.display()
            )),
            "{err}"
        );
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
    }

    fn exported(position: u32, kind: &str, in_use: bool, refreshes: bool) -> Exported {
        use tagteam_core::{AccountId, CLAUDE_CODE, ProviderId};
        let email = format!("u{position}@x.co");
        Exported {
            row: AccountRow {
                id: AccountId::from_string(format!("id-{position}")),
                provider: ProviderId::new(CLAUDE_CODE),
                position,
                identity_key: format!("{email}\n"),
                label: email.clone(),
                email: Some(email),
                org_uuid: String::new(),
                org_name: None,
                account_uuid: None,
                kind: kind.into(),
                alias: None,
                disabled: false,
                identity_json: json!({}),
                login_expires_at: None,
                login_epoch: 0,
                replacing_fp: None,
                quarantine_reason: None,
                quarantine_fp: None,
                quarantine_at: None,
                added_at: 1,
            },
            source: Source::Vault,
            in_use,
            refreshes,
        }
    }

    #[test]
    fn only_an_in_use_account_that_refreshes_is_warned_about() {
        // §13.3: a setup token or an API key never rotates, so its exported copy keeps working
        // while this machine uses it.
        let result = ExportResult {
            envelope: Vec::new(),
            accounts: vec![
                exported(1, "oauth", true, true),
                exported(2, "setup-token", true, false),
                exported(3, "api-key", true, false),
                exported(4, "oauth", false, true),
            ],
            skipped: Vec::new(),
        };
        let warnings = export_warnings(&result);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert_eq!(warnings[0], HAND_OFF);
        assert!(
            warnings[1].starts_with("#1 u1@x.co is in use here"),
            "{warnings:?}"
        );

        let tokens_only = ExportResult {
            envelope: Vec::new(),
            accounts: vec![exported(2, "setup-token", true, false)],
            skipped: Vec::new(),
        };
        assert!(export_warnings(&tokens_only).is_empty());
    }

    #[test]
    fn an_import_report_reads_one_line_per_account_and_a_failure_is_not_ok() {
        use tagteam_core::{CLAUDE_CODE, ProviderId};
        use tagteam_engine::import::{Imported, Outcome};

        let empty = ImportReport::default();
        assert_eq!(import_human(&empty), "The file holds no account.\n");
        assert_eq!(import_json(&empty)["ok"], true);

        let report = ImportReport {
            accounts: vec![
                Imported {
                    provider: ProviderId::new(CLAUDE_CODE),
                    position: 1,
                    email: "a@x.co".into(),
                    outcome: Outcome::Created,
                    message: "added".into(),
                },
                Imported {
                    provider: ProviderId::new(CLAUDE_CODE),
                    position: 12,
                    email: "b@x.co".into(),
                    outcome: Outcome::Failed,
                    message: "no".into(),
                },
            ],
            warnings: vec!["w".into()],
        };
        assert_eq!(
            import_human(&report),
            "created  #1 a@x.co: added\nfailed   #12 b@x.co: no\n"
        );
        let v = import_json(&report);
        assert_eq!(
            (&v["ok"], &v["accounts"][1]["number"], &v["warnings"][0]),
            (&json!(false), &json!(12), &json!("w"))
        );
    }
}
