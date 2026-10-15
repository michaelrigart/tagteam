//! D9's service units. `tagteam daemon install` and `uninstall` (the daemon crate) write and
//! load them, and `doctor` (D10) reads them, so they live in the engine (plan B Decision 4).
//! This module holds:
//! - where each unit lives, and the variables it records (D9.1)
//! - rendering and quoting (D9.2, D9.3), and a reader for exactly what is rendered
//!   (Decision 5)
//! - a read-only probe of what the service manager says of the unit
//!
//! `launchctl`, `systemctl` and `loginctl` are found on the environment's `PATH`, as `doctor`
//! finds `claude`. Each call goes through the `ProcessSpawner` port, bounded by the caller's
//! timeout (D9.4, D10). Each program is optional (D1.1): without it, what it would tell is
//! unknown, and the state says why.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::Duration;

use tagteam_provider::process::{Captured, ProcessSpawner, SpawnSpec, current_uid};
use tagteam_provider::{Cancel, Env};

use crate::daemon::find_on_path;

/// D9.2: the agent's label, and its plist's name.
pub const LAUNCHD_LABEL: &str = "io.github.michaelrigart.tagteam";
/// D9.3: the user unit's name.
pub const SYSTEMD_UNIT: &str = "tagteam.service";

/// D9.1: the variables a unit records, in this order, each only when it is set at install time.
/// They make the daemon resolve the same store, Claude Code home, proxy and log as the shell
/// that installed it. `PATH` is where it finds `notify-send` (D6.2). No other variable is
/// recorded.
pub const RECORDED_VARS: &[&str] = &[
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
    "CLAUDE_CONFIG_DIR",
    "CLAUDE_SECURESTORAGE_CONFIG_DIR",
    "HTTPS_PROXY",
    "https_proxy",
    "HTTP_PROXY",
    "http_proxy",
    "ALL_PROXY",
    "all_proxy",
    "NO_PROXY",
    "no_proxy",
    "TAGTEAM_LOG",
    "PATH",
];

/// What an error calls the program path. A variable is called by its name. Neither error ever
/// shows the value, because a proxy URL can carry a password.
const PROGRAM_PATH: &str = "the program path";
/// The words `launchctl print` uses for an agent launchd has not loaded.
const NOT_FOUND: &str = "Could not find service";

/// The service manager of a platform this build runs on (D9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceKind {
    Launchd,
    Systemd,
}

impl ServiceKind {
    /// launchd on macOS, systemd on every other platform this build runs on (Linux).
    pub fn for_platform(macos: bool) -> Self {
        if macos {
            ServiceKind::Launchd
        } else {
            ServiceKind::Systemd
        }
    }

    /// The JSON `kind` of `daemon status` and `install` (D8.2, D9.4).
    pub fn as_str(self) -> &'static str {
        match self {
            ServiceKind::Launchd => "launchd",
            ServiceKind::Systemd => "systemd",
        }
    }

    /// What a person calls the unit, in messages.
    pub fn noun(self) -> &'static str {
        match self {
            ServiceKind::Launchd => "launchd agent",
            ServiceKind::Systemd => "systemd user unit",
        }
    }

    /// The name the manager knows the unit by: the agent's label, or the unit's file name.
    pub fn label(self) -> &'static str {
        match self {
            ServiceKind::Launchd => LAUNCHD_LABEL,
            ServiceKind::Systemd => SYSTEMD_UNIT,
        }
    }
}

/// D9.2: `~/Library/LaunchAgents/<label>.plist`. D9.3:
/// `$XDG_CONFIG_HOME/systemd/user/tagteam.service`, `~/.config` standing in for an unset
/// `XDG_CONFIG_HOME` as systemd reads it.
pub fn unit_path(env: &Env, kind: ServiceKind) -> PathBuf {
    let path = match kind {
        ServiceKind::Launchd => env
            .home
            .join("Library/LaunchAgents")
            .join(format!("{LAUNCHD_LABEL}.plist")),
        ServiceKind::Systemd => env
            .xdg_config_home
            .clone()
            .unwrap_or_else(|| env.home.join(".config"))
            .join("systemd/user")
            .join(SYSTEMD_UNIT),
    };
    env.guard(path)
}

/// `gui/<uid>`: the launchd domain of this user's login session (D9.2).
pub fn launchd_domain() -> String {
    format!("gui/{}", current_uid())
}

/// `gui/<uid>/<label>`: the daemon's agent in that domain.
pub fn launchd_target() -> String {
    format!("{}/{LAUNCHD_LABEL}", launchd_domain())
}

/// Why a unit cannot be written. It names the variable, or the program path, never the value.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UnitError {
    /// D9.3 refuses a newline and a NUL. XML 1.0, which the plist is written in, cannot carry
    /// any other control character either.
    #[error(
        "{0} holds a newline, a NUL or another control character, which a unit file cannot carry"
    )]
    Unrepresentable(String),
    #[error("{0} is not valid UTF-8")]
    NotUtf8(String),
}

/// D9.1: the variables of `RECORDED_VARS` that `var` finds set, in that order, each with its
/// value as set, an empty one included. A value that is not UTF-8 cannot go into a unit.
pub fn recorded_env(
    var: &dyn Fn(&str) -> Option<OsString>,
) -> Result<Vec<(String, String)>, UnitError> {
    let mut recorded = Vec::new();
    for name in RECORDED_VARS {
        if let Some(value) = var(name) {
            let value = value
                .into_string()
                .map_err(|_| UnitError::NotUtf8((*name).to_owned()))?;
            recorded.push(((*name).to_owned(), value));
        }
    }
    Ok(recorded)
}

/// Refuses every control character but the tab in `text`, which `name` names.
fn carried(name: &str, text: &str) -> Result<(), UnitError> {
    if text.chars().any(|c| c.is_control() && c != '\t') {
        return Err(UnitError::Unrepresentable(name.to_owned()));
    }
    Ok(())
}

/// The program path as a unit's text: UTF-8, and carried.
fn program_text(program: &Path) -> Result<&str, UnitError> {
    let text = program
        .to_str()
        .ok_or_else(|| UnitError::NotUtf8(PROGRAM_PATH.to_owned()))?;
    carried(PROGRAM_PATH, text)?;
    Ok(text)
}

/// XML 1.0's five escapes, for a plist `<key>` or `<string>`.
fn xml(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// The inverse of `xml`, `&amp;` last so an escaped entity stays one. `parse_unit` checks the
/// result by rendering it again.
fn unxml(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// D9.2: the agent. Its keys are in a fixed order, indented with tabs as `plutil` writes them:
/// - `Label`
/// - `ProgramArguments` `[program, "daemon"]`
/// - `EnvironmentVariables` (left out when there is none)
/// - `RunAtLoad` true
/// - `KeepAlive` `{SuccessfulExit: false}`: restarted after a failure and after exit 75,
///   never after exit 0
/// - `ProcessType` `Background`
/// - `StandardOutPath` and `StandardErrorPath` `/dev/null`, since the log file is the record
pub fn render_launchd(program: &Path, env: &[(String, String)]) -> Result<String, UnitError> {
    let program = program_text(program)?;
    let mut s = String::new();
    s.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    s.push_str("<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n");
    s.push_str("<plist version=\"1.0\">\n<dict>\n");
    s.push_str(&format!(
        "\t<key>Label</key>\n\t<string>{LAUNCHD_LABEL}</string>\n"
    ));
    s.push_str("\t<key>ProgramArguments</key>\n\t<array>\n");
    s.push_str(&format!("\t\t<string>{}</string>\n", xml(program)));
    s.push_str("\t\t<string>daemon</string>\n\t</array>\n");
    if !env.is_empty() {
        s.push_str("\t<key>EnvironmentVariables</key>\n\t<dict>\n");
        for (name, value) in env {
            carried(name, name)?;
            carried(name, value)?;
            s.push_str(&format!(
                "\t\t<key>{}</key>\n\t\t<string>{}</string>\n",
                xml(name),
                xml(value)
            ));
        }
        s.push_str("\t</dict>\n");
    }
    s.push_str("\t<key>RunAtLoad</key>\n\t<true/>\n");
    s.push_str("\t<key>KeepAlive</key>\n\t<dict>\n");
    s.push_str("\t\t<key>SuccessfulExit</key>\n\t\t<false/>\n\t</dict>\n");
    s.push_str("\t<key>ProcessType</key>\n\t<string>Background</string>\n");
    s.push_str("\t<key>StandardOutPath</key>\n\t<string>/dev/null</string>\n");
    s.push_str("\t<key>StandardErrorPath</key>\n\t<string>/dev/null</string>\n");
    s.push_str("</dict>\n</plist>\n");
    Ok(s)
}

/// D9.3's quoting, double quotes around:
/// - `Environment=` (systemd.exec(5)): `\` and `"` are escaped, and `%` is written `%%`, since
///   specifiers expand there. `$` stays, since `Environment=` expands no variables.
/// - `ExecStart=` (systemd.service(5), with `exec`): the same, and `$` is written `$$`, since a
///   command line also expands variables.
fn quoted(text: &str, exec: bool) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '%' => out.push_str("%%"),
            '$' if exec => out.push_str("$$"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The inverse of `quoted`; `None` for text it never writes. `parse_unit` checks the result by
/// rendering it again.
fn unquote(text: &str, exec: bool) -> Option<String> {
    let inner = text.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.push(chars.next()?),
            '%' => {
                if chars.next()? != '%' {
                    return None;
                }
                out.push('%');
            }
            '$' if exec => {
                if chars.next()? != '$' {
                    return None;
                }
                out.push('$');
            }
            c => out.push(c),
        }
    }
    Some(out)
}

/// D9.3: the user unit, with one `Environment=` line per recorded variable, in order. Its
/// standard error follows its standard output (systemd's default `StandardError=inherit`),
/// so both are discarded and the log file is the record (D11).
pub fn render_systemd(program: &Path, env: &[(String, String)]) -> Result<String, UnitError> {
    let program = program_text(program)?;
    let mut s = String::from("[Unit]\nDescription=tagteam daemon\n\n[Service]\nType=simple\n");
    s.push_str(&format!("ExecStart={} daemon\n", quoted(program, true)));
    s.push_str("Restart=on-failure\nRestartSec=10\nStandardOutput=null\n");
    for (name, value) in env {
        carried(name, name)?;
        carried(name, value)?;
        s.push_str(&format!(
            "Environment={}\n",
            quoted(&format!("{name}={value}"), false)
        ));
    }
    s.push_str("\n[Install]\nWantedBy=default.target\n");
    Ok(s)
}

/// The unit of `kind`.
pub fn render_unit(
    kind: ServiceKind,
    program: &Path,
    env: &[(String, String)],
) -> Result<String, UnitError> {
    match kind {
        ServiceKind::Launchd => render_launchd(program, env),
        ServiceKind::Systemd => render_systemd(program, env),
    }
}

/// What a unit tagteam wrote starts, and with which variables.
#[derive(Clone, PartialEq, Eq)]
pub struct UnitContents {
    pub program: PathBuf,
    pub env: Vec<(String, String)>,
}

/// The variables' names only: their values may be proxy URLs carrying a password (§14.2).
impl std::fmt::Debug for UnitContents {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnitContents")
            .field("program", &self.program)
            .field("env", &self.env.iter().map(|(k, _)| k).collect::<Vec<_>>())
            .finish()
    }
}

/// Decision 5: the program and variables of a unit exactly as `render_*` wrote it. It returns
/// `None` for any other text: a hand-edited unit, another tool's, another kind's. `doctor`
/// reports such a unit as installed, with its environment unknown. What is read is rendered
/// again and must give back `contents` byte for byte, so no near miss is ever taken for
/// tagteam's.
pub fn parse_unit(kind: ServiceKind, contents: &str) -> Option<UnitContents> {
    let read = match kind {
        ServiceKind::Launchd => read_launchd(contents)?,
        ServiceKind::Systemd => read_systemd(contents)?,
    };
    let again = render_unit(kind, &read.program, &read.env).ok()?;
    (again == contents).then_some(read)
}

/// The program and variables where `render_launchd` puts them.
fn read_launchd(text: &str) -> Option<UnitContents> {
    let (_, rest) = text.split_once("\t<key>ProgramArguments</key>\n\t<array>\n\t\t<string>")?;
    let (program, rest) = rest.split_once("</string>\n")?;
    let mut env = Vec::new();
    if let Some((_, block)) = rest.split_once("\t<key>EnvironmentVariables</key>\n\t<dict>\n") {
        let (block, _) = block.split_once("\t</dict>\n")?;
        let mut lines = block.lines();
        while let Some(key) = lines.next() {
            let key = key.strip_prefix("\t\t<key>")?.strip_suffix("</key>")?;
            let value = lines
                .next()?
                .strip_prefix("\t\t<string>")?
                .strip_suffix("</string>")?;
            env.push((unxml(key), unxml(value)));
        }
    }
    Some(UnitContents {
        program: PathBuf::from(unxml(program)),
        env,
    })
}

/// The program and variables where `render_systemd` puts them.
fn read_systemd(text: &str) -> Option<UnitContents> {
    let mut program = None;
    let mut env = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("ExecStart=") {
            program = Some(unquote(rest.strip_suffix(" daemon")?, true)?);
        } else if let Some(rest) = line.strip_prefix("Environment=") {
            let assignment = unquote(rest, false)?;
            let (name, value) = assignment.split_once('=')?;
            env.push((name.to_owned(), value.to_owned()));
        }
    }
    Some(UnitContents {
        program: PathBuf::from(program?),
        env,
    })
}

/// How one call of `launchctl`, `systemctl` or `loginctl` ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagerRun {
    /// The program is not on the environment's `PATH`, so nothing ran.
    Missing,
    /// It exited. `code` is its exit code, or 128 + the signal when one ended it. `stderr` is
    /// trimmed.
    Exited {
        code: i32,
        stdout: String,
        stderr: String,
    },
    /// It gave no exit: it ran past the timeout, could not be started, or was interrupted. The
    /// reason names the program.
    NoAnswer(String),
}

/// Runs `program`, found on `env`'s `PATH`, with `args` through `spawner`, bounded by `timeout`.
/// It runs under a token of its own: a probe is short and bounded, and the caller's next
/// cancellation point meets a signal. `install`'s unload and load are one change, which a
/// signal between them would leave half made. The child gets this process's environment,
/// which `systemctl --user` needs to find the user's manager.
pub fn run_manager<A: AsRef<OsStr>>(
    env: &Env,
    program: &str,
    args: &[A],
    spawner: &dyn ProcessSpawner,
    timeout: Duration,
) -> ManagerRun {
    let Some(path) = find_on_path(program, env.var("PATH")) else {
        return ManagerRun::Missing;
    };
    let spec = SpawnSpec {
        program: path,
        args: args.iter().map(|a| a.as_ref().to_os_string()).collect(),
        ..SpawnSpec::default()
    };
    match spawner.run_captured(&spec, timeout, &Cancel::new()) {
        Captured::Exited {
            code,
            signal,
            stdout,
            stderr,
        } => ManagerRun::Exited {
            code: code.unwrap_or_else(|| 128 + signal.unwrap_or_default()),
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).trim().to_owned(),
        },
        Captured::TimedOut => ManagerRun::NoAnswer(format!(
            "{program} did not answer within {} s",
            timeout.as_secs()
        )),
        Captured::SpawnFailed(e) => {
            ManagerRun::NoAnswer(format!("{program} could not be run: {e}"))
        }
        Captured::Interrupted(signal) => {
            ManagerRun::NoAnswer(format!("{program} was interrupted (signal {signal})"))
        }
    }
}

/// What the service manager says of the daemon's unit (D8.2, D10). Each `Option` is `None` when
/// it cannot be told, and `unknown` then says why.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServiceState {
    /// The unit's file exists.
    pub installed: bool,
    pub loaded: Option<bool>,
    pub running: Option<bool>,
    pub last_exit_status: Option<i32>,
    /// Why the manager's view is unknown: it is missing, it hung, or it failed.
    pub unknown: Option<String>,
}

/// D8.2, D10: whether the unit's file exists, and the manager's view of it from
/// `launchctl print gui/<uid>/<label>` or `systemctl --user show tagteam.service`. Neither
/// command changes anything. A manager that is missing, hangs past `timeout` or fails leaves
/// `loaded`, `running` and `last_exit_status` unknown, and `unknown` gives the reason.
pub fn service_state(
    env: &Env,
    kind: ServiceKind,
    spawner: &dyn ProcessSpawner,
    timeout: Duration,
) -> ServiceState {
    let mut state = ServiceState {
        installed: unit_path(env, kind).exists(),
        ..ServiceState::default()
    };
    match kind {
        ServiceKind::Launchd => {
            let target = launchd_target();
            match run_manager(
                env,
                "launchctl",
                &["print", target.as_str()],
                spawner,
                timeout,
            ) {
                ManagerRun::Exited {
                    code: 0, stdout, ..
                } => {
                    let (running, last) = launchctl_print(&stdout);
                    state.loaded = Some(true);
                    state.running = Some(running);
                    state.last_exit_status = last;
                }
                // An agent launchd has not loaded: exit 113 and "Could not find service" on
                // current macOS. The words count alone too, should the code differ.
                ManagerRun::Exited {
                    code,
                    stdout,
                    stderr,
                } if code == 113 || stdout.contains(NOT_FOUND) || stderr.contains(NOT_FOUND) => {
                    state.loaded = Some(false);
                    state.running = Some(false);
                }
                other => {
                    let command = format!("launchctl print {target}");
                    state.unknown = Some(why_unknown("launchctl", &command, other));
                }
            }
        }
        ServiceKind::Systemd => {
            let command = format!("systemctl --user show {SYSTEMD_UNIT}");
            let args = [
                "--user",
                "show",
                SYSTEMD_UNIT,
                "-p",
                "LoadState,ActiveState,ExecMainStatus",
            ];
            match run_manager(env, "systemctl", &args, spawner, timeout) {
                ManagerRun::Exited {
                    code: 0, stdout, ..
                } => match systemctl_show(&stdout) {
                    Some((loaded, running, status)) => {
                        state.loaded = Some(loaded);
                        state.running = Some(running);
                        state.last_exit_status = status;
                    }
                    None => state.unknown = Some(format!("`{command}` printed no LoadState")),
                },
                other => state.unknown = Some(why_unknown("systemctl", &command, other)),
            }
        }
    }
    state
}

/// Why a manager's answer gives no state. `command` is what ran, for an exit's message.
fn why_unknown(program: &str, command: &str, run: ManagerRun) -> String {
    match run {
        ManagerRun::Missing => format!("{program} is not on PATH"),
        ManagerRun::NoAnswer(why) => why,
        ManagerRun::Exited { code, stderr, .. } if stderr.is_empty() => {
            format!("`{command}` exited {code}")
        }
        ManagerRun::Exited { code, stderr, .. } => format!("`{command}` exited {code}: {stderr}"),
    }
}

/// From `launchctl print`: whether the service's own `state` is `running`, and its
/// `last exit code` (`N`, or `N: NAME`; `(never exited)` gives none). Only lines one brace
/// level in count, since nested blocks have keys of their own.
fn launchctl_print(stdout: &str) -> (bool, Option<i32>) {
    let (mut depth, mut running, mut last) = (0usize, false, None);
    for line in stdout.lines() {
        let line = line.trim();
        if line == "}" {
            depth = depth.saturating_sub(1);
            continue;
        }
        if depth == 1
            && let Some((key, value)) = line.split_once(" = ")
        {
            match key {
                "state" => running = value == "running",
                "last exit code" => {
                    last = value
                        .split(|c: char| c == ':' || c.is_whitespace())
                        .next()
                        .and_then(|n| n.parse().ok());
                }
                _ => {}
            }
        }
        if line.ends_with('{') {
            depth += 1;
        }
    }
    (running, last)
}

/// From `systemctl --user show -p LoadState,ActiveState,ExecMainStatus`: loaded
/// (`LoadState=loaded`), running (`ActiveState` `active` or `reloading`), and a loaded unit's
/// `ExecMainStatus`. `None` without a `LoadState` line.
fn systemctl_show(stdout: &str) -> Option<(bool, bool, Option<i32>)> {
    let (mut load, mut active, mut status) = (None, None, None);
    for line in stdout.lines() {
        match line.split_once('=') {
            Some(("LoadState", v)) => load = Some(v.trim().to_owned()),
            Some(("ActiveState", v)) => active = Some(v.trim().to_owned()),
            Some(("ExecMainStatus", v)) => status = v.trim().parse().ok(),
            _ => {}
        }
    }
    let loaded = load? == "loaded";
    let running = loaded && matches!(active.as_deref(), Some("active" | "reloading"));
    Some((loaded, running, status.filter(|_| loaded)))
}

/// D9.3: whether lingering is on for `user`, by `loginctl show-user <user> -p Linger`. `Err`
/// says why that cannot be told, as `ServiceState::unknown` does: no `loginctl`, a hang, a
/// failure (a user who is neither logged in nor lingering among them), or an answer with no
/// `Linger` in it.
pub fn lingering(
    env: &Env,
    spawner: &dyn ProcessSpawner,
    user: &str,
    timeout: Duration,
) -> Result<bool, String> {
    let command = format!("loginctl show-user {user} -p Linger");
    match run_manager(
        env,
        "loginctl",
        &["show-user", user, "-p", "Linger"],
        spawner,
        timeout,
    ) {
        ManagerRun::Exited {
            code: 0, stdout, ..
        } => match stdout.trim() {
            "Linger=yes" => Ok(true),
            "Linger=no" => Ok(false),
            _ => Err(format!("`{command}` printed no Linger")),
        },
        other => Err(why_unknown("loginctl", &command, other)),
    }
}
#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::fs::PermissionsExt;

    use tagteam_provider::ScriptedSpawner;

    use super::*;

    const BIN: &str = "/opt/homebrew/bin/tagteam";
    const FIVE: Duration = Duration::from_secs(5);
    const KINDS: [ServiceKind; 2] = [ServiceKind::Launchd, ServiceKind::Systemd];

    #[test]
    fn debug_of_a_parsed_unit_names_its_variables_and_never_their_values() {
        // Review I2: a recorded proxy URL can carry a password.
        let env = vec![(
            "HTTPS_PROXY".to_owned(),
            "http://user:secret@proxy".to_owned(),
        )];
        for kind in KINDS {
            let text = render_unit(kind, Path::new(BIN), &env).unwrap();
            let unit = parse_unit(kind, &text).unwrap();
            assert_eq!(unit.env, env, "{kind:?}: the value is still read back");
            let shown = format!("{unit:?}");
            assert!(!shown.contains("secret"), "{kind:?}: {shown}");
            assert!(shown.contains("HTTPS_PROXY"), "{kind:?}: {shown}");
            assert!(shown.contains(BIN), "{kind:?}: {shown}");
        }
    }

    fn plain_env() -> Vec<(String, String)> {
        vec![
            (
                "XDG_DATA_HOME".to_owned(),
                "/Users/me/.local/share".to_owned(),
            ),
            (
                "PATH".to_owned(),
                "/opt/homebrew/bin:/usr/bin:/bin".to_owned(),
            ),
        ]
    }

    /// D9.2's plist for `BIN` and `plain_env`, key by key.
    const PLIST: &str = concat!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
        "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n",
        "<plist version=\"1.0\">\n",
        "<dict>\n",
        "\t<key>Label</key>\n",
        "\t<string>io.github.michaelrigart.tagteam</string>\n",
        "\t<key>ProgramArguments</key>\n",
        "\t<array>\n",
        "\t\t<string>/opt/homebrew/bin/tagteam</string>\n",
        "\t\t<string>daemon</string>\n",
        "\t</array>\n",
        "\t<key>EnvironmentVariables</key>\n",
        "\t<dict>\n",
        "\t\t<key>XDG_DATA_HOME</key>\n",
        "\t\t<string>/Users/me/.local/share</string>\n",
        "\t\t<key>PATH</key>\n",
        "\t\t<string>/opt/homebrew/bin:/usr/bin:/bin</string>\n",
        "\t</dict>\n",
        "\t<key>RunAtLoad</key>\n",
        "\t<true/>\n",
        "\t<key>KeepAlive</key>\n",
        "\t<dict>\n",
        "\t\t<key>SuccessfulExit</key>\n",
        "\t\t<false/>\n",
        "\t</dict>\n",
        "\t<key>ProcessType</key>\n",
        "\t<string>Background</string>\n",
        "\t<key>StandardOutPath</key>\n",
        "\t<string>/dev/null</string>\n",
        "\t<key>StandardErrorPath</key>\n",
        "\t<string>/dev/null</string>\n",
        "</dict>\n",
        "</plist>\n",
    );

    /// `PLIST`'s environment block, which an empty environment leaves out.
    const PLIST_ENV: &str = concat!(
        "\t<key>EnvironmentVariables</key>\n",
        "\t<dict>\n",
        "\t\t<key>XDG_DATA_HOME</key>\n",
        "\t\t<string>/Users/me/.local/share</string>\n",
        "\t\t<key>PATH</key>\n",
        "\t\t<string>/opt/homebrew/bin:/usr/bin:/bin</string>\n",
        "\t</dict>\n",
    );

    /// D9.3's unit for `BIN` and `plain_env`.
    const UNIT: &str = concat!(
        "[Unit]\n",
        "Description=tagteam daemon\n",
        "\n",
        "[Service]\n",
        "Type=simple\n",
        "ExecStart=\"/opt/homebrew/bin/tagteam\" daemon\n",
        "Restart=on-failure\n",
        "RestartSec=10\n",
        "StandardOutput=null\n",
        "Environment=\"XDG_DATA_HOME=/Users/me/.local/share\"\n",
        "Environment=\"PATH=/opt/homebrew/bin:/usr/bin:/bin\"\n",
        "\n",
        "[Install]\n",
        "WantedBy=default.target\n",
    );

    fn contents(program: &Path, env: &[(String, String)]) -> Option<UnitContents> {
        Some(UnitContents {
            program: program.to_path_buf(),
            env: env.to_vec(),
        })
    }

    #[test]
    fn each_unit_lives_where_its_manager_looks_and_is_named_as_it_names_it() {
        let mut env = Env::for_test(Path::new("/tmp/fixture"));
        assert_eq!(
            unit_path(&env, ServiceKind::Launchd),
            Path::new(
                "/tmp/fixture/home/Library/LaunchAgents/io.github.michaelrigart.tagteam.plist"
            )
        );
        assert_eq!(
            unit_path(&env, ServiceKind::Systemd),
            Path::new("/tmp/fixture/home/.config/systemd/user/tagteam.service")
        );
        env.xdg_config_home = Some(PathBuf::from("/cfg"));
        assert_eq!(
            unit_path(&env, ServiceKind::Systemd),
            Path::new("/cfg/systemd/user/tagteam.service")
        );
        assert_eq!(ServiceKind::for_platform(true), ServiceKind::Launchd);
        assert_eq!(ServiceKind::for_platform(false), ServiceKind::Systemd);
        let l = ServiceKind::Launchd;
        let s = ServiceKind::Systemd;
        assert_eq!(
            (l.as_str(), l.label(), l.noun()),
            (
                "launchd",
                "io.github.michaelrigart.tagteam",
                "launchd agent"
            )
        );
        assert_eq!(
            (s.as_str(), s.label(), s.noun()),
            ("systemd", "tagteam.service", "systemd user unit")
        );
        assert_eq!(launchd_domain(), format!("gui/{}", current_uid()));
        assert_eq!(
            launchd_target(),
            format!("gui/{}/io.github.michaelrigart.tagteam", current_uid())
        );
    }

    #[test]
    fn the_plist_and_the_unit_are_rendered_key_by_key() {
        // D9.2, D9.3: fixed key order, so a snapshot pins both.
        assert_eq!(render_launchd(Path::new(BIN), &plain_env()).unwrap(), PLIST);
        assert_eq!(render_systemd(Path::new(BIN), &plain_env()).unwrap(), UNIT);
        // No variable set: no EnvironmentVariables key, and no Environment= line.
        assert_eq!(
            render_launchd(Path::new(BIN), &[]).unwrap(),
            PLIST.replace(PLIST_ENV, "")
        );
        assert_eq!(
            render_systemd(Path::new(BIN), &[]).unwrap(),
            UNIT.replace("Environment=\"XDG_DATA_HOME=/Users/me/.local/share\"\n", "")
                .replace("Environment=\"PATH=/opt/homebrew/bin:/usr/bin:/bin\"\n", "")
        );
        for kind in KINDS {
            assert_eq!(
                render_unit(kind, Path::new(BIN), &plain_env()).unwrap(),
                match kind {
                    ServiceKind::Launchd => PLIST,
                    ServiceKind::Systemd => UNIT,
                }
            );
        }
    }

    #[test]
    fn unit_files_carry_a_program_path_with_spaces_and_unicode() {
        // Review Focus 3: the path a person's tools put tagteam at, which both units must
        // carry so the service starts that very file.
        let program =
            Path::new("/Users/Zoë Ångström/Library/Application Support/tagteam bin/tagteam");
        let env = vec![(
            "XDG_DATA_HOME".to_owned(),
            "/Users/Zoë Ångström/Library/Application Support/tagteam data".to_owned(),
        )];
        for kind in KINDS {
            let text = render_unit(kind, program, &env).unwrap();
            assert_eq!(parse_unit(kind, &text), contents(program, &env), "{kind:?}");
        }
        let plist = render_launchd(program, &env).unwrap();
        assert!(
            plist.contains(
                "\t\t<string>/Users/Zoë Ångström/Library/Application Support/tagteam bin/tagteam</string>\n"
            ),
            "{plist}"
        );
        let unit = render_systemd(program, &env).unwrap();
        assert!(
            unit.contains(
                "ExecStart=\"/Users/Zoë Ångström/Library/Application Support/tagteam bin/tagteam\" daemon\n"
            ),
            "{unit}"
        );
    }

    #[test]
    fn dollar_percent_quote_and_backslash_are_quoted_by_each_unit_s_rules() {
        // D9.3, D14: a proxy URL can hold any of them, and must reach the daemon unchanged.
        let proxy = r#"http://us$er:p%40ss"w\rd@proxy.example:8080"#;
        let env = vec![("HTTPS_PROXY".to_owned(), proxy.to_owned())];
        let program = Path::new(r#"/opt/te$t 100%/"q"\tagteam"#);
        let unit = render_systemd(program, &env).unwrap();
        assert!(
            unit.contains(r#"ExecStart="/opt/te$$t 100%%/\"q\"\\tagteam" daemon"#),
            "{unit}"
        );
        assert!(
            unit.contains(
                r#"Environment="HTTPS_PROXY=http://us$er:p%%40ss\"w\\rd@proxy.example:8080""#
            ),
            "{unit}"
        );
        let plist = render_launchd(program, &env).unwrap();
        assert!(
            plist.contains("<string>/opt/te$t 100%/&quot;q&quot;\\tagteam</string>"),
            "{plist}"
        );
        assert!(
            plist.contains("<string>http://us$er:p%40ss&quot;w\\rd@proxy.example:8080</string>"),
            "{plist}"
        );
        for kind in KINDS {
            let text = render_unit(kind, program, &env).unwrap();
            assert_eq!(parse_unit(kind, &text), contents(program, &env), "{kind:?}");
        }
        // XML's other three, and an entity spelled out in the path itself.
        let odd = Path::new("/a&b/<c>/'d'/&amp;");
        let plist = render_launchd(odd, &[]).unwrap();
        assert!(
            plist.contains("<string>/a&amp;b/&lt;c&gt;/&apos;d&apos;/&amp;amp;</string>"),
            "{plist}"
        );
        assert_eq!(parse_unit(ServiceKind::Launchd, &plist), contents(odd, &[]));
    }

    #[test]
    fn a_control_character_or_a_path_that_is_not_utf8_is_refused_and_never_shown() {
        for bad in ["a\nb", "a\0b", "a\rb", "a\x1bb", "a\x7fb"] {
            let env = vec![("HTTPS_PROXY".to_owned(), format!("http://user:{bad}@proxy"))];
            let program = PathBuf::from(format!("/opt/{bad}/tagteam"));
            for kind in KINDS {
                let e = render_unit(kind, Path::new(BIN), &env).unwrap_err();
                assert_eq!(
                    e,
                    UnitError::Unrepresentable("HTTPS_PROXY".into()),
                    "{bad:?}"
                );
                assert!(!e.to_string().contains("user:"), "{e}");
                assert_eq!(
                    render_unit(kind, &program, &[]).unwrap_err(),
                    UnitError::Unrepresentable("the program path".into()),
                    "{bad:?}"
                );
            }
        }
        // A tab is carried.
        for kind in KINDS {
            let program = Path::new("/opt/a\tb/tagteam");
            let text = render_unit(kind, program, &[]).unwrap();
            assert_eq!(parse_unit(kind, &text), contents(program, &[]));
        }
        let program = PathBuf::from(OsString::from_vec(b"/opt/\xff/tagteam".to_vec()));
        for kind in KINDS {
            assert_eq!(
                render_unit(kind, &program, &[]).unwrap_err(),
                UnitError::NotUtf8("the program path".into())
            );
        }
        assert_eq!(
            UnitError::Unrepresentable("HTTPS_PROXY".into()).to_string(),
            "HTTPS_PROXY holds a newline, a NUL or another control character, which a unit file cannot carry"
        );
        assert_eq!(
            UnitError::NotUtf8("XDG_DATA_HOME".into()).to_string(),
            "XDG_DATA_HOME is not valid UTF-8"
        );
    }

    #[test]
    fn recorded_env_keeps_the_list_s_order_and_only_what_is_set() {
        // D9.1: XDG first, then Claude Code's, the proxies, the log and PATH; nothing else.
        let set: BTreeMap<&str, OsString> = [
            ("PATH", "/usr/bin"),
            ("HTTPS_PROXY", "http://p:3128"),
            ("XDG_DATA_HOME", "/d"),
            ("HOME", "/home/me"),
            ("ANTHROPIC_API_KEY", "sk-ant-x"),
            ("NO_PROXY", ""),
            ("https_proxy", "http://q:3128"),
        ]
        .into_iter()
        .map(|(k, v)| (k, OsString::from(v)))
        .collect();
        let pairs = |list: &[(&str, &str)]| -> Vec<(String, String)> {
            list.iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect()
        };
        assert_eq!(
            recorded_env(&|k| set.get(k).cloned()).unwrap(),
            pairs(&[
                ("XDG_DATA_HOME", "/d"),
                ("HTTPS_PROXY", "http://p:3128"),
                ("https_proxy", "http://q:3128"),
                ("NO_PROXY", ""),
                ("PATH", "/usr/bin"),
            ])
        );
        assert_eq!(recorded_env(&|_| None).unwrap(), Vec::new());
        let bad = OsString::from_vec(vec![b'/', 0xff]);
        assert_eq!(
            recorded_env(&|k| (k == "XDG_STATE_HOME").then(|| bad.clone())),
            Err(UnitError::NotUtf8("XDG_STATE_HOME".into()))
        );
        assert_eq!(
            RECORDED_VARS,
            [
                "XDG_CONFIG_HOME",
                "XDG_DATA_HOME",
                "XDG_STATE_HOME",
                "CLAUDE_CONFIG_DIR",
                "CLAUDE_SECURESTORAGE_CONFIG_DIR",
                "HTTPS_PROXY",
                "https_proxy",
                "HTTP_PROXY",
                "http_proxy",
                "ALL_PROXY",
                "all_proxy",
                "NO_PROXY",
                "no_proxy",
                "TAGTEAM_LOG",
                "PATH",
            ]
        );
    }

    #[test]
    fn a_unit_tagteam_did_not_write_as_it_stands_is_not_read() {
        // Decision 5: only what render_* writes; anything else is "environment unknown".
        assert_eq!(
            parse_unit(ServiceKind::Launchd, PLIST),
            contents(Path::new(BIN), &plain_env())
        );
        assert_eq!(
            parse_unit(ServiceKind::Systemd, UNIT),
            contents(Path::new(BIN), &plain_env())
        );
        for edited in [
            PLIST.replace("<true/>", "<false/>"),
            PLIST.replace('\t', "    "),
            format!("{PLIST}\n"),
            PLIST.replace("/dev/null", "/tmp/log"),
            String::new(),
        ] {
            assert_eq!(parse_unit(ServiceKind::Launchd, &edited), None, "{edited}");
        }
        for edited in [
            UNIT.replace("Restart=on-failure", "Restart=always"),
            UNIT.replace(
                "ExecStart=\"/opt/homebrew/bin/tagteam\"",
                "ExecStart=/opt/homebrew/bin/tagteam",
            ),
            format!("# mine\n{UNIT}"),
            String::new(),
        ] {
            assert_eq!(parse_unit(ServiceKind::Systemd, &edited), None, "{edited}");
        }
        assert_eq!(
            parse_unit(ServiceKind::Systemd, PLIST),
            None,
            "another kind's file"
        );
        assert_eq!(
            parse_unit(ServiceKind::Launchd, UNIT),
            None,
            "another kind's file"
        );
    }

    /// A fixture Env whose `PATH` holds an executable file for each of `programs`: enough for
    /// `find_on_path`. None of them ever runs: a scripted spawner answers in their place.
    fn with_managers(root: &Path, programs: &[&str]) -> Env {
        let bin = root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        for p in programs {
            let file = bin.join(p);
            fs::write(&file, "#!/bin/sh\nexit 99\n").unwrap();
            fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut env = Env::for_test(root);
        env.vars.insert("PATH".into(), bin.into_os_string());
        env
    }

    fn exited(code: i32, stdout: &str, stderr: &str) -> Captured {
        Captured::Exited {
            code: Some(code),
            signal: None,
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    fn args(words: &[&str]) -> Vec<OsString> {
        words.iter().map(OsString::from).collect()
    }

    /// `launchctl print` of a loaded agent that runs. A nested block has a `state` of its own,
    /// which is not the service's.
    const RUNNING: &str = "gui/501/io.github.michaelrigart.tagteam = {\n\tactive count = 1\n\tpath = /Users/me/Library/LaunchAgents/io.github.michaelrigart.tagteam.plist\n\ttype = LaunchAgent\n\tstate = running\n\n\tprogram = /opt/homebrew/bin/tagteam\n\targuments = {\n\t\t/opt/homebrew/bin/tagteam\n\t\tdaemon\n\t}\n\n\tendpoints = {\n\t\tstate = active\n\t}\n\tpid = 4242\n\tlast exit code = (never exited)\n}\n";
    /// The same agent stopped after exit 75; its nested block says `running`, which is not the
    /// service's state.
    const STOPPED: &str = "gui/501/io.github.michaelrigart.tagteam = {\n\tactive count = 0\n\tstate = not running\n\tspawn = {\n\t\tstate = running\n\t}\n\tlast exit code = 75: EX_TEMPFAIL\n}\n";

    #[test]
    fn launchctl_print_tells_running_stopped_and_not_loaded() {
        let d = tempfile::tempdir().unwrap();
        let env = with_managers(d.path(), &["launchctl"]);
        let s = ScriptedSpawner::new();
        s.push(exited(0, RUNNING, ""));
        s.push(exited(0, STOPPED, ""));
        s.push(exited(
            113,
            "",
            "Could not find service \"io.github.michaelrigart.tagteam\" in domain for user gui: 501\n",
        ));
        s.push(exited(
            1,
            "",
            "Could not find service \"x\" in domain for user gui: 501",
        ));
        assert_eq!(
            service_state(&env, ServiceKind::Launchd, &s, FIVE),
            ServiceState {
                installed: false,
                loaded: Some(true),
                running: Some(true),
                last_exit_status: None,
                unknown: None,
            }
        );
        let path = unit_path(&env, ServiceKind::Launchd);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, PLIST).unwrap();
        assert_eq!(
            service_state(&env, ServiceKind::Launchd, &s, FIVE),
            ServiceState {
                installed: true,
                loaded: Some(true),
                running: Some(false),
                last_exit_status: Some(75),
                unknown: None,
            }
        );
        let not_loaded = ServiceState {
            installed: true,
            loaded: Some(false),
            running: Some(false),
            last_exit_status: None,
            unknown: None,
        };
        assert_eq!(
            service_state(&env, ServiceKind::Launchd, &s, FIVE),
            not_loaded
        );
        assert_eq!(
            service_state(&env, ServiceKind::Launchd, &s, FIVE),
            not_loaded,
            "the words alone are enough"
        );
        for spec in s.specs() {
            assert_eq!(spec.program, d.path().join("bin/launchctl"));
            assert_eq!(spec.args, args(&["print", launchd_target().as_str()]));
        }
    }

    #[test]
    fn systemctl_show_tells_running_stopped_and_not_found() {
        let d = tempfile::tempdir().unwrap();
        let env = with_managers(d.path(), &["systemctl"]);
        let s = ScriptedSpawner::new();
        s.push(exited(
            0,
            "LoadState=loaded\nActiveState=active\nExecMainStatus=0\n",
            "",
        ));
        s.push(exited(
            0,
            "ExecMainStatus=1\nLoadState=loaded\nActiveState=failed\n",
            "",
        ));
        s.push(exited(
            0,
            "LoadState=not-found\nActiveState=inactive\nExecMainStatus=0\n",
            "",
        ));
        s.push(exited(0, "", ""));
        let state = |s: &ScriptedSpawner| service_state(&env, ServiceKind::Systemd, s, FIVE);
        let st = state(&s);
        assert_eq!(
            (st.loaded, st.running, st.last_exit_status, st.unknown),
            (Some(true), Some(true), Some(0), None)
        );
        let st = state(&s);
        assert_eq!(
            (st.loaded, st.running, st.last_exit_status),
            (Some(true), Some(false), Some(1))
        );
        let st = state(&s);
        assert_eq!(
            (st.loaded, st.running, st.last_exit_status),
            (Some(false), Some(false), None)
        );
        let st = state(&s);
        assert_eq!(
            st.unknown.as_deref(),
            Some("`systemctl --user show tagteam.service` printed no LoadState")
        );
        assert_eq!(st.loaded, None);
        for spec in s.specs() {
            assert_eq!(spec.program, d.path().join("bin/systemctl"));
            assert_eq!(
                spec.args,
                args(&[
                    "--user",
                    "show",
                    "tagteam.service",
                    "-p",
                    "LoadState,ActiveState,ExecMainStatus",
                ])
            );
        }
    }

    #[test]
    fn a_missing_hanging_or_failing_manager_leaves_the_state_unknown_with_its_reason() {
        let d = tempfile::tempdir().unwrap();
        let s = ScriptedSpawner::new();
        let nothing = Env::for_test(d.path());
        for (kind, program) in [
            (ServiceKind::Launchd, "launchctl"),
            (ServiceKind::Systemd, "systemctl"),
        ] {
            let st = service_state(&nothing, kind, &s, FIVE);
            assert_eq!(st.unknown, Some(format!("{program} is not on PATH")));
            assert_eq!(
                (st.loaded, st.running, st.last_exit_status),
                (None, None, None)
            );
        }
        assert!(
            s.specs().is_empty(),
            "nothing is spawned without the program"
        );
        let env = with_managers(d.path(), &["launchctl", "systemctl"]);
        s.push(Captured::TimedOut);
        s.push(exited(1, "", "Failed to connect to bus: No medium found\n"));
        s.push(Captured::SpawnFailed(
            "Permission denied (os error 13)".into(),
        ));
        s.push(exited(5, "", ""));
        let unknown = |kind| service_state(&env, kind, &s, FIVE).unknown;
        assert_eq!(
            unknown(ServiceKind::Launchd).as_deref(),
            Some("launchctl did not answer within 5 s")
        );
        assert_eq!(
            unknown(ServiceKind::Systemd).as_deref(),
            Some(
                "`systemctl --user show tagteam.service` exited 1: Failed to connect to bus: No medium found"
            )
        );
        assert_eq!(
            unknown(ServiceKind::Launchd).as_deref(),
            Some("launchctl could not be run: Permission denied (os error 13)")
        );
        assert_eq!(
            unknown(ServiceKind::Launchd),
            Some(format!("`launchctl print {}` exited 5", launchd_target()))
        );
    }

    #[test]
    fn run_manager_finds_its_program_on_the_environment_s_path_and_says_how_it_ended() {
        let d = tempfile::tempdir().unwrap();
        let s = ScriptedSpawner::new();
        assert_eq!(
            run_manager(&Env::for_test(d.path()), "systemctl", &["--user"], &s, FIVE),
            ManagerRun::Missing
        );
        let env = with_managers(d.path(), &["systemctl"]);
        s.push(Captured::Exited {
            code: None,
            signal: Some(9),
            stdout: b"out".to_vec(),
            stderr: b"  killed \n".to_vec(),
        });
        s.push(Captured::Interrupted(2));
        assert_eq!(
            run_manager(&env, "systemctl", &["--user", "daemon-reload"], &s, FIVE),
            ManagerRun::Exited {
                code: 137,
                stdout: "out".into(),
                stderr: "killed".into(),
            }
        );
        assert_eq!(
            run_manager(&env, "systemctl", &["--user"], &s, FIVE),
            ManagerRun::NoAnswer("systemctl was interrupted (signal 2)".into())
        );
        // A token of its own: a signal recorded on the process's token does not cut a call.
        env.cancel.request(2);
        s.push(exited(0, "", ""));
        assert!(matches!(
            run_manager(&env, "systemctl", &["--user"], &s, FIVE),
            ManagerRun::Exited { code: 0, .. }
        ));
        assert_eq!(s.specs()[0].args, args(&["--user", "daemon-reload"]));
    }

    #[test]
    fn lingering_is_read_from_loginctl_and_says_why_it_cannot_be() {
        // Review Focus 5: a `loginctl` that is missing, hangs, fails or answers nothing usable
        // leaves lingering unknown, with the reason, for doctor's warning.
        let d = tempfile::tempdir().unwrap();
        let env = with_managers(d.path(), &["loginctl"]);
        let s = ScriptedSpawner::new();
        s.push(exited(0, "Linger=no\n", ""));
        s.push(exited(0, "Linger=yes\n", ""));
        s.push(exited(
            1,
            "",
            "Failed to get user: User ID 1000 is not logged in or lingering\n",
        ));
        s.push(Captured::TimedOut);
        s.push(exited(0, "Linger=maybe\n", ""));
        assert_eq!(lingering(&env, &s, "tester", FIVE), Ok(false));
        assert_eq!(lingering(&env, &s, "tester", FIVE), Ok(true));
        assert_eq!(
            lingering(&env, &s, "tester", FIVE),
            Err("`loginctl show-user tester -p Linger` exited 1: Failed to get user: User ID 1000 is not logged in or lingering".to_owned())
        );
        assert_eq!(
            lingering(&env, &s, "tester", FIVE),
            Err("loginctl did not answer within 5 s".to_owned())
        );
        assert_eq!(
            lingering(&env, &s, "tester", FIVE),
            Err("`loginctl show-user tester -p Linger` printed no Linger".to_owned())
        );
        assert_eq!(
            lingering(&Env::for_test(d.path()), &s, "tester", FIVE),
            Err("loginctl is not on PATH".to_owned()),
            "no loginctl, and nothing spawned"
        );
        assert_eq!(s.specs().len(), 5);
        assert_eq!(
            s.specs()[0].args,
            args(&["show-user", "tester", "-p", "Linger"])
        );
    }
}
