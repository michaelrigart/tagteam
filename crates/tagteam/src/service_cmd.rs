//! `tagteam daemon install`, `uninstall` and `status` (D8.2, D9.4). This module holds the
//! program a unit starts, what the daemon's socket or lock says of it, and each command's
//! human and JSON forms. `app.rs` runs the commands; `tagteam_daemon::service` and the engine
//! do the work.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use tagteam_daemon::client::{ClientError, Connect, connect};
use tagteam_daemon::protocol::{Status, StatusJob, StatusNotifications, StatusProvider};
use tagteam_daemon::service::{InstallReport, Uninstalled};
use tagteam_daemon::watch::program_path;
use tagteam_engine::daemon::{daemon_lock_path, daemon_socket_path, read_daemon_record};
use tagteam_engine::service::{ServiceKind, ServiceState};
use tagteam_provider::{Env, ProcessStamp, Read};

/// D8.2: the LSB status convention's "not running".
pub(crate) const EXIT_NOT_RUNNING: i32 = 3;

/// D9: `install` always says this.
const UNATTENDED: &str = "The daemon will switch accounts unattended; pause it with `tagteam config set daemon.autoswitch false`.";

/// D9.1: the program a unit starts. That is the first `tagteam` on `PATH` when it is this
/// binary (a stable link from Homebrew or cargo, which survives an upgrade), else this
/// binary's own path. The relative argv[0] keeps the rule D9.1's alone: D4.5's preference for
/// an absolute argv[0] is for the daemon's own start.
pub(crate) fn install_program(env: &Env) -> std::io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    Ok(program_path(OsStr::new("tagteam"), env.var("PATH"), &exe))
}

/// The service a unit is, as `install`'s JSON names it (D9.4).
fn service_value(kind: ServiceKind, path: &Path) -> Value {
    json!({"kind": kind.as_str(), "path": path.display().to_string(), "label": kind.label()})
}

/// `install --print --json`: the unit, as a string, beside where it would go.
pub(crate) fn print_json(kind: ServiceKind, path: &Path, unit: &str) -> Value {
    json!({"schemaVersion": 1, "service": service_value(kind, path), "unit": unit})
}

pub(crate) fn install_human(r: &InstallReport) -> String {
    let done = if r.started {
        ", and started the daemon."
    } else {
        "; it starts at the next login."
    };
    format!(
        "Installed the {} at {}{done}\n{UNATTENDED}\n",
        r.kind.noun(),
        r.path.display()
    )
}

/// D9.4: `{schemaVersion, ok, service: {kind, path, label}, started, warnings}`.
pub(crate) fn install_json(r: &InstallReport) -> Value {
    json!({
        "schemaVersion": 1,
        "ok": true,
        "service": service_value(r.kind, &r.path),
        "started": r.started,
        "warnings": r.warnings,
    })
}

pub(crate) fn uninstall_human(kind: ServiceKind, path: &Path, found: Uninstalled) -> String {
    match found {
        Uninstalled::Removed => format!(
            "Stopped the daemon and removed the {} at {}.\n",
            kind.noun(),
            path.display()
        ),
        Uninstalled::StoppedOnly => format!(
            "Stopped the daemon; no unit file was left at {}.\n",
            path.display()
        ),
        Uninstalled::Nothing => {
            format!("No {} is installed at {}.\n", kind.noun(), path.display())
        }
    }
}

/// D9.4: `{schemaVersion, ok, removed, stopped}`. `removed` is true unless nothing was found;
/// `stopped` is true when the manager stopped a unit whose file was already gone.
pub(crate) fn uninstall_json(found: Uninstalled) -> Value {
    json!({"schemaVersion": 1, "ok": true, "removed": found.removed(), "stopped": found.stopped()})
}

/// What `daemon status` found (D8.2).
pub(crate) enum Found {
    /// The daemon answered `status` (D5.4).
    Answered(Box<Status>),
    /// A daemon speaks another protocol (D5.6): one from before an upgrade, which restarts on
    /// the new binary within a minute (D4.5). `pid` is its lock record's.
    Mismatch {
        server: u32,
        client: u32,
        pid: Option<u32>,
    },
    /// A live process holds `daemon.lock`, but no socket answers in time: there is none (D5.1),
    /// or it hangs (D8.2).
    Silent {
        pid: u32,
    },
    NotRunning,
}

impl Found {
    pub(crate) fn running(&self) -> bool {
        !matches!(self, Found::NotRunning)
    }

    fn pid(&self) -> Option<u32> {
        match self {
            Found::Answered(s) => Some(s.daemon.pid),
            Found::Mismatch { pid, .. } => *pid,
            Found::Silent { pid } => Some(*pid),
            Found::NotRunning => None,
        }
    }
}

/// D8.2: the daemon's own `status`, from its socket. Without an answer, it is the holder
/// `daemon.lock` names, judged by an exact pid and start-time match (§12.6). A daemon that
/// greets or answers too late (`Timeout`) is there but hung. With a live holder it runs and
/// does not answer, as D8.2 says. Without one, whatever listens is no daemon tagteam knows,
/// and the timeout is the error.
pub(crate) fn find_daemon(env: &Env) -> Result<Found, ClientError> {
    let holder = || match read_daemon_record(&daemon_lock_path(env)) {
        Read::Present(r)
            if (ProcessStamp {
                pid: r.pid,
                start: r.start,
            })
            .is_live() =>
        {
            Some(r.pid)
        }
        _ => None,
    };
    let silent = |pid: Option<u32>| pid.map_or(Found::NotRunning, |pid| Found::Silent { pid });
    let hung = || {
        holder()
            .map(|pid| Found::Silent { pid })
            .ok_or(ClientError::Timeout)
    };
    // No socket file means nothing to connect to. A path too long to bind (D5.1) can never
    // exist, and `connect` would report it as an error rather than find no daemon.
    if fs::symlink_metadata(daemon_socket_path(env)).is_err() {
        return Ok(silent(holder()));
    }
    match connect(env) {
        Ok(Connect::Connected(mut client)) => match client.status() {
            Ok(status) => Ok(Found::Answered(Box::new(status))),
            Err(ClientError::Timeout) => hung(),
            Err(e) => Err(e),
        },
        Ok(Connect::Mismatch { server, client }) => Ok(Found::Mismatch {
            server,
            client,
            pid: holder(),
        }),
        Ok(Connect::NoDaemon) => Ok(silent(holder())),
        Err(ClientError::Timeout) => hung(),
        Err(e) => Err(e),
    }
}

/// D8.2's JSON: `{schemaVersion, running, pid, daemon, service}`. `daemon` is D5.4's object,
/// with every field present and `null` when absent, or `null` without an answer. `service` is
/// `{kind, path, installed, loaded, lastExitStatus}`.
pub(crate) fn status_json(
    found: &Found,
    kind: ServiceKind,
    path: &Path,
    state: &ServiceState,
) -> Value {
    json!({
        "schemaVersion": 1,
        "running": found.running(),
        "pid": found.pid(),
        "daemon": match found {
            Found::Answered(s) => daemon_value(s),
            _ => Value::Null,
        },
        "service": {
            "kind": kind.as_str(),
            "path": path.display().to_string(),
            "installed": state.installed,
            "loaded": state.loaded,
            "lastExitStatus": state.last_exit_status,
        },
    })
}

/// D5.4's object, from the client's typed view of it.
fn daemon_value(s: &Status) -> Value {
    let providers: Vec<Value> = s
        .providers
        .iter()
        .map(|p| {
            json!({
                "provider": p.provider,
                "mode": p.mode,
                "autoswitch": p.autoswitch,
                "waitingFor": p.waiting_for,
                "holderPid": p.holder_pid,
                "backoff": p.backoff.as_ref().map(|b| json!({"reason": b.reason, "until": b.until})),
                "lastTickAt": p.last_tick_at,
                "nextTickAt": p.next_tick_at,
                "lastOutcome": p.last_outcome.as_ref().map(|o| json!({"kind": o.kind, "reason": o.reason, "at": o.at})),
                "lastSwitch": p.last_switch.as_ref().map(|w| json!({"at": w.at, "from": w.from, "to": w.to, "trigger": w.trigger})),
            })
        })
        .collect();
    let jobs: Vec<Value> = s
        .jobs
        .iter()
        .map(|j| {
            json!({
                "name": j.name,
                "lastRunAt": j.last_run_at,
                "nextRunAt": j.next_run_at,
                "lastError": j.last_error,
            })
        })
        .collect();
    let d = &s.daemon;
    let n = &s.notifications;
    json!({
        "daemon": {
            "pid": d.pid,
            "version": d.version,
            "protocol": d.protocol,
            "startedAt": d.started_at,
            "socket": d.socket,
            "log": d.log,
        },
        "notifications": {"enabled": n.enabled, "backend": n.backend, "events": n.events},
        "providers": providers,
        "jobs": jobs,
        "subscribers": s.subscribers,
    })
}

/// D8.2's text: what runs, then the service.
pub(crate) fn status_human(
    found: &Found,
    kind: ServiceKind,
    path: &Path,
    state: &ServiceState,
) -> String {
    let mut s = String::new();
    match found {
        Found::Answered(st) => {
            let d = &st.daemon;
            s.push_str(&format!(
                "The daemon runs as pid {}: tagteam {}, protocol {}, since {}.\n",
                d.pid, d.version, d.protocol, d.started_at
            ));
            s.push_str(&format!(
                "  socket: {}\n",
                d.socket.as_deref().unwrap_or("none (the log says why)")
            ));
            s.push_str(&format!("  log: {}\n", d.log));
            s.push_str(&notifications_line(&st.notifications));
            for p in &st.providers {
                s.push_str(&provider_line(p));
            }
            for j in &st.jobs {
                s.push_str(&job_line(j));
            }
            s.push_str(&format!("  subscribers: {}\n", st.subscribers));
        }
        Found::Mismatch {
            server,
            client,
            pid,
        } => {
            let who = pid.map(|p| format!(" as pid {p}")).unwrap_or_default();
            s.push_str(&format!(
                "A daemon runs{who}, but it speaks protocol {server} and this tagteam {client}; it restarts on the new tagteam within a minute.\n"
            ));
        }
        Found::Silent { pid } => s.push_str(&format!(
            "A daemon runs as pid {pid}, but its socket does not answer; the log says why.\n"
        )),
        Found::NotRunning => s.push_str("The daemon is not running.\n"),
    }
    s.push_str(&service_line(kind, path, state));
    s
}

fn notifications_line(n: &StatusNotifications) -> String {
    match (n.enabled, &n.backend) {
        (false, _) => "  notifications: off\n".to_owned(),
        (true, None) => "  notifications: on, but nothing here can show them\n".to_owned(),
        (true, Some(backend)) if n.events.is_empty() => {
            format!("  notifications: none chosen (notify.events), through {backend}\n")
        }
        (true, Some(backend)) => {
            format!(
                "  notifications: {}, through {backend}\n",
                n.events.join(", ")
            )
        }
    }
}

fn provider_line(p: &StatusProvider) -> String {
    let mut line = format!("  {}: {}", p.provider, p.mode);
    if let Some(what) = &p.waiting_for {
        line.push_str(&format!(" for {what}"));
        if let Some(pid) = p.holder_pid {
            line.push_str(&format!(" (pid {pid})"));
        }
    }
    if !p.autoswitch {
        line.push_str(", auto-switching paused");
    }
    if let Some(b) = &p.backoff {
        line.push_str(&format!(", backing off ({}) until {}", b.reason, b.until));
    }
    if let Some(o) = &p.last_outcome {
        let reason = o
            .reason
            .as_ref()
            .map(|r| format!(" ({r})"))
            .unwrap_or_default();
        line.push_str(&format!("; last {}{reason} at {}", o.kind, o.at));
    }
    if let Some(next) = &p.next_tick_at {
        line.push_str(&format!("; next at {next}"));
    }
    if let Some(w) = &p.last_switch {
        line.push_str(&format!(
            "; last switch {} → {} ({}) at {}",
            w.from, w.to, w.trigger, w.at
        ));
    }
    line.push('\n');
    line
}

fn job_line(j: &StatusJob) -> String {
    let mut line = format!("  {}:", j.name);
    match &j.last_run_at {
        Some(at) => line.push_str(&format!(" last run {at}")),
        None => line.push_str(" not run yet"),
    }
    if let Some(next) = &j.next_run_at {
        line.push_str(&format!(", next {next}"));
    }
    if let Some(e) = &j.last_error {
        line.push_str(&format!(", last error: {e}"));
    }
    line.push('\n');
    line
}

/// The service's line: whether the unit is installed, and what its manager says, or why that
/// cannot be told.
fn service_line(kind: ServiceKind, path: &Path, s: &ServiceState) -> String {
    let installed = if s.installed {
        "installed"
    } else {
        "not installed"
    };
    let state = match (&s.unknown, s.loaded, s.running, s.last_exit_status) {
        (Some(why), ..) => format!("; its state cannot be read ({why})"),
        (None, Some(true), Some(true), _) => ", loaded and running".to_owned(),
        (None, Some(true), _, Some(code)) => {
            format!(", loaded, not running (last exit status {code})")
        }
        (None, Some(true), _, None) => ", loaded, not running".to_owned(),
        (None, Some(false), ..) => ", not loaded".to_owned(),
        (None, None, ..) => String::new(),
    };
    format!(
        "Service: the {} at {} is {installed}{state}.\n",
        kind.noun(),
        path.display()
    )
}

#[cfg(test)]
mod tests {
    use tagteam_daemon::protocol::{StatusBackoff, StatusDaemon, StatusOutcome, StatusSwitch};

    use super::*;

    const UNATTENDED_LINE: &str = "The daemon will switch accounts unattended; pause it with `tagteam config set daemon.autoswitch false`.\n";

    fn agent() -> PathBuf {
        PathBuf::from("/Users/me/Library/LaunchAgents/io.github.michaelrigart.tagteam.plist")
    }

    fn status() -> Status {
        Status {
            daemon: StatusDaemon {
                pid: 4242,
                version: "20261014.2045.0".into(),
                protocol: 1,
                started_at: "2026-10-14T20:45:00Z".into(),
                socket: Some("~/.local/share/tagteam/daemon.sock".into()),
                log: "~/.local/state/tagteam/tagteam.log".into(),
            },
            notifications: StatusNotifications {
                enabled: true,
                backend: Some("osascript".into()),
                events: vec!["switch".into(), "quarantine".into(), "limit".into()],
            },
            providers: vec![
                StatusProvider {
                    provider: "claude-code".into(),
                    mode: "auto".into(),
                    autoswitch: true,
                    waiting_for: None,
                    holder_pid: None,
                    backoff: None,
                    last_tick_at: Some("2026-10-14T21:00:00Z".into()),
                    next_tick_at: Some("2026-10-14T21:05:00Z".into()),
                    last_outcome: Some(StatusOutcome {
                        kind: "no-switch".into(),
                        reason: Some("below-threshold".into()),
                        at: "2026-10-14T21:00:00Z".into(),
                    }),
                    last_switch: Some(StatusSwitch {
                        at: "2026-10-14T20:50:00Z".into(),
                        from: 1,
                        to: 2,
                        trigger: "proactive".into(),
                    }),
                },
                StatusProvider {
                    provider: "fake-agent".into(),
                    mode: "waiting".into(),
                    autoswitch: false,
                    waiting_for: Some("engine-lock".into()),
                    holder_pid: Some(777),
                    backoff: Some(StatusBackoff {
                        reason: "keychain".into(),
                        until: "2026-10-14T21:15:00Z".into(),
                    }),
                    last_tick_at: None,
                    next_tick_at: Some("2026-10-14T21:01:00Z".into()),
                    last_outcome: None,
                    last_switch: None,
                },
            ],
            jobs: vec![
                StatusJob {
                    name: "store-watch".into(),
                    last_run_at: Some("2026-10-14T21:00:58Z".into()),
                    next_run_at: Some("2026-10-14T21:01:00Z".into()),
                    last_error: None,
                },
                StatusJob {
                    name: "upgrade-watch".into(),
                    last_run_at: None,
                    next_run_at: Some("2026-10-14T21:01:00Z".into()),
                    last_error: Some("the program path cannot be read".into()),
                },
            ],
            subscribers: 1,
        }
    }

    fn running() -> ServiceState {
        ServiceState {
            installed: true,
            loaded: Some(true),
            running: Some(true),
            last_exit_status: None,
            unknown: None,
        }
    }

    #[test]
    fn status_with_a_daemon_shows_its_own_status_and_its_service() {
        let found = Found::Answered(Box::new(status()));
        assert!(found.running());
        assert_eq!(
            status_human(&found, ServiceKind::Launchd, &agent(), &running()),
            "The daemon runs as pid 4242: tagteam 20261014.2045.0, protocol 1, since 2026-10-14T20:45:00Z.\n\
             \x20 socket: ~/.local/share/tagteam/daemon.sock\n\
             \x20 log: ~/.local/state/tagteam/tagteam.log\n\
             \x20 notifications: switch, quarantine, limit, through osascript\n\
             \x20 claude-code: auto; last no-switch (below-threshold) at 2026-10-14T21:00:00Z; next at 2026-10-14T21:05:00Z; last switch 1 → 2 (proactive) at 2026-10-14T20:50:00Z\n\
             \x20 fake-agent: waiting for engine-lock (pid 777), auto-switching paused, backing off (keychain) until 2026-10-14T21:15:00Z; next at 2026-10-14T21:01:00Z\n\
             \x20 store-watch: last run 2026-10-14T21:00:58Z, next 2026-10-14T21:01:00Z\n\
             \x20 upgrade-watch: not run yet, next 2026-10-14T21:01:00Z, last error: the program path cannot be read\n\
             \x20 subscribers: 1\n\
             Service: the launchd agent at /Users/me/Library/LaunchAgents/io.github.michaelrigart.tagteam.plist is installed, loaded and running.\n"
        );
        assert_eq!(
            status_json(&found, ServiceKind::Launchd, &agent(), &running()),
            json!({
                "schemaVersion": 1,
                "running": true,
                "pid": 4242,
                "daemon": {
                    "daemon": {"pid": 4242, "version": "20261014.2045.0", "protocol": 1,
                               "startedAt": "2026-10-14T20:45:00Z",
                               "socket": "~/.local/share/tagteam/daemon.sock",
                               "log": "~/.local/state/tagteam/tagteam.log"},
                    "notifications": {"enabled": true, "backend": "osascript",
                                      "events": ["switch", "quarantine", "limit"]},
                    "providers": [
                        {"provider": "claude-code", "mode": "auto", "autoswitch": true,
                         "waitingFor": null, "holderPid": null, "backoff": null,
                         "lastTickAt": "2026-10-14T21:00:00Z", "nextTickAt": "2026-10-14T21:05:00Z",
                         "lastOutcome": {"kind": "no-switch", "reason": "below-threshold",
                                         "at": "2026-10-14T21:00:00Z"},
                         "lastSwitch": {"at": "2026-10-14T20:50:00Z", "from": 1, "to": 2,
                                        "trigger": "proactive"}},
                        {"provider": "fake-agent", "mode": "waiting", "autoswitch": false,
                         "waitingFor": "engine-lock", "holderPid": 777,
                         "backoff": {"reason": "keychain", "until": "2026-10-14T21:15:00Z"},
                         "lastTickAt": null, "nextTickAt": "2026-10-14T21:01:00Z",
                         "lastOutcome": null, "lastSwitch": null}
                    ],
                    "jobs": [
                        {"name": "store-watch", "lastRunAt": "2026-10-14T21:00:58Z",
                         "nextRunAt": "2026-10-14T21:01:00Z", "lastError": null},
                        {"name": "upgrade-watch", "lastRunAt": null,
                         "nextRunAt": "2026-10-14T21:01:00Z",
                         "lastError": "the program path cannot be read"}
                    ],
                    "subscribers": 1
                },
                "service": {"kind": "launchd",
                            "path": "/Users/me/Library/LaunchAgents/io.github.michaelrigart.tagteam.plist",
                            "installed": true, "loaded": true, "lastExitStatus": null}
            })
        );
    }

    #[test]
    fn status_without_an_answer_says_what_the_lock_and_the_service_say() {
        let unit = PathBuf::from("/home/me/.config/systemd/user/tagteam.service");
        let unknown = ServiceState {
            installed: false,
            unknown: Some("systemctl is not on PATH".into()),
            ..ServiceState::default()
        };
        let stopped = ServiceState {
            installed: true,
            loaded: Some(true),
            running: Some(false),
            last_exit_status: Some(1),
            unknown: None,
        };
        let not_loaded = ServiceState {
            installed: true,
            loaded: Some(false),
            running: Some(false),
            ..ServiceState::default()
        };
        let cases = [
            (
                Found::NotRunning,
                &unknown,
                "The daemon is not running.\nService: the systemd user unit at /home/me/.config/systemd/user/tagteam.service is not installed; its state cannot be read (systemctl is not on PATH).\n",
                json!(null),
            ),
            (
                Found::Silent { pid: 4242 },
                &stopped,
                "A daemon runs as pid 4242, but its socket does not answer; the log says why.\nService: the systemd user unit at /home/me/.config/systemd/user/tagteam.service is installed, loaded, not running (last exit status 1).\n",
                json!(4242),
            ),
            (
                Found::Mismatch {
                    server: 2,
                    client: 1,
                    pid: Some(4242),
                },
                &not_loaded,
                "A daemon runs as pid 4242, but it speaks protocol 2 and this tagteam 1; it restarts on the new tagteam within a minute.\nService: the systemd user unit at /home/me/.config/systemd/user/tagteam.service is installed, not loaded.\n",
                json!(4242),
            ),
        ];
        for (found, state, human, pid) in cases {
            assert_eq!(
                status_human(&found, ServiceKind::Systemd, &unit, state),
                human
            );
            let v = status_json(&found, ServiceKind::Systemd, &unit, state);
            assert_eq!(v["running"], json!(found.running()));
            assert_eq!(v["pid"], pid);
            assert_eq!(v["daemon"], Value::Null);
            assert_eq!(
                v["service"],
                json!({"kind": "systemd", "path": unit.display().to_string(),
                       "installed": state.installed, "loaded": state.loaded,
                       "lastExitStatus": state.last_exit_status})
            );
        }
        assert!(!Found::NotRunning.running());
        assert!(
            Found::Mismatch {
                server: 2,
                client: 1,
                pid: None
            }
            .running()
        );
    }

    #[test]
    fn install_and_uninstall_say_what_they_did() {
        let path = PathBuf::from("/home/me/.config/systemd/user/tagteam.service");
        let started = InstallReport {
            kind: ServiceKind::Systemd,
            path: path.clone(),
            label: "tagteam.service".into(),
            started: true,
            warnings: vec!["lingering is off".into()],
        };
        assert_eq!(
            install_human(&started),
            format!(
                "Installed the systemd user unit at {}, and started the daemon.\n{UNATTENDED_LINE}",
                path.display()
            )
        );
        assert_eq!(
            install_json(&started),
            json!({"schemaVersion": 1, "ok": true,
                   "service": {"kind": "systemd", "path": path.display().to_string(),
                               "label": "tagteam.service"},
                   "started": true, "warnings": ["lingering is off"]})
        );
        let waiting = InstallReport {
            started: false,
            ..started
        };
        assert_eq!(
            install_human(&waiting),
            format!(
                "Installed the systemd user unit at {}; it starts at the next login.\n{UNATTENDED_LINE}",
                path.display()
            )
        );
        assert_eq!(
            uninstall_human(ServiceKind::Launchd, &agent(), Uninstalled::Removed),
            "Stopped the daemon and removed the launchd agent at /Users/me/Library/LaunchAgents/io.github.michaelrigart.tagteam.plist.\n"
        );
        assert_eq!(
            uninstall_human(ServiceKind::Launchd, &agent(), Uninstalled::StoppedOnly),
            "Stopped the daemon; no unit file was left at /Users/me/Library/LaunchAgents/io.github.michaelrigart.tagteam.plist.\n"
        );
        assert_eq!(
            uninstall_human(ServiceKind::Launchd, &agent(), Uninstalled::Nothing),
            "No launchd agent is installed at /Users/me/Library/LaunchAgents/io.github.michaelrigart.tagteam.plist.\n"
        );
        for (found, removed, stopped) in [
            (Uninstalled::Removed, true, false),
            (Uninstalled::StoppedOnly, true, true),
            (Uninstalled::Nothing, false, false),
        ] {
            assert_eq!(
                uninstall_json(found),
                json!({"schemaVersion": 1, "ok": true, "removed": removed, "stopped": stopped}),
                "{found:?}"
            );
        }
        assert_eq!(
            print_json(ServiceKind::Launchd, &agent(), "<plist/>"),
            json!({"schemaVersion": 1,
                   "service": {"kind": "launchd", "path": agent().display().to_string(),
                               "label": "io.github.michaelrigart.tagteam"},
                   "unit": "<plist/>"})
        );
    }

    #[test]
    fn the_program_installed_is_this_binary_when_no_tagteam_on_path_is_it() {
        // D9.1: no PATH, so no stable link to prefer.
        let d = tempfile::tempdir().unwrap();
        let env = Env::for_test(d.path());
        assert_eq!(
            install_program(&env).unwrap(),
            std::env::current_exe().unwrap()
        );
    }

    #[test]
    fn a_daemon_that_holds_its_lock_but_does_not_greet_is_running() {
        // D8.2: a listener that never greets is a daemon there but hung (Task 3's Timeout).
        // With a live holder of daemon.lock it runs, with the lock's pid. Without one, whatever
        // listens is no daemon tagteam knows, and that is an error.
        let short = tempfile::Builder::new()
            .prefix("tt")
            .tempdir_in("/tmp")
            .or_else(|_| tempfile::tempdir())
            .unwrap();
        let mut env = Env::for_test(short.path());
        env.xdg_data_home = Some(short.path().join("d"));
        let sock = daemon_socket_path(&env);
        fs::create_dir_all(sock.parent().unwrap()).unwrap();
        let _listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        let me = ProcessStamp::current().unwrap();
        let lock = daemon_lock_path(&env);
        fs::create_dir_all(lock.parent().unwrap()).unwrap();
        fs::write(
            &lock,
            format!(
                "{{\"pid\":{},\"start\":{},\"version\":\"0\"}}\n",
                me.pid, me.start
            ),
        )
        .unwrap();
        assert!(matches!(find_daemon(&env), Ok(Found::Silent { pid }) if pid == me.pid));
        fs::remove_file(&lock).unwrap();
        assert!(matches!(find_daemon(&env), Err(ClientError::Timeout)));
    }
}
