//! Judging `config.toml`'s directory mappings against this machine's directories and accounts
//! (config-mappings spec §3.1, §3.2, §3.4). The read (`settings`) keeps each entry as written;
//! everything that depends on this machine is decided here, at lookup, so one dotfile can serve
//! several machines.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use tagteam_core::ProviderId;
use tagteam_core::validate::{AccountRefInput, parse_account_ref};
use tagteam_provider::Provider;

use crate::engine::Engine;
use crate::error::EngineError;
use crate::refs::text_candidates;
use crate::settings::{MappingEntry, Settings};
use crate::store::{AccountRow, Store, StoreError};

/// A mapping's directory as this machine has it (config-mappings spec §3.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MappedDir {
    /// The directory exists: its canonical path, rebuilt from its components.
    Found(PathBuf),
    /// It does not exist here, or is not a directory: the expanded path as given.
    Missing(PathBuf),
}

impl MappedDir {
    pub fn path(&self) -> &Path {
        match self {
            MappedDir::Found(p) | MappedDir::Missing(p) => p,
        }
    }
}

/// The one directory comparison every caller of a file mapping uses (config-mappings spec
/// §3.2, §3.3): `dir` canonicalised, when that names a directory. Anything else, a path that is
/// gone or a file, is `Missing`.
pub fn mapped_dir(dir: &Path) -> MappedDir {
    match fs::canonicalize(dir) {
        Ok(canonical) if canonical.is_dir() => MappedDir::Found(canonical.components().collect()),
        _ => MappedDir::Missing(dir.to_path_buf()),
    }
}

/// Why a file entry does not apply here (config-mappings spec §3.2). `judge` reports the first
/// that holds: a missing directory, a conflict, a position, then what the value names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MappingProblem {
    MissingDirectory,
    NoAccount,
    Ambiguous,
    ApiKey,
    Position,
    Conflict,
}

impl MappingProblem {
    /// The `--json` and doctor spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            MappingProblem::MissingDirectory => "missing-directory",
            MappingProblem::NoAccount => "no-account",
            MappingProblem::Ambiguous => "ambiguous",
            MappingProblem::ApiKey => "api-key",
            MappingProblem::Position => "position",
            MappingProblem::Conflict => "conflict",
        }
    }

    /// A phrase that completes "the mapping …", for the run warning, the listing and doctor.
    pub fn describe(self) -> &'static str {
        match self {
            MappingProblem::MissingDirectory => "names a directory that does not exist here",
            MappingProblem::NoAccount => "names no account here",
            MappingProblem::Ambiguous => "names more than one account",
            MappingProblem::ApiKey => "names an API-key account, which never starts a session",
            MappingProblem::Position => "is a position, which a mapping never names",
            MappingProblem::Conflict => {
                "shares its directory with another entry that names a different account"
            }
        }
    }
}

/// One file entry, judged on this machine (config-mappings spec §3.4).
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigMapping {
    pub provider: ProviderId,
    pub entry: MappingEntry,
    pub dir: MappedDir,
    /// Set whenever the value names exactly one account, whatever the problem.
    pub account: Option<AccountRow>,
    /// `None` when the entry applies.
    pub problem: Option<MappingProblem>,
}

/// Judges `entries`, in file order. Entries whose directories canonicalise alike are
/// equivalent: they conflict, all of them, when their values (as written) differ.
pub(crate) fn judge(
    store: Option<&Store>,
    p: &dyn Provider,
    entries: &[MappingEntry],
) -> Result<Vec<ConfigMapping>, StoreError> {
    let dirs: Vec<MappedDir> = entries.iter().map(|e| mapped_dir(&e.dir)).collect();
    let mut values: HashMap<PathBuf, HashSet<&str>> = HashMap::new();
    for (entry, dir) in entries.iter().zip(&dirs) {
        if let MappedDir::Found(path) = dir {
            values.entry(path.clone()).or_default().insert(&entry.value);
        }
    }
    let provider = p.id();
    entries
        .iter()
        .zip(dirs)
        .map(|(entry, dir)| {
            let (account, named) = resolve_value(store, p, &entry.value)?;
            let conflict = matches!(&dir, MappedDir::Found(path) if values[path].len() > 1);
            let problem = if matches!(dir, MappedDir::Missing(_)) {
                Some(MappingProblem::MissingDirectory)
            } else if conflict {
                Some(MappingProblem::Conflict)
            } else {
                named
            };
            Ok(ConfigMapping {
                provider: provider.clone(),
                entry: entry.clone(),
                dir,
                account,
                problem,
            })
        })
        .collect()
}

/// What `value` names for `p` (config-mappings spec §3.1): an account when it names exactly one,
/// and the problem when it does not apply. A position is never resolved, so it reads no store.
pub(crate) fn resolve_value(
    store: Option<&Store>,
    p: &dyn Provider,
    value: &str,
) -> Result<(Option<AccountRow>, Option<MappingProblem>), StoreError> {
    let text = match parse_account_ref(value) {
        Some(AccountRefInput::Position(_)) => return Ok((None, Some(MappingProblem::Position))),
        Some(AccountRefInput::Text(text)) => text,
        None => return Ok((None, Some(MappingProblem::NoAccount))),
    };
    let Some(store) = store else {
        return Ok((None, Some(MappingProblem::NoAccount)));
    };
    let mut found = text_candidates(store, &text, Some(&p.id()))?;
    Ok(match found.len() {
        0 => (None, Some(MappingProblem::NoAccount)),
        1 => {
            let row = found.remove(0);
            let problem = p
                .kind_traits(&row.kind)
                .managed_key_axis
                .then_some(MappingProblem::ApiKey);
            (Some(row), problem)
        }
        _ => (None, Some(MappingProblem::Ambiguous)),
    })
}

impl Engine {
    /// `provider`'s file entries as `config.toml` holds them now, judged against this machine
    /// (config-mappings spec §3.2). Reads the file afresh, drops the read's warnings and never
    /// creates a store.
    pub fn config_mappings(
        &self,
        provider: &ProviderId,
    ) -> Result<Vec<ConfigMapping>, EngineError> {
        let p = self.provider(provider)?;
        let entries = Settings::load(&self.env, provider).0.mappings;
        let store = self.existing_store()?;
        Ok(judge(store.as_deref(), &*p, &entries)?)
    }

    /// What `value` would name for `provider`, for `map` to check before it writes
    /// (config-mappings spec §3.3). Never creates a store.
    pub fn judge_value(
        &self,
        provider: &ProviderId,
        value: &str,
    ) -> Result<(Option<AccountRow>, Option<MappingProblem>), EngineError> {
        let p = self.provider(provider)?;
        let store = self.existing_store()?;
        Ok(resolve_value(store.as_deref(), &*p, value)?)
    }
}
