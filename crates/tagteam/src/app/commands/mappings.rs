use std::path::{Path, PathBuf};

use serde_json::json;
use tagteam_engine::EngineError;
use tagteam_engine::store::{Mapping, StoreError};

use crate::{displaced_cmd, render};

use crate::app::App;
use crate::app::failure::{Failure, KIND_INVALID_INPUT, KIND_NEEDS_CONFIRMATION, cancelled};

const YES_WITHOUT_PURGE: &str =
    "--yes confirms --purge; name the entries to delete with --purge ID...";
const PURGE_NEEDS_YES: &str =
    "displaced credentials cannot be recovered once deleted; pass --yes to delete them";

/// §12.7: PATH (default: the current directory) as `map` stores it: canonical (symlinks
/// resolved, absolute), and a directory. Mappings are text, so a path that is not UTF-8 is
/// refused.
fn canonical_dir(path: Option<PathBuf>) -> Result<String, Failure> {
    let given = path.unwrap_or_else(|| PathBuf::from("."));
    let canonical =
        std::fs::canonicalize(&given).map_err(|e| invalid_path(&given, &e.to_string()))?;
    if !canonical.is_dir() {
        return Err(invalid_path(&given, "not a directory"));
    }
    stored_path(canonical)
}

/// `unmap`'s PATH: canonical, as `map` stored it, or, for a directory that is gone, absolute as
/// given, so the path `map` listed still unmaps.
fn unmapped_path(path: Option<PathBuf>) -> Result<String, Failure> {
    let given = path.unwrap_or_else(|| PathBuf::from("."));
    let resolved = match std::fs::canonicalize(&given) {
        Ok(p) => p,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::path::absolute(&given).map_err(|e| invalid_path(&given, &e.to_string()))?
        }
        Err(e) => return Err(invalid_path(&given, &e.to_string())),
    };
    stored_path(resolved)
}

/// A path as the store holds it: rebuilt from its components (no trailing `/`), as text.
fn stored_path(path: PathBuf) -> Result<String, Failure> {
    let path: PathBuf = path.components().collect();
    path.into_os_string().into_string().map_err(|raw| {
        invalid_path(
            Path::new(&raw),
            "not valid UTF-8, and mappings are stored as text",
        )
    })
}

fn invalid_path(path: &Path, why: &str) -> Failure {
    Failure::Message(KIND_INVALID_INPUT, format!("{}: {why}", path.display()))
}

impl App<'_, '_> {
    /// §12.7: `map` with no arguments lists the mappings, only `--provider`'s when it is given.
    /// It never creates the store (§5).
    pub(in crate::app) fn list_mappings(&mut self) -> Result<(), Failure> {
        let mut rows = Vec::new();
        if let Some(store) = self.engine.existing_store()? {
            for m in store.mappings().map_err(EngineError::from)? {
                if self
                    .provider_flag
                    .as_ref()
                    .is_some_and(|p| p != &m.provider)
                {
                    continue;
                }
                // `None` only when a `remove` raced this read: its mappings went with it.
                if let Some(account) = store.account(&m.account_id).map_err(EngineError::from)? {
                    rows.push((m, account));
                }
            }
        }
        self.print(&render::mappings_human(&rows), render::mappings_json(&rows));
        Ok(())
    }

    /// §12.7: maps PATH (default: the current directory), stored canonical, for the account's
    /// own provider, replacing the mapping PATH held for that provider.
    pub(in crate::app) fn map(
        &mut self,
        account: &str,
        path: Option<PathBuf>,
    ) -> Result<(), Failure> {
        let row = self.resolve(account)?;
        let mapping = Mapping {
            path: canonical_dir(path)?,
            provider: row.provider.clone(),
            account_id: row.id.clone(),
            added_at: self.engine.now_ms(),
        };
        self.engine
            .store()?
            .set_mapping(
                &mapping.path,
                &mapping.provider,
                &mapping.account_id,
                mapping.added_at,
            )
            .map_err(|e| match e {
                // A `remove` raced this command.
                StoreError::NoSuchAccount => EngineError::NoSuchAccount(account.to_owned()),
                e => e.into(),
            })?;
        let human = format!(
            "Mapped {} to {} (position {}).\n",
            mapping.path,
            render::name(&row),
            row.position
        );
        let mapping = render::mapping_json(&mapping, &row);
        let json = json!({"schemaVersion": 1, "ok": true, "mapping": mapping});
        self.print(&human, json);
        Ok(())
    }

    /// §12.7: removes PATH's mappings (default: the current directory), only `--provider`'s
    /// when it is given. Removing nothing is not an error, and the store is never created.
    pub(in crate::app) fn unmap(&mut self, path: Option<PathBuf>) -> Result<(), Failure> {
        let path = unmapped_path(path)?;
        let removed = match self.engine.existing_store()? {
            Some(store) => store
                .remove_mappings(&path, self.provider_flag.as_ref())
                .map_err(EngineError::from)?,
            None => 0,
        };
        let human = if removed == 0 {
            format!("No mapping for {path}.\n")
        } else {
            format!("Unmapped {path}.\n")
        };
        let json = json!({"schemaVersion": 1, "ok": true, "path": path, "removed": removed});
        self.print(&human, json);
        Ok(())
    }

    /// §6.3: the listing, or `--purge`'s deletions once confirmed. Every ID is checked before
    /// the question, so an unknown one fails without asking. A person is asked on a terminal.
    /// Anywhere else, and under `--json`, `--yes` is required.
    pub(in crate::app) fn displaced(
        &mut self,
        purge: Vec<String>,
        yes: bool,
    ) -> Result<(), Failure> {
        if purge.is_empty() {
            if yes {
                return Err(Failure::Usage(YES_WITHOUT_PURGE.into()));
            }
            let list = self.engine.displaced()?;
            let human = displaced_cmd::human(&list, &|e| displaced_cmd::identity(&self.engine, e));
            self.print(&human, displaced_cmd::json(&list));
            return Ok(());
        }
        let ids = self.engine.known_displaced(&purge)?;
        if !yes {
            if !self.can_prompt() {
                return Err(Failure::Message(
                    KIND_NEEDS_CONFIRMATION,
                    PURGE_NEEDS_YES.into(),
                ));
            }
            let question = displaced_cmd::purge_question(ids.len());
            let confirmed = self.io.prompter.confirm(&question, false);
            self.after_prompt()?;
            if !confirmed {
                return Err(cancelled());
            }
        }
        let deleted = self.engine.purge_displaced(&ids)?;
        self.print(
            &displaced_cmd::purged_human(&deleted),
            displaced_cmd::purged_json(&deleted),
        );
        Ok(())
    }
}
