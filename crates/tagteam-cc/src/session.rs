//! Claude Code's half of §12: what a profile shares (§12.2's tables), the record of the outer
//! home, and the two views every profile credential operation resolves through (§12.5): the
//! recorded spelling for its Keychain items, its actual directory for its files (Decision 19).

use std::ffi::OsString;
use std::path::Path;

use serde_json::{Value, json};
use tagteam_provider::{EntryKind, Env, ProviderError};

use crate::paths::CcPaths;

pub(crate) const CC_SHARED: &[&str] = &[
    "CLAUDE.md",
    "settings.json",
    "keybindings.json",
    "agents",
    "commands",
    "skills",
    "plugins",
    "hooks",
    "output-styles",
    "themes",
    "rules",
    "workflows",
    "file-history",
    "paste-cache",
    "shell-snapshots",
    "session-env",
];

pub(crate) const CC_MUST_SHARE: &[(&str, EntryKind)] = &[
    ("projects", EntryKind::Dir),
    ("history.jsonl", EntryKind::File),
];

pub(crate) const CC_PRIVATE: &[&str] = &[
    ".credentials.json",
    ".claude.json",
    ".claude-*-oauth.json",
    ".config.json",
    "sessions",
    "ide",
    "jobs",
    "daemon",
    "daemon.*",
    "daemon-auth-cooldown",
    "daemon-auth-status.json",
    "backups",
    "cache",
    "mcp-needs-auth-cache.json",
    "stats-cache.json",
    "policy-limits.json*",
    "remote-settings.json",
    "remote-settings-consent.json",
    "remote-settings-helper-consent",
    ".session_ingress_token",
    "hfi-auth.json",
    "state",
    "seed-admin",
    "bridge-spawn",
    "chrome",
    "debug",
    "feedback",
    "routines",
    "settings.local.json",
    ".last-cleanup",
    ".cc-writes",
    ".device-keys.json",
    "*.lock",
    "*.lock.owner",
    ".storage-write",
    ".*_auth_refresh-*",
    ".tagteam-*",
];

const CONFIG_DIR: &str = "CLAUDE_CONFIG_DIR";
const SECURE_STORAGE_DIR: &str = "CLAUDE_SECURESTORAGE_CONFIG_DIR";

/// One variable as the record keeps it: its text, or `null` when undefined. A value that is not
/// UTF-8 is kept as the same lossy text Keychain naming already hashes (Appendix A.2).
fn recorded(v: Option<&OsString>) -> Value {
    v.map_or(Value::Null, |s| {
        Value::String(s.to_string_lossy().into_owned())
    })
}

/// `outer` is `{"CLAUDE_CONFIG_DIR": <string>|null, "CLAUDE_SECURESTORAGE_CONFIG_DIR": <string>|null}`:
/// null means undefined; "" is kept (defined-but-empty, Appendix A.1).
pub(crate) fn outer_home(env: &Env) -> Value {
    json!({
        CONFIG_DIR: recorded(env.claude_config_dir.as_ref()),
        SECURE_STORAGE_DIR: recorded(env.claude_securestorage_config_dir.as_ref()),
    })
}

/// `env` with both variables as `outer` records them. Anything but a string or `null` under
/// each key is refused: the outer home would be a guess. A variable `env.vars` captured
/// (Decision 5: `CLAUDE_CONFIG_DIR` is the session variable) follows the restored value, and is
/// dropped where `outer` records it undefined, so nothing reading `vars` still finds the
/// profile.
pub(crate) fn apply_outer_home(env: &Env, outer: &Value) -> Result<Env, ProviderError> {
    let restored = |key: &str| match outer.get(key) {
        Some(Value::String(s)) => Ok(Some(OsString::from(s))),
        Some(Value::Null) => Ok(None),
        _ => Err(ProviderError::Invalid(format!(
            "the profile marker's outer record has no valid {key}"
        ))),
    };
    let mut out = env.clone();
    out.claude_config_dir = restored(CONFIG_DIR)?;
    out.claude_securestorage_config_dir = restored(SECURE_STORAGE_DIR)?;
    for (key, value) in [
        (CONFIG_DIR, &out.claude_config_dir),
        (SECURE_STORAGE_DIR, &out.claude_securestorage_config_dir),
    ] {
        if out.vars.contains_key(key) {
            match value {
                Some(v) => out.vars.insert(key.to_owned(), v.clone()),
                None => out.vars.remove(key),
            };
        }
    }
    Ok(out)
}

/// The profile `Env`: `claude_config_dir = Some(spelling)`, `claude_securestorage_config_dir = None`.
/// It is the session environment's view of the profile (§12.5 scrubs the secure-storage dir),
/// so the profile's Keychain items, named from the recorded spelling (§12.2), resolve from it.
/// Its files resolve from it only while `spelling` still names where the profile is; a read
/// finds them with `profile_paths` (Decision 19).
pub(crate) fn profile_env(env: &Env, spelling: &str) -> Env {
    let mut out = env.clone();
    out.claude_config_dir = Some(OsString::from(spelling));
    out.claude_securestorage_config_dir = None;
    out
}

/// The files of the profile in `dir`, its actual directory, as Claude Code resolves them with
/// `CLAUDE_CONFIG_DIR` naming it and secure storage undefined (Decision 19). Its Keychain items
/// are named from the recorded spelling instead (`profile_env`), which names the old path once
/// the data directory has moved (§12.2).
pub(crate) fn profile_paths(env: &Env, dir: &Path) -> CcPaths {
    let mut at = env.clone();
    at.claude_config_dir = Some(dir.as_os_str().to_owned());
    at.claude_securestorage_config_dir = None;
    CcPaths::resolve(&at)
}
