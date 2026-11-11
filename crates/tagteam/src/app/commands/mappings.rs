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

/// What launching in an entry's directory does (config-mappings spec §3.6): a session as one
/// account, or plain.
#[derive(Clone, Copy)]
enum Outcome<'a> {
    Names(&'a AccountRow),
    Plain,
}

impl<'a> Outcome<'a> {
    /// A session as the entry's account, when it names exactly one that can start one. A missing
    /// directory does not change who it names: it launches as that account once it exists.
    /// Every other problem is plain, as is no entry.
    fn of(entry: Option<&'a ConfigMapping>) -> Self {
        match entry {
            Some(m) if matches!(m.problem, None | Some(MappingProblem::MissingDirectory)) => {
                m.account.as_ref().map_or(Outcome::Plain, Outcome::Names)
            }
            _ => Outcome::Plain,
        }
    }

    fn differs(self, other: Self) -> bool {
        match (self, other) {
            (Outcome::Names(a), Outcome::Names(b)) => a.id != b.id,
            (Outcome::Plain, Outcome::Plain) => false,
            _ => true,
        }
    }
}

/// Warnings for the file entries whose launch changed between `before` and `after`, when `before`
/// named an account or more than one (config-mappings spec §3.6): the entry's [`Outcome`] differs.
/// Each says what the entry named and what it does now. Entries are matched by key and value, as
/// written. `launch` is the provider's launch command, which runs plain where an entry does not
/// name a session's account.
pub(in crate::app) fn mapping_warnings(
    before: &[ConfigMapping],
    after: &[ConfigMapping],
    file: &Path,
    launch: &str,
) -> Vec<String> {
    let account = |a: &AccountRow| format!("{} (position {})", render::name(a), a.position);
    before
        .iter()
        .filter_map(|was| {
            let now = after
                .iter()
                .find(|m| m.entry.written == was.entry.written && m.entry.value == was.entry.value);
            let (was_outcome, now_outcome) = (Outcome::of(Some(was)), Outcome::of(now));
            if !was_outcome.differs(now_outcome)
                || (was.account.is_none() && was.problem != Some(MappingProblem::Ambiguous))
            {
                return None;
            }
            let named = match (was_outcome, &was.account) {
                (Outcome::Names(a), _) => format!("no longer names {}", account(a)),
                (Outcome::Plain, Some(a)) => {
                    format!("named {}, which did not start a session", account(a))
                }
                (Outcome::Plain, None) => "named more than one account".to_owned(),
            };
            let outcome = match now_outcome {
                Outcome::Names(a) => format!("it now names {}", account(a)),
                Outcome::Plain => format!("`{launch}` runs plain there, with a warning"),
            };
            Some(format!(
                "{}: provider.{}.mappings \"{}\" = \"{}\" {named}; {outcome}",
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
            named: Named::Applies(row.clone()),
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

    /// `entry(position, email)` with `problem`; the account is dropped when `named` is false.
    fn judged(position: u32, named: bool, problem: Option<MappingProblem>) -> ConfigMapping {
        let mut m = entry(
            position,
            &format!("{}@x.co", (b'a' + position as u8 - 1) as char),
        );
        if !named {
            m.account = None;
        }
        if let Some(problem) = problem {
            m.named = Named::DoesNot {
                account: m.account.clone(),
                problem,
            };
        }
        m.problem = problem;
        m
    }

    #[test]
    fn a_mapping_warns_when_what_it_launches_changes_and_says_what_it_does_now() {
        use MappingProblem::{Ambiguous, ApiKey, Conflict, MissingDirectory, NoAccount};
        // (name, before, after, words the warning has, or None for silence)
        let table: Vec<(&str, ConfigMapping, ConfigMapping, Option<&str>)> = vec![
            (
                "applies A, then none",
                judged(1, true, None),
                judged(1, false, Some(NoAccount)),
                Some("`fakeagent` runs plain there, with a warning"),
            ),
            (
                "applies A, then applies B",
                judged(1, true, None),
                judged(2, true, None),
                Some("it now names b@x.co (position 2)"),
            ),
            (
                "missing directory A, then missing directory B",
                judged(1, true, Some(MissingDirectory)),
                judged(2, true, Some(MissingDirectory)),
                Some("it now names b@x.co (position 2)"),
            ),
            (
                "API key A, then applies B",
                judged(1, true, Some(ApiKey)),
                judged(2, true, None),
                Some("it now names b@x.co (position 2)"),
            ),
            (
                "API key A, then API key B",
                judged(1, true, Some(ApiKey)),
                judged(2, true, Some(ApiKey)),
                None,
            ),
            (
                "conflict, then conflict",
                judged(1, true, Some(Conflict)),
                judged(2, true, Some(Conflict)),
                None,
            ),
            (
                "ambiguous, then applies B",
                judged(1, false, Some(Ambiguous)),
                judged(2, true, None),
                Some("it now names b@x.co (position 2)"),
            ),
            (
                "ambiguous, then API key",
                judged(1, false, Some(Ambiguous)),
                judged(2, true, Some(ApiKey)),
                None,
            ),
            (
                "applies A, then applies A",
                judged(1, true, None),
                judged(1, true, None),
                None,
            ),
            (
                "API key A, then API key A",
                judged(1, true, Some(ApiKey)),
                judged(1, true, Some(ApiKey)),
                None,
            ),
        ];
        for (name, before, after, says) in table {
            let warnings = mapping_warnings(
                &[before],
                &[after],
                Path::new("/c/config.toml"),
                "fakeagent",
            );
            match says {
                Some(words) => {
                    assert_eq!(warnings.len(), 1, "{name}: {warnings:?}");
                    assert!(warnings[0].contains(words), "{name}: {warnings:?}");
                }
                None => assert!(warnings.is_empty(), "{name}: {warnings:?}"),
            }
        }
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
