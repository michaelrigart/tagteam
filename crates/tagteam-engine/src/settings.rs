//! Read-only `config.toml` (§6.4): the keys M2b consumes. `tagteam config` and its writes land
//! in M5; this module only reads, and never fails: a missing file, a corrupt file and an invalid
//! value each fall back to the default, the last two with a warning for the caller to print.

use std::io::ErrorKind;

use tagteam_core::ProviderId;
use tagteam_provider::Env;
use toml_edit::{DocumentMut, Item, TableLike};

pub const DEFAULT_THRESHOLD: f64 = 90.0;
pub const DEFAULT_HISTORY_RETENTION_DAYS: u32 = 180;
pub const DEFAULT_STATUSLINE_FORMAT: &str = "{account} · 5h {5h}% · 7d {7d}%{stale}";

/// `ui.color`. `NO_COLOR`, `FORCE_COLOR` and `--no-color` are the CLI's to apply on top.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// `autoswitch.threshold`: 50–99.9. The provider's own table first.
    pub threshold: f64,
    /// `autoswitch.models`: model display names, or `all`. The provider's own table first.
    pub models: Vec<String>,
    /// `usage.history_retention_days`: 1–3650.
    pub history_retention_days: u32,
    /// `statusline.format`. The provider's own table first.
    pub statusline_format: String,
    /// `ui.color`.
    pub color: ColorMode,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            threshold: DEFAULT_THRESHOLD,
            models: Vec::new(),
            history_retention_days: DEFAULT_HISTORY_RETENTION_DAYS,
            statusline_format: DEFAULT_STATUSLINE_FORMAT.to_owned(),
            color: ColorMode::Auto,
        }
    }
}

impl Settings {
    /// `env.config_dir()/config.toml`. Forgiving (§6.4): a missing file gives the defaults
    /// silently; a corrupt or unreadable file gives the defaults and one warning naming the
    /// path; an invalid value gives its default and one warning naming the key. The rest of the
    /// file still applies.
    pub fn load(env: &Env, provider: &ProviderId) -> (Settings, Vec<String>) {
        let path = env.config_dir().join("config.toml");
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
            Ok(doc) => from_document(&doc, provider),
            Err(_) => {
                let warning = format!(
                    "{}: the settings file is not valid TOML; using the defaults",
                    path.display()
                );
                (Settings::default(), vec![warning])
            }
        }
    }
}

struct Reader<'a> {
    doc: &'a DocumentMut,
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
                    self.warn(format!("config.toml: `{dotted}` must be a table (ignored)"));
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
                    self.warn(format!("config.toml: `{dotted}` {expect} (ignored)"));
                }
            }
        }
        None
    }
}

fn from_document(doc: &DocumentMut, provider: &ProviderId) -> (Settings, Vec<String>) {
    let defaults = Settings::default();
    let mut reader = Reader {
        doc,
        warnings: Vec::new(),
    };
    let global_autoswitch: &[&str] = &["autoswitch"];
    let global_statusline: &[&str] = &["statusline"];
    let provider_autoswitch = ["provider", provider.as_str(), "autoswitch"];
    let provider_statusline = ["provider", provider.as_str(), "statusline"];

    let threshold = reader
        .read(
            &[&provider_autoswitch[..], global_autoswitch],
            "threshold",
            "must be a number from 50 to 99.9",
            parse_threshold,
        )
        .unwrap_or(defaults.threshold);
    let models = reader
        .read(
            &[&provider_autoswitch[..], global_autoswitch],
            "models",
            "must be a model name or a list of model names",
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
    let statusline_format = reader
        .read(
            &[&provider_statusline[..], global_statusline],
            "format",
            "must be a non-empty string",
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

    let settings = Settings {
        threshold,
        models,
        history_retention_days,
        statusline_format,
        color,
    };
    (settings, reader.warnings)
}

fn parse_threshold(item: &Item) -> Option<f64> {
    let value = item
        .as_float()
        .or_else(|| item.as_integer().map(|i| i as f64))?;
    (50.0..=99.9).contains(&value).then_some(value)
}

fn parse_retention(item: &Item) -> Option<u32> {
    u32::try_from(item.as_integer()?)
        .ok()
        .filter(|days| (1..=3650).contains(days))
}

fn parse_models(item: &Item) -> Option<Vec<String>> {
    if let Some(one) = item.as_str() {
        return name(one).map(|n| vec![n]);
    }
    item.as_array()?
        .iter()
        .map(|v| v.as_str().and_then(name))
        .collect()
}

fn name(s: &str) -> Option<String> {
    let trimmed = s.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn parse_format(item: &Item) -> Option<String> {
    let s = item.as_str()?;
    (!s.trim().is_empty()).then(|| s.to_owned())
}

fn parse_color(item: &Item) -> Option<ColorMode> {
    match item.as_str()? {
        "auto" => Some(ColorMode::Auto),
        "always" => Some(ColorMode::Always),
        "never" => Some(ColorMode::Never),
        _ => None,
    }
}
