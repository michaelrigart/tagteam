use std::path::{Path, PathBuf};

use serde_json::json;
use tagteam_engine::EngineError;
use tagteam_engine::store::{AccountRow, Mapping};

use crate::SCHEMA_VERSION;
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
    /// §12.7: `map` with no arguments lists the mappings of both sources, only `--provider`'s
    /// when it is given (config-mappings spec §3.4). It never creates the store (§5).
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
        let providers = match &self.provider_flag {
            Some(p) => vec![p.clone()],
            None => self.engine.provider_ids(),
        };
        let mut config = Vec::new();
        for provider in &providers {
            config.extend(self.engine.config_mappings(provider)?);
        }
        self.print(
            &render::mappings_human(&rows, &config),
            render::mappings_json(&rows, &config),
        );
        Ok(())
    }

    /// §12.7, config-mappings spec §3.3: maps PATH (default: the current directory), canonical,
    /// for the account's own provider, in `config.toml`, replacing every entry for that
    /// directory and the database row, if any. The value is the account's alias, else its
    /// email, and must name this account alone.
    pub(in crate::app) fn map(
        &mut self,
        account: &str,
        path: Option<PathBuf>,
    ) -> Result<(), Failure> {
        let row = self.resolve(account)?;
        let dir = canonical_dir(path)?;
        let provider = self.engine.provider(&row.provider)?;
        if provider.kind_traits(&row.kind).managed_key_axis {
            return Err(Failure::Message(
                KIND_INVALID_INPUT,
                format!(
                    "{} is an API-key account, which has no session to map a directory to",
                    render::name(&row)
                ),
            ));
        }
        let value = self.mapping_value(&row)?;
        let mapping = Mapping {
            path: dir,
            provider: row.provider.clone(),
            account_id: row.id.clone(),
            added_at: self.engine.now_ms(),
        };
        let write = self
            .engine
            .write_mapping(&row.provider, Path::new(&mapping.path), &value)?;
        // The file is now the only source for this directory.
        if let Some(store) = self.engine.existing_store()? {
            store
                .remove_mappings(&mapping.path, Some(&row.provider))
                .map_err(EngineError::from)?;
        }
        let human = format!(
            "Mapped {} to {} (position {}) in {}.\n",
            write.written,
            render::name(&row),
            row.position,
            write.path.display()
        );
        let mut mapped = render::mapping_json(&mapping, &row);
        mapped["source"] = json!("config");
        let json = json!({"schemaVersion": SCHEMA_VERSION, "ok": true, "mapping": mapped});
        self.print(&human, json);
        Ok(())
    }

    /// What `map` writes for `row`: its alias, else its email, when that names this account and
    /// no other. Anything else is refused, saying to give the account an alias.
    fn mapping_value(&self, row: &AccountRow) -> Result<String, Failure> {
        let refuse = |why: String| {
            Failure::Message(
                KIND_INVALID_INPUT,
                format!(
                    "{why}; give it an alias first (`tagteam alias {} NAME`)",
                    row.position
                ),
            )
        };
        let Some(value) = row.alias.clone().or_else(|| row.email.clone()) else {
            return Err(refuse(format!(
                "{} has no alias or email to map by",
                render::name(row)
            )));
        };
        let (named, _) = self.engine.judge_value(&row.provider, &value)?;
        if named.is_some_and(|n| n.id == row.id) {
            Ok(value)
        } else {
            Err(refuse(format!(
                "\"{value}\" does not name {} alone",
                render::name(row)
            )))
        }
    }

    /// §12.7: removes PATH's mappings (default: the current directory) from `config.toml` and
    /// the database, only `--provider`'s when it is given. Removing nothing is not an error, and
    /// neither source is created.
    pub(in crate::app) fn unmap(&mut self, path: Option<PathBuf>) -> Result<(), Failure> {
        let path = unmapped_path(path)?;
        let from_file = self
            .engine
            .remove_config_mappings(self.provider_flag.as_ref(), Path::new(&path))?;
        let from_database = match self.engine.existing_store()? {
            Some(store) => store
                .remove_mappings(&path, self.provider_flag.as_ref())
                .map_err(EngineError::from)?,
            None => 0,
        };
        let removed = from_file + from_database;
        let human = if removed == 0 {
            format!("No mapping for {path}.\n")
        } else {
            format!("Unmapped {path}.\n")
        };
        let json =
            json!({"schemaVersion": SCHEMA_VERSION, "ok": true, "path": path, "removed": removed});
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
