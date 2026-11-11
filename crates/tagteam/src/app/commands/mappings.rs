use std::path::{Path, PathBuf};

use serde_json::json;
use tagteam_core::ProviderId;
use tagteam_engine::EngineError;
use tagteam_engine::mappings::{ConfigMapping, MappingProblem, Named};
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

/// `unmap`'s PATH, twice: as given, made absolute and rebuilt from its components, for the file,
/// which compares it as it compares its keys; and canonical, as `map` stored it, or, for a path
/// that cannot be resolved (gone, or under something that is no longer a directory), the same
/// absolute path, so the path `map` listed still unmaps, for the database and the output.
fn unmapped_path(path: Option<PathBuf>) -> Result<(PathBuf, String), Failure> {
    let given = path.unwrap_or_else(|| PathBuf::from("."));
    let absolute: PathBuf = std::path::absolute(&given)
        .map_err(|e| invalid_path(&given, &e.to_string()))?
        .components()
        .collect();
    let resolved = std::fs::canonicalize(&given).unwrap_or_else(|_| absolute.clone());
    Ok((absolute, stored_path(resolved)?))
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

/// The command whose change to the accounts [`mapping_warnings`] reports (config-mappings spec
/// §3.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum AccountChange {
    /// `alias`: set, renamed or unset.
    Alias,
    Remove,
}

/// Warnings for the file entries whose value `change` moved off the account it named
/// (config-mappings spec §3.6), judged from the value alone, whether or not its directory exists
/// here:
/// - a value that named one account, an API-key account included, and now names another or none;
/// - for `remove`, a value that named more than one account, the removed one among them, and now
///   names one.
///
/// A value that names the same account as before warns about nothing. Each warning names the
/// file, the entry as written and what it named, and says what it does now as if its directory
/// existed. Entries are matched by key and value, as written, and one that has left the file is
/// not this command's doing. `launch` is the provider's launch command.
pub(in crate::app) fn mapping_warnings(
    change: AccountChange,
    before: &[ConfigMapping],
    after: &[ConfigMapping],
    file: &Path,
    launch: &str,
) -> Vec<String> {
    let who = |a: &AccountRow| format!("{} (position {})", render::name(a), a.position);
    let plain = format!("`{launch}` runs plain there");
    let conflict = "it conflicts with another entry for the same directory";
    before
        .iter()
        .filter_map(|was| {
            let now = after.iter().find(|m| {
                m.entry.written == was.entry.written && m.entry.value == was.entry.value
            })?;
            let ambiguous = matches!(
                was.named,
                Named::DoesNot {
                    problem: MappingProblem::Ambiguous,
                    ..
                }
            );
            let named = match (was.named.account(), now.named.account()) {
                (Some(a), Some(b)) if a.id == b.id => return None,
                (Some(a), _) => format!("no longer names {}", who(a)),
                (None, Some(_)) if ambiguous && change == AccountChange::Remove => {
                    "named more than one account".to_owned()
                }
                (None, _) => return None,
            };
            let (names, starts) = match &now.named {
                Named::Applies(b) => (Some(format!("it now names {}", who(b))), true),
                Named::DoesNot { account, .. } => (
                    account
                        .as_ref()
                        .map(|b| format!("it now names {}, an API-key account", who(b))),
                    false,
                ),
            };
            let does = match (names, now.problem == Some(MappingProblem::Conflict)) {
                (Some(names), false) if starts => names,
                (Some(names), false) => format!("{names}, so {plain}"),
                (None, false) => format!("{plain}, with a warning"),
                (Some(names), true) => format!("{names}, but {conflict}, so {plain}"),
                (None, true) => format!("{conflict}, so {plain}"),
            };
            Some(format!(
                "{}: provider.{}.mappings \"{}\" = \"{}\" {named}; {does}",
                file.display(),
                was.provider,
                was.entry.written,
                was.entry.value,
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

    /// Reads `provider`'s entries again and warns about each in `before` whose value `change`
    /// moved off the account it named (config-mappings spec §3.6, [`mapping_warnings`]).
    pub(in crate::app) fn warn_changed_mappings(
        &mut self,
        change: AccountChange,
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
        for w in mapping_warnings(change, before, &after, &file, p.launch_command()) {
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
        let named = self.engine.judge_value(&row.provider, &value)?;
        if matches!(named, Named::Applies(n) if n.id == row.id) {
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
        let (given, path) = unmapped_path(path)?;
        let from_file = self
            .engine
            .remove_config_mappings(self.provider_flag.as_ref(), &given)?;
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

    use tagteam_engine::mappings::{ConfigMapping, MappedDir, MappingProblem, Named};
    use tagteam_engine::settings::MappingEntry;
    use tagteam_engine::store::AccountRow;
    use tagteam_engine::views::UsageStatus;

    use super::{AccountChange, mapping_warnings};
    use crate::render::testutil::{OAUTH, unread, view};

    /// What the entry's value names in a case: account 1 is a@x.co, account 2 is b@x.co.
    #[derive(Clone, Copy)]
    enum Names {
        Applies(u32),
        ApiKey(u32),
        Nothing,
        Ambiguous,
    }

    fn account(position: u32) -> AccountRow {
        let email = ["a@x.co", "b@x.co"][position as usize - 1];
        view(
            position,
            email,
            OAUTH,
            unread(UsageStatus::Unavailable, None, None),
        )
        .row
    }

    /// The entry `"~/x" = "work"`, judged as the engine judges it: a missing directory first,
    /// then a conflict, which only an existing directory can have, then the value's problem.
    fn judged(names: Names, present: bool, conflict: bool) -> ConfigMapping {
        let none = |problem| Named::DoesNot {
            account: None,
            problem,
        };
        let named = match names {
            Names::Applies(n) => Named::Applies(account(n)),
            Names::ApiKey(n) => Named::DoesNot {
                account: Some(account(n)),
                problem: MappingProblem::ApiKey,
            },
            Names::Nothing => none(MappingProblem::NoAccount),
            Names::Ambiguous => none(MappingProblem::Ambiguous),
        };
        let path = PathBuf::from("/h/x");
        let (dir, problem) = match (&named, present, conflict) {
            (_, false, _) => (
                MappedDir::Missing(path),
                Some(MappingProblem::MissingDirectory),
            ),
            (_, true, true) => (MappedDir::Found(path), Some(MappingProblem::Conflict)),
            (Named::Applies(_), true, false) => (MappedDir::Found(path), None),
            (Named::DoesNot { problem, .. }, true, false) => {
                (MappedDir::Found(path), Some(*problem))
            }
        };
        ConfigMapping {
            provider: account(1).provider,
            entry: MappingEntry {
                written: "~/x".into(),
                dir: "/h/x".into(),
                value: "work".into(),
            },
            dir,
            account: named.account().cloned(),
            named,
            problem,
        }
    }

    #[test]
    fn alias_and_remove_warn_when_the_value_names_another_account_or_none_wherever_it_points() {
        use AccountChange::{Alias, Remove};
        use Names::{Ambiguous, ApiKey, Applies, Nothing};
        const BOTH: &[AccountChange] = &[Alias, Remove];
        const A: &str = "no longer names a@x.co (position 1)";
        const B: &str = "it now names b@x.co (position 2)";
        const B_KEY: &str =
            "it now names b@x.co (position 2), an API-key account, so `fakeagent` runs plain there";
        const PLAIN: &str = "`fakeagent` runs plain there, with a warning";
        const SEVERAL: &str = "named more than one account";
        // (name, commands, before, after, in a conflict, what it named and what it does now)
        type Case = (
            &'static str,
            &'static [AccountChange],
            Names,
            Names,
            bool,
            Option<(&'static str, &'static str)>,
        );
        let cases: [Case; 17] = [
            (
                "applies A, then A",
                BOTH,
                Applies(1),
                Applies(1),
                false,
                None,
            ),
            (
                "applies A, then B",
                BOTH,
                Applies(1),
                Applies(2),
                false,
                Some((A, B)),
            ),
            (
                "applies A, then API key B",
                BOTH,
                Applies(1),
                ApiKey(2),
                false,
                Some((A, B_KEY)),
            ),
            (
                "applies A, then none",
                BOTH,
                Applies(1),
                Nothing,
                false,
                Some((A, PLAIN)),
            ),
            ("API key A, then A", BOTH, ApiKey(1), ApiKey(1), false, None),
            (
                "API key A, then API key B",
                BOTH,
                ApiKey(1),
                ApiKey(2),
                false,
                Some((A, B_KEY)),
            ),
            (
                "API key A, then none",
                BOTH,
                ApiKey(1),
                Nothing,
                false,
                Some((A, PLAIN)),
            ),
            (
                "API key A, then applies B",
                BOTH,
                ApiKey(1),
                Applies(2),
                false,
                Some((A, B)),
            ),
            (
                "ambiguous, then applies B, as remove",
                &[Remove],
                Ambiguous,
                Applies(2),
                false,
                Some((SEVERAL, B)),
            ),
            (
                "ambiguous, then API key B, as remove",
                &[Remove],
                Ambiguous,
                ApiKey(2),
                false,
                Some((SEVERAL, B_KEY)),
            ),
            (
                "ambiguous, then applies B, as alias: it named no one account before",
                &[Alias],
                Ambiguous,
                Applies(2),
                false,
                None,
            ),
            (
                "ambiguous, then ambiguous",
                BOTH,
                Ambiguous,
                Ambiguous,
                false,
                None,
            ),
            (
                "none, then applies B",
                BOTH,
                Nothing,
                Applies(2),
                false,
                None,
            ),
            (
                "in conflict, applies A, then A",
                BOTH,
                Applies(1),
                Applies(1),
                true,
                None,
            ),
            (
                "in conflict, applies A, then none",
                BOTH,
                Applies(1),
                Nothing,
                true,
                Some((
                    A,
                    "it conflicts with another entry for the same directory, so `fakeagent` runs \
                     plain there",
                )),
            ),
            (
                "in conflict, applies A, then B",
                BOTH,
                Applies(1),
                Applies(2),
                true,
                Some((
                    A,
                    "it now names b@x.co (position 2), but it conflicts with another entry for \
                     the same directory, so `fakeagent` runs plain there",
                )),
            ),
            (
                "in conflict, API key A, then B",
                BOTH,
                ApiKey(1),
                Applies(2),
                true,
                Some((
                    A,
                    "it now names b@x.co (position 2), but it conflicts with another entry for \
                     the same directory, so `fakeagent` runs plain there",
                )),
            ),
        ];
        let file = Path::new("/c/config.toml");
        for (name, changes, before, after, conflict, says) in cases {
            for &change in changes {
                for present in [true, false] {
                    let warnings = mapping_warnings(
                        change,
                        &[judged(before, present, conflict)],
                        &[judged(after, present, conflict)],
                        file,
                        "fakeagent",
                    );
                    let at = format!(
                        "{name}, under {change:?}, directory {}: {warnings:?}",
                        if present { "present" } else { "missing" }
                    );
                    let Some((named, does)) = says else {
                        assert!(warnings.is_empty(), "{at}");
                        continue;
                    };
                    assert_eq!(warnings.len(), 1, "{at}");
                    let said = format!(
                        "/c/config.toml: provider.claude-code.mappings \"~/x\" = \"work\" {named}; "
                    );
                    assert!(warnings[0].starts_with(&said), "{at}");
                    // A missing directory is never in a conflict, so there a conflict case only
                    // warns as it does in one; the cases without one pin what it then says.
                    if present || !conflict {
                        assert_eq!(warnings[0], format!("{said}{does}"), "{at}");
                    }
                }
            }
        }
    }
}
