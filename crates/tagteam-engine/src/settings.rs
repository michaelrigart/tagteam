//! `config.toml` (§6.4): the key registry and the forgiving reader. Every key is declared once,
//! in [`KEYS`], with its kind, its "must be …" phrase and whether a provider table may override
//! it. Reads, `config list` and `config get`, the strict writes and completions all go through
//! it, so none of them accepts a key or value another rejects.
//!
//! A read never fails: a missing file, a corrupt file and an invalid value each fall back to
//! the default, the last two with a warning for the caller to print. Every warning names the
//! full path of the settings file.

use std::io::ErrorKind;
use std::ops::RangeInclusive;
use std::path::PathBuf;
use std::time::SystemTime;

use tagteam_core::autoswitch::Strategy;
use tagteam_core::{CLAUDE_CODE, ProviderId};
use tagteam_provider::{Env, LockError};
use toml_edit::{DocumentMut, Item, TableLike};

pub const DEFAULT_THRESHOLD: f64 = 90.0;
pub const DEFAULT_INTERVAL_SECONDS: i64 = 60;
pub const DEFAULT_COOLDOWN_SECONDS: i64 = 300;
pub const DEFAULT_HYSTERESIS_PCT: f64 = 10.0;
pub const DEFAULT_UNHEALTHY_TICKS: u32 = 3;
pub const DEFAULT_HISTORY_RETENTION_DAYS: u32 = 180;
pub const DEFAULT_STATUSLINE_FORMAT: &str = "{account} · 5h {5h}% · 7d {7d}%{stale}";

/// §6.4's valid ranges. A value in the file outside its range falls back to the default, with a
/// warning; a CLI flag is clamped into it. The registry's bounds ([`KEYS`]) are these.
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

/// Entry names tagteam keeps for itself in a profile (§12.2): `run.share_extra` never names one.
const OWN_PREFIX: &str = ".tagteam-";

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

/// One entry of the source home, as `run.share_extra` names it: not empty, not `.` or `..`,
/// and without a `/`. A dot inside a name is fine.
pub fn is_share_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains('/')
}

/// A provider id as `default_provider` and a `provider.<id>` table name it: 1 to 64 lowercase
/// ASCII letters, digits and dashes, with no dash at either end (`claude-code`).
fn is_provider_id(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !s.starts_with('-')
        && !s.ends_with('-')
}

/// `ui.color`. `NO_COLOR`, `FORCE_COLOR` and `--no-color` are the CLI's to apply on top.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    Auto,
    Always,
    Never,
}

impl ColorMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ColorMode::Auto => "auto",
            ColorMode::Always => "always",
            ColorMode::Never => "never",
        }
    }
}

/// What a key holds: how it is read from the file, parsed from the command line and written.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum KeyKind {
    /// A number in `min..=max`. The file may hold it as an integer.
    Float { min: f64, max: f64 },
    /// A whole number in `min..=max`.
    Int { min: i64, max: i64 },
    /// In the file a TOML boolean, the integer 1 or 0, or one of [`parse_bool`]'s words as a
    /// string; on the command line the words alone. A write stores a TOML boolean.
    Bool,
    /// One of these strings, exactly.
    Choice(&'static [&'static str]),
    /// `autoswitch.models`: model display names, or `all` alone. A name repeated in another
    /// case collapses into its first spelling.
    Models,
    /// `statusline.format`: plain text and §13.5's placeholders.
    Format,
    /// `run.share_extra`: entry names of the source home (§12.2).
    ShareNames,
    /// `default_provider`: a well-formed provider id. Whether this build has that provider is
    /// the CLI's check.
    Provider,
}

/// One §6.4 key.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Key {
    /// The dotted name `config` takes: `autoswitch.threshold`, or `default_provider` at the top
    /// level.
    pub name: &'static str,
    pub kind: KeyKind,
    /// Whether a `[provider.<id>.<table>]` table may override it.
    pub per_provider: bool,
}

const STRATEGIES: &[&str] = &["best", "consume-first"];
const COLORS: &[&str] = &["auto", "always", "never"];

/// Every §6.4 key, in the order of the spec's table. The auto-switch bounds are M3b's
/// `*_RANGE` constants, which `auto` clamps its flags to.
pub const KEYS: &[Key] = &[
    Key {
        name: "default_provider",
        kind: KeyKind::Provider,
        per_provider: false,
    },
    Key {
        name: "autoswitch.threshold",
        kind: KeyKind::Float {
            min: *THRESHOLD_RANGE.start(),
            max: *THRESHOLD_RANGE.end(),
        },
        per_provider: true,
    },
    Key {
        name: "autoswitch.interval_seconds",
        kind: KeyKind::Int {
            min: *INTERVAL_SECONDS_RANGE.start(),
            max: *INTERVAL_SECONDS_RANGE.end(),
        },
        per_provider: true,
    },
    Key {
        name: "autoswitch.cooldown_seconds",
        kind: KeyKind::Int {
            min: *COOLDOWN_SECONDS_RANGE.start(),
            max: *COOLDOWN_SECONDS_RANGE.end(),
        },
        per_provider: true,
    },
    Key {
        name: "autoswitch.hysteresis_pct",
        kind: KeyKind::Float {
            min: *HYSTERESIS_PCT_RANGE.start(),
            max: *HYSTERESIS_PCT_RANGE.end(),
        },
        per_provider: true,
    },
    Key {
        name: "autoswitch.strategy",
        kind: KeyKind::Choice(STRATEGIES),
        per_provider: true,
    },
    Key {
        name: "autoswitch.include_api_key_accounts",
        kind: KeyKind::Bool,
        per_provider: true,
    },
    Key {
        name: "autoswitch.unhealthy_ticks",
        kind: KeyKind::Int {
            min: *UNHEALTHY_TICKS_RANGE.start() as i64,
            max: *UNHEALTHY_TICKS_RANGE.end() as i64,
        },
        per_provider: true,
    },
    Key {
        name: "autoswitch.models",
        kind: KeyKind::Models,
        per_provider: true,
    },
    Key {
        name: "usage.history_retention_days",
        kind: KeyKind::Int { min: 1, max: 3650 },
        per_provider: false,
    },
    Key {
        name: "statusline.format",
        kind: KeyKind::Format,
        per_provider: true,
    },
    Key {
        name: "run.share_extra",
        kind: KeyKind::ShareNames,
        per_provider: true,
    },
    Key {
        name: "ui.color",
        kind: KeyKind::Choice(COLORS),
        per_provider: false,
    },
];

/// The registry's key called `name`, a dotted name without any `provider.<id>.` prefix.
pub fn key(name: &str) -> Option<&'static Key> {
    KEYS.iter().find(|k| k.name == name)
}

impl Key {
    /// The table the key lives in: `autoswitch` for `autoswitch.threshold`, `None` for a key at
    /// the top level.
    pub fn table(&self) -> Option<&'static str> {
        let name: &'static str = self.name;
        name.rsplit_once('.').map(|(table, _)| table)
    }

    /// The key's name within its table: `threshold` for `autoswitch.threshold`.
    pub fn leaf(&self) -> &'static str {
        let name: &'static str = self.name;
        name.rsplit_once('.').map_or(name, |(_, leaf)| leaf)
    }

    /// What a valid value is, as every warning and refusal words it: "must be a number from 50
    /// to 99.9". The auto-switch keys' phrases are M3b's, which `auto`'s tests pin.
    pub fn expect(&self) -> String {
        match self.kind {
            KeyKind::Float { min, max } => format!("must be a number from {min} to {max}"),
            KeyKind::Int { min, max } => {
                let unit = if self.name.ends_with("_days") {
                    " of days"
                } else if self.name.ends_with("_seconds") {
                    " of seconds"
                } else if self.name.ends_with("_ticks") {
                    " of ticks"
                } else {
                    ""
                };
                format!("must be a whole number{unit} from {min} to {max}")
            }
            KeyKind::Bool => "must be true, false, 1, 0, yes or no".to_owned(),
            KeyKind::Choice(values) => format!("must be {}", one_of(values)),
            KeyKind::Models => {
                "must be a model name, a list of model names, or [\"all\"] alone".to_owned()
            }
            KeyKind::Format => format!(
                "must be a non-empty string using only the placeholders {} and {{{STATUSLINE_MODEL_PREFIX}<name>}}, each closed",
                STATUSLINE_PLACEHOLDERS
                    .iter()
                    .map(|name| format!("{{{name}}}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            KeyKind::ShareNames => "must be an entry name or a list of entry names".to_owned(),
            KeyKind::Provider => {
                "must be a provider id of lowercase letters, digits and dashes, such as \"claude-code\""
                    .to_owned()
            }
        }
    }

    /// The value `item` holds, read forgivingly from the file: `None` when it is not valid for
    /// this key, and the caller warns with [`Key::expect`] and tries the next table. Only
    /// `ShareNames` reports item by item: an item that is not an entry name is dropped through
    /// `warn`, which gets the text that follows the key in the warning, and the rest stand.
    pub fn parse_item(&self, item: &Item, warn: &mut dyn FnMut(String)) -> Option<Value> {
        match self.kind {
            KeyKind::Float { min, max } => {
                let n = item
                    .as_float()
                    .or_else(|| item.as_integer().map(|i| i as f64))?;
                (min..=max).contains(&n).then_some(Value::Float(n))
            }
            KeyKind::Int { min, max } => {
                let n = item.as_integer()?;
                (min..=max).contains(&n).then_some(Value::Int(n))
            }
            KeyKind::Bool => parse_bool_item(item).map(Value::Bool),
            KeyKind::Choice(values) => {
                let s = item.as_str()?;
                values.contains(&s).then(|| Value::Str(s.to_owned()))
            }
            KeyKind::Models => models_from_item(item).map(Value::List),
            KeyKind::Format => {
                let s = item.as_str()?;
                is_format(s).then(|| Value::Str(s.to_owned()))
            }
            KeyKind::ShareNames => share_names_from_item(item, warn).map(Value::List),
            KeyKind::Provider => {
                let s = item.as_str()?;
                is_provider_id(s).then(|| Value::Str(s.to_owned()))
            }
        }
    }

    /// A value typed on the command line, read strictly (§6.4). Numbers are taken as typed and
    /// never clamped. Booleans are `true/false/1/0/yes/no`. Lists are comma-separated, each item
    /// trimmed, with no empty item, and `''` is the empty list. `all` in `autoswitch.models`
    /// stands alone, and a `run.share_extra` item is not one of tagteam's own (`.tagteam-*`).
    /// `Err` is the reason, starting with "must be".
    pub fn parse_arg(&self, raw: &str) -> Result<Value, String> {
        match self.kind {
            KeyKind::Float { min, max } => raw
                .parse::<f64>()
                .ok()
                .filter(|n| (min..=max).contains(n))
                .map(Value::Float)
                .ok_or_else(|| self.expect()),
            KeyKind::Int { min, max } => raw
                .parse::<i64>()
                .ok()
                .filter(|n| (min..=max).contains(n))
                .map(Value::Int)
                .ok_or_else(|| self.expect()),
            KeyKind::Bool => parse_bool(raw)
                .map(Value::Bool)
                .ok_or_else(|| self.expect()),
            KeyKind::Choice(values) if values.contains(&raw) => Ok(Value::Str(raw.to_owned())),
            KeyKind::Choice(_) => Err(self.expect()),
            KeyKind::Models => {
                let names = list_arg(raw, "model names")?;
                collapse_models(names).map(Value::List).ok_or_else(|| {
                    "must be \"all\" alone, or model names without \"all\"".to_owned()
                })
            }
            KeyKind::Format if is_format(raw) => Ok(Value::Str(raw.to_owned())),
            KeyKind::Format => Err(self.expect()),
            KeyKind::ShareNames => {
                let names = list_arg(raw, "entry names")?;
                if let Some(bad) = names
                    .iter()
                    .find(|n| !is_share_name(n) || n.starts_with(OWN_PREFIX))
                {
                    return Err(format!(
                        "must be entry names of the source home, each without a `/`, not `.` or `..`, and not `{OWN_PREFIX}*`: {bad:?} is not one"
                    ));
                }
                let mut unique: Vec<String> = Vec::with_capacity(names.len());
                for name in names {
                    if !unique.contains(&name) {
                        unique.push(name);
                    }
                }
                Ok(Value::List(unique))
            }
            KeyKind::Provider if is_provider_id(raw) => Ok(Value::Str(raw.to_owned())),
            KeyKind::Provider => Err(self.expect()),
        }
    }
}

/// `"a"`, `"a" or "b"`, `"a", "b" or "c"`.
fn one_of(values: &[&str]) -> String {
    let quoted: Vec<String> = values.iter().map(|v| format!("{v:?}")).collect();
    match quoted.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
        None => String::new(),
    }
}

/// A list on the command line (§6.4): comma-separated, each item trimmed, and no item empty.
/// `''` alone is the empty list.
fn list_arg(raw: &str, what: &str) -> Result<Vec<String>, String> {
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    let items: Vec<&str> = raw.split(',').map(str::trim).collect();
    if items.iter().any(|item| item.is_empty()) {
        return Err(format!(
            "must be {what} separated by commas, with no empty item ('' alone is the empty list)"
        ));
    }
    Ok(items.into_iter().map(str::to_owned).collect())
}

/// A model name or a list of them, as the file holds them. Names are trimmed, and an empty one
/// makes the whole value invalid.
fn models_from_item(item: &Item) -> Option<Vec<String>> {
    let names: Vec<String> = match item.as_str() {
        Some(one) => vec![model_name(one)?],
        None => item
            .as_array()?
            .iter()
            .map(|v| v.as_str().and_then(model_name))
            .collect::<Option<_>>()?,
    };
    collapse_models(names)
}

fn model_name(s: &str) -> Option<String> {
    let trimmed = s.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// §6.4's rule for model names, from the file or the command line alike: a name repeated in
/// another case collapses into its first spelling, and `all` (any case) cannot be mixed with
/// names.
fn collapse_models(names: Vec<String>) -> Option<Vec<String>> {
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

/// A non-empty format whose every `{…}` is one of §13.5's placeholders. The scan is the
/// renderer's: a `{` runs to the next `}`, and an unclosed `{` is invalid.
fn is_format(s: &str) -> bool {
    if s.trim().is_empty() {
        return false;
    }
    let mut rest = s;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}').map(|i| open + i) else {
            return false;
        };
        if !is_statusline_placeholder(&rest[open + 1..close]) {
            return false;
        }
        rest = &rest[close + 1..];
    }
    true
}

/// `run.share_extra` as the file holds it: a name or a list of names, or `None` for any other
/// value. Within a list, an item that is not a string, or not an entry name
/// ([`is_share_name`]), is dropped with a warning naming it; the rest stand, and a repeated
/// name collapses into its first.
fn share_names_from_item(item: &Item, warn: &mut dyn FnMut(String)) -> Option<Vec<String>> {
    let items: Vec<Option<&str>> = match (item.as_str(), item.as_array()) {
        (Some(one), _) => vec![Some(one)],
        (None, Some(list)) => list.iter().map(|v| v.as_str()).collect(),
        (None, None) => return None,
    };
    let mut names: Vec<String> = Vec::new();
    for item in items {
        match item {
            Some(name) if is_share_name(name) => {
                if !names.iter().any(|n| n == name) {
                    names.push(name.to_owned());
                }
            }
            Some(name) => warn(format!(
                "entry {name:?} is not an entry name of the source home (ignored)"
            )),
            None => warn("holds an item that is not a string (ignored)".to_owned()),
        }
    }
    Some(names)
}

/// A key's value, whatever its kind.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Float(f64),
    Int(i64),
    Bool(bool),
    Str(String),
    List(Vec<String>),
}

impl Value {
    /// The TOML a write stores: a list is an array of strings, a boolean a TOML boolean.
    pub fn to_item(&self) -> Item {
        match self {
            Value::Float(n) => toml_edit::value(*n),
            Value::Int(n) => toml_edit::value(*n),
            Value::Bool(b) => toml_edit::value(*b),
            Value::Str(s) => toml_edit::value(s.as_str()),
            Value::List(items) => toml_edit::value(
                items
                    .iter()
                    .map(String::as_str)
                    .collect::<toml_edit::Array>(),
            ),
        }
    }

    /// The value as `config get` prints it, in the form `config set` takes: a list joined with
    /// commas, and the empty list as the empty string.
    pub fn display(&self) -> String {
        match self {
            Value::Float(n) => n.to_string(),
            Value::Int(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Str(s) => s.clone(),
            Value::List(items) => items.join(","),
        }
    }

    /// The value under `--json`: a list is an array.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Value::Float(n) => serde_json::json!(n),
            Value::Int(n) => serde_json::json!(n),
            Value::Bool(b) => serde_json::json!(b),
            Value::Str(s) => serde_json::json!(s),
            Value::List(items) => serde_json::json!(items),
        }
    }
}

/// Where a key's effective value came from (§6.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Neither table holds a valid value.
    Default,
    /// The key's own table: `[autoswitch]`, or the top level.
    Global,
    /// The provider's `[provider.<id>.<table>]`.
    Provider,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Default => "default",
            Source::Global => "global",
            Source::Provider => "provider",
        }
    }
}

/// One key as `config list` shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyState {
    pub key: &'static Key,
    /// The effective value: what `Settings::load` gives.
    pub value: Value,
    pub default: Value,
    pub source: Source,
}

/// The settings file as one provider's commands read it (§6.4): each key's effective value and
/// source, in [`KEYS`] order, the keys the registry does not know, and the read's warnings.
#[derive(Debug, Clone, PartialEq)]
pub struct Inspection {
    pub path: PathBuf,
    /// Whether there is anything at `path` to read; a file that cannot be read still exists.
    pub exists: bool,
    pub keys: Vec<KeyState>,
    /// Dotted names in the file that no registry key accounts for, sorted. Under
    /// `provider.<id>`, for any id, a key is known only when a provider table may override it.
    pub unknown: Vec<String>,
    pub warnings: Vec<String>,
}

/// `$XDG_CONFIG_HOME/tagteam/config.toml` (§5), whether or not it exists.
pub fn config_path(env: &Env) -> PathBuf {
    env.config_dir().join("config.toml")
}

/// The file as `provider`'s commands read it: forgiving, and read exactly as `Settings::load`
/// reads it. Never creates anything.
pub fn inspect(env: &Env, provider: &ProviderId) -> Inspection {
    let read = read_file(env, provider);
    let defaults = Settings::default();
    let keys = KEYS
        .iter()
        .zip(read.sources)
        .map(|(key, source)| KeyState {
            key,
            value: read.settings.value(key),
            default: defaults.value(key),
            source,
        })
        .collect();
    Inspection {
        path: read.path,
        exists: read.exists,
        keys,
        unknown: read.unknown,
        warnings: read.warnings,
    }
}

/// A key as a `config` command names it, and the provider table it names, if any.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub key: &'static Key,
    pub provider: Option<ProviderId>,
}

/// `provider.<id>.<key>`, or `<key>` with `--provider` as `flag`: the two spellings of one
/// entry (§6.4). Both may be given only when they name the same provider, and a provider table
/// is refused for a key no provider can override. Whether this build has the provider is the
/// caller's to check, as it checks `--provider`, so both spellings fail alike.
pub fn resolve(name: &str, flag: Option<&ProviderId>) -> Result<Resolved, SettingsError> {
    let unknown = || SettingsError::UnknownKey(name.to_owned());
    let (prefix, bare) = match name.strip_prefix("provider.") {
        Some(rest) => {
            let (id, bare) = rest.split_once('.').ok_or_else(unknown)?;
            (Some(id), bare)
        }
        None => (None, name),
    };
    let key = key(bare).ok_or_else(unknown)?;
    let provider = match (prefix, flag) {
        (Some(id), Some(flag)) if id != flag.as_str() => {
            return Err(SettingsError::ProviderMismatch {
                prefix: id.to_owned(),
                flag: flag.to_string(),
            });
        }
        (Some(id), _) => Some(ProviderId::new(id)),
        (None, flag) => flag.cloned(),
    };
    if provider.is_some() && !key.per_provider {
        return Err(SettingsError::NotPerProvider(key.name.to_owned()));
    }
    Ok(Resolved { key, provider })
}

/// Why a `config` command refused (§6.4). The engine reports each as `EngineError::Settings`.
#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("there is no setting `{0}`; `tagteam config list` shows them all")]
    UnknownKey(String),
    #[error(
        "`{0}` is the same for every provider, so no provider table can set it; name it without `provider.<id>.` and without --provider"
    )]
    NotPerProvider(String),
    #[error(
        "`provider.{prefix}.…` and `--provider {flag}` name different providers; give only one of them"
    )]
    ProviderMismatch { prefix: String, flag: String },
    #[error("`{key}` {reason}")]
    Invalid { key: String, reason: String },
    /// A `default_provider` value naming a provider this build does not have (Decision 2).
    #[error("unknown provider {0:?}; name one this build has, such as claude-code")]
    UnknownProvider(String),
    /// `set` and `unset` never edit a file that is not valid TOML (§6.4).
    #[error("{} is not valid TOML ({detail}); fix it, then retry", path.display())]
    Corrupt { path: PathBuf, detail: String },
    /// The settings lock (§4.3) could not be taken.
    #[error(transparent)]
    Lock(#[from] LockError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// `default_provider`, read from the top level only. Whether this build has it is the
    /// CLI's to check; it falls back to claude-code.
    pub default_provider: ProviderId,
    /// `autoswitch.threshold`: 50–99.9. The provider's own table first.
    pub threshold: f64,
    /// `autoswitch.interval_seconds`: 15–3600. The provider's own table first.
    pub interval_seconds: i64,
    /// `autoswitch.cooldown_seconds`: 0–86400. The provider's own table first.
    pub cooldown_seconds: i64,
    /// `autoswitch.hysteresis_pct`: 0–50. The provider's own table first.
    pub hysteresis_pct: f64,
    /// `autoswitch.strategy`: `best` or `consume-first`. The provider's own table first.
    pub strategy: Strategy,
    /// `autoswitch.include_api_key_accounts`. The provider's own table first.
    pub include_api_key_accounts: bool,
    /// `autoswitch.unhealthy_ticks`: 1–100. The provider's own table first.
    pub unhealthy_ticks: u32,
    /// `autoswitch.models`: model display names, or `all`. The provider's own table first.
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
            default_provider: ProviderId::new(CLAUDE_CODE),
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

impl Settings {
    /// `config.toml`, as `provider`'s commands read it. Forgiving (§6.4): a missing file gives
    /// the defaults silently; a corrupt or unreadable file gives the defaults and one warning
    /// naming the path; an invalid value gives its default and one warning naming the path and
    /// the key. The rest of the file still applies.
    pub fn load(env: &Env, provider: &ProviderId) -> (Settings, Vec<String>) {
        let read = read_file(env, provider);
        (read.settings, read.warnings)
    }

    /// `config.toml`'s modification time; `None` when the file is missing or its metadata
    /// cannot be read. A running `auto` re-reads the settings before a tick whenever this
    /// changes (§11.4). Read it before `load`, so a write landing between the two counts as
    /// one more change at the next check rather than going unseen.
    pub fn mtime(env: &Env) -> Option<SystemTime> {
        std::fs::metadata(config_path(env))
            .and_then(|m| m.modified())
            .ok()
    }

    /// `key`'s value, as `config` shows it.
    pub fn value(&self, key: &Key) -> Value {
        match key.name {
            "default_provider" => Value::Str(self.default_provider.to_string()),
            "autoswitch.threshold" => Value::Float(self.threshold),
            "autoswitch.interval_seconds" => Value::Int(self.interval_seconds),
            "autoswitch.cooldown_seconds" => Value::Int(self.cooldown_seconds),
            "autoswitch.hysteresis_pct" => Value::Float(self.hysteresis_pct),
            "autoswitch.strategy" => Value::Str(self.strategy.as_str().to_owned()),
            "autoswitch.include_api_key_accounts" => Value::Bool(self.include_api_key_accounts),
            "autoswitch.unhealthy_ticks" => Value::Int(self.unhealthy_ticks.into()),
            "autoswitch.models" => Value::List(self.models.clone()),
            "usage.history_retention_days" => Value::Int(self.history_retention_days.into()),
            "statusline.format" => Value::Str(self.statusline_format.clone()),
            "run.share_extra" => Value::List(self.share_extra.clone()),
            "ui.color" => Value::Str(self.color.as_str().to_owned()),
            other => unreachable!("`{other}` is not a registry key"),
        }
    }

    /// Stores what `key.parse_item` gave. The registry's kinds and ranges make each pairing
    /// below the only one that can occur.
    fn apply(&mut self, key: &Key, value: Value) {
        match (key.name, value) {
            ("default_provider", Value::Str(id)) => self.default_provider = ProviderId::new(id),
            ("autoswitch.threshold", Value::Float(n)) => self.threshold = n,
            ("autoswitch.interval_seconds", Value::Int(n)) => self.interval_seconds = n,
            ("autoswitch.cooldown_seconds", Value::Int(n)) => self.cooldown_seconds = n,
            ("autoswitch.hysteresis_pct", Value::Float(n)) => self.hysteresis_pct = n,
            ("autoswitch.strategy", Value::Str(s)) => {
                self.strategy = Strategy::parse(&s).unwrap_or(Strategy::Best)
            }
            ("autoswitch.include_api_key_accounts", Value::Bool(b)) => {
                self.include_api_key_accounts = b
            }
            ("autoswitch.unhealthy_ticks", Value::Int(n)) => self.unhealthy_ticks = whole(n),
            ("autoswitch.models", Value::List(names)) => self.models = names,
            ("usage.history_retention_days", Value::Int(n)) => {
                self.history_retention_days = whole(n)
            }
            ("statusline.format", Value::Str(s)) => self.statusline_format = s,
            ("run.share_extra", Value::List(names)) => self.share_extra = names,
            ("ui.color", Value::Str(s)) => {
                self.color = match s.as_str() {
                    "always" => ColorMode::Always,
                    "never" => ColorMode::Never,
                    _ => ColorMode::Auto,
                }
            }
            (name, value) => unreachable!("the registry never gives `{name}` {value:?}"),
        }
    }
}

/// An `Int` key's value as a `u32` field holds it: the ranges of `autoswitch.unhealthy_ticks`
/// and `usage.history_retention_days` lie within `u32`. The two seconds keys are `i64`, as
/// M3b's `AutoConfig` takes them.
fn whole(n: i64) -> u32 {
    u32::try_from(n).expect("a u32 field's Int range lies within u32")
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

/// One read of the file for `provider`: what `Settings::load` and `inspect` both report.
struct FileRead {
    path: PathBuf,
    exists: bool,
    settings: Settings,
    /// Per [`KEYS`] entry, the table its value came from.
    sources: Vec<Source>,
    unknown: Vec<String>,
    warnings: Vec<String>,
}

fn read_file(env: &Env, provider: &ProviderId) -> FileRead {
    let mut read = FileRead {
        path: config_path(env),
        exists: true,
        settings: Settings::default(),
        sources: vec![Source::Default; KEYS.len()],
        unknown: Vec::new(),
        warnings: Vec::new(),
    };
    let text = match std::fs::read_to_string(&read.path) {
        Ok(text) => text,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            read.exists = false;
            return read;
        }
        Err(e) => {
            read.warnings.push(format!(
                "{}: cannot read the settings file ({}); using the defaults",
                read.path.display(),
                e.kind()
            ));
            return read;
        }
    };
    let Ok(doc) = text.parse::<DocumentMut>() else {
        read.warnings.push(format!(
            "{}: the settings file is not valid TOML; using the defaults",
            read.path.display()
        ));
        return read;
    };
    let shown = read.path.display().to_string();
    let mut reader = Reader {
        doc: &doc,
        path: &shown,
        warnings: Vec::new(),
    };
    for (key, source) in KEYS.iter().zip(read.sources.iter_mut()) {
        if let Some((value, from)) = reader.read_key(key, provider) {
            read.settings.apply(key, value);
            *source = from;
        }
    }
    read.warnings = reader.warnings;
    unknown_keys(doc.as_table(), &mut Vec::new(), &mut read.unknown);
    read.unknown.sort();
    read
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

    /// `key`'s value for `provider`, and the table it came from: the first table that holds a
    /// valid one, the provider's own first when the key is per provider. A present value that
    /// is not valid warns, naming the dotted key, and the next table is tried.
    fn read_key(&mut self, key: &Key, provider: &ProviderId) -> Option<(Value, Source)> {
        let own: Vec<&str> = key.table().into_iter().collect();
        let mut tables: Vec<(Vec<&str>, Source)> = Vec::with_capacity(2);
        if key.per_provider {
            let mut path = vec!["provider", provider.as_str()];
            path.extend(&own);
            tables.push((path, Source::Provider));
        }
        tables.push((own, Source::Global));
        for (path, source) in tables {
            let Some(table) = self.table(&path) else {
                continue;
            };
            let Some(item) = table.get(key.leaf()) else {
                continue;
            };
            let dotted = path
                .iter()
                .copied()
                .chain([key.leaf()])
                .collect::<Vec<_>>()
                .join(".");
            let mut details = Vec::new();
            let value = key.parse_item(item, &mut |detail| details.push(detail));
            for detail in details {
                self.warn(format!("{}: `{dotted}` {detail}", self.path));
            }
            match value {
                Some(value) => return Some((value, source)),
                None => self.warn(format!(
                    "{}: `{dotted}` {} (ignored)",
                    self.path,
                    key.expect()
                )),
            }
        }
        None
    }
}

/// Where a dotted path of the file stands in the registry.
enum Place {
    Key,
    Table,
    Unknown,
}

/// `path`, from the top of the file. `provider` and `provider.<id>` are tables for any id, and
/// under them only a per-provider key, and its table, are known.
fn place(path: &[&str]) -> Place {
    match path {
        ["provider"] | ["provider", _] => Place::Table,
        ["provider", _, rest @ ..] => place_in(rest, true),
        rest => place_in(rest, false),
    }
}

fn place_in(path: &[&str], in_provider: bool) -> Place {
    let mut place = Place::Unknown;
    for k in KEYS.iter().filter(|k| k.per_provider || !in_provider) {
        match (k.table(), path) {
            (None, [leaf]) if *leaf == k.leaf() => return Place::Key,
            (Some(table), [t, leaf]) if *t == table && *leaf == k.leaf() => return Place::Key,
            (Some(table), [t]) if *t == table => place = Place::Table,
            _ => {}
        }
    }
    place
}

/// Appends to `unknown` the dotted name of every key under `table`, which sits at `path`, that
/// the registry does not know. A known table holding something other than a table is the
/// reader's to warn about, not an unknown key.
fn unknown_keys(table: &dyn TableLike, path: &mut Vec<String>, unknown: &mut Vec<String>) {
    for (name, item) in table.iter() {
        path.push(name.to_owned());
        let at = {
            let segments: Vec<&str> = path.iter().map(String::as_str).collect();
            place(&segments)
        };
        match at {
            Place::Key => {}
            Place::Table => {
                if let Some(inner) = item.as_table_like() {
                    unknown_keys(inner, path, unknown);
                }
            }
            Place::Unknown => every_key(item, path, unknown),
        }
        path.pop();
    }
}

/// Appends the dotted name of every key inside `item`, which sits at `path`: of each value in
/// a table, at any depth, or of `path` itself when it holds no table with a key in it.
fn every_key(item: &Item, path: &mut Vec<String>, unknown: &mut Vec<String>) {
    match item.as_table_like() {
        Some(table) if !table.is_empty() => {
            for (name, inner) in table.iter() {
                path.push(name.to_owned());
                every_key(inner, path, unknown);
                path.pop();
            }
        }
        _ => unknown.push(path.join(".")),
    }
}
