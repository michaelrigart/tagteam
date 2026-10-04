//! Claude Code's half of §12: what a profile shares (§12.2's tables), the record of the outer
//! home, and the two views every profile credential operation resolves through (§12.5): the
//! recorded spelling for its Keychain items, its actual directory for its files (Decision 19).

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};
use tagteam_core::merge::{MergeKey, three_way};
use tagteam_provider::atomic::write_atomic_with;
use tagteam_provider::process::Captured;
use tagteam_provider::profile::{
    has_own_file, read_own_bytes, refuse_linked_file, remove_own_file, write_own_json_with,
};
use tagteam_provider::splice::{self, SpliceError};
use tagteam_provider::{
    Cancel, EntryKind, Env, Identity, LiveLockSet, MergeReport, ProviderError, Read, SessionEnv,
    Validity,
};

use crate::config::read_bytes;
use crate::locks;
use crate::paths::CcPaths;
use crate::provider::CONFIG_REMEDY;

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

/// §12.4: the seed's record of the `projects` and `mcpServers` it put in the profile.
pub(crate) const BASELINE_FILE: &str = ".tagteam-baseline.json";
const BASELINE_FORMAT: &str = "tagteam-baseline";

/// What to do about a profile's own `.claude.json` that cannot be spliced. `CONFIG_REMEDY`
/// names `~/.claude/backups/`, which holds the default home's backups, not the profile's.
const PROFILE_CONFIG_REMEDY: &str =
    "repair it, or remove it so the next launch seeds a new one, then retry";

/// The `theme` a seed sets when neither the profile nor the default file has one (§12.4).
const DEFAULT_THEME: &str = "dark";

const PROJECTS: &str = "projects";
const MCP_SERVERS: &str = "mcpServers";

fn unsplicable(path: &Path, remedy: &'static str) -> ProviderError {
    ProviderError::ConfigUnsplicable {
        path: path.to_path_buf(),
        remedy,
    }
}

/// `key` set to `value` in `doc`, or removed when `value` is `None`.
fn put(doc: &[u8], key: &str, value: Option<&Value>) -> Result<Vec<u8>, SpliceError> {
    match value {
        Some(v) => splice::replace_top_level(doc, key, v),
        None => splice::remove_top_level(doc, key),
    }
}

/// One top-level value of `doc`, `Null` when absent (`three_way`'s convention).
fn top(doc: &[u8], key: &str) -> Result<Value, SpliceError> {
    Ok(splice::get_top_level(doc, key)?.unwrap_or(Value::Null))
}

/// §12.4's seed of the profile in `dir`, under its own config lock. From the profile's current
/// file, or `{}`: copy `projects` and top-level `mcpServers` from the outer home's global config
/// (`env`), their absence included; set `oauthAccount` from `identity`, `hasCompletedOnboarding:
/// true`, and `theme` when the profile has none (the default file's, else "dark"). Then write the
/// baseline, exactly the `projects` and `mcpServers` the profile now holds. The config goes
/// first, so a seed that stops between the two leaves no baseline over an unseeded file. The
/// profile's config is its own (`CC_PRIVATE`), so one that is a link refuses first, before any
/// lock: written through a link to the default home's, it would put this account's login there.
/// The default home's own config is written through its link (§9.5); a profile's never is.
pub(crate) fn seed(
    env: &Env,
    dir: &Path,
    identity: &Identity,
    budget: Duration,
) -> Result<(), ProviderError> {
    // CC replaces the outer file by rename, so this read sees one whole version unlocked.
    let outer = CcPaths::resolve(env).global_config;
    let (projects, servers, theme) = match read_bytes(&outer) {
        Read::Present(b) => {
            let torn = |_: SpliceError| unsplicable(&outer, CONFIG_REMEDY);
            (
                splice::get_top_level(&b, PROJECTS).map_err(torn)?,
                splice::get_top_level(&b, MCP_SERVERS).map_err(torn)?,
                splice::get_top_level(&b, "theme").map_err(torn)?,
            )
        }
        Read::Absent => (None, None, None),
        Read::Unreadable(_) => return Err(unsplicable(&outer, CONFIG_REMEDY)),
    };
    let paths = profile_paths(env, dir);
    refuse_linked_file(&paths.global_config, "a session's config")?;
    let lock = locks::acquire_config(&paths, budget, &env.cancel)?;
    let fence = || lock.check_owned().map_err(ProviderError::from);
    let config = &paths.global_config;
    let torn = |_: SpliceError| unsplicable(config, PROFILE_CONFIG_REMEDY);
    let before = match read_bytes(config) {
        Read::Present(b) => Some(b),
        Read::Absent => None,
        Read::Unreadable(_) => return Err(unsplicable(config, PROFILE_CONFIG_REMEDY)),
    };
    let start = before.clone().unwrap_or_else(|| b"{}\n".to_vec());
    let has_theme = splice::get_top_level(&start, "theme")
        .map_err(torn)?
        .is_some();
    let mut new = put(&start, PROJECTS, projects.as_ref()).map_err(torn)?;
    new = put(&new, MCP_SERVERS, servers.as_ref()).map_err(torn)?;
    new = splice::replace_top_level(&new, "oauthAccount", &identity.raw).map_err(torn)?;
    new = splice::replace_top_level(&new, "hasCompletedOnboarding", &Value::Bool(true))
        .map_err(torn)?;
    if !has_theme {
        let theme = theme.unwrap_or_else(|| json!(DEFAULT_THEME));
        new = splice::replace_top_level(&new, "theme", &theme).map_err(torn)?;
    }
    if before.as_deref() != Some(new.as_slice()) {
        write_atomic_with(config, &new, 0o600, fence)?;
    }
    let baseline = json!({
        "format": BASELINE_FORMAT,
        "version": 1,
        "projects": projects.unwrap_or(Value::Null),
        "mcpServers": servers.unwrap_or(Value::Null),
    });
    write_own_json_with(dir, BASELINE_FILE, &baseline, fence)
}

/// §12.4: a baseline is waiting in `dir`, from a session whose merge-back never ran. One that
/// cannot even be looked at counts as waiting: its merge-back then fails, and nothing seeds
/// over it.
pub(crate) fn has_baseline(dir: &Path) -> bool {
    has_own_file(dir, BASELINE_FILE)
}

/// The baseline's `projects` and `mcpServers`; `None` when there is none. One that cannot be
/// read, or is not a version 1 tagteam baseline, is an error: the merge-back cannot run without
/// it, and nothing may seed over it (§12.4). It is one of tagteam's own files, so a link at its
/// path that resolves to nothing, or crosses a file, is unreadable, never absent (M4a's rule).
fn read_baseline(dir: &Path) -> Result<Option<(Value, Value)>, ProviderError> {
    let bytes = match read_own_bytes(dir, BASELINE_FILE) {
        Read::Present(b) => b,
        Read::Absent => return Ok(None),
        Read::Unreadable(e) => return Err(ProviderError::Unreadable(e)),
    };
    let invalid = || {
        ProviderError::Invalid(format!(
            "{} is not a tagteam baseline; remove it to drop the session's unmerged changes",
            dir.join(BASELINE_FILE).display()
        ))
    };
    let v: Value = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if v["format"].as_str() != Some(BASELINE_FORMAT) || v["version"].as_i64() != Some(1) {
        return Err(invalid());
    }
    match (v.get(PROJECTS), v.get(MCP_SERVERS)) {
        (Some(p), Some(m)) => Ok(Some((p.clone(), m.clone()))),
        _ => Err(invalid()),
    }
}

/// A merged key as the summary's log names it (§12.4 step 3): the path JSON-quoted, since a
/// path may hold dots.
fn key_name(k: &MergeKey) -> String {
    let quoted = |s: &str| serde_json::to_string(s).expect("a string always serializes");
    match k {
        MergeKey::Project { path, key } => format!("projects[{}].{key}", quoted(path)),
        MergeKey::McpServer { name } => format!("mcpServers[{}]", quoted(name)),
    }
}

/// §12.4's merge-back of the profile in `dir`. The profile is quiescent (the caller holds
/// `MutationGuard` and the account lock), so its file and baseline are read without a lock. The
/// default home's global config is then read, merged and written under its config lock alone
/// (§4.3), waited for under `cancel`: one write replacing the `projects` and `mcpServers` spans,
/// skipped when nothing was applied. The baseline goes last. Any failure before that leaves it,
/// and the profile, as they were.
pub(crate) fn merge_back(
    env: &Env,
    dir: &Path,
    budget: Duration,
    cancel: &Cancel,
) -> Result<MergeReport, ProviderError> {
    let Some(base) = read_baseline(dir)? else {
        return Ok(MergeReport::default());
    };
    let profile = profile_paths(env, dir).global_config;
    let mine = match read_bytes(&profile) {
        Read::Present(b) => {
            let torn = |_: SpliceError| unsplicable(&profile, PROFILE_CONFIG_REMEDY);
            (
                top(&b, PROJECTS).map_err(torn)?,
                top(&b, MCP_SERVERS).map_err(torn)?,
            )
        }
        Read::Absent => {
            tracing::warn!(
                "{} is gone, so its session has nothing to merge back",
                profile.display()
            );
            remove_own_file(dir, BASELINE_FILE)?;
            return Ok(MergeReport::default());
        }
        Read::Unreadable(_) => return Err(unsplicable(&profile, PROFILE_CONFIG_REMEDY)),
    };
    let paths = CcPaths::resolve(env);
    let lock = locks::acquire_config(&paths, budget, cancel)?;
    let fence = || lock.check_owned().map_err(ProviderError::from);
    let config = &paths.global_config;
    let torn = |_: SpliceError| unsplicable(config, CONFIG_REMEDY);
    let before = match read_bytes(config) {
        Read::Present(b) => Some(b),
        Read::Absent => None,
        Read::Unreadable(_) => return Err(unsplicable(config, CONFIG_REMEDY)),
    };
    let theirs = match &before {
        Some(b) => (
            top(b, PROJECTS).map_err(torn)?,
            top(b, MCP_SERVERS).map_err(torn)?,
        ),
        None => (Value::Null, Value::Null),
    };
    let merged = three_way(
        (&base.0, &base.1),
        (&mine.0, &mine.1),
        (&theirs.0, &theirs.1),
    );
    if merged.projects.is_some() || merged.mcp_servers.is_some() {
        // §9.5: a missing file is created holding only the merged keys.
        let mut new = before.unwrap_or_else(|| b"{}\n".to_vec());
        if let Some(v) = &merged.projects {
            new = splice::replace_top_level(&new, PROJECTS, v).map_err(torn)?;
        }
        if let Some(v) = &merged.mcp_servers {
            new = splice::replace_top_level(&new, MCP_SERVERS, v).map_err(torn)?;
        }
        write_atomic_with(config, &new, 0o600, fence)?;
    }
    // The config lock covers only the default file.
    drop(lock);
    remove_own_file(dir, BASELINE_FILE)?;
    Ok(MergeReport {
        applied: merged.applied.len(),
        conflicts: merged.conflicts.iter().map(key_name).collect(),
    })
}

/// §12.5 "Environment": scrubbed from every session, because each supplies or redirects the
/// login, or renames CC's config file or Keychain item (Appendix A.1, A.7). Every
/// `CLAUDE_CODE_*_FILE_DESCRIPTOR` set in the process joins them (`session_env`).
pub(crate) const CC_SCRUB: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_REFRESH_TOKEN",
    "CLAUDE_CODE_OAUTH_SCOPES",
    "CLAUDE_CODE_OAUTH_CLIENT_ID",
    "CLAUDE_CODE_ACCOUNT_UUID",
    "CLAUDE_CODE_USER_EMAIL",
    "CLAUDE_CODE_ORGANIZATION_UUID",
    "ANTHROPIC_PROFILE",
    "ANTHROPIC_CONFIG_DIR",
    "ANTHROPIC_FEDERATION_RULE_ID",
    "ANTHROPIC_IDENTITY_TOKEN",
    "ANTHROPIC_IDENTITY_TOKEN_FILE",
    "CLAUDE_CODE_CUSTOM_OAUTH_URL",
    "USE_LOCAL_OAUTH",
    "USE_STAGING_OAUTH",
    "CLAUDE_SECURESTORAGE_CONFIG_DIR",
];

const DESCRIPTOR_PREFIX: &[u8] = b"CLAUDE_CODE_";
const DESCRIPTOR_SUFFIX: &[u8] = b"_FILE_DESCRIPTOR";

/// `CLAUDE_CODE_*_FILE_DESCRIPTOR`, the `*` any run of bytes, possibly empty.
fn is_token_descriptor(name: &OsStr) -> bool {
    let b = name.as_bytes();
    b.len() >= DESCRIPTOR_PREFIX.len() + DESCRIPTOR_SUFFIX.len()
        && b.starts_with(DESCRIPTOR_PREFIX)
        && b.ends_with(DESCRIPTOR_SUFFIX)
}

/// §12.5: `CLAUDE_CONFIG_DIR` set to the recorded spelling. `CC_SCRUB` is removed, then every
/// name in `present` (the process environment's names) that is a token file descriptor,
/// sorted, once each.
pub(crate) fn session_env(
    spelling: &str,
    present: impl IntoIterator<Item = OsString>,
) -> SessionEnv {
    let mut descriptors: Vec<OsString> = present
        .into_iter()
        .filter(|n| is_token_descriptor(n))
        .collect();
    descriptors.sort();
    descriptors.dedup();
    SessionEnv {
        set: vec![(OsString::from(CONFIG_DIR), OsString::from(spelling))],
        remove: CC_SCRUB
            .iter()
            .map(OsString::from)
            .chain(descriptors)
            .collect(),
    }
}

/// `claude auth status`'s limit (§12.3 step 8).
pub(crate) const AUTH_STATUS_TIMEOUT: Duration = Duration::from_secs(10);
/// What an interrupted login check reports (§12.5); the caller maps it by reading the token.
pub(crate) const INTERRUPTED: &str = "interrupted";
/// The account's own login (§12.3 step 8): CC's OAuth accounts, and setup-token accounts
/// (*inferred*).
const CLAUDE_AI: &str = "claude.ai";

/// The fields of `claude auth status --json` that §12.3's table reads (Appendix A.7).
struct AuthStatus {
    logged_in: bool,
    auth_method: String,
    config_directory: String,
    email: Option<String>,
    org_id: Option<String>,
    api_key_source: Option<String>,
}

impl AuthStatus {
    /// `None` unless `stdout` is one JSON object holding the three fields CC always prints.
    fn parse(stdout: &[u8]) -> Option<Self> {
        let v: Value = serde_json::from_slice(stdout).ok()?;
        let text = |k: &str| {
            v.get(k)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        Some(Self {
            logged_in: v.get("loggedIn")?.as_bool()?,
            auth_method: v.get("authMethod")?.as_str()?.to_owned(),
            config_directory: v.get("configDirectory")?.as_str()?.to_owned(),
            email: text("email"),
            org_id: text("orgId"),
            api_key_source: text("apiKeySource"),
        })
    }
}

/// §12.3 step 8's table, from what `claude auth status --json` did. Rows are tried in order:
/// 1. A reply about another config dir says nothing about this profile (`drifted`).
/// 2. Then not logged in (`invalid`), unless another method claims not to be logged in
///    (`unknown`: the reply contradicts itself).
/// 3. Then another method (`overridden`).
/// 4. Then another account or org (`invalid`), which needs the email to be there to say so.
/// 5. Then `valid`, which also needs exit 0.
///
/// A login that cannot be confirmed is `unknown`, never `invalid`: only `invalid` deletes a
/// profile. No detail names an email (§4.4).
pub(crate) fn validity(reply: Captured, spelling: &str, expect: &Identity) -> Validity {
    let (code, signal, stdout) = match reply {
        Captured::Exited {
            code,
            signal,
            stdout,
            ..
        } => (code, signal, stdout),
        Captured::TimedOut => {
            return Validity::Unknown(format!(
                "`claude auth status` did not answer within {} s",
                AUTH_STATUS_TIMEOUT.as_secs()
            ));
        }
        Captured::Interrupted(_) => return Validity::Unknown(INTERRUPTED.into()),
        Captured::SpawnFailed(e) => return Validity::Unreachable(e),
    };
    let ended = match (code, signal) {
        (Some(c), _) => format!("exit {c}"),
        (None, Some(s)) => format!("signal {s}"),
        (None, None) => "no exit status".into(),
    };
    let Some(status) = AuthStatus::parse(&stdout) else {
        return Validity::Unknown(format!(
            "`claude auth status` printed no status tagteam can read ({ended})"
        ));
    };
    if status.config_directory != spelling {
        return Validity::Drifted {
            reported: status.config_directory,
        };
    }
    if status.auth_method == "none" {
        return Validity::Invalid("not logged in".into());
    }
    if !status.logged_in {
        // Only the account's own login (or none) says it is gone; another method that claims
        // not to be logged in contradicts itself, and `invalid` deletes the profile.
        return if status.auth_method == CLAUDE_AI {
            Validity::Invalid("not logged in".into())
        } else {
            Validity::Unknown("inconsistent login state".into())
        };
    }
    if status.auth_method != CLAUDE_AI {
        return Validity::Overridden {
            method: status.auth_method,
            source: status.api_key_source,
        };
    }
    let Some(email) = status.email else {
        return Validity::Unknown(
            "`claude auth status` named no email for the claude.ai login".into(),
        );
    };
    if email != expect.email.as_deref().unwrap_or(&expect.label) {
        return Validity::Invalid("logged in to claude.ai as another account".into());
    }
    if status
        .org_id
        .as_deref()
        .is_some_and(|org| !expect.org_uuid.is_empty() && org != expect.org_uuid)
    {
        return Validity::Invalid("logged in to claude.ai in another organization".into());
    }
    if code != Some(0) {
        return Validity::Unknown(format!(
            "`claude auth status` reported this login but ended with {ended}"
        ));
    }
    Validity::Valid
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;

    #[test]
    fn every_token_file_descriptor_the_process_holds_is_scrubbed_too() {
        let not_utf8 = OsString::from_vec(b"CLAUDE_CODE_\xff_FILE_DESCRIPTOR".to_vec());
        let present = [
            "PATH",
            "CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR",
            "CLAUDE_CODE_API_KEY_FILE_DESCRIPTOR",
            "CLAUDE_CODE__FILE_DESCRIPTOR",
            "CLAUDE_CODE_FILE_DESCRIPTOR",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "XCLAUDE_CODE_A_FILE_DESCRIPTOR",
            "CLAUDE_CODE_A_FILE_DESCRIPTOR_X",
            "CLAUDE_CODE_API_KEY_FILE_DESCRIPTOR",
        ]
        .map(OsString::from)
        .into_iter()
        .chain([not_utf8.clone()]);

        let env = session_env("/p/0192", present);

        assert_eq!(
            env.set,
            [(
                OsString::from("CLAUDE_CONFIG_DIR"),
                OsString::from("/p/0192")
            )]
        );
        let (literal, expanded) = env.remove.split_at(CC_SCRUB.len());
        assert_eq!(
            literal.to_vec(),
            CC_SCRUB.iter().map(OsString::from).collect::<Vec<_>>()
        );
        assert_eq!(
            expanded,
            [
                OsString::from("CLAUDE_CODE_API_KEY_FILE_DESCRIPTOR"),
                OsString::from("CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR"),
                OsString::from("CLAUDE_CODE__FILE_DESCRIPTOR"),
                not_utf8,
            ],
            "sorted, once each; `CLAUDE_CODE_FILE_DESCRIPTOR` is too short to match"
        );
    }
}
