//! §13.6's Claude Code checks (`Provider::doctor_checks`): the binary and its version against
//! the tested one, the paths and Keychain names Claude Code resolves here, the Keychain's
//! state, the environment a `claude` started here inherits, the default home's login, and lock
//! directories left past their staleness. They read and probe only: nothing is written or
//! unlocked, nothing asks, and every spawn is bounded by its timeout (Decision 2).

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use tagteam_provider::doctor::quoted;
use tagteam_provider::process::{Captured, ProcessSpawner, SpawnSpec, find_on_path};
use tagteam_provider::{Cancel, Check, Env, Keychain, LockState, Read};

use crate::config;
use crate::live::{LiveStore, Platform};
use crate::locks::{CONFIG_STALE, CRED_STALE, STORAGE_WRITE_STALE};
use crate::naming::{ItemKind, keychain_account, keychain_service};
use crate::paths::CcPaths;
use crate::session::{AUTH_STATUS_TIMEOUT, AuthStatus, CC_SCRUB, CLAUDE_AI, is_token_descriptor};

/// The newest Claude Code release tagteam was checked against (§15.4).
const TESTED_CC_VERSION: &str = include_str!("../compat/tested-cc-version");
/// How long `claude --version` may take.
const VERSION_TIMEOUT: Duration = Duration::from_secs(10);
/// Appendix A.1's non-production OAuth switches.
const NON_PRODUCTION_OAUTH: &[&str] = &[
    "CLAUDE_CODE_CUSTOM_OAUTH_URL",
    "USE_LOCAL_OAUTH",
    "USE_STAGING_OAUTH",
];
const CONFIG_DIR: &str = "CLAUDE_CONFIG_DIR";
const SECURE_STORAGE_DIR: &str = "CLAUDE_SECURESTORAGE_CONFIG_DIR";
/// Appendix A.2's account name when `$USER` and the passwd name are both unusable.
const FALLBACK_ACCOUNT: &str = "claude-code-user";

/// `MAJOR.MINOR.PATCH` as three numbers, compared in that order.
pub(crate) type Version = (u64, u64, u64);

/// `MAJOR.MINOR.PATCH`, each part ASCII digits and nothing else (no sign, no space).
fn version(s: &str) -> Option<Version> {
    let mut parts = s.split('.').map(|p| {
        (!p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
            .then(|| p.parse::<u64>().ok())
            .flatten()
    });
    let v = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(v)
}

fn shown((a, b, c): Version) -> String {
    format!("{a}.{b}.{c}")
}

/// `tested-cc-version`'s version: its one line that is neither blank nor a `#` comment, which
/// must be `MAJOR.MINOR.PATCH`. Two such lines are no version. The one reader of the file in
/// Rust: `compat.rs`'s pinning test (Task 12) reads it through this too.
pub(crate) fn tested_version(text: &str) -> Option<Version> {
    let mut entries = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'));
    let line = entries.next()?;
    entries.next().is_none().then_some(())?;
    version(line)
}

/// The tested version (§15.4).
fn tested() -> Option<Version> {
    tested_version(TESTED_CC_VERSION)
}

/// What `claude --version` prints: `X.Y.Z (Claude Code)` on its first line.
fn reported(stdout: &[u8]) -> Option<Version> {
    let line = String::from_utf8_lossy(stdout);
    let (v, rest) = line.lines().next()?.trim().split_once(' ')?;
    (rest.trim() == "(Claude Code)").then_some(())?;
    version(v)
}

/// The scheme and host of `url`, as a root URL (`https://api.anthropic.com/`).
pub(crate) fn origin(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let host = rest.split('/').next().filter(|h| !h.is_empty())?;
    Some(format!("{scheme}://{host}/"))
}

/// Whether `dir` is a spelling of the default home, `~/.claude` (Appendix A.2).
fn names_default_home(env: &Env, dir: &OsStr) -> bool {
    let default = env.home.join(".claude");
    let given = Path::new(dir);
    given == default
        || fs::canonicalize(given).is_ok_and(|c| fs::canonicalize(&default).is_ok_and(|d| c == d))
}

/// `env` with the config home set to `dir` and secure storage undefined: how an item named
/// from `dir` is named (Appendix A.2).
fn named_from(env: &Env, dir: &Path) -> Env {
    let mut at = env.clone();
    at.claude_config_dir = Some(dir.as_os_str().to_owned());
    at.claude_securestorage_config_dir = None;
    at
}

fn delete_item(svc: &str, acct: &str) -> String {
    format!("security delete-generic-password -s '{svc}' -a '{acct}'")
}

/// §13.6's Claude Code checks, in `env`, the effective (outer) environment.
pub(crate) fn checks(
    live: &LiveStore,
    env: &Env,
    spawner: &dyn ProcessSpawner,
    cancel: &Cancel,
) -> Vec<Check> {
    let mut out = Vec::new();
    let paths = CcPaths::resolve(env);
    let claude = find_on_path("claude", env.var("PATH"));
    binary(claude.as_deref(), spawner, cancel, &mut out);
    let mac = live.platform() == Platform::MacOs;
    let acct = keychain_account(env);
    let mut where_ = format!(
        "Claude Code's config home is {}, its global config {}, and its secure storage {}",
        paths.config_home.display(),
        paths.global_config.display(),
        paths.secure_storage_dir.display()
    );
    if mac {
        where_.push_str(&format!(
            "; its login is the Keychain item {} of account {acct}",
            keychain_service(env, ItemKind::OAuth)
        ));
    }
    out.push(Check::info("cc.paths", where_));
    let state = mac.then(|| keychain(live.keychain(), env, &acct, &mut out));
    environment(env, &mut out);
    // `claude auth status` reads the stored login, which a keychain that is locked, or whose
    // state cannot be told, would ask SecurityAgent to unlock (§13.6): `cc.keychain` has said
    // that what reads it is skipped.
    let keychain_readable = state.is_none_or(|s| s == LockState::Unlocked);
    if let Some(bin) = claude.as_deref().filter(|_| keychain_readable) {
        auth(bin, env, &paths, spawner, cancel, &mut out);
    }
    locks(&paths, &mut out);
    out
}

/// `claude --version` against the tested version (§15.4): newer warns, and whichever of the two
/// cannot be read is named as the one that cannot.
fn version_check(reported: Option<Version>, tested: Option<Version>) -> Check {
    match (reported, tested) {
        (Some(v), Some(t)) if v > t => Check::warn(
            "cc.version",
            format!(
                "Claude Code {} is newer than {}, the newest version tagteam was checked against",
                shown(v),
                shown(t)
            ),
        )
        .fix("watch for a tagteam release that covers it; until then, what it changed may not be handled"),
        (Some(v), Some(t)) => Check::ok(
            "cc.version",
            format!("Claude Code {} (checked against {})", shown(v), shown(t)),
        ),
        (_, None) => Check::warn(
            "cc.version",
            "the tested Claude Code version (compat/tested-cc-version) built into this tagteam cannot be read, so `claude`'s version is not compared with it",
        )
        .fix("this tagteam build is damaged: reinstall tagteam or update it"),
        (None, Some(_)) => Check::warn(
            "cc.version",
            "`claude --version` printed a version tagteam cannot read",
        )
        .fix("run `claude --version` yourself; if it does not print `X.Y.Z (Claude Code)`, update Claude Code or tagteam"),
    }
}

/// The binary on `PATH`, and its version against the tested one (§15.4): newer warns.
fn binary(
    claude: Option<&Path>,
    spawner: &dyn ProcessSpawner,
    cancel: &Cancel,
    out: &mut Vec<Check>,
) {
    let Some(bin) = claude else {
        out.push(
            Check::warn(
                "cc.binary",
                "`claude` is not on PATH, so `tagteam run` cannot start it",
            )
            .fix("install Claude Code, or add its directory to PATH"),
        );
        return;
    };
    out.push(Check::ok(
        "cc.binary",
        format!("`claude` is {}", bin.display()),
    ));
    let spec = SpawnSpec {
        program: bin.to_path_buf(),
        args: vec![OsString::from("--version")],
        ..SpawnSpec::default()
    };
    let check = match spawner.run_captured(&spec, VERSION_TIMEOUT, cancel) {
        Captured::Exited {
            code: Some(0),
            stdout,
            ..
        } => version_check(reported(&stdout), tested()),
        Captured::Exited { code, signal, .. } => Check::warn(
            "cc.version",
            format!(
                "`claude --version` ended with {}",
                code.map_or_else(
                    || format!("signal {}", signal.unwrap_or_default()),
                    |c| format!("exit {c}")
                )
            ),
        )
        .fix("run `claude --version` yourself to see why; reinstall Claude Code if it fails"),
        Captured::TimedOut => Check::warn(
            "cc.version",
            format!(
                "`claude --version` did not answer within {} s",
                VERSION_TIMEOUT.as_secs()
            ),
        )
        .fix("run `claude --version` yourself; if it hangs, reinstall Claude Code"),
        Captured::SpawnFailed(e) => Check::warn(
            "cc.version",
            format!("`claude --version` could not be run: {e}"),
        )
        .fix(format!(
            "check that {} is an executable this user may run",
            quoted(bin)
        )),
        Captured::Interrupted(_) => Check::warn("cc.version", "`claude --version` was interrupted")
            .fix("run `tagteam doctor` again"),
    };
    out.push(check);
}

/// Appendix A.2 and A.3: the lock state, the account name, an empty managed-key item, and the
/// items under a former fallback name of this home. Only the managed-key check reads a
/// secret, and only from an unlocked keychain; the others read attributes or nothing. Returns
/// the lock state, which decides whether `claude auth status` may run.
fn keychain(kc: &dyn Keychain, env: &Env, acct: &str, out: &mut Vec<Check>) -> LockState {
    let state = kc.lock_state();
    out.push(match state {
        LockState::Unlocked => Check::ok("cc.keychain", "the login keychain is unlocked"),
        LockState::Locked => Check::warn(
            "cc.keychain",
            "the login keychain is locked (common over SSH): `claude` started here cannot read its login, and doctor skipped what reads it, `claude auth status` included",
        )
        .fix("`security unlock-keychain ~/Library/Keychains/login.keychain-db`"),
        LockState::Unknown => Check::warn(
            "cc.keychain",
            "the login keychain's lock state cannot be told, so doctor skipped what reads it, `claude auth status` included",
        )
        .fix("`security show-keychain-info` shows why"),
    });
    out.push(if acct == FALLBACK_ACCOUNT {
        Check::info(
            "cc.keychain-account",
            format!(
                "Claude Code's Keychain account falls back to {FALLBACK_ACCOUNT}: neither $USER nor the passwd name is a plain name"
            ),
        )
    } else {
        Check::ok(
            "cc.keychain-account",
            format!("Claude Code's Keychain account is {acct}"),
        )
    });
    if state == LockState::Unlocked {
        let svc = keychain_service(env, ItemKind::ManagedKey);
        match kc.find(&svc, acct) {
            Read::Present(key) if key.is_empty() => out.push(
                Check::fail(
                    "cc.managed-key",
                    format!("the managed-key item {svc} is present but empty: every switch refuses on it"),
                )
                .fix(delete_item(&svc, acct)),
            ),
            Read::Present(_) | Read::Absent => {}
            Read::Unreadable(e) => out.push(
                Check::warn(
                    "cc.managed-key",
                    format!("the managed-key item {svc} cannot be read: {e}"),
                )
                .fix(
                    "the Keychain would not give the item up: unlock the login keychain, or allow `tagteam` access to it when macOS asks, then `tagteam doctor` again",
                ),
            ),
        }
    }
    for svc in former_items(env) {
        match kc.exists(&svc, acct) {
            Read::Present(()) => out.push(
                Check::info(
                    "cc.former-item",
                    format!(
                        "the Keychain item {svc} is one Claude Code read for this home before 2.1.286; it now reads {} only, so {svc} is inert",
                        keychain_service(env, if svc.contains("-credentials") { ItemKind::OAuth } else { ItemKind::ManagedKey })
                    ),
                )
                .fix(delete_item(&svc, acct)),
            ),
            Read::Absent => {}
            Read::Unreadable(e) => out.push(
                Check::warn(
                    "cc.former-item",
                    format!(
                        "whether the Keychain holds {svc}, an item Claude Code read for this home before 2.1.286, cannot be told: {e}"
                    ),
                )
                .fix(
                    "the Keychain would not answer for the item: unlock the login keychain, or allow `tagteam` access to it when macOS asks, then `tagteam doctor` again",
                ),
            ),
        }
    }
    state
}

/// Appendix A.2's former fallback names of the home `env` names: the unsuffixed items when
/// `CLAUDE_CONFIG_DIR` names the default home, and the items named from a symlinked config
/// dir's resolved target. None differs from what Claude Code now reads.
fn former_items(env: &Env) -> Vec<String> {
    if env.claude_securestorage_config_dir.is_some() {
        return Vec::new();
    }
    let Some(dir) = env.claude_config_dir.as_deref().filter(|d| !d.is_empty()) else {
        return Vec::new();
    };
    let kinds = [ItemKind::OAuth, ItemKind::ManagedKey];
    let mut names = Vec::new();
    if names_default_home(env, dir) {
        let mut default = env.clone();
        default.claude_config_dir = None;
        names.extend(kinds.map(|k| keychain_service(&default, k)));
    }
    let link = Path::new(dir);
    if fs::symlink_metadata(link).is_ok_and(|m| m.file_type().is_symlink()) {
        if let Ok(target) = fs::canonicalize(link) {
            names.extend(kinds.map(|k| keychain_service(&named_from(env, &target), k)));
        }
    }
    let current: Vec<String> = kinds.map(|k| keychain_service(env, k)).to_vec();
    names.retain(|n| !current.contains(n));
    names.dedup();
    names
}

/// The variables a `claude` started from this shell inherits (Appendix A.1, A.7, §12.5).
fn environment(env: &Env, out: &mut Vec<Check>) {
    let before = out.len();
    match env.claude_config_dir.as_deref() {
        Some(v) if v.is_empty() => out.push(
            Check::warn(
                "cc.env.config-dir",
                "CLAUDE_CONFIG_DIR is set but empty: Claude Code 2.1.286 then puts some of its paths in the working directory, and reads another Keychain item than a switch writes",
            )
            .fix("unset CLAUDE_CONFIG_DIR"),
        ),
        Some(v) if names_default_home(env, v) => out.push(
            Check::warn(
                "cc.env.config-dir",
                format!(
                    "CLAUDE_CONFIG_DIR={} names the default home: Claude Code 2.1.286 then reads the Keychain item {}, not the one a switch run without it writes",
                    v.to_string_lossy(),
                    keychain_service(env, ItemKind::OAuth)
                ),
            )
            .fix("unset CLAUDE_CONFIG_DIR"),
        ),
        _ => {}
    }
    for name in NON_PRODUCTION_OAUTH {
        if env.var(name).is_some() {
            out.push(
                Check::warn(
                    "cc.env.oauth",
                    format!(
                        "{name} is set: Claude Code then uses non-production OAuth names for its config and Keychain items, which tagteam does not manage"
                    ),
                )
                .fix(format!("unset {name}")),
            );
        }
    }
    if let Some(dir) = &env.claude_securestorage_config_dir {
        out.push(
            Check::warn(
                "cc.env.scrubbed",
                format!(
                    "{SECURE_STORAGE_DIR} is set ({}): it moves Claude Code's credential out of its config home",
                    dir.to_string_lossy()
                ),
            )
            .fix(format!("unset {SECURE_STORAGE_DIR}")),
        );
    }
    for name in env.vars.keys() {
        let scrubbed = CC_SCRUB.contains(&name.as_str())
            && !NON_PRODUCTION_OAUTH.contains(&name.as_str())
            && name != SECURE_STORAGE_DIR;
        if scrubbed || is_token_descriptor(OsStr::new(name)) {
            out.push(
                Check::warn(
                    "cc.env.scrubbed",
                    format!(
                        "{name} is set: it supplies or redirects Claude Code's login, so `claude` started here may not use the account tagteam switched to"
                    ),
                )
                .fix(format!("unset {name}")),
            );
        }
    }
    if out.len() == before {
        out.push(Check::ok(
            "cc.env",
            "no variable redirects Claude Code's login or its names",
        ));
    }
}

/// Where to run `claude auth status` by hand: in the home doctor checked, and outside any
/// `tagteam run` session, whose own profile it would otherwise ask about (§12.8).
fn auth_fix(paths: &CcPaths) -> String {
    format!(
        "run `claude auth status` yourself outside any `tagteam run` session, with CLAUDE_CONFIG_DIR as it is here (the home doctor checked: {}), to see why, then `tagteam doctor` again",
        paths.config_home.display()
    )
}

/// The default home's login, by `claude auth status` (Appendix A.7) in the outer home's
/// environment: `env`'s two home variables replace the process's, so inside a run shell it
/// asks about the default home, not the profile (§12.8). Claude Code writes nothing there once it
/// has initialized the global config (§15.4). Where the config is absent, or present but not yet
/// initialized by Claude Code (one tagteam seeded, or the first start after an upgrade), its
/// start-up writes the config and `backups/` and may leave a config lock behind (Appendix A.7).
/// So `claude` is not run where the global config does not exist (§13.6): a missing one is
/// reported as a home Claude Code has not been started in, and one that cannot be told is a
/// warning. Where it exists, doctor still cannot tell an initialized config from a seeded one,
/// and the `claude` it runs may make those start-up writes (B.67).
fn auth(
    bin: &Path,
    env: &Env,
    paths: &CcPaths,
    spawner: &dyn ProcessSpawner,
    cancel: &Cancel,
    out: &mut Vec<Check>,
) {
    // (what could not be read, whether the path is a link: a dangling link is no absent file)
    let unstarted = match fs::symlink_metadata(&paths.global_config) {
        Err(e) => Some((e, false)),
        Ok(meta) if meta.file_type().is_symlink() => {
            fs::metadata(&paths.global_config).err().map(|e| (e, true))
        }
        Ok(_) => None,
    };
    match unstarted {
        Some((e, false)) if e.kind() == ErrorKind::NotFound => {
            out.push(Check::info(
                "cc.auth",
                format!(
                    "Claude Code has not been started in this home ({} does not exist), so `claude auth status` was not run: it would create the config",
                    paths.global_config.display()
                ),
            ));
            return;
        }
        Some((e, _)) => {
            out.push(
                Check::warn(
                    "cc.auth",
                    format!(
                        "whether Claude Code has been started in this home cannot be told: {} cannot be read ({}), so `claude auth status` was not run",
                        paths.global_config.display(),
                        e.kind()
                    ),
                )
                .fix(format!(
                    "make {} and the directories above it readable by this user",
                    quoted(&paths.global_config)
                )),
            );
            return;
        }
        None => {}
    }
    let mut spec = SpawnSpec {
        program: bin.to_path_buf(),
        args: ["auth", "status", "--json"].map(OsString::from).to_vec(),
        ..SpawnSpec::default()
    };
    for (name, value) in [
        (CONFIG_DIR, &env.claude_config_dir),
        (SECURE_STORAGE_DIR, &env.claude_securestorage_config_dir),
    ] {
        match value {
            Some(v) => spec.set.push((OsString::from(name), v.clone())),
            None => spec.remove.push(OsString::from(name)),
        }
    }
    let stdout = match spawner.run_captured(&spec, AUTH_STATUS_TIMEOUT, cancel) {
        Captured::Exited { stdout, .. } => stdout,
        Captured::TimedOut => {
            out.push(
                Check::warn(
                    "cc.auth",
                    format!(
                        "`claude auth status` did not answer within {} s",
                        AUTH_STATUS_TIMEOUT.as_secs()
                    ),
                )
                .fix(auth_fix(paths)),
            );
            return;
        }
        Captured::SpawnFailed(e) => {
            out.push(
                Check::warn(
                    "cc.auth",
                    format!("`claude auth status` could not be run: {e}"),
                )
                .fix(auth_fix(paths)),
            );
            return;
        }
        Captured::Interrupted(_) => {
            out.push(
                Check::warn("cc.auth", "`claude auth status` was interrupted")
                    .fix("run `tagteam doctor` again"),
            );
            return;
        }
    };
    let Some(status) = AuthStatus::parse(&stdout) else {
        out.push(
            Check::warn(
                "cc.auth",
                "`claude auth status` printed no status tagteam can read",
            )
            .fix(auth_fix(paths)),
        );
        return;
    };
    if !status.logged_in || status.auth_method == "none" {
        out.push(
            Check::info("cc.auth", "`claude` started here is not logged in")
                .fix("log in with `claude`, or `tagteam switch` to a stored account"),
        );
        return;
    }
    if status.auth_method != CLAUDE_AI {
        let source = status
            .api_key_source
            .map_or_else(String::new, |s| format!(" from {s}"));
        out.push(
            Check::warn(
                "cc.auth",
                format!(
                    "`claude` started here logs in by {}{source}, not by its stored claude.ai login, so a switch changes nothing for it",
                    status.auth_method
                ),
            )
            .fix("remove what supplies that login: the variable, or the `apiKeyHelper` or `env` entry in Claude Code's settings"),
        );
        return;
    }
    let live = match config::live_identity(paths) {
        Read::Present(live) => live,
        Read::Absent => {
            out.push(Check::ok(
                "cc.auth",
                "`claude` started here is logged in to claude.ai",
            ));
            return;
        }
        Read::Unreadable(e) => {
            out.push(
                Check::warn(
                    "cc.auth",
                    format!(
                        "`claude` started here is logged in to claude.ai, but whether as the live login cannot be told: {e}"
                    ),
                )
                .fix(format!(
                    "check that {} is readable JSON",
                    quoted(&paths.global_config)
                )),
            );
            return;
        }
    };
    let email = live.email.as_deref().unwrap_or(&live.label);
    let other_email = status.email.as_deref().is_some_and(|e| e != email);
    let other_org = status
        .org_id
        .as_deref()
        .is_some_and(|o| !live.org_uuid.is_empty() && o != live.org_uuid);
    out.push(if other_email || other_org {
        Check::warn(
            "cc.auth",
            format!(
                "`claude auth status` names another {} than the live login in {}",
                if other_email {
                    "account"
                } else {
                    "organization"
                },
                paths.global_config.display()
            ),
        )
        .fix("log in again with `claude`, or `tagteam switch --force` to the account you want live")
    } else {
        Check::ok(
            "cc.auth",
            "`claude` started here is logged in to claude.ai as the live login",
        )
    });
}

/// §9.1's lock directories, present and older than their staleness window: a crashed process
/// left them. Judged by wall-clock age, as `proper-lockfile` judges it.
fn locks(paths: &CcPaths, out: &mut Vec<Check>) {
    let before = out.len();
    let now = SystemTime::now();
    let dirs: [(PathBuf, Duration); 5] = [
        (paths.refresh_lock.clone(), CRED_STALE),
        (paths.legacy_lock(), CRED_STALE),
        (paths.config_lock.clone(), CONFIG_STALE),
        (paths.storage_write_lock.clone(), STORAGE_WRITE_STALE),
        (paths.storage_write_lock_v2.clone(), STORAGE_WRITE_STALE),
    ];
    for (dir, stale) in dirs {
        let untold = |e: std::io::Error| {
            Check::warn(
                "cc.locks",
                format!(
                    "whether {} is a stale Claude Code lock cannot be told: it cannot be read ({})",
                    dir.display(),
                    e.kind()
                ),
            )
            .fix(format!(
                "make {} and the directories above it readable by this user",
                quoted(&dir)
            ))
        };
        let meta = match fs::symlink_metadata(&dir) {
            Ok(meta) => meta,
            Err(e) if e.kind() == ErrorKind::NotFound => continue,
            Err(e) => {
                out.push(untold(e));
                continue;
            }
        };
        if !meta.is_dir() {
            continue;
        }
        let modified = match meta.modified() {
            Ok(modified) => modified,
            Err(e) => {
                out.push(untold(e));
                continue;
            }
        };
        // A modification time ahead of the clock gives no age: a skewed clock, not a crash.
        let age = now.duration_since(modified).unwrap_or_default();
        if age > stale {
            out.push(
                Check::warn(
                    "cc.locks",
                    format!(
                        "{} is a Claude Code lock {} s old, past its {} s staleness: a process that crashed left it",
                        dir.display(),
                        age.as_secs(),
                        stale.as_secs()
                    ),
                )
                .fix(format!(
                    "nothing to do: the next writer takes it over; if it stays, `rmdir {}` once no `claude` runs",
                    quoted(&dir)
                )),
            );
        }
    }
    if out.len() == before {
        out.push(Check::ok("cc.locks", "no Claude Code lock is stale"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_as_numbers() {
        assert_eq!(version("2.1.286"), Some((2, 1, 286)));
        assert!(version("2.1.300").unwrap() > version("2.1.286").unwrap());
        assert!(version("2.10.0").unwrap() > version("2.9.999").unwrap());
        for bad in ["2.1", "2.1.2.3", "2.x.1", "", "v2.1.286"] {
            assert_eq!(version(bad), None, "{bad}");
        }
    }

    #[test]
    fn an_unreadable_tested_version_is_blamed_on_the_tested_version() {
        let bad_tested = version_check(Some((2, 1, 286)), None);
        assert!(
            bad_tested.message.contains("compat/tested-cc-version"),
            "{}",
            bad_tested.message
        );
        assert!(!bad_tested.message.contains("`claude --version` printed"));
        assert!(bad_tested.fix.is_some());
        let bad_claude = version_check(None, Some((2, 1, 286)));
        assert!(
            bad_claude.message.contains("`claude --version` printed"),
            "{}",
            bad_claude.message
        );
    }

    #[test]
    fn the_tested_version_file_has_one_version() {
        assert!(tested().is_some(), "compat/tested-cc-version");
    }

    #[test]
    fn claude_version_output_is_read_from_its_first_line() {
        assert_eq!(reported(b"2.1.286 (Claude Code)\n"), Some((2, 1, 286)));
        assert_eq!(
            reported(b"2.1.286 (Claude Code)\nextra\n"),
            Some((2, 1, 286))
        );
        for bad in [
            &b"2.1.286\n"[..],
            b"Claude Code 2.1.286\n",
            b"",
            b"2.1 (Claude Code)",
        ] {
            assert_eq!(reported(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn an_origin_is_a_url_s_scheme_and_host() {
        assert_eq!(
            origin("https://api.anthropic.com/api/oauth/usage").as_deref(),
            Some("https://api.anthropic.com/")
        );
        assert_eq!(
            origin("http://127.0.0.1:9/v1/oauth/token").as_deref(),
            Some("http://127.0.0.1:9/")
        );
        assert_eq!(origin("no-scheme"), None);
        assert_eq!(origin("https:///path"), None);
    }
}
