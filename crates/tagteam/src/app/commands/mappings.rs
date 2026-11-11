use std::path::{Path, PathBuf};

use serde_json::json;
use tagteam_core::ProviderId;
use tagteam_engine::EngineError;
use tagteam_engine::mappings::ConfigMapping;
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

/// Warnings for the file entries that named an account in `before` and name another, or none,
/// in `after` (config-mappings spec §3.6). Entries are matched by key and value, as written.
/// `launch` is the provider's launch command, which runs plain where an entry names none.
pub(in crate::app) fn mapping_warnings(
    before: &[ConfigMapping],
    after: &[ConfigMapping],
    file: &Path,
    launch: &str,
) -> Vec<String> {
    before
        .iter()
        .filter_map(|was| {
            let named = was.account.as_ref()?;
            let now = after
                .iter()
                .find(|m| m.entry.written == was.entry.written && m.entry.value == was.entry.value)
                .and_then(|m| m.account.as_ref());
            let outcome = match now {
                Some(a) if a.id == named.id => return None,
                Some(a) => format!(
                    "it now names {} (position {})",
                    render::name(a),
                    a.position
                ),
                None => format!("`{launch}` runs plain there, with a warning"),
            };
            Some(format!(
                "{}: provider.{}.mappings \"{}\" = \"{}\" no longer names {} (position {}); {outcome}",
                file.display(),
                was.provider,
                was.entry.written,
                was.entry.value,
                render::name(named),
                named.position
            ))
        })
        .collect()
}

impl App<'_, '_> {
    /// `provider`'s file entries as they stand, for `alias` and `remove` to compare. Best-effort:
    /// a failed read gives none, which warns about nothing, and the command goes on.
    pub(in crate::app) fn mappings_snapshot(&self, provider: &ProviderId) -> Vec<ConfigMapping> {
        self.engine.config_mappings(provider).unwrap_or_default()
    }

    /// Reads `provider`'s entries again and warns about each in `before` that now names another
    /// account, or none (config-mappings spec §3.6).
    pub(in crate::app) fn warn_changed_mappings(
        &mut self,
        provider: &ProviderId,
        before: &[ConfigMapping],
    ) {
        if before.is_empty() {
            return;
        }
        // Not `mappings_snapshot`: an empty `after` would read as every entry gone.
        let Ok(after) = self.engine.config_mappings(provider) else {
            return;
        };
        let Ok(p) = self.engine.provider(provider) else {
            return;
        };
        let file = tagteam_engine::settings::config_path(self.engine.env());
        for w in mapping_warnings(before, &after, &file, p.launch_command()) {
            self.io.warn(&w);
        }
    }

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

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use tagteam_engine::mappings::{ConfigMapping, MappedDir, MappingProblem};
    use tagteam_engine::settings::MappingEntry;

    use super::mapping_warnings;
    use crate::render::testutil::{OAUTH, unread, view};
    use tagteam_engine::views::UsageStatus;

    fn entry(position: u32, email: &str) -> ConfigMapping {
        let row = view(
            position,
            email,
            OAUTH,
            unread(UsageStatus::Unavailable, None, None),
        )
        .row;
        ConfigMapping {
            provider: row.provider.clone(),
            entry: MappingEntry {
                written: "~/x".into(),
                dir: PathBuf::from("/h/x"),
                value: "work".into(),
            },
            dir: MappedDir::Found(PathBuf::from("/h/x")),
            account: Some(row),
            problem: None,
        }
    }

    #[test]
    fn an_entry_that_names_another_account_now_warns_with_its_position() {
        let warnings = mapping_warnings(
            &[entry(1, "a@x.co")],
            &[entry(2, "b@x.co")],
            Path::new("/c/config.toml"),
            "fakeagent",
        );
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].contains("it now names b@x.co (position 2)"),
            "{warnings:?}"
        );
        assert!(warnings[0].contains("\"~/x\" = \"work\""), "{warnings:?}");
        assert!(
            mapping_warnings(
                &[entry(1, "a@x.co")],
                &[entry(1, "a@x.co")],
                Path::new("/c"),
                "fakeagent"
            )
            .is_empty()
        );
    }

    #[test]
    fn an_entry_that_names_no_account_now_says_which_launch_command_runs_plain() {
        let mut gone = entry(1, "a@x.co");
        gone.account = None;
        gone.problem = Some(MappingProblem::NoAccount);
        let warnings = mapping_warnings(
            &[entry(1, "a@x.co")],
            &[gone],
            Path::new("/c/config.toml"),
            "fakeagent",
        );
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].contains("`fakeagent` runs plain there, with a warning"),
            "{warnings:?}"
        );
    }
}
