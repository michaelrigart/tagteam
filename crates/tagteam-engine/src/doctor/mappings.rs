use std::collections::BTreeMap;
use std::path::PathBuf;

use tagteam_provider::{Check, Provider};

use crate::mappings::{ConfigMapping, MappingProblem, judge};
use crate::settings::{Settings, config_path};

use super::{Id, Run, command};

impl Run<'_> {
    /// Config-mappings spec §3.5, for `p`: the `config.toml` entries that do not apply here, and
    /// the database mappings left. With a store that cannot be read, values are not judged:
    /// only the entries whose directory is missing are reported, and `store.*` reports the store.
    pub(super) fn mappings(&mut self, p: &dyn Provider) {
        let id = p.id();
        let file = config_path(&self.engine.env);
        let usable = self.all_accounts().is_some();
        let entries = Settings::load(&self.engine.env, &id).0.mappings;
        // No store at all still judges values: on a fresh machine an entry names no account.
        let Ok(judged) = judge(self.store_ref(), p, &entries) else {
            return;
        };
        let mut conflicts: BTreeMap<PathBuf, Vec<&ConfigMapping>> = BTreeMap::new();
        let mut found = Vec::new();
        for m in &judged {
            let key = &m.entry.written;
            match m.problem {
                Some(MappingProblem::MissingDirectory) => found.push(
                    Check::info(
                        Id::MAPPINGS_DIRECTORY,
                        format!(
                            "{} maps `{key}`, a directory that does not exist here",
                            file.display()
                        ),
                    )
                    .fix(format!(
                        "if no machine uses it, remove the entry by hand or run `{}`",
                        command(self.engine, &id, &format!("unmap {key}"))
                    )),
                ),
                _ if !usable => {}
                Some(MappingProblem::Conflict) => {
                    conflicts
                        .entry(m.dir.path().to_owned())
                        .or_default()
                        .push(m);
                }
                Some(
                    problem @ (MappingProblem::NoAccount
                    | MappingProblem::Ambiguous
                    | MappingProblem::ApiKey
                    | MappingProblem::Position),
                ) => found.push(
                    Check::warn(
                        Id::MAPPINGS_VALUE,
                        format!(
                            "{} maps `{key}` to `{}`, which {}; `claude` runs plain there",
                            file.display(),
                            m.entry.value,
                            problem.describe()
                        ),
                    )
                    .fix(format!(
                        "`{}` writes a value that resolves",
                        command(self.engine, &id, &format!("map ACCOUNT {key}"))
                    )),
                ),
                _ => {}
            }
        }
        for (dir, entries) in conflicts {
            let named: Vec<String> = entries
                .iter()
                .map(|m| format!("`{}` = `{}`", m.entry.written, m.entry.value))
                .collect();
            found.push(
                Check::warn(
                    Id::MAPPINGS_CONFLICT,
                    format!(
                        "{} maps {} entries to {} with different values: {}; none applies",
                        file.display(),
                        named.len(),
                        dir.display(),
                        named.join(", ")
                    ),
                )
                .fix(format!(
                    "keep one entry, or run `{}`",
                    command(self.engine, &id, &format!("map ACCOUNT {}", dir.display()))
                )),
            );
        }
        if let Some(store) = self.store_ref() {
            let left = store
                .mappings()
                .map(|rows| rows.iter().filter(|r| r.provider == id).count())
                .unwrap_or(0);
            if left > 0 {
                found.push(
                    Check::info(
                        Id::MAPPINGS_DATABASE,
                        format!("{left} directory mapping(s) are still in the database"),
                    )
                    .fix(format!(
                        "`{}` moves each to config.toml",
                        command(self.engine, &id, "map")
                    )),
                );
            }
        }
        for check in found {
            self.push(Some(&id), check);
        }
    }
}
