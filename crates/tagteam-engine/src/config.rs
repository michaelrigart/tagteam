//! `tagteam config set` and `unset` (§6.4): strict edits of one entry of `config.toml`. They go
//! through `toml_edit`, so every comment and every other entry keeps its bytes. Reads stay in
//! `settings`, which never needs an engine. A write needs this engine (Decision 3) for two
//! things. Its registry: `default_provider` must name a provider there, and `run.share_extra`
//! may not name what those providers' profiles keep private. Its cancel token, which the
//! settings lock waits on.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tagteam_core::ProviderId;
use tagteam_provider::FlockGuard;
use tagteam_provider::atomic::{ensure_private_dir, write_atomic};
use toml_edit::{Array, Decor, DocumentMut, Item, RawString, Table, TableLike, Value as TomlValue};

use crate::engine::Engine;
use crate::error::EngineError;
use crate::hooks;
use crate::profiles::is_private;
use crate::settings::{Key, KeyKind, SettingsError, Value, config_path, resolve};

/// How long a write waits for the settings lock (§4.3, §6.4). Each poll of the wait is a
/// cancellation point (§14.1).
pub const SETTINGS_LOCK_TIMEOUT: Duration = Duration::from_secs(5);

/// The settings lock, under the data dir (§5): a standalone `flock`, outside the lock order.
const SETTINGS_LOCK: &str = "locks/config.lock";

/// What one `config set` or `config unset` did: §6.4's `{key, value, changed}`, and the file.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingsChange {
    /// The entry's dotted name in the file. A provider table's entry starts `provider.<id>.`,
    /// whether the command named it that way or with `--provider`.
    pub key: String,
    /// What `set` wrote, or found there already; `None` for `unset`.
    pub value: Option<Value>,
    /// Whether the file changed. When it did not, nothing was written or created.
    pub changed: bool,
    /// The settings file, as `config path` names it: a symlink is not resolved.
    pub path: PathBuf,
}

/// One entry of the file, resolved and checked against this engine's registry.
struct Entry {
    key: &'static Key,
    provider: Option<ProviderId>,
    /// The dotted name, as `SettingsChange::key` reports it.
    name: String,
    /// The tables that hold the entry, outermost first. A provider table's entry: `provider`,
    /// the provider's id, then the key's table. A global entry: the key's table, or none.
    tables: Vec<String>,
}

impl Engine {
    /// `config set NAME RAW` (§6.4), with `provider` from `--provider`. The value is checked
    /// as strictly as the registry declares, then against this engine's providers, and is
    /// never clamped. A value the file already holds, as a read takes it, writes nothing.
    pub fn config_set(
        &self,
        name: &str,
        provider: Option<&ProviderId>,
        raw: &str,
    ) -> Result<SettingsChange, EngineError> {
        let entry = self.settings_entry(name, provider)?;
        let value = entry
            .key
            .parse_arg(raw)
            .map_err(|reason| invalid(&entry, reason))?;
        self.check_value(&entry, &value)?;
        self.edit(entry, Some(value))
    }

    /// `config unset NAME` (§6.4): removes the entry, then each table that leaves empty, unless
    /// it holds a comment. An absent entry, or an absent file, changes nothing and succeeds.
    pub fn config_unset(
        &self,
        name: &str,
        provider: Option<&ProviderId>,
    ) -> Result<SettingsChange, EngineError> {
        let entry = self.settings_entry(name, provider)?;
        self.edit(entry, None)
    }

    /// `name` as the registry resolves it, with `flag` from `--provider`. `resolve` refuses a
    /// key no provider table may hold under either spelling (§6.4). A provider this engine does
    /// not register is refused as `--provider` is, with `unknown-provider`, in both spellings
    /// (Decision 4).
    fn settings_entry(&self, name: &str, flag: Option<&ProviderId>) -> Result<Entry, EngineError> {
        let resolved = resolve(name, flag)?;
        let key = resolved.key;
        let mut tables = Vec::new();
        let mut entry_name = String::new();
        if let Some(p) = &resolved.provider {
            self.provider(p)?;
            tables.extend(["provider".to_owned(), p.to_string()]);
            entry_name = format!("provider.{p}.");
        }
        tables.extend(key.table().map(str::to_owned));
        entry_name.push_str(key.name);
        Ok(Entry {
            key,
            provider: resolved.provider,
            name: entry_name,
            tables,
        })
    }

    /// The checks only the engine can make (§6.4 "Key-specific rules"):
    /// - `default_provider` must name a provider this build registers.
    /// - A `run.share_extra` name must not be one a profile keeps private (§12.2). A provider's
    ///   own entry is checked against that provider's list. The global entry reaches every
    ///   profile, so it is checked against the list of every provider that has sessions.
    fn check_value(&self, entry: &Entry, value: &Value) -> Result<(), SettingsError> {
        match (&entry.key.kind, value) {
            (KeyKind::Provider, Value::Str(id)) => {
                if self.registry.get(&ProviderId::new(id.as_str())).is_none() {
                    return Err(SettingsError::UnknownProvider(id.clone()));
                }
            }
            (KeyKind::ShareNames, Value::List(names)) => {
                for p in self.registry.all() {
                    let concerned = match &entry.provider {
                        Some(id) => p.id() == *id,
                        None => p.capabilities().sessions,
                    };
                    if !concerned {
                        continue;
                    }
                    let policy = p.share_policy(&self.env);
                    if let Some(private) = names.iter().find(|name| is_private(&policy, name)) {
                        return Err(invalid(
                            entry,
                            format!(
                                "{private} stays private to each {} profile, so it cannot be shared",
                                p.display_name()
                            ),
                        ));
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// One edit of the file, in two passes:
    /// 1. A first look without the lock. When the edit changes nothing it returns here, so it
    ///    creates nothing, not even the lock file (§5).
    /// 2. Otherwise, under the settings lock: the read, the edit, and the write. The atomic
    ///    writer replaces the file through a symlink to its target, keeps an existing file's
    ///    mode and creates a new one 0600 (§6.4).
    ///
    /// Both reads refuse a corrupt file, which is never written. A write is logged once the
    /// lock is released: nothing else is taken while it is held (§4.3).
    fn edit(&self, entry: Entry, value: Option<Value>) -> Result<SettingsChange, EngineError> {
        let path = config_path(&self.env);
        if apply(&entry, value.as_ref(), read(&path)?)?.is_none() {
            return Ok(SettingsChange {
                key: entry.name,
                value,
                changed: false,
                path,
            });
        }
        let changed = {
            let _lock = FlockGuard::lock(
                &self.env.data_dir().join(SETTINGS_LOCK),
                SETTINGS_LOCK_TIMEOUT,
                &self.env.cancel,
            )
            .map_err(SettingsError::Lock)?;
            let current = read(&path)?;
            hooks::point(self, "settings-read")?;
            match apply(&entry, value.as_ref(), current)? {
                None => false,
                Some(text) => {
                    ensure_private_dir(&self.env.config_dir()).map_err(SettingsError::Io)?;
                    write_atomic(&path, text.as_bytes(), 0o600).map_err(SettingsError::Io)?;
                    true
                }
            }
        };
        if changed {
            // The key alone: a value can be free text (`statusline.format`) that holds an email
            // or a name, which the log never records (§14.2).
            let action = if value.is_some() { "set" } else { "unset" };
            tracing::info!(key = %entry.name, action, "settings written");
        }
        Ok(SettingsChange {
            key: entry.name,
            value,
            changed,
            path,
        })
    }
}

/// Why `set` and `unset` refuse an entry that is a table, inline or not.
const HOLDS_A_TABLE: &str = "the settings file holds a table there, not a value";

fn invalid(entry: &Entry, reason: String) -> SettingsError {
    SettingsError::Invalid {
        key: entry.name.clone(),
        reason,
    }
}

/// The file's text and document, or `None` when there is no file. A file that is not UTF-8,
/// or not TOML, is `Corrupt`, and a strict write never touches it (§6.4).
fn read(path: &Path) -> Result<Option<(String, DocumentMut)>, SettingsError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(SettingsError::Io(e)),
    };
    let corrupt = |detail: String| SettingsError::Corrupt {
        path: path.to_path_buf(),
        detail,
    };
    let text = String::from_utf8(bytes).map_err(|_| corrupt("not UTF-8".to_owned()))?;
    let doc = text.parse::<DocumentMut>().map_err(|e| {
        let at = e.span().map_or(0, |span| span.start).min(text.len());
        let line = text.as_bytes()[..at]
            .iter()
            .filter(|&&b| b == b'\n')
            .count()
            + 1;
        let what = e.message().lines().next().unwrap_or_default();
        corrupt(format!("not valid TOML (line {line}: {what})"))
    })?;
    Ok(Some((text, doc)))
}

/// The file's new text after the edit, or `None` when the edit leaves the file as it is.
/// Three edits leave it as it is: a `set` of the value already there, as a read takes it; an
/// `unset` of an absent entry; any `unset` when there is no file.
fn apply(
    entry: &Entry,
    value: Option<&Value>,
    file: Option<(String, DocumentMut)>,
) -> Result<Option<String>, SettingsError> {
    let (before, mut doc) = match file {
        Some(file) => file,
        None if value.is_none() => return Ok(None),
        None => (String::new(), DocumentMut::new()),
    };
    let tables: Vec<&str> = entry.tables.iter().map(String::as_str).collect();
    let edited = match value {
        Some(value) => {
            let table = table_at(doc.as_table_mut(), &tables, 0).map_err(|segment| {
                invalid(
                    entry,
                    format!("`{segment}` in the settings file is not a table"),
                )
            })?;
            put(table, entry, value)?
        }
        None => {
            if entry_is_table(doc.as_table(), &tables, entry.key.leaf()) {
                return Err(invalid(entry, HOLDS_A_TABLE.to_owned()));
            }
            remove_entry(doc.as_table_mut(), &tables, entry.key.leaf())
        }
    };
    if !edited {
        return Ok(None);
    }
    let after = tidy(&before, doc.to_string());
    Ok((after != before).then_some(after))
}

/// The table at `path[at..]` under `table`, made where absent:
/// - `provider` and `provider.<id>` are made implicit, so they get no header of their own;
/// - the last table on the path is made explicit (§6.4);
/// - a table made under a dotted table is dotted too, and one made under an inline table is
///   inline, so the file keeps its style.
///
/// `Err` names the first segment that exists but is not a table.
fn table_at<'t>(
    table: &'t mut dyn TableLike,
    path: &[&str],
    at: usize,
) -> Result<&'t mut dyn TableLike, String> {
    let Some(&segment) = path.get(at) else {
        return Ok(table);
    };
    if !table.contains_key(segment) {
        let mut made = Table::new();
        made.set_implicit(at + 1 < path.len());
        made.set_dotted(table.is_dotted());
        table.insert(segment, Item::Table(made));
    }
    match table.get_mut(segment).and_then(Item::as_table_like_mut) {
        Some(child) => table_at(child, path, at + 1),
        None => Err(path[..=at].join(".")),
    }
}

/// Writes `value` as the entry's leaf in `table`. An entry already there keeps its decor: the
/// comments above its line stay with its key, and the comment at the end of its line with its
/// value. Returns `false` when the entry already holds `value`, as a read takes it.
fn put(table: &mut dyn TableLike, entry: &Entry, value: &Value) -> Result<bool, SettingsError> {
    let mut item = value.to_item();
    match table.get_mut(entry.key.leaf()) {
        Some(old) => {
            if entry.key.parse_item(old, &mut |_: String| {}).as_ref() == Some(value) {
                return Ok(false);
            }
            // An inline table is a value to toml_edit, but a table to the file's reader.
            let Some(decor) = old
                .as_value()
                .filter(|v| !v.is_inline_table())
                .map(|v| v.decor().clone())
            else {
                return Err(invalid(entry, HOLDS_A_TABLE.to_owned()));
            };
            if let Some(new) = item.as_value_mut() {
                *new.decor_mut() = decor;
                if let (Some(old), Some(new)) = (
                    old.as_value().and_then(TomlValue::as_array),
                    new.as_array_mut(),
                ) {
                    carry_array_decor(old, new);
                }
            }
            *old = item;
        }
        None => {
            table.insert(entry.key.leaf(), item);
        }
    }
    Ok(true)
}

/// The text in an array between two of its elements, split at the first line break: what
/// precedes it is on the line of the element or bracket before it.
struct Gap {
    same_line: String,
    breaks: bool,
    rest: String,
}

impl Gap {
    fn new(text: &str) -> Self {
        match text.split_once('\n') {
            Some((same_line, rest)) => Gap {
                same_line: same_line.to_owned(),
                breaks: true,
                rest: rest.to_owned(),
            },
            None => Gap {
                same_line: String::new(),
                breaks: false,
                rest: text.to_owned(),
            },
        }
    }
}

fn raw(text: Option<&RawString>) -> &str {
    text.and_then(RawString::as_str).unwrap_or_default()
}

/// Lays `new`, a list of strings, out as `old` was, so a replaced list keeps its comments (§6.4):
/// - an element also in `old` keeps its own comments, matched by value, in order, first unused
///   one; the comment at the end of its line (which toml_edit keeps with what follows it) moves
///   with it;
/// - a new element gets the layout of `old`'s last element;
/// - a comment above the closing bracket stays, and so does the trailing comma;
/// - a removed element's comments go with it.
fn carry_array_decor(old: &Array, new: &mut Array) {
    let n = old.len();
    let trailing_comma = old.trailing_comma();
    // gaps[k] sits before element k, and gaps[n] before the closing bracket.
    let mut gaps: Vec<Gap> = old
        .iter()
        .map(|v| Gap::new(raw(v.decor().prefix())))
        .collect();
    let mut own_suffix: Vec<&str> = old.iter().map(|v| raw(v.decor().suffix())).collect();
    gaps.push(Gap::new(match (n, trailing_comma) {
        (0, _) | (_, true) => old.trailing().as_str().unwrap_or_default(),
        _ => std::mem::take(&mut own_suffix[n - 1]),
    }));

    let mut used = vec![false; n];
    let matched: Vec<Option<usize>> = new
        .iter()
        .map(|v| {
            let name = v.as_str()?;
            let at = (0..n)
                .find(|&i| !used[i] && old.get(i).and_then(TomlValue::as_str) == Some(name))?;
            used[at] = true;
            Some(at)
        })
        .collect();

    // The indentation a gap's text ends in: whitespace alone, never the comments above it.
    let indent = |text: &str| text.rsplit('\n').next().unwrap_or_default().to_owned();
    // Where a new element goes: the line breaks and indentation of the old list, no comment.
    let fresh = |at: usize| match (n, at) {
        (0, 0) => (false, String::new()),
        (0, _) => (false, " ".to_owned()),
        (_, 0) => (gaps[0].breaks, indent(&gaps[0].rest)),
        _ if gaps[n - 1].breaks => (true, indent(&gaps[n - 1].rest)),
        _ if n >= 2 => (false, indent(&gaps[n - 1].rest)),
        _ => (false, " ".to_owned()),
    };
    let layout = |at: usize, matched: Option<usize>| match matched {
        Some(i) => (gaps[i].breaks, gaps[i].rest.clone()),
        None => fresh(at),
    };
    // What precedes position `at`: the comment ending the previous line, the break, the lead.
    let before = |at: usize| {
        let same_line = match at.checked_sub(1) {
            Some(prev) => matched[prev].map_or("", |i| gaps[i + 1].same_line.as_str()),
            None => gaps[0].same_line.as_str(),
        };
        (same_line, !same_line.is_empty())
    };

    let count = matched.len();
    let (same_line, forced) = before(count);
    let closing_breaks = forced || gaps[n].breaks;
    let closing = format!(
        "{same_line}{}{}",
        if closing_breaks { "\n" } else { "" },
        gaps[n].rest
    );
    let trailing_comma = trailing_comma && count > 0;
    for (at, value) in new.iter_mut().enumerate() {
        let (same_line, forced) = before(at);
        let (breaks, lead) = layout(at, matched[at]);
        let prefix = format!(
            "{same_line}{}{lead}",
            if breaks || forced { "\n" } else { "" },
        );
        let mut suffix = matched[at].map_or("", |i| own_suffix[i]).to_owned();
        if at + 1 == count && !trailing_comma {
            suffix.push_str(&closing);
        }
        let decor = value.decor_mut();
        decor.set_prefix(prefix);
        decor.set_suffix(suffix);
    }
    new.set_trailing_comma(trailing_comma);
    if trailing_comma || (count == 0 && closing.contains('#')) {
        new.set_trailing(closing);
    }
}

/// Whether the entry `leaf` in the table at `path` under `table` is itself a table: a table, an
/// inline table or an array of tables. `unset` refuses one rather than delete what it holds.
fn entry_is_table(table: &dyn TableLike, path: &[&str], leaf: &str) -> bool {
    let mut current = table;
    for segment in path {
        match current.get(segment).and_then(Item::as_table_like) {
            Some(child) => current = child,
            None => return false,
        }
    }
    current
        .get(leaf)
        .is_some_and(|item| item.is_table_like() || item.is_array_of_tables())
}

/// Removes `leaf` from the table at `path` under `table`, then each table on `path` the
/// removal leaves empty, deepest first, unless that table holds a comment. Returns whether
/// `leaf` was there.
fn remove_entry(table: &mut dyn TableLike, path: &[&str], leaf: &str) -> bool {
    let Some((&first, rest)) = path.split_first() else {
        return table.remove(leaf).is_some();
    };
    let Some(child) = table.get_mut(first).and_then(Item::as_table_like_mut) else {
        return false;
    };
    let removed = remove_entry(child, rest, leaf);
    let emptied = removed && child.is_empty();
    if emptied && !holds_comment(table, first) {
        table.remove(first);
    }
    removed
}

/// Whether the table `name` in `parent` holds a comment (§6.4). Where the comment may sit
/// depends on how the table is written:
/// - a `[header]` table: above its header, or at the end of the header's line;
/// - an inline table: above its line, or at the end of it.
///
/// A comment between keys belongs to the key below it, so an empty table holds no other.
fn holds_comment(parent: &dyn TableLike, name: &str) -> bool {
    let own = match parent.get(name) {
        Some(Item::Table(table)) => Some(table.decor()),
        Some(Item::Value(value)) => Some(value.decor()),
        _ => None,
    };
    let key = parent.key(name).map(|key| key.leaf_decor());
    own.into_iter().chain(key).any(has_comment)
}

fn has_comment(decor: &Decor) -> bool {
    [decor.prefix(), decor.suffix()]
        .into_iter()
        .flatten()
        .any(|raw| raw.as_str().is_some_and(|text| text.contains('#')))
}

/// The new text, without a blank first line that the file did not have. Removing the first
/// table would otherwise leave the next table's leading blank line at the top.
fn tidy(before: &str, after: String) -> String {
    if before.starts_with('\n') {
        after
    } else {
        after.trim_start_matches('\n').to_owned()
    }
}
