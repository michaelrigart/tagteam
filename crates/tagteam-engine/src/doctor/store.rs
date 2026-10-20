use std::path::Path;

use tagteam_provider::Check;
use tagteam_provider::doctor::quoted;

use crate::engine::Engine;
use crate::store::{SCHEMA_VERSION, Store};

use super::files::readable;
use super::{Id, Run, Stored};

/// Whether SQLite found the store itself damaged: pages it cannot read as a database
/// (`SQLITE_CORRUPT`), or a file that is no database (`SQLITE_NOTADB`). Any other error keeps
/// the store out of reach, which says nothing of its integrity (§13.6).
fn is_damage(e: &crate::store::StoreError) -> bool {
    use rusqlite::ErrorCode::{DatabaseCorrupt, NotADatabase};
    matches!(
        e,
        crate::store::StoreError::Sqlite(rusqlite::Error::SqliteFailure(f, _))
            if matches!(f.code, DatabaseCorrupt | NotADatabase)
    )
}

/// `PRAGMA quick_check` run once more before it fails the store: an immutable open reads the
/// file while a writer that began after the probe may be checkpointing it, which can show a
/// torn page that a second run does not. Two failures stand.
fn settled_quick_check(
    first: Result<Vec<String>, crate::store::StoreError>,
    again: impl FnOnce() -> Result<Vec<String>, crate::store::StoreError>,
) -> Result<Vec<String>, crate::store::StoreError> {
    match first {
        Ok(rows) if rows == ["ok"] => Ok(rows),
        _ => match again() {
            Ok(rows) if rows == ["ok"] => Ok(rows),
            second => second,
        },
    }
}

/// The fix for a damaged store.
fn restore_backup(path: &Path) -> String {
    format!(
        "restore {} from a backup; failing that, move it aside and add the accounts again",
        quoted(path)
    )
}

/// `store.integrity` for a store this user cannot open or read: a `warn` naming the cause,
/// never a failure of its integrity, which cannot be told.
fn out_of_reach(path: &Path, e: &crate::store::StoreError) -> Check {
    Check::warn(
        Id::STORE_INTEGRITY,
        format!(
            "{} cannot be opened ({e}), so its integrity cannot be told",
            path.display()
        ),
    )
    .fix(format!(
        "{}; `store.mode` names the `chmod` when the store's mode is the cause",
        readable(path)
    ))
}

/// The fix for store rows that cannot be read, in a store that passed `quick_check`.
fn restore(engine: &Engine) -> String {
    format!(
        "run `tagteam doctor` again; if it persists, restore {} from a backup",
        quoted(&engine.env.data_dir().join("tagteam.db"))
    )
}

/// A warning for store rows that cannot be read, with the fix that goes with it.
pub(super) fn warn_restore(engine: &Engine, id: &str, message: impl Into<String>) -> Check {
    Check::warn(id, message).fix(restore(engine))
}

fn store_skipped(why: &str) -> Check {
    Check::info(
        Id::STORE_SKIPPED,
        format!("the checks that read the store's rows were skipped: {why}"),
    )
}

impl Run<'_> {
    /// §13.6 Store: whether there is one, `PRAGMA quick_check`, and its schema version. Only a
    /// sound store at this build's schema is read by the checks after this one.
    pub(super) fn store(&mut self) {
        let data = self.engine.env.data_dir();
        let path = data.join("tagteam.db");
        match data.try_exists() {
            Ok(true) => {}
            Ok(false) => {
                self.push(
                    None,
                    Check::info(
                        Id::STORE_PRESENT,
                        format!(
                            "tagteam has no state yet: {} does not exist",
                            data.display()
                        ),
                    ),
                );
                return;
            }
            Err(e) => {
                self.stored = Stored::Unusable;
                self.push(
                    None,
                    Check::warn(
                        Id::STORE_PRESENT,
                        format!(
                            "whether tagteam has any state cannot be told: {} cannot be read ({})",
                            data.display(),
                            e.kind()
                        ),
                    )
                    .fix(readable(&data)),
                );
                self.push(None, store_skipped("the store cannot be read"));
                return;
            }
        }
        let store = match Store::open_read_only(&path) {
            Ok(Some(s)) => s,
            Ok(None) => {
                self.push(
                    None,
                    Check::info(
                        Id::STORE_PRESENT,
                        format!(
                            "tagteam has no store yet: {} does not exist",
                            path.display()
                        ),
                    ),
                );
                return;
            }
            Err(e) => {
                self.stored = Stored::Unusable;
                let check = if let crate::store::StoreError::LopsidedWal { .. } = e {
                    Check::warn(
                        Id::STORE_INTEGRITY,
                        format!(
                            "{} has a write-ahead log doctor cannot read without writing, so its integrity cannot be told ({e})",
                            path.display()
                        ),
                    )
                    .fix("run any tagteam command, such as `tagteam list`: it recovers the log, then `tagteam doctor` again")
                } else if is_damage(&e) {
                    Check::fail(
                        Id::STORE_INTEGRITY,
                        format!("{} cannot be opened: {e}", path.display()),
                    )
                    .fix(restore_backup(&path))
                } else {
                    out_of_reach(&path, &e)
                };
                self.push(None, check);
                self.push(None, store_skipped("the store cannot be read"));
                return;
            }
        };
        let sound = match settled_quick_check(store.quick_check(), || store.quick_check()) {
            Ok(rows) if rows == ["ok"] => {
                self.push(
                    None,
                    Check::ok(
                        Id::STORE_INTEGRITY,
                        format!("{} passes PRAGMA quick_check", path.display()),
                    ),
                );
                true
            }
            found => {
                let check = match found {
                    Err(e) if !is_damage(&e) => out_of_reach(&path, &e),
                    found => {
                        let detail = match found {
                            Ok(rows) => rows.into_iter().take(5).collect::<Vec<_>>().join("; "),
                            Err(e) => e.to_string(),
                        };
                        Check::fail(
                            Id::STORE_INTEGRITY,
                            format!("{} fails PRAGMA quick_check: {detail}", path.display()),
                        )
                        .fix(restore_backup(&path))
                    }
                };
                self.push(None, check);
                false
            }
        };
        let current = match store.schema_version() {
            Ok(v) if v > SCHEMA_VERSION => {
                self.push(
                    None,
                    Check::fail(
                        Id::STORE_SCHEMA,
                        format!(
                            "the store's schema is v{v}, newer than this tagteam's v{SCHEMA_VERSION}: a newer tagteam wrote it, and this one refuses it"
                        ),
                    )
                    .fix("upgrade tagteam to the version that wrote the store"),
                );
                false
            }
            Ok(v) if v < SCHEMA_VERSION => {
                self.push(
                    None,
                    Check::info(
                        Id::STORE_SCHEMA,
                        format!(
                            "the store's schema is v{v}, older than this tagteam's v{SCHEMA_VERSION}; the next tagteam command that opens it migrates it"
                        ),
                    )
                    .fix("run `tagteam list` to migrate it, then `tagteam doctor` again"),
                );
                false
            }
            Ok(v) => {
                self.push(
                    None,
                    Check::ok(Id::STORE_SCHEMA, format!("the store's schema is v{v}")),
                );
                true
            }
            Err(e) => {
                self.push(
                    None,
                    warn_restore(
                        self.engine,
                        Id::STORE_SCHEMA,
                        format!("the store's schema version cannot be read: {e}"),
                    ),
                );
                false
            }
        };
        self.stored = if sound && current {
            let accounts = match store.all_accounts() {
                Ok(rows) => Some(rows),
                Err(e) => {
                    self.push(
                        None,
                        warn_restore(self.engine,
                            Id::STORE_ACCOUNTS,
                            format!(
                                "the store's accounts cannot be read ({e}), so orphaned vault files, rescue files, session profiles and which providers have accounts are not checked"
                            )),
                    );
                    None
                }
            };
            Stored::Usable { store, accounts }
        } else {
            self.push(
                None,
                store_skipped("the store is not one this tagteam can read"),
            );
            Stored::Unusable
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_integrity_failure_stands_only_when_a_second_run_finds_it_too() {
        let bad = || Ok(vec!["row 1 missing from index".to_owned()]);
        let ok = || Ok(vec!["ok".to_owned()]);
        let rows = |r: Result<Vec<String>, crate::store::StoreError>| r.unwrap();
        assert_eq!(
            rows(settled_quick_check(ok(), || panic!("no second run"))),
            rows(ok())
        );
        assert_eq!(rows(settled_quick_check(bad(), ok)), rows(ok()));
        assert_eq!(rows(settled_quick_check(bad(), bad)), rows(bad()));
        let unreadable = || Err(crate::store::StoreError::Corrupt("x".into()));
        assert!(settled_quick_check(unreadable(), unreadable).is_err());
    }
}
