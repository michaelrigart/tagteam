//! Read-only `config.toml` (§6.4): the keys M2b and M3 consume. `tagteam config` and its writes
//! land in M5; this module only reads, and never fails: a missing file, a corrupt file and an
//! invalid value each fall back to the default, the last two with a warning for the caller to
//! print. Every warning names the full path of the settings file.

use std::io::ErrorKind;
use std::ops::RangeInclusive;
use std::path::PathBuf;
use std::time::SystemTime;

use tagteam_core::ProviderId;
use tagteam_core::autoswitch::Strategy;
use tagteam_provider::Env;
use toml_edit::{DocumentMut, Item, TableLike};

pub const DEFAULT_THRESHOLD: f64 = 90.0;
pub const DEFAULT_INTERVAL_SECONDS: i64 = 60;
pub const DEFAULT_COOLDOWN_SECONDS: i64 = 300;
pub const DEFAULT_HYSTERESIS_PCT: f64 = 10.0;
pub const DEFAULT_UNHEALTHY_TICKS: u32 = 3;
pub const DEFAULT_HISTORY_RETENTION_DAYS: u32 = 180;
pub const DEFAULT_STATUSLINE_FORMAT: &str = "{account} · 5h {5h}% · 7d {7d}%{stale}";

/// §6.4's valid ranges. A value in the file outside its range falls back to the default, with a
/// warning; a CLI flag is clamped into it.
pub const THRESHOLD_RANGE: RangeInclusive<f64> = 50.0..=99.9;
pub const INTERVAL_SECONDS_RANGE: RangeInclusive<i64> = 15..=3600;
pub const COOLDOWN_SECONDS_RANGE: RangeInclusive<i64> = 0..=86_400;
pub const HYSTERESIS_PCT_RANGE: RangeInclusive<f64> = 0.0..=50.0;
pub const UNHEALTHY_TICKS_RANGE: RangeInclusive<u32> = 1..=100;

/// The statusline placeholders §13.5 defines, without their braces. `{model:<name>}` is the one
/// parameterised placeholder; see [`is_statusline_placeholder`]. The `statusline.format` check
/// and the statusline renderer share this list.
pub const STATUSLINE_PLACEHOLDERS: &[&str] = &[
    "account", "position", "email", "5h", "7d", "5h_reset", "7d_reset", "spend", "stale",
];

/// The prefix of the parameterised `{model:<name>}` placeholder.
pub const STATUSLINE_MODEL_PREFIX: &str = "model:";

/// Whether `name`, the text between a placeholder's braces, is one of §13.5's placeholders:
/// one of [`STATUSLINE_PLACEHOLDERS`], or `model:` followed by a model name. A model name is
/// non-empty, equals its trimmed form (the renderer matches it as written) and holds no brace.
pub fn is_statusline_placeholder(name: &str) -> bool {
    STATUSLINE_PLACEHOLDERS.contains(&name)
        || name
            .strip_prefix(STATUSLINE_MODEL_PREFIX)
            .is_some_and(|model| {
                !model.is_empty() && model == model.trim() && !model.contains(['{', '}'])
            })
}

/// `ui.color`. `NO_COLOR`, `FORCE_COLOR` and `--no-color` are the CLI's to apply on top.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    Auto,
    Always,
    Never,
}

/// Every `autoswitch.*` key is read from the provider's own table first, then from
/// `[autoswitch]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// `autoswitch.threshold`: 50–99.9.
    pub threshold: f64,
    /// `autoswitch.interval_seconds`: 15–3600.
    pub interval_seconds: i64,
    /// `autoswitch.cooldown_seconds`: 0–86400.
    pub cooldown_seconds: i64,
    /// `autoswitch.hysteresis_pct`: 0–50.
    pub hysteresis_pct: f64,
    /// `autoswitch.strategy`: `best` or `consume-first`.
    pub strategy: Strategy,
    /// `autoswitch.include_api_key_accounts`.
    pub include_api_key_accounts: bool,
    /// `autoswitch.unhealthy_ticks`: 1–100.
    pub unhealthy_ticks: u32,
    /// `autoswitch.models`: model display names, or `all`.
    pub models: Vec<String>,
    /// `usage.history_retention_days`: 1–3650.
    pub history_retention_days: u32,
    /// `statusline.format`. The provider's own table first.
    pub statusline_format: String,
    /// `ui.color`.
    pub color: ColorMode,
    /// `run.share_extra`: entry names of the source home a profile shares besides the
    /// provider's allowlist (§6.4, §12.2). The provider's own table first. A name that is not
    /// an entry name (`is_share_name`) is dropped here with a warning; the link sync drops a
    /// known-private one, also with a warning.
    pub share_extra: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            threshold: DEFAULT_THRESHOLD,
            interval_seconds: DEFAULT_INTERVAL_SECONDS,
            cooldown_seconds: DEFAULT_COOLDOWN_SECONDS,
            hysteresis_pct: DEFAULT_HYSTERESIS_PCT,
            strategy: Strategy::Best,
            include_api_key_accounts: false,
            unhealthy_ticks: DEFAULT_UNHEALTHY_TICKS,
            models: Vec::new(),
            history_retention_days: DEFAULT_HISTORY_RETENTION_DAYS,
            statusline_format: DEFAULT_STATUSLINE_FORMAT.to_owned(),
            color: ColorMode::Auto,
            share_extra: Vec::new(),
        }
    }
}

/// One entry of the source home, as `run.share_extra` names it: not empty, not `.` or `..`,
/// and without a `/`. A dot inside a name is fine.
pub(crate) fn is_share_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains('/')
}

impl Settings {
    /// `env.config_dir()/config.toml`. Forgiving (§6.4): a missing file gives the defaults
    /// silently; a corrupt or unreadable file gives the defaults and one warning naming the
    /// path; an invalid value gives its default and one warning naming the path and the key.
    /// The rest of the file still applies.
    pub fn load(env: &Env, provider: &ProviderId) -> (Settings, Vec<String>) {
        let path = path(env);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == ErrorKind::NotFound => return (Settings::default(), Vec::new()),
            Err(e) => {
                let warning = format!(
                    "{}: cannot read the settings file ({}); using the defaults",
                    path.display(),
                    e.kind()
                );
                return (Settings::default(), vec![warning]);
            }
        };
        match text.parse::<DocumentMut>() {
            Ok(doc) => from_document(&doc, &path.display().to_string(), provider),
            Err(_) => {
                let warning = format!(
                    "{}: the settings file is not valid TOML; using the defaults",
                    path.display()
                );
                (Settings::default(), vec![warning])
            }
        }
    }

    /// `config.toml`'s modification time; `None` when the file is missing or its metadata
    /// cannot be read. A running `auto` re-reads the settings before a tick whenever this
    /// changes (§11.4). Read it before `load`, so a write landing between the two counts as
    /// one more change at the next check rather than going unseen.
    pub fn mtime(env: &Env) -> Option<SystemTime> {
        std::fs::metadata(path(env)).and_then(|m| m.modified()).ok()
    }
}

fn path(env: &Env) -> PathBuf {
    env.config_dir().join("config.toml")
}

/// §6.4's booleans: `true`, `false`, `1`, `0`, `yes` and `no`, exactly. The file also takes a
/// TOML boolean and the integers 1 and 0; a flag's value is the words alone.
pub fn parse_bool(s: &str) -> Option<bool> {
    match s {
        "true" | "1" | "yes" => Some(true),
        "false" | "0" | "no" => Some(false),
        _ => None,
    }
}

struct Reader<'a> {
    doc: &'a DocumentMut,
    /// The settings file's full path, which every warning starts with.
    path: &'a str,
    warnings: Vec<String>,
}

impl<'a> Reader<'a> {
    fn warn(&mut self, message: String) {
        if !self.warnings.contains(&message) {
            self.warnings.push(message);
        }
    }

    /// The table at `path`, or `None` when it is absent. A segment that exists but is not a
    /// table warns and reads as absent.
    fn table(&mut self, path: &[&str]) -> Option<&'a dyn TableLike> {
        let doc: &'a DocumentMut = self.doc;
        let mut current: &'a dyn TableLike = doc.as_table();
        for (depth, segment) in path.iter().enumerate() {
            let item = current.get(segment)?;
            match item.as_table_like() {
                Some(table) => current = table,
                None => {
                    let dotted = path[..=depth].join(".");
                    self.warn(format!(
                        "{}: `{dotted}` must be a table (ignored)",
                        self.path
                    ));
                    return None;
                }
            }
        }
        Some(current)
    }

    /// The first valid value of `key` across `tables`, most specific first. A present value
    /// that `parse` rejects warns, naming the key, and the next table is tried.
    fn read<T>(
        &mut self,
        tables: &[&[&str]],
        key: &str,
        expect: &str,
        parse: impl Fn(&Item) -> Option<T>,
    ) -> Option<T> {
        for path in tables {
            let Some(table) = self.table(path) else {
                continue;
            };
            let Some(item) = table.get(key) else {
                continue;
            };
            match parse(item) {
                Some(value) => return Some(value),
                None => {
                    let dotted = path
                        .iter()
                        .copied()
                        .chain([key])
                        .collect::<Vec<_>>()
                        .join(".");
                    self.warn(format!("{}: `{dotted}` {expect} (ignored)", self.path));
                }
            }
        }
        None
    }

    /// `run.share_extra` (§6.4): the first of `tables`, most specific first, that holds the
    /// key. Its value is a name or a list of names; any other value warns, and the next table
    /// is tried. Within a list, an item that is not a string, or not an entry name
    /// (`is_share_name`), warns, naming it, and is dropped; the rest stand, and a repeated name
    /// collapses into its first.
    fn share_extra(&mut self, tables: &[&[&str]]) -> Vec<String> {
        for path in tables {
            let Some(table) = self.table(path) else {
                continue;
            };
            let Some(item) = table.get("share_extra") else {
                continue;
            };
            let dotted = path
                .iter()
                .copied()
                .chain(["share_extra"])
                .collect::<Vec<_>>()
                .join(".");
            let items: Vec<Option<&str>> = match (item.as_str(), item.as_array()) {
                (Some(one), _) => vec![Some(one)],
                (None, Some(list)) => list.iter().map(|v| v.as_str()).collect(),
                (None, None) => {
                    self.warn(format!(
                        "{}: `{dotted}` must be an entry name or a list of entry names (ignored)",
                        self.path
                    ));
                    continue;
                }
            };
            let mut names: Vec<String> = Vec::new();
            for item in items {
                match item {
                    Some(name) if is_share_name(name) => {
                        if !names.iter().any(|n| n == name) {
                            names.push(name.to_owned());
                        }
                    }
                    Some(name) => self.warn(format!(
                        "{}: `{dotted}` entry {name:?} is not an entry name of the source home (ignored)",
                        self.path
                    )),
                    None => self.warn(format!(
                        "{}: `{dotted}` holds an item that is not a string (ignored)",
                        self.path
                    )),
                }
            }
            return names;
        }
        Vec::new()
    }
}

fn from_document(doc: &DocumentMut, path: &str, provider: &ProviderId) -> (Settings, Vec<String>) {
    let defaults = Settings::default();
    let mut reader = Reader {
        doc,
        path,
        warnings: Vec::new(),
    };
    let global_autoswitch: &[&str] = &["autoswitch"];
    let global_statusline: &[&str] = &["statusline"];
    let global_run: &[&str] = &["run"];
    let provider_autoswitch = ["provider", provider.as_str(), "autoswitch"];
    let provider_statusline = ["provider", provider.as_str(), "statusline"];
    let provider_run = ["provider", provider.as_str(), "run"];

    let autoswitch: &[&[&str]] = &[&provider_autoswitch[..], global_autoswitch];

    let threshold = reader
        .read(
            autoswitch,
            "threshold",
            "must be a number from 50 to 99.9",
            |item| number(item).filter(|v| THRESHOLD_RANGE.contains(v)),
        )
        .unwrap_or(defaults.threshold);
    let interval_seconds = reader
        .read(
            autoswitch,
            "interval_seconds",
            "must be a whole number of seconds from 15 to 3600",
            |item| {
                item.as_integer()
                    .filter(|v| INTERVAL_SECONDS_RANGE.contains(v))
            },
        )
        .unwrap_or(defaults.interval_seconds);
    let cooldown_seconds = reader
        .read(
            autoswitch,
            "cooldown_seconds",
            "must be a whole number of seconds from 0 to 86400",
            |item| {
                item.as_integer()
                    .filter(|v| COOLDOWN_SECONDS_RANGE.contains(v))
            },
        )
        .unwrap_or(defaults.cooldown_seconds);
    let hysteresis_pct = reader
        .read(
            autoswitch,
            "hysteresis_pct",
            "must be a number from 0 to 50",
            |item| number(item).filter(|v| HYSTERESIS_PCT_RANGE.contains(v)),
        )
        .unwrap_or(defaults.hysteresis_pct);
    let strategy = reader
        .read(
            autoswitch,
            "strategy",
            "must be \"best\" or \"consume-first\"",
            |item| Strategy::parse(item.as_str()?),
        )
        .unwrap_or(defaults.strategy);
    let include_api_key_accounts = reader
        .read(
            autoswitch,
            "include_api_key_accounts",
            "must be true, false, 1, 0, yes or no",
            parse_bool_item,
        )
        .unwrap_or(defaults.include_api_key_accounts);
    let unhealthy_ticks = reader
        .read(
            autoswitch,
            "unhealthy_ticks",
            "must be a whole number of ticks from 1 to 100",
            |item| {
                u32::try_from(item.as_integer()?)
                    .ok()
                    .filter(|n| UNHEALTHY_TICKS_RANGE.contains(n))
            },
        )
        .unwrap_or(defaults.unhealthy_ticks);
    let models = reader
        .read(
            autoswitch,
            "models",
            "must be a model name, a list of model names, or [\"all\"] alone",
            parse_models,
        )
        .unwrap_or(defaults.models);
    let history_retention_days = reader
        .read(
            &[&["usage"]],
            "history_retention_days",
            "must be a whole number of days from 1 to 3650",
            parse_retention,
        )
        .unwrap_or(defaults.history_retention_days);
    let format_expect = format!(
        "must be a non-empty string using only the placeholders {} and {{{STATUSLINE_MODEL_PREFIX}<name>}}, each closed",
        STATUSLINE_PLACEHOLDERS
            .iter()
            .map(|name| format!("{{{name}}}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let statusline_format = reader
        .read(
            &[&provider_statusline[..], global_statusline],
            "format",
            &format_expect,
            parse_format,
        )
        .unwrap_or(defaults.statusline_format);
    let color = reader
        .read(
            &[&["ui"]],
            "color",
            "must be \"auto\", \"always\" or \"never\"",
            parse_color,
        )
        .unwrap_or(defaults.color);
    let share_extra = reader.share_extra(&[&provider_run[..], global_run]);

    let settings = Settings {
        threshold,
        interval_seconds,
        cooldown_seconds,
        hysteresis_pct,
        strategy,
        include_api_key_accounts,
        unhealthy_ticks,
        models,
        history_retention_days,
        statusline_format,
        color,
        share_extra,
    };
    (settings, reader.warnings)
}

/// A float or an integer. A non-finite float is never in a range, so it is rejected there.
fn number(item: &Item) -> Option<f64> {
    item.as_float()
        .or_else(|| item.as_integer().map(|i| i as f64))
}

/// A TOML boolean, the integer 1 or 0, or one of `parse_bool`'s words.
fn parse_bool_item(item: &Item) -> Option<bool> {
    if let Some(b) = item.as_bool() {
        return Some(b);
    }
    match item.as_integer() {
        Some(1) => Some(true),
        Some(0) => Some(false),
        Some(_) => None,
        None => parse_bool(item.as_str()?),
    }
}

fn parse_retention(item: &Item) -> Option<u32> {
    u32::try_from(item.as_integer()?)
        .ok()
        .filter(|days| (1..=3650).contains(days))
}

/// A model name or a list of them. Names are trimmed; a name repeated in another case collapses
/// into its first spelling. `all` (any case) cannot be mixed with names.
fn parse_models(item: &Item) -> Option<Vec<String>> {
    let names: Vec<String> = match item.as_str() {
        Some(one) => vec![name(one)?],
        None => item
            .as_array()?
            .iter()
            .map(|v| v.as_str().and_then(name))
            .collect::<Option<_>>()?,
    };
    let mut unique: Vec<String> = Vec::with_capacity(names.len());
    for candidate in names {
        if !unique
            .iter()
            .any(|n| n.to_lowercase() == candidate.to_lowercase())
        {
            unique.push(candidate);
        }
    }
    let mixed = unique.len() > 1 && unique.iter().any(|n| n.eq_ignore_ascii_case("all"));
    (!mixed).then_some(unique)
}

fn name(s: &str) -> Option<String> {
    let trimmed = s.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// A non-empty format whose every `{…}` is one of §13.5's placeholders. The scan is the
/// renderer's: a `{` runs to the next `}`, and an unclosed `{` is invalid.
fn parse_format(item: &Item) -> Option<String> {
    let s = item.as_str()?;
    if s.trim().is_empty() {
        return None;
    }
    let mut rest = s;
    while let Some(open) = rest.find('{') {
        let close = open + rest[open..].find('}')?;
        if !is_statusline_placeholder(&rest[open + 1..close]) {
            return None;
        }
        rest = &rest[close + 1..];
    }
    Some(s.to_owned())
}

fn parse_color(item: &Item) -> Option<ColorMode> {
    match item.as_str()? {
        "auto" => Some(ColorMode::Auto),
        "always" => Some(ColorMode::Always),
        "never" => Some(ColorMode::Never),
        _ => None,
    }
}
