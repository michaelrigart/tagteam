//! FakeAgent's transcripts (token report spec §3.2), deliberately unlike Claude Code's: one flat
//! directory, one line per snapshot, short keys, and a model of its own.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use tagteam_core::tokens::{ModelPrice, PriceTable, RateCard, TokenCounts};
use tagteam_provider::{Env, LineError, UsageLine};

use crate::paths::FakePaths;

/// FakeAgent's one model, `fake-1`.
pub(crate) static PRICES: PriceTable = PriceTable {
    as_of: "2026-01-01",
    models: &[(
        "fake-1",
        ModelPrice {
            base: RateCard {
                input: 1.0,
                output: 2.0,
                cache_write_5m: 3.0,
                cache_write_1h: 4.0,
                cache_read: 0.5,
            },
            long_prompt: None,
            fast: None,
            web_search_per_1k: Some(10.0),
        },
    )],
};

/// One usage line for a test to write, with the fields of the `UsageLine` FakeAgent reads back
/// from it. The default is a final `fake-1` line an hour before the engine fixtures' clock
/// (1_790_000_000_000).
#[derive(Debug, Clone, PartialEq)]
pub struct FakeUsage {
    pub message_id: String,
    pub request_id: String,
    pub session_id: String,
    pub at_ms: i64,
    pub model: String,
    pub project: Option<String>,
    pub sidechain: bool,
    pub counts: TokenCounts,
    pub speed: Option<String>,
    pub final_line: bool,
}

impl Default for FakeUsage {
    fn default() -> Self {
        Self {
            message_id: "m1".into(),
            request_id: "r1".into(),
            session_id: "s1".into(),
            at_ms: 1_790_000_000_000 - 3_600_000,
            model: "fake-1".into(),
            project: Some("/w".into()),
            sidechain: false,
            counts: TokenCounts {
                input: 1,
                output: 2,
                cache_write_5m: 3,
                cache_write_1h: 4,
                cache_read: 5,
                web_search: 0,
            },
            speed: None,
            final_line: true,
        }
    }
}

impl FakeUsage {
    /// What `parse_usage_line` reads from `usage_line_json(self)`.
    pub fn usage_line(&self) -> UsageLine {
        UsageLine {
            message_id: self.message_id.clone(),
            request_id: self.request_id.clone(),
            session_id: self.session_id.clone(),
            at_ms: self.at_ms,
            model: self.model.clone(),
            project: self.project.clone(),
            sidechain: self.sidechain,
            counts: self.counts,
            speed: self.speed.clone(),
            final_line: self.final_line,
        }
    }
}

/// `u` as one FakeAgent transcript line, without its newline.
pub fn usage_line_json(u: &FakeUsage) -> String {
    json!({
        "t": "usage",
        "id": u.message_id,
        "req": u.request_id,
        "session": u.session_id,
        "at": u.at_ms,
        "model": u.model,
        "cwd": u.project,
        "side": u.sidechain,
        "final": u.final_line,
        "in": u.counts.input,
        "out": u.counts.output,
        "cw5": u.counts.cache_write_5m,
        "cw1": u.counts.cache_write_1h,
        "cr": u.counts.cache_read,
        "ws": u.counts.web_search,
        "speed": u.speed,
    })
    .to_string()
}

/// Appends `text` as it is to `<transcripts>/<name>`, creating the directory: a test helper that
/// panics on I/O failure. Returns the file's path.
pub fn append_transcript(env: &Env, name: &str, text: &str) -> PathBuf {
    let dir = FakePaths::resolve(env).transcripts;
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| f.write_all(text.as_bytes()))
        .unwrap();
    path
}

/// Every `*.jsonl` regular file in `dir`, sorted; none when `dir` is missing.
pub(crate) fn discover(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let listing = match fs::read_dir(dir) {
        Ok(l) => l,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for entry in listing {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|x| x == "jsonl") && entry.file_type()?.is_file() {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

fn invalid(key: &str) -> LineError {
    LineError(format!("a FakeAgent usage line has no valid {key:?}"))
}

fn text(o: &Map<String, Value>, key: &str) -> Result<String, LineError> {
    o.get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| invalid(key))
}

/// Text or null; absent counts as null.
fn optional_text(o: &Map<String, Value>, key: &str) -> Result<Option<String>, LineError> {
    match o.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(invalid(key)),
    }
}

fn count(o: &Map<String, Value>, key: &str) -> Result<u64, LineError> {
    o.get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid(key))
}

fn flag(o: &Map<String, Value>, key: &str) -> Result<bool, LineError> {
    o.get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| invalid(key))
}

/// `None` for a line whose `t` is not `"usage"`; `Err` for one that is not a JSON object, or a
/// usage line with a field missing or of the wrong type. An absent `req` is `""` (ruling R5).
pub(crate) fn parse_line(line: &[u8]) -> Result<Option<UsageLine>, LineError> {
    let Ok(Value::Object(o)) = serde_json::from_slice::<Value>(line) else {
        return Err(LineError(
            "a FakeAgent transcript line is not a JSON object".into(),
        ));
    };
    if o.get("t").and_then(Value::as_str) != Some("usage") {
        return Ok(None);
    }
    Ok(Some(UsageLine {
        message_id: text(&o, "id")?,
        request_id: optional_text(&o, "req")?.unwrap_or_default(),
        session_id: text(&o, "session")?,
        at_ms: o
            .get("at")
            .and_then(Value::as_i64)
            .ok_or_else(|| invalid("at"))?,
        model: text(&o, "model")?,
        project: optional_text(&o, "cwd")?,
        sidechain: flag(&o, "side")?,
        counts: TokenCounts {
            input: count(&o, "in")?,
            output: count(&o, "out")?,
            cache_write_5m: count(&o, "cw5")?,
            cache_write_1h: count(&o, "cw1")?,
            cache_read: count(&o, "cr")?,
            web_search: count(&o, "ws")?,
        },
        speed: optional_text(&o, "speed")?,
        final_line: flag(&o, "final")?,
    }))
}
