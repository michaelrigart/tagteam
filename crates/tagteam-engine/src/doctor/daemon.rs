use std::ffi::OsStr;
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::Path;

use tagteam_provider::doctor::quoted;
use tagteam_provider::process::current_uid;
use tagteam_provider::{Check, Liveness, ProcessStamp, Read};

use crate::daemon::{
    DaemonRecord, MAX_SOCKET_PATH, daemon_lock_path, daemon_socket_path, find_on_path,
    read_daemon_record,
};
use crate::service::{
    LAUNCHD_LABEL, SYSTEMD_UNIT, ServiceKind, ServiceState, UnitContents, lingering, parse_unit,
    service_state, unit_path,
};
use crate::settings::Settings;

use super::files::readable;
use super::{Run, and_list};

/// D10: how long `launchctl print`, `systemctl --user show` and `loginctl show-user` may take.
const MANAGER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// `daemon.running` for `daemon.lock`'s record (D4.1): a live daemon of another version is told
/// to restart (D10).
fn daemon_record_check(r: &DaemonRecord, liveness: Liveness) -> Check {
    const ID: &str = "daemon.running";
    let this = env!("CARGO_PKG_VERSION");
    match liveness {
        Liveness::Live if r.version == this => Check::info(
            ID,
            format!("the daemon runs (pid {}, version {})", r.pid, r.version),
        ),
        Liveness::Live => Check::warn(
            ID,
            format!(
                "the daemon runs (pid {}) on version {}, not this tagteam's {this}",
                r.pid, r.version
            ),
        )
        .fix("restart it: a daemon restarts by itself within a minute of an upgrade; if this one has not, `tagteam daemon install` restarts the service's daemon, and a foreground one restarts once you stop it (Ctrl-C) and start it again"),
        Liveness::Dead => Check::info(
            ID,
            format!(
                "the daemon is not running (its last run, pid {}, has exited)",
                r.pid
            ),
        ),
        Liveness::Unknown(e) => Check::warn(
            ID,
            format!(
                "whether the daemon runs as pid {} cannot be told: the process's start time cannot be read ({e})",
                r.pid
            ),
        )
        .fix(format!(
            "check whether pid {} is still a `tagteam daemon`, then `tagteam doctor` again",
            r.pid
        )),
    }
}

/// `daemon.service`'s finding for an installed unit, from what its manager says (D10). A
/// manager that is missing, hangs or fails is a warning with its reason, never a failure
/// (§13.6).
fn service_state_check(kind: ServiceKind, path: &Path, state: &ServiceState) -> Check {
    const ID: &str = "daemon.service";
    let unit = format!(
        "the daemon's {} is installed at {}",
        kind.noun(),
        path.display()
    );
    if let Some(why) = &state.unknown {
        let fix = match kind {
            ServiceKind::Launchd => {
                format!("`launchctl print gui/$(id -u)/{LAUNCHD_LABEL}` shows its state")
            }
            ServiceKind::Systemd => {
                format!("`systemctl --user status {SYSTEMD_UNIT}` shows its state")
            }
        };
        return Check::warn(
            ID,
            format!("{unit}, but whether it is loaded cannot be told: {why}"),
        )
        .fix(fix);
    }
    match (state.loaded, state.running, state.last_exit_status) {
        (Some(true), Some(true), _) => Check::info(ID, format!("{unit}, loaded and running")),
        (Some(true), _, Some(code)) => Check::info(
            ID,
            format!("{unit}, loaded but not running (its last exit status was {code})"),
        ),
        (Some(true), _, None) => Check::info(ID, format!("{unit}, loaded but not running")),
        _ => Check::info(
            ID,
            format!("{unit} but not loaded, so it starts at the next login"),
        )
        .fix("`tagteam daemon install` loads and starts it now"),
    }
}

impl Run<'_> {
    /// D10: the daemon, its service unit, its socket and its notifications. They are read-only
    /// like every check (§13.6):
    /// - `daemon.lock`'s record is read without trying the lock.
    /// - The socket is looked at and never connected to.
    /// - The service manager is only asked, within 5 s each: `launchctl print`,
    ///   `systemctl --user show`, `loginctl show-user`.
    ///
    /// `service` is this platform's manager. Without one, the unit and the notifications are
    /// not checked, and the notifications are checked only while a daemon runs or a unit is
    /// installed.
    pub(super) fn daemon(&mut self, service: Option<ServiceKind>) {
        let live = self.daemon_running();
        let (installed, unit) = match service {
            Some(kind) => self.daemon_service(kind),
            None => (false, None),
        };
        self.daemon_socket(live, installed);
        // Decision 12's gate, as `daemon.socket` has it: a machine that does not use the daemon
        // carries no warning for its notifications.
        if let Some(kind) = service
            && (live || installed)
        {
            self.daemon_notify(kind, unit.as_ref());
        }
    }

    /// `daemon.running`: the daemon `daemon.lock`'s record names (D4.1), judged by an exact pid
    /// and start-time match (§12.6). It returns whether the daemon runs. A liveness that cannot
    /// be told counts as running, as `is_live` counts it.
    fn daemon_running(&mut self) -> bool {
        let lock = daemon_lock_path(&self.engine.env);
        let (check, live) = match read_daemon_record(&lock) {
            Read::Absent => (
                Check::info("daemon.running", "the daemon is not running"),
                false,
            ),
            Read::Present(r) => {
                let liveness = ProcessStamp {
                    pid: r.pid,
                    start: r.start,
                }
                .liveness();
                let live = liveness != Liveness::Dead;
                (daemon_record_check(&r, liveness), live)
            }
            Read::Unreadable(e) => (
                Check::warn(
                    "daemon.running",
                    format!("whether the daemon runs cannot be told: {e}"),
                )
                .fix(readable(&lock)),
                false,
            ),
        };
        self.push(None, check);
        live
    }

    /// Whether `holder`, an engine lock's record, names the process `daemon.lock` names, by pid
    /// and start time (D10).
    pub(super) fn held_by_daemon(&self, holder: &ProcessStamp) -> bool {
        matches!(
            read_daemon_record(&daemon_lock_path(&self.engine.env)),
            Read::Present(r) if r.pid == holder.pid && r.start == holder.start
        )
    }

    /// `daemon.service` (D10) reports:
    /// - whether the unit is installed, and what its manager says of it
    /// - the program it starts, which must exist
    /// - the variables it records against doctor's own (D9.1)
    /// - on Linux, lingering (D9.3)
    ///
    /// The manager is asked only about a unit that is there. It returns whether one is
    /// installed, and the unit as read when tagteam wrote it as it stands (Decision 5).
    fn daemon_service(&mut self, kind: ServiceKind) -> (bool, Option<UnitContents>) {
        const ID: &str = "daemon.service";
        let engine = self.engine;
        let env = &engine.env;
        let path = unit_path(env, kind);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                self.push(
                    None,
                    Check::info(
                        ID,
                        format!(
                            "the daemon's {} is not installed ({})",
                            kind.noun(),
                            path.display()
                        ),
                    ),
                );
                return (false, None);
            }
            Err(e) => {
                self.push(
                    None,
                    Check::warn(
                        ID,
                        format!(
                            "the daemon's {} at {} cannot be read ({})",
                            kind.noun(),
                            path.display(),
                            e.kind()
                        ),
                    )
                    .fix(readable(&path)),
                );
                return (true, None);
            }
        };
        let state = service_state(env, kind, engine.spawner.as_ref(), MANAGER_TIMEOUT);
        self.push(None, service_state_check(kind, &path, &state));
        let unit = parse_unit(kind, &text);
        match &unit {
            None => self.push(
                None,
                Check::info(
                    ID,
                    format!(
                        "{} was not written by tagteam as it stands, so the program it starts and its environment are not checked",
                        path.display()
                    ),
                ),
            ),
            Some(u) => {
                match u.program.try_exists() {
                    Ok(true) => {}
                    Ok(false) => self.push(
                        None,
                        Check::fail(
                            ID,
                            format!(
                                "{} starts {}, which does not exist, so the service cannot start the daemon",
                                path.display(),
                                u.program.display()
                            ),
                        )
                        .fix("`tagteam daemon install` writes the unit again for this tagteam"),
                    ),
                    Err(e) => self.push(
                        None,
                        Check::warn(
                            ID,
                            format!(
                                "whether {}, which {} starts, exists cannot be told ({})",
                                u.program.display(),
                                path.display(),
                                e.kind()
                            ),
                        )
                        .fix(readable(&u.program)),
                    ),
                }
                // PATH is left out: it places no store or home, and a version manager changes
                // it in every directory it enters.
                let mut differ = Vec::new();
                for name in self.engine.recorded_vars().into_iter().filter(|n| *n != "PATH") {
                    let recorded = u
                        .env
                        .iter()
                        .find(|(k, _)| k == name)
                        .map(|(_, v)| OsStr::new(v.as_str()));
                    if recorded != env.var(name) {
                        differ.push(name);
                    }
                }
                if !differ.is_empty() {
                    self.push(
                        None,
                        Check::warn(
                            ID,
                            format!(
                                "{} records {} differently from this environment, so the daemon may use another store, home, proxy or log level",
                                path.display(),
                                and_list(&differ)
                            ),
                        )
                        .fix("run `tagteam daemon install` again from the environment whose store and home the daemon should use"),
                    );
                }
            }
        }
        if kind == ServiceKind::Systemd {
            let user = env
                .user
                .clone()
                .unwrap_or_else(|| current_uid().to_string());
            match lingering(env, engine.spawner.as_ref(), &user, MANAGER_TIMEOUT) {
                Ok(true) => {}
                Ok(false) => self.push(
                    None,
                    Check::info(
                        ID,
                        format!(
                            "lingering is off for {user}, so systemd stops the daemon when you log out"
                        ),
                    )
                    .fix(format!("`loginctl enable-linger {user}` keeps it running")),
                ),
                // A `loginctl` missing, hung or failing is a warning with its
                // reason, never silence and never a failure.
                Err(why) => self.push(
                    None,
                    Check::warn(
                        ID,
                        format!("whether lingering is on for {user} cannot be told: {why}"),
                    )
                    .fix(format!("`loginctl show-user {user} -p Linger` shows it")),
                ),
            }
        }
        (true, unit)
    }

    /// `daemon.socket` (D5.1, D10): the socket file, against the daemon that should own it. An
    /// entry that is not a socket warns first, since the daemon cannot bind over it. A
    /// path too long to bind matters once a daemon runs or a unit is installed: the daemon then
    /// runs, or would run, without its socket, and the check fails. With neither, the length is
    /// not judged.
    fn daemon_socket(&mut self, live: bool, installed: bool) {
        const ID: &str = "daemon.socket";
        let path = daemon_socket_path(&self.engine.env);
        let len = path.as_os_str().len();
        if len > MAX_SOCKET_PATH && (live || installed) {
            self.push(
                None,
                Check::fail(
                    ID,
                    format!(
                        "{} is {len} bytes, too long for a socket here (at most {MAX_SOCKET_PATH}), so the daemon runs without it and nothing can reach it",
                        path.display()
                    ),
                )
                .fix("set XDG_DATA_HOME to a shorter directory, then `tagteam daemon install` again"),
            );
            return;
        }
        let check = match fs::symlink_metadata(&path) {
            Ok(m) if !m.file_type().is_socket() => {
                let (what, remove) = if m.is_dir() {
                    ("a directory", "rmdir")
                } else if m.is_file() {
                    ("a file", "rm")
                } else if m.file_type().is_symlink() {
                    ("a symbolic link", "rm")
                } else {
                    ("something else", "rm")
                };
                Check::warn(
                    ID,
                    format!(
                        "{} is {what}, not a socket, so the daemon cannot listen there",
                        path.display()
                    ),
                )
                .fix(format!("remove it: {remove} {}", quoted(&path)))
            }
            Ok(m) => {
                let mode = m.permissions().mode() & 0o777;
                if mode != 0o600 {
                    let fix = if live {
                        format!(
                            "`chmod 600 {}`, or restart the daemon, which creates it 0600",
                            quoted(&path)
                        )
                    } else {
                        format!("rm {}", quoted(&path))
                    };
                    Check::warn(
                        ID,
                        format!(
                            "{} has mode {mode:o}, not 600: another user may reach the daemon through it",
                            path.display()
                        ),
                    )
                    .fix(fix)
                } else if live {
                    Check::ok(ID, format!("the daemon listens on {}", path.display()))
                } else {
                    Check::info(
                        ID,
                        format!(
                            "{} is left from a daemon that is not running; the next one to start replaces it",
                            path.display()
                        ),
                    )
                }
            }
            Err(e) if e.kind() == ErrorKind::NotFound => {
                if live {
                    Check::warn(
                        ID,
                        format!(
                            "the daemon runs without its socket ({} does not exist), so `tagteam daemon status` and the TUI cannot reach it",
                            path.display()
                        ),
                    )
                    .fix("the log says why it could not bind; restart the daemon once that is fixed")
                } else {
                    Check::ok(ID, "no daemon runs, and none left a socket behind")
                }
            }
            Err(e) => Check::warn(
                ID,
                format!("{} cannot be read ({})", path.display(), e.kind()),
            )
            .fix(readable(&path)),
        };
        self.push(None, check);
    }

    /// `daemon.notify` (D6.2, D10): notifications that are on, with nothing to show them, while
    /// a daemon runs or a unit is installed. macOS always has `/usr/bin/osascript`. Linux needs
    /// `notify-send` on the daemon's `PATH`: the one its unit records, when doctor can read the
    /// unit, else doctor's own.
    /// `notify.enabled` is read from `config.toml`, as the `auto` check reads its settings.
    fn daemon_notify(&mut self, kind: ServiceKind, unit: Option<&UnitContents>) {
        const ID: &str = "daemon.notify";
        let engine = self.engine;
        let (settings, _) = Settings::load(&engine.env, engine.default_provider());
        let check = if !settings.notify_enabled {
            Check::info(ID, "notifications are off (notify.enabled is false)")
        } else {
            match kind {
                ServiceKind::Launchd => Check::ok(ID, "notifications are on, through osascript"),
                ServiceKind::Systemd => {
                    let recorded = unit
                        .and_then(|u| u.env.iter().find(|(k, _)| k == "PATH"))
                        .map(|(_, v)| OsStr::new(v.as_str()));
                    match find_on_path("notify-send", recorded.or_else(|| engine.env.var("PATH"))) {
                        Some(bin) => Check::ok(
                            ID,
                            format!("notifications are on, through {}", bin.display()),
                        ),
                        None => Check::warn(
                            ID,
                            "notifications are on, but notify-send is not on the daemon's PATH, so none is shown",
                        )
                        .fix("install notify-send (libnotify), or turn them off with `tagteam config set notify.enabled false`"),
                    }
                }
            }
        };
        self.push(None, check);
    }
}
