//! `tagteam config list|get|path` (§6.4): the human and JSON forms of what `settings` reads.

use std::path::Path;

use serde_json::{Value, json};
use tagteam_core::ProviderId;
use tagteam_engine::settings::{Inspection, KeyState};

/// How `config list` shows an empty value: as the empty argument `config set` takes.
const EMPTY: &str = "''";

/// Every key in the registry's order, with the table its value comes from and the value, then
/// the keys the registry does not know.
pub(crate) fn list_human(inspection: &Inspection) -> String {
    let key_w = inspection
        .keys
        .iter()
        .map(|s| s.key.name.len())
        .fold("KEY".len(), usize::max);
    let mut out = format!("{:<key_w$}  {:<8}  VALUE\n", "KEY", "SOURCE");
    for s in &inspection.keys {
        out.push_str(&format!(
            "{:<key_w$}  {:<8}  {}\n",
            s.key.name,
            s.source.as_str(),
            shown(s)
        ));
    }
    if !inspection.unknown.is_empty() {
        out.push_str("\nUnknown keys, ignored:\n");
        for key in &inspection.unknown {
            out.push_str(&format!("  {key}\n"));
        }
    }
    out
}

fn shown(s: &KeyState) -> String {
    let text = s.value.display();
    if text.is_empty() {
        EMPTY.to_owned()
    } else {
        text
    }
}

/// `{schemaVersion, path, provider, keys: [{key, value, default, source}], unknown: [key]}`.
pub(crate) fn list_json(inspection: &Inspection, provider: &ProviderId) -> Value {
    let keys: Vec<Value> = inspection
        .keys
        .iter()
        .map(|s| {
            json!({
                "key": s.key.name,
                "value": s.value.to_json(),
                "default": s.default.to_json(),
                "source": s.source.as_str(),
            })
        })
        .collect();
    json!({
        "schemaVersion": 1,
        "path": inspection.path.display().to_string(),
        "provider": provider.as_str(),
        "keys": keys,
        "unknown": inspection.unknown,
    })
}

/// The value alone, for scripts: a list comma-separated, and the empty list as an empty line.
pub(crate) fn get_human(state: &KeyState) -> String {
    format!("{}\n", state.value.display())
}

/// `{schemaVersion, key, provider, value, source}`, with a list as an array.
pub(crate) fn get_json(state: &KeyState, provider: &ProviderId) -> Value {
    json!({
        "schemaVersion": 1,
        "key": state.key.name,
        "provider": provider.as_str(),
        "value": state.value.to_json(),
        "source": state.source.as_str(),
    })
}

pub(crate) fn path_human(path: &Path) -> String {
    format!("{}\n", path.display())
}

/// `{schemaVersion, path, exists}`.
pub(crate) fn path_json(path: &Path, exists: bool) -> Value {
    json!({"schemaVersion": 1, "path": path.display().to_string(), "exists": exists})
}
