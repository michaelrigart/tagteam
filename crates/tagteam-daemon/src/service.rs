//! D9: `tagteam daemon install` and `uninstall`. They write or remove the engine's units
//! (`tagteam_engine::service`, plan B Decision 4) and load or unload them with `launchctl` or
//! `systemctl`. Each call goes through the `ProcessSpawner` port with a 10 s timeout (D9.4),
//! under a token of its own (`run_manager`): unloading the old unit and loading the new one are
//! one change, which a Ctrl-C between them would leave half made.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tagteam_engine::daemon::{daemon_lock_path, find_on_path, read_daemon_record};
use tagteam_engine::service::{
    ManagerRun, SYSTEMD_UNIT, ServiceKind, UnitError, launchd_domain, launchd_target, lingering,
    render_unit, run_manager, unit_path,
};
use tagteam_provider::atomic::{ensure_private_dir, remove_target, write_atomic_private};
use tagteam_provider::process::{ProcessSpawner, current_uid};
use tagteam_provider::{Env, ProcessStamp, Read};

/// D9.4: how long each `launchctl`, `systemctl` and `loginctl` call may take.
pub const MANAGER_TIMEOUT: Duration = Duration::from_secs(10);

/// What `install` did (D9.4's JSON).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallReport {
    pub kind: ServiceKind,
    pub path: PathBuf,
    /// The name the manager knows the unit by: the agent's label, or the unit's file name.
    pub label: String,
    /// The daemon was started, or restarted. False under `--no-start`.
    pub started: bool,
    /// What the person should know, one sentence each.
    pub warnings: Vec<String>,
}

/// Why `install` or `uninstall` failed (D9.4).
#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error(transparent)]
    Unit(#[from] UnitError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// The manager refused: its words and its exit status.
    #[error("{program} failed (exit {status}): {stderr}")]
    Manager {
        program: String,
        status: String,
        stderr: String,
    },
    /// The manager timed out, could not be started, or was interrupted.
    #[error("{program} gave no answer: {reason}")]
    NoAnswer { program: String, reason: String },
    #[error(
        "{0} was not found; `tagteam daemon install --print` prints the unit to install yourself"
    )]
    NoManager(String),
}

/// The program that loads `kind`'s units.
fn manager_of(kind: ServiceKind) -> &'static str {
    match kind {
        ServiceKind::Launchd => "launchctl",
        ServiceKind::Systemd => "systemctl",
    }
}

/// D9: writes the unit for `program` and `recorded`, 0600 through `O_EXCL`, write and rename
/// (D9.1). With `start`, it loads the unit and starts the daemon, restarting one the unit
/// already runs, so a new program path and environment run at once (D9.2, D9.3). Without
/// `start` (`--no-start`), the unit is only written, and on Linux enabled for the next login.
/// A daemon that runs then keeps its old unit until it restarts, and a warning says so.
/// Nothing is written when the platform's manager is missing, or when a value cannot be
/// carried.
pub fn install(
    env: &Env,
    kind: ServiceKind,
    program: &Path,
    recorded: &[(String, String)],
    start: bool,
    spawner: &dyn ProcessSpawner,
) -> Result<InstallReport, InstallError> {
    let unit = render_unit(kind, program, recorded)?;
    let manager = manager_of(kind);
    if find_on_path(manager, env.var("PATH")).is_none() {
        return Err(InstallError::NoManager(manager.to_owned()));
    }
    let path = unit_path(env, kind);
    if let Some(dir) = path.parent() {
        ensure_private_dir(dir)?;
    }
    write_atomic_private(&path, unit.as_bytes(), 0o600)?;
    match kind {
        ServiceKind::Launchd if start => {
            bootout(env, spawner)?;
            let domain = launchd_domain();
            let args = [
                OsStr::new("bootstrap"),
                OsStr::new(&domain),
                path.as_os_str(),
            ];
            call(env, "launchctl", "launchctl bootstrap", &args, spawner)?;
        }
        ServiceKind::Launchd => {}
        ServiceKind::Systemd => {
            let reload = ["--user", "daemon-reload"];
            call(
                env,
                "systemctl",
                "systemctl daemon-reload",
                &reload,
                spawner,
            )?;
            let enable = ["--user", "enable", SYSTEMD_UNIT];
            call(env, "systemctl", "systemctl enable", &enable, spawner)?;
            if start {
                let restart = ["--user", "restart", SYSTEMD_UNIT];
                call(env, "systemctl", "systemctl restart", &restart, spawner)?;
            }
        }
    }
    let mut warnings = Vec::new();
    if !start && let Some(pid) = running_daemon(env) {
        warnings.push(format!(
            "a daemon already runs (pid {pid}) and keeps its old unit until it restarts; `tagteam daemon install` without --no-start restarts it now"
        ));
    }
    if kind == ServiceKind::Systemd {
        let user = env
            .user
            .clone()
            .unwrap_or_else(|| current_uid().to_string());
        match lingering(env, spawner, &user, MANAGER_TIMEOUT) {
            Ok(false) => warnings.push(format!(
                "lingering is off for {user}, so systemd stops the daemon when you log out; `loginctl enable-linger {user}` keeps it running"
            )),
            Ok(true) => {}
            // D9.3 adds its hint only for a `no`; `doctor` reports an unknown (D10).
            Err(why) => tracing::debug!(why = %why, "whether lingering is on could not be told"),
        }
    }
    tracing::info!(
        kind = kind.as_str(),
        started = start,
        "installed the daemon's service unit"
    );
    Ok(InstallReport {
        kind,
        path,
        label: kind.label().to_owned(),
        started: start,
        warnings,
    })
}

/// D9: stops the daemon the unit runs, and removes the unit. On macOS that is `launchctl
/// bootout`, then the file. On Linux it is `systemctl --user disable --now`, the file, then
/// `daemon-reload`. With no unit installed, it runs nothing and returns false (D9.4).
pub fn uninstall(
    env: &Env,
    kind: ServiceKind,
    spawner: &dyn ProcessSpawner,
) -> Result<bool, InstallError> {
    let path = unit_path(env, kind);
    if !path.exists() {
        return Ok(false);
    }
    match kind {
        ServiceKind::Launchd => {
            bootout(env, spawner)?;
            remove_target(&path)?;
        }
        ServiceKind::Systemd => {
            let disable = ["--user", "disable", "--now", SYSTEMD_UNIT];
            call(env, "systemctl", "systemctl disable", &disable, spawner)?;
            remove_target(&path)?;
            // The unit is now disabled, stopped and gone. A reload that fails leaves only
            // systemd's cached copy, which its next reload drops, so the failure is logged and
            // nothing more.
            let reload = ["--user", "daemon-reload"];
            if let Err(e) = call(
                env,
                "systemctl",
                "systemctl daemon-reload",
                &reload,
                spawner,
            ) {
                tracing::warn!("systemd was not reloaded after the daemon's unit was removed: {e}");
            }
        }
    }
    tracing::info!(kind = kind.as_str(), "removed the daemon's service unit");
    Ok(true)
}

/// D9.2: stops and unloads the agent when it is loaded. An agent that is not loaded
/// (`not_loaded`) is no error; any other failure is, because the agent then still runs.
/// `install` and `uninstall` both unload through this.
fn bootout(env: &Env, spawner: &dyn ProcessSpawner) -> Result<(), InstallError> {
    const WHAT: &str = "launchctl bootout";
    let target = launchd_target();
    match run_manager(
        env,
        "launchctl",
        &["bootout", target.as_str()],
        spawner,
        MANAGER_TIMEOUT,
    ) {
        ManagerRun::Exited { code: 0, .. } => Ok(()),
        ManagerRun::Exited { code, stderr, .. } if not_loaded(code, &stderr) => {
            tracing::debug!(
                code,
                "launchctl bootout stopped nothing: the agent was not loaded"
            );
            Ok(())
        }
        ManagerRun::Exited { code, stderr, .. } => Err(refused(WHAT, code, stderr)),
        ManagerRun::NoAnswer(reason) => Err(InstallError::NoAnswer {
            program: WHAT.into(),
            reason,
        }),
        ManagerRun::Missing => Err(InstallError::NoManager("launchctl".into())),
    }
}

/// D9.2: how `launchctl bootout` reports an agent that is not loaded: exit 113, "Could not find
/// specified service", on current macOS, and exit 3, "No such process", on older releases.
/// Either the code or the words is enough, should one of them differ on a later release.
fn not_loaded(code: i32, stderr: &str) -> bool {
    matches!(code, 3 | 113)
        || stderr.contains("Could not find specified service")
        || stderr.contains("No such process")
}

/// D9.4: a manager call that exited non-zero, with its status and its own words.
fn refused(what: &str, code: i32, stderr: String) -> InstallError {
    InstallError::Manager {
        program: what.to_owned(),
        status: code.to_string(),
        stderr: if stderr.is_empty() {
            "it printed nothing".to_owned()
        } else {
            stderr
        },
    }
}

/// One manager call that must succeed. `what` names it in an error, such as `systemctl
/// enable`.
fn call<A: AsRef<OsStr>>(
    env: &Env,
    program: &str,
    what: &str,
    args: &[A],
    spawner: &dyn ProcessSpawner,
) -> Result<(), InstallError> {
    match run_manager(env, program, args, spawner, MANAGER_TIMEOUT) {
        ManagerRun::Exited { code: 0, .. } => Ok(()),
        ManagerRun::Exited { code, stderr, .. } => Err(refused(what, code, stderr)),
        ManagerRun::NoAnswer(reason) => Err(InstallError::NoAnswer {
            program: what.to_owned(),
            reason,
        }),
        ManagerRun::Missing => Err(InstallError::NoManager(program.to_owned())),
    }
}

/// The pid of a daemon that runs now: `daemon.lock`'s record, judged by an exact pid and
/// start-time match (D4.1, §12.6).
fn running_daemon(env: &Env) -> Option<u32> {
    match read_daemon_record(&daemon_lock_path(env)) {
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
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Mutex;

    use tagteam_engine::service::render_unit;
    use tagteam_provider::Cancel;
    use tagteam_provider::process::{Captured, ScriptedSpawner, SpawnSpec};

    use super::*;

    const ALL: [&str; 3] = ["launchctl", "systemctl", "loginctl"];
    const KINDS: [ServiceKind; 2] = [ServiceKind::Launchd, ServiceKind::Systemd];

    fn exited(code: i32, stdout: &str, stderr: &str) -> Captured {
        Captured::Exited {
            code: Some(code),
            signal: None,
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    /// A fixture Env whose `PATH` holds an executable file for each of `programs`, for
    /// `find_on_path`. None of them ever runs: the scripted spawner answers in their place.
    fn env_with(root: &Path, programs: &[&str]) -> Env {
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

    /// Each call as words: the program's file name, then its arguments.
    fn calls(specs: &[SpawnSpec]) -> Vec<Vec<String>> {
        specs
            .iter()
            .map(|s| {
                let mut words = vec![
                    s.program
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                ];
                words.extend(s.args.iter().map(|a| a.to_string_lossy().into_owned()));
                words
            })
            .collect()
    }

    fn words(list: &[&str]) -> Vec<String> {
        list.iter().map(|w| (*w).to_owned()).collect()
    }

    fn mode(p: &Path) -> u32 {
        fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    fn program() -> PathBuf {
        PathBuf::from("/opt/homebrew/bin/tagteam")
    }

    fn recorded() -> Vec<(String, String)> {
        vec![("XDG_DATA_HOME".to_owned(), "/new/data".to_owned())]
    }

    fn unit(kind: ServiceKind) -> String {
        render_unit(kind, &program(), &recorded()).unwrap()
    }

    /// D4.1's record in `daemon.lock`, naming this test process: a daemon that runs.
    fn live_daemon(env: &Env) -> ProcessStamp {
        let me = ProcessStamp::current().unwrap();
        let lock = daemon_lock_path(env);
        fs::create_dir_all(lock.parent().unwrap()).unwrap();
        fs::write(
            &lock,
            format!(
                "{{\"pid\":{},\"start\":{},\"version\":\"0.0.1\"}}\n",
                me.pid, me.start
            ),
        )
        .unwrap();
        me
    }

    const LINGERING_OFF: &str = "lingering is off for tester, so systemd stops the daemon when you log out; `loginctl enable-linger tester` keeps it running";

    #[test]
    fn reinstall_replaces_the_unit_and_restarts_a_running_daemon() {
        // Review Focus 4: a unit from another data directory is replaced, and the daemon it
        // runs is stopped and started again, so the new program and environment are the ones
        // that run.
        for kind in KINDS {
            let d = tempfile::tempdir().unwrap();
            let env = env_with(d.path(), &ALL);
            let path = unit_path(&env, kind);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let old = vec![("XDG_DATA_HOME".to_owned(), "/old/data".to_owned())];
            fs::write(
                &path,
                render_unit(kind, Path::new("/usr/local/bin/tagteam"), &old).unwrap(),
            )
            .unwrap();
            let s = ScriptedSpawner::new();
            let expected = match kind {
                ServiceKind::Launchd => {
                    s.push(exited(0, "", "")); // bootout: it was loaded, and is stopped
                    s.push(exited(0, "", "")); // bootstrap: RunAtLoad starts the new one
                    let (target, domain) = (launchd_target(), launchd_domain());
                    vec![
                        words(&["launchctl", "bootout", target.as_str()]),
                        words(&[
                            "launchctl",
                            "bootstrap",
                            domain.as_str(),
                            path.to_str().unwrap(),
                        ]),
                    ]
                }
                ServiceKind::Systemd => {
                    for _ in 0..3 {
                        s.push(exited(0, "", ""));
                    }
                    s.push(exited(0, "Linger=yes\n", ""));
                    vec![
                        words(&["systemctl", "--user", "daemon-reload"]),
                        words(&["systemctl", "--user", "enable", "tagteam.service"]),
                        words(&["systemctl", "--user", "restart", "tagteam.service"]),
                        words(&["loginctl", "show-user", "tester", "-p", "Linger"]),
                    ]
                }
            };
            let report = install(&env, kind, &program(), &recorded(), true, &s).unwrap();
            assert_eq!(
                report,
                InstallReport {
                    kind,
                    path: path.clone(),
                    label: kind.label().to_owned(),
                    started: true,
                    warnings: vec![],
                }
            );
            assert_eq!(fs::read_to_string(&path).unwrap(), unit(kind), "{kind:?}");
            assert_eq!(mode(&path), 0o600, "D9.1: a proxy URL can carry a password");
            assert_eq!(calls(&s.specs()), expected, "{kind:?}");
        }
    }

    #[test]
    fn no_start_writes_the_unit_and_starts_or_stops_nothing() {
        // D9: on macOS the plist is written, not bootstrapped; on Linux the unit is enabled for
        // the next login. A daemon that runs keeps its old unit until it restarts, and says so.
        for kind in KINDS {
            let d = tempfile::tempdir().unwrap();
            let env = env_with(d.path(), &ALL);
            let me = live_daemon(&env);
            let s = ScriptedSpawner::new();
            let mut warnings = vec![format!(
                "a daemon already runs (pid {}) and keeps its old unit until it restarts; `tagteam daemon install` without --no-start restarts it now",
                me.pid
            )];
            let expected = match kind {
                ServiceKind::Launchd => vec![],
                ServiceKind::Systemd => {
                    s.push(exited(0, "", ""));
                    s.push(exited(0, "", ""));
                    s.push(exited(0, "Linger=no\n", ""));
                    warnings.push(LINGERING_OFF.to_owned());
                    vec![
                        words(&["systemctl", "--user", "daemon-reload"]),
                        words(&["systemctl", "--user", "enable", "tagteam.service"]),
                        words(&["loginctl", "show-user", "tester", "-p", "Linger"]),
                    ]
                }
            };
            let report = install(&env, kind, &program(), &recorded(), false, &s).unwrap();
            assert!(!report.started);
            assert_eq!(report.warnings, warnings, "{kind:?}");
            assert_eq!(
                fs::read_to_string(unit_path(&env, kind)).unwrap(),
                unit(kind)
            );
            assert_eq!(calls(&s.specs()), expected, "{kind:?}");
        }
    }

    #[test]
    fn without_its_manager_install_writes_nothing_and_names_print() {
        // D9.3: without systemctl, install fails and names --print. launchd alike.
        for (kind, manager) in [
            (ServiceKind::Launchd, "launchctl"),
            (ServiceKind::Systemd, "systemctl"),
        ] {
            let d = tempfile::tempdir().unwrap();
            let env = Env::for_test(d.path());
            let s = ScriptedSpawner::new();
            let e = install(&env, kind, &program(), &recorded(), true, &s).unwrap_err();
            assert!(
                matches!(&e, InstallError::NoManager(m) if m == manager),
                "{e:?}"
            );
            assert_eq!(
                e.to_string(),
                format!(
                    "{manager} was not found; `tagteam daemon install --print` prints the unit to install yourself"
                )
            );
            assert!(!unit_path(&env, kind).exists());
            assert!(s.specs().is_empty());
        }
    }

    #[test]
    fn a_bootout_with_nothing_loaded_is_no_error_and_a_failed_bootstrap_is() {
        let d = tempfile::tempdir().unwrap();
        let env = env_with(d.path(), &["launchctl"]);
        let s = ScriptedSpawner::new();
        let not_loaded = || {
            exited(
                113,
                "",
                "Boot-out failed: 113: Could not find specified service\n",
            )
        };
        s.push(not_loaded());
        s.push(exited(0, "", ""));
        assert!(
            install(&env, ServiceKind::Launchd, &program(), &[], true, &s)
                .unwrap()
                .started
        );
        s.push(not_loaded());
        s.push(exited(5, "", "Bootstrap failed: 5: Input/output error\n"));
        let e = install(&env, ServiceKind::Launchd, &program(), &[], true, &s).unwrap_err();
        assert_eq!(
            e.to_string(),
            "launchctl bootstrap failed (exit 5): Bootstrap failed: 5: Input/output error"
        );
        assert!(
            unit_path(&env, ServiceKind::Launchd).exists(),
            "the unit stays for the next login"
        );
        s.push(Captured::TimedOut);
        let e = install(&env, ServiceKind::Launchd, &program(), &[], true, &s).unwrap_err();
        assert_eq!(
            e.to_string(),
            "launchctl bootout gave no answer: launchctl did not answer within 10 s"
        );
        assert_eq!(
            s.specs().len(),
            5,
            "nothing after a bootout that gave no answer"
        );
    }

    #[test]
    fn a_systemctl_call_that_fails_stops_the_install_with_its_stderr() {
        let d = tempfile::tempdir().unwrap();
        let env = env_with(d.path(), &ALL);
        let s = ScriptedSpawner::new();
        s.push(exited(0, "", ""));
        s.push(exited(
            1,
            "",
            "Failed to enable unit: Unit file tagteam.service does not exist.\n",
        ));
        let e = install(
            &env,
            ServiceKind::Systemd,
            &program(),
            &recorded(),
            true,
            &s,
        )
        .unwrap_err();
        assert_eq!(
            e.to_string(),
            "systemctl enable failed (exit 1): Failed to enable unit: Unit file tagteam.service does not exist."
        );
        assert_eq!(s.specs().len(), 2, "nothing after the failure");
        s.push(exited(0, "", ""));
        s.push(exited(0, "", ""));
        s.push(exited(1, "", ""));
        let e = install(
            &env,
            ServiceKind::Systemd,
            &program(),
            &recorded(),
            true,
            &s,
        )
        .unwrap_err();
        assert_eq!(
            e.to_string(),
            "systemctl restart failed (exit 1): it printed nothing"
        );
    }

    #[test]
    fn a_value_no_unit_can_carry_is_refused_before_anything_is_written() {
        let d = tempfile::tempdir().unwrap();
        let env = env_with(d.path(), &ALL);
        let s = ScriptedSpawner::new();
        let bad = vec![("HTTPS_PROXY".to_owned(), "http://a\nb".to_owned())];
        let e = install(&env, ServiceKind::Systemd, &program(), &bad, true, &s).unwrap_err();
        assert!(
            matches!(&e, InstallError::Unit(UnitError::Unrepresentable(n)) if n == "HTTPS_PROXY"),
            "{e:?}"
        );
        assert!(!unit_path(&env, ServiceKind::Systemd).exists());
        assert!(s.specs().is_empty());
    }

    #[test]
    fn uninstall_stops_and_removes_the_unit_and_a_second_run_finds_nothing() {
        // D9.2, D9.3, D9.4: bootout then delete; disable --now, delete, then daemon-reload.
        for kind in KINDS {
            let d = tempfile::tempdir().unwrap();
            let env = env_with(d.path(), &ALL);
            let path = unit_path(&env, kind);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, unit(kind)).unwrap();
            let s = ScriptedSpawner::new();
            let expected = match kind {
                ServiceKind::Launchd => {
                    s.push(exited(0, "", ""));
                    vec![words(&["launchctl", "bootout", launchd_target().as_str()])]
                }
                ServiceKind::Systemd => {
                    s.push(exited(0, "", ""));
                    s.push(exited(1, "", "Failed to reload daemon: Access denied\n"));
                    vec![
                        words(&["systemctl", "--user", "disable", "--now", "tagteam.service"]),
                        words(&["systemctl", "--user", "daemon-reload"]),
                    ]
                }
            };
            assert!(uninstall(&env, kind, &s).unwrap(), "{kind:?}");
            assert!(!path.exists());
            assert!(
                !uninstall(&env, kind, &s).unwrap(),
                "nothing installed: removed false"
            );
            assert_eq!(
                calls(&s.specs()),
                expected,
                "{kind:?}: a failed reload after the removal is only logged"
            );
        }
        // A unit whose manager is gone stays where it is.
        let d = tempfile::tempdir().unwrap();
        let env = Env::for_test(d.path());
        let path = unit_path(&env, ServiceKind::Systemd);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, unit(ServiceKind::Systemd)).unwrap();
        let e = uninstall(&env, ServiceKind::Systemd, &ScriptedSpawner::new()).unwrap_err();
        assert!(
            matches!(&e, InstallError::NoManager(m) if m == "systemctl"),
            "{e:?}"
        );
        assert!(path.exists());
    }

    #[test]
    fn an_unload_that_fails_stops_the_command_and_keeps_the_unit() {
        // D9.2, D9.3, D9.4: only "not loaded" is no error. Any other failure to unload leaves
        // the daemon's unit loaded and running, so `uninstall` keeps its file and reports the
        // manager's status and words, and `install` never loads over it.
        let failed = || exited(5, "", "Boot-out failed: 5: Input/output error\n");
        let bootout_failed =
            "launchctl bootout failed (exit 5): Boot-out failed: 5: Input/output error";
        let d = tempfile::tempdir().unwrap();
        let env = env_with(d.path(), &ALL);
        let path = unit_path(&env, ServiceKind::Launchd);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, unit(ServiceKind::Launchd)).unwrap();
        let s = ScriptedSpawner::new();
        s.push(failed());
        let e = uninstall(&env, ServiceKind::Launchd, &s).unwrap_err();
        assert!(matches!(&e, InstallError::Manager { .. }), "{e:?}");
        assert_eq!(e.to_string(), bootout_failed);
        assert!(
            path.exists(),
            "the agent still runs from its plist, which stays"
        );
        // An older macOS's "No such process" is "not loaded", as 113 is.
        s.push(exited(3, "", "Boot-out failed: 3: No such process\n"));
        assert!(uninstall(&env, ServiceKind::Launchd, &s).unwrap());
        assert!(!path.exists());
        // `install` unloads by the same rule: no bootstrap over a loaded old agent.
        s.push(failed());
        let e = install(
            &env,
            ServiceKind::Launchd,
            &program(),
            &recorded(),
            true,
            &s,
        )
        .unwrap_err();
        assert_eq!(e.to_string(), bootout_failed);
        assert_eq!(
            calls(&s.specs()),
            vec![words(&["launchctl", "bootout", launchd_target().as_str()]); 3],
            "nothing after a failed bootout"
        );

        // systemd: a failed `disable --now` leaves the unit enabled and perhaps running, so its
        // file stays and no reload follows.
        let d = tempfile::tempdir().unwrap();
        let env = env_with(d.path(), &ALL);
        let path = unit_path(&env, ServiceKind::Systemd);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, unit(ServiceKind::Systemd)).unwrap();
        let s = ScriptedSpawner::new();
        s.push(exited(1, "", "Failed to connect to bus: No medium found\n"));
        let e = uninstall(&env, ServiceKind::Systemd, &s).unwrap_err();
        assert_eq!(
            e.to_string(),
            "systemctl disable failed (exit 1): Failed to connect to bus: No medium found"
        );
        assert!(path.exists());
        assert_eq!(
            s.specs().len(),
            1,
            "no daemon-reload after a failed disable"
        );
    }

    /// The scripted spawner, recording each call's timeout and whether its token was set.
    #[derive(Default)]
    struct Recording {
        inner: ScriptedSpawner,
        seen: Mutex<Vec<(Duration, bool)>>,
    }

    impl ProcessSpawner for Recording {
        fn run_captured(&self, spec: &SpawnSpec, timeout: Duration, cancel: &Cancel) -> Captured {
            self.seen
                .lock()
                .unwrap()
                .push((timeout, cancel.requested().is_some()));
            self.inner.run_captured(spec, timeout, cancel)
        }
    }

    #[test]
    fn every_call_is_bounded_at_ten_seconds_and_a_signal_does_not_cut_an_install_in_half() {
        // D9.4's timeout, and a token of the install's own: a Ctrl-C already recorded on the
        // process's token neither skips the load nor stops the unload's partner.
        let d = tempfile::tempdir().unwrap();
        let env = env_with(d.path(), &ALL);
        env.cancel.request(2);
        let s = Recording::default();
        for _ in 0..3 {
            s.inner.push(exited(0, "", ""));
        }
        s.inner.push(exited(0, "Linger=yes\n", ""));
        s.inner.push(exited(0, "", ""));
        s.inner.push(exited(0, "", ""));
        install(
            &env,
            ServiceKind::Systemd,
            &program(),
            &recorded(),
            true,
            &s,
        )
        .unwrap();
        assert!(uninstall(&env, ServiceKind::Systemd, &s).unwrap());
        let seen = s.seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 6);
        assert!(
            seen.iter()
                .all(|&(t, cancelled)| t == Duration::from_secs(10) && !cancelled),
            "{seen:?}"
        );
    }
}
