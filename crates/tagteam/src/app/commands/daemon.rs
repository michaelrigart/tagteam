use std::ffi::OsStr;

use tagteam_core::ProviderId;
use tagteam_core::autoswitch::Strategy;
use tagteam_daemon::auto::{self, AutoError, AutoFlags, AutoRun};
use tagteam_daemon::daemon::{Daemon, DaemonExit, DaemonOptions};
use tagteam_daemon::notify::Backend;
use tagteam_daemon::scheduler::{ThreadSleeper, uniform_jitter};
use tagteam_daemon::service::{self as daemon_service, InstallError, MANAGER_TIMEOUT};
use tagteam_daemon::watch::{ProgramStamp, UPGRADE_WATCH_MS};
use tagteam_engine::daemon::{daemon_lock_path, read_daemon_record};
use tagteam_engine::service::{recorded_env, render_unit, service_state, unit_path};
use tagteam_engine::settings::parse_bool;
use tagteam_engine::{Engine, EngineError};
use tagteam_provider::profile::RunShell;
use tagteam_provider::{Platform, Read};

use crate::auto::{HumanSink, JsonSink};
use crate::cli::{AutoStrategyArg, DaemonAction};
use crate::{daemon_cmd, service_cmd};

use super::accounts::model_list;
use crate::app::context::{Io, note_to};
use crate::app::failure::{
    Failure, KIND_DAEMON, KIND_ENGINE_RUNNING, KIND_INVALID_INPUT, KIND_IO, KIND_NO_CANDIDATES,
    KIND_NO_SERVICE_MANAGER, KIND_SERVICE,
};
use crate::app::{App, display_name};

const NOTHING_TO_SWITCH: &str =
    "auto-switch needs two switchable accounts on a provider; add another with `tagteam add`";
const BAD_THRESHOLD: &str = "--threshold takes a number from 50 to 99.9";
const BAD_INCLUDE: &str = "--include-api-key-accounts takes true, false, 1, 0, yes or no";

/// `auto`'s flags as the loop takes them (§11.4); each is clamped there (§6.4). clap reads `nan`
/// and `inf` as numbers, so `--threshold` must be finite. `--include-api-key-accounts` takes
/// §6.4's booleans.
pub(in crate::app) fn auto_flags(
    threshold: Option<f64>,
    interval: Option<i64>,
    cooldown: Option<i64>,
    strategy: Option<AutoStrategyArg>,
    model: Option<String>,
    include_api_key_accounts: Option<String>,
) -> Result<AutoFlags, Failure> {
    if threshold.is_some_and(|t| !t.is_finite()) {
        return Err(Failure::Usage(BAD_THRESHOLD.into()));
    }
    let include_api_key_accounts = match include_api_key_accounts.as_deref() {
        Some(v) => Some(parse_bool(v).ok_or_else(|| Failure::Usage(BAD_INCLUDE.into()))?),
        None => None,
    };
    Ok(AutoFlags {
        threshold,
        interval_s: interval,
        cooldown_s: cooldown,
        strategy: strategy.map(|s| match s {
            AutoStrategyArg::Best => Strategy::Best,
            AutoStrategyArg::ConsumeFirst => Strategy::ConsumeFirst,
        }),
        models: model.as_deref().map(model_list),
        include_api_key_accounts,
    })
}

/// How `auto` fails as a command, each provider named by `name`. A provider whose engine is the
/// daemon's (`daemon` gives its pid) is named with the daemon and how to pause it (D8.3).
/// Consume-first named for a provider without a long window is a usage error, exit 2, as every
/// bad flag value is (§4.5).
fn auto_failure(
    e: AutoError,
    name: &dyn Fn(&ProviderId) -> String,
    daemon: &dyn Fn(&ProviderId) -> Option<u32>,
) -> Failure {
    match e {
        AutoError::AlreadyRuns(providers) => {
            let names: Vec<String> = providers
                .iter()
                .map(|p| match daemon(p) {
                    Some(pid) => format!(
                        "{} (the daemon, pid {pid}; pause it with tagteam config set daemon.autoswitch false)",
                        name(p)
                    ),
                    None => name(p),
                })
                .collect();
            Failure::Message(
                KIND_ENGINE_RUNNING,
                format!("auto-switch already runs for {}", names.join(", ")),
            )
        }
        AutoError::NothingToSwitch => {
            Failure::Message(KIND_NO_CANDIDATES, NOTHING_TO_SWITCH.into())
        }
        AutoError::NoLongWindow(p) => Failure::Usage(format!(
            "--strategy consume-first needs a long usage window to rank by, and {} has none",
            name(&p)
        )),
        AutoError::Engine(e) => e.into(),
    }
}

/// How `daemon install` and `uninstall` fail as a command (D9.4): exit 1, with the manager's
/// own words. D9.2: launchd loads an agent only in a GUI login session, which a failed
/// `bootstrap`'s message says.
fn install_failure(e: InstallError) -> Failure {
    let kind = match &e {
        InstallError::Unit(_) => KIND_INVALID_INPUT,
        InstallError::Io(_) => KIND_IO,
        InstallError::NoManager(_) => KIND_NO_SERVICE_MANAGER,
        InstallError::Manager { .. } | InstallError::NoAnswer { .. } => KIND_SERVICE,
    };
    let message = match &e {
        InstallError::Manager { program, .. } if program == "launchctl bootstrap" => format!(
            "{e}; launchd loads an agent only in a GUI login session, not over SSH alone: run `tagteam daemon` in the foreground instead"
        ),
        _ => e.to_string(),
    };
    Failure::Message(kind, message)
}

/// D8.3: the daemon's pid when `provider`'s engine lock record names the process `daemon.lock`
/// names, by pid and start time; `None` for any other holder, or a record that cannot be read.
fn daemon_engine(engine: &Engine, provider: &ProviderId) -> Option<u32> {
    let Read::Present(holder) = engine.engine_holder(provider) else {
        return None;
    };
    match read_daemon_record(&daemon_lock_path(engine.env())) {
        Read::Present(d) if d.pid == holder.pid && d.start == holder.start => Some(d.pid),
        _ => None,
    }
}

impl App<'_, '_> {
    pub(in crate::app) fn auto_command(
        &mut self,
        once: bool,
        dry_run: bool,
        flags: AutoFlags,
    ) -> Result<i32, Failure> {
        self.auto(AutoRun {
            provider: self.provider_flag.clone(),
            flags,
            once,
            dry_run,
        })
    }

    /// §11: `auto`. Like every command that changes the live login it refuses inside a run
    /// shell, except with `--dry-run` (§11.1); `run_command` has run the Keychain check. Events
    /// go to stdout as human lines or JSONL (§11.4), warnings and errors to stderr.
    fn auto(&mut self, run: AutoRun) -> Result<i32, Failure> {
        if !run.dry_run && matches!(self.engine.run_shell(), RunShell::Inside { .. }) {
            return Err(EngineError::InsideRunShell.into());
        }
        let several = auto::providers(&self.engine, run.provider.as_ref())?.len() > 1;
        let color = self.color();
        let engine = &self.engine;
        let now_ms = || engine.now_ms();
        let names = |p: &ProviderId| display_name(engine, p);
        let daemon = |p: &ProviderId| daemon_engine(engine, p);
        let Io { out, err, .. } = &mut *self.io;
        let ended = if self.json {
            let sink = JsonSink::new(&mut **out, &now_ms).stopping(engine.cancel());
            auto::run_loop(engine, &run, &sink, &ThreadSleeper, &mut uniform_jitter)
        } else {
            let sink = HumanSink::new(&mut **out, &mut **err, &now_ms, &names, color, several)
                .stopping(engine.cancel());
            auto::run_loop(engine, &run, &sink, &ThreadSleeper, &mut uniform_jitter)
        };
        ended.map_err(|e| auto_failure(e, &names, &daemon))
    }

    /// D8, D9: `daemon install`, `uninstall` and `status`. `tagteam daemon` with no action is
    /// the foreground run.
    pub(in crate::app) fn daemon_action(&mut self, action: DaemonAction) -> Result<i32, Failure> {
        match action {
            DaemonAction::Install { print, no_start } => self.daemon_install(print, no_start),
            DaemonAction::Uninstall => self.daemon_uninstall(),
            DaemonAction::Status => self.daemon_status(),
        }
    }

    /// D9: writes the unit and loads it, restarting a daemon it already runs. `--no-start`
    /// writes it (and on Linux enables it) only, and `--print` prints it and changes nothing.
    /// It refuses inside a run shell, whose environment is a session's, not the one the daemon
    /// should keep: `--print` too, since what it prints would record that environment.
    fn daemon_install(&mut self, print: bool, no_start: bool) -> Result<i32, Failure> {
        if matches!(self.engine.run_shell(), RunShell::Inside { .. }) {
            return Err(EngineError::InsideRunShell.into());
        }
        let env = self.engine.env().clone();
        let kind = self.service();
        let program = service_cmd::install_program(&env).map_err(|e| {
            Failure::Message(
                KIND_IO,
                format!("this tagteam's own path cannot be read: {e}"),
            )
        })?;
        let recorded = recorded_env(&self.engine.recorded_vars(), &|name| {
            env.var(name).map(OsStr::to_os_string)
        })
        .map_err(|e| Failure::Message(KIND_INVALID_INPUT, e.to_string()))?;
        if print {
            let unit = render_unit(kind, &program, &recorded)
                .map_err(|e| Failure::Message(KIND_INVALID_INPUT, e.to_string()))?;
            let json = service_cmd::print_json(kind, &unit_path(&env, kind), &unit);
            self.print(&unit, json);
            return Ok(0);
        }
        let spawner = self.engine.spawner();
        let report =
            daemon_service::install(&env, kind, &program, &recorded, !no_start, spawner.as_ref())
                .map_err(install_failure)?;
        if !self.json {
            for w in &report.warnings {
                self.io.warn(w);
            }
        }
        self.print(
            &service_cmd::install_human(&report),
            service_cmd::install_json(&report),
        );
        Ok(0)
    }

    /// D9: stops the daemon the unit runs and removes the unit. With none installed it changes
    /// nothing and succeeds; with no file but a unit still loaded, it stops that (D9.4). It
    /// refuses inside a run shell, as `install` does.
    fn daemon_uninstall(&mut self) -> Result<i32, Failure> {
        if matches!(self.engine.run_shell(), RunShell::Inside { .. }) {
            return Err(EngineError::InsideRunShell.into());
        }
        let env = self.engine.env().clone();
        let kind = self.service();
        let spawner = self.engine.spawner();
        let found =
            daemon_service::uninstall(&env, kind, spawner.as_ref()).map_err(install_failure)?;
        self.print(
            &service_cmd::uninstall_human(kind, &unit_path(&env, kind), found),
            service_cmd::uninstall_json(found),
        );
        Ok(0)
    }

    /// D8.2: the daemon's own `status` from its socket, else what its lock and its service
    /// say. It exits 0 when a daemon runs, 3 when none does, and 1 on an error. `--json`
    /// prints its object either way (plan B Decision 6). It is read-only, so it answers inside
    /// a run shell too.
    fn daemon_status(&mut self) -> Result<i32, Failure> {
        let env = self.engine.env().clone();
        let kind = self.service();
        let found = service_cmd::find_daemon(&env)
            .map_err(|e| Failure::Message(KIND_DAEMON, e.to_string()))?;
        let spawner = self.engine.spawner();
        let state = service_state(&env, kind, spawner.as_ref(), MANAGER_TIMEOUT);
        let path = unit_path(&env, kind);
        self.print(
            &service_cmd::status_human(&found, kind, &path, &state),
            service_cmd::status_json(&found, kind, &path, &state),
        );
        Ok(if found.running() {
            0
        } else {
            service_cmd::EXIT_NOT_RUNNING
        })
    }

    /// D8.1: `tagteam daemon` in the foreground. It stamps its program file first (Decision
    /// 17). Like `auto`, it refuses inside a run shell (D4.1 step 1). On a terminal it runs
    /// Appendix A.3's check and offers to unlock, as `auto` does. Without a terminal it runs
    /// none: a service has nobody to answer, and the check can block on a SecurityAgent dialog
    /// (D4.1 step 3, Decision 14). It exits 0 once a signal stops it, and 75 once a newer
    /// tagteam must take over (D4.5).
    pub(in crate::app) fn daemon(&mut self) -> Result<i32, Failure> {
        // A test-support build's hooks; a release build ignores them.
        let hook = |k: &str| std::env::var_os(k);
        // D4.5, Decision 17: before anything that can wait (the unlock offer, the
        // `daemon.lock` wait), so a binary replaced meanwhile is an upgrade at the upgrade
        // watch's first run, never this daemon's own baseline.
        let program = ProgramStamp::take(daemon_cmd::test_program(
            daemon_cmd::program(self.engine.env().var("PATH")),
            &hook,
        ));
        if matches!(self.engine.run_shell(), RunShell::Inside { .. }) {
            return Err(EngineError::InsideRunShell.into());
        }
        if self.io.prompter.interactive() {
            self.ensure_unlocked()?;
        }
        let color = self.color();
        let engine = &self.engine;
        let path = engine.env().var("PATH");
        let opts = DaemonOptions {
            provider: self.provider_flag.clone(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            program,
            upgrade_watch_ms: daemon_cmd::test_upgrade_watch_ms(UPGRADE_WATCH_MS, &hook),
            keychain_counts: self.keychain_counts.clone(),
            backend: Backend::detect(self.platform == Platform::MacOs, path),
        };
        let several = self.provider_flag.is_none() && engine.provider_ids().len() > 1;
        let now_ms = || engine.now_ms();
        let names = |p: &ProviderId| display_name(engine, p);
        let Io { out, err, .. } = &mut *self.io;
        let started = Daemon::start(engine, opts, &mut |notice| {
            note_to(err, notice);
        })?;
        // D4.1 step 5: the socket, once `daemon.lock` is held and before the scheduler runs.
        let server = daemon_cmd::bind_socket(engine.env(), &started.board());
        let exit = if self.json {
            let sink = daemon_cmd::DaemonJson::new(&mut **out, &now_ms).stopping(engine.cancel());
            daemon_cmd::serve(started, &sink, server.as_ref())
        } else {
            let auto = HumanSink::new(&mut **out, &mut **err, &now_ms, &names, color, several)
                .stopping(engine.cancel());
            let sink = daemon_cmd::DaemonHuman::new(auto, engine.env());
            sink.started(std::process::id());
            daemon_cmd::serve(started, &sink, server.as_ref())
        };
        // D4.6: the hub removed the socket file when `stopping` went out, while `daemon.lock`
        // was still held. Each connection has written `stopping`; this closes them.
        if let Some(server) = server {
            server.shutdown();
        }
        Ok(match exit {
            DaemonExit::Signal => 0,
            DaemonExit::Restart => daemon_cmd::EXIT_RESTART,
        })
    }
}

#[cfg(test)]
mod tests {
    use tagteam_cc::CLAUDE_CODE;
    use tagteam_engine::settings::Settings;

    use crate::cli::{Cli, Command};

    use super::{
        AutoError, Failure, KIND_ENGINE_RUNNING, ProviderId, auto, auto_failure, auto_flags,
    };

    /// `auto`'s flags as parsed from `args` and resolved against the default settings.
    fn parsed_auto_config(args: &[&str]) -> (f64, i64, i64) {
        use clap::Parser;
        let cli =
            Cli::try_parse_from(std::iter::once("tagteam").chain(args.iter().copied())).unwrap();
        let Some(Command::Auto {
            threshold,
            interval,
            cooldown,
            ..
        }) = cli.command
        else {
            panic!("not auto");
        };
        let Ok(flags) = auto_flags(threshold, interval, cooldown, None, None, None) else {
            panic!("bad flags");
        };
        let c = auto::auto_config(&Settings::default(), &flags, None);
        (c.threshold, c.interval_s, c.cooldown_s)
    }

    #[test]
    fn negative_numeric_flags_parse_and_clamp_to_their_ranges() {
        // §6.4: a flag is clamped to its setting's range, so a negative value reaches the
        // clamp instead of being refused as an unexpected argument.
        assert_eq!(parsed_auto_config(&["auto", "--cooldown", "-1"]).2, 0);
        assert_eq!(parsed_auto_config(&["auto", "--interval", "-5"]).1, 15);
        assert_eq!(parsed_auto_config(&["auto", "--threshold", "-3"]).0, 50.0);
    }

    #[test]
    fn consume_first_named_for_a_provider_without_a_long_window_is_a_usage_error() {
        // §4.5. FakeAgent names no long window; `run_loop` refuses the combination before any
        // engine starts, and the command exits 2 in the usage error's shape.
        use tagteam_fake::FakeAgent;
        use tagteam_provider::Provider;
        let fake = FakeAgent::new();
        assert_eq!(fake.primary_long_window(), None);
        let name = |_: &ProviderId| fake.display_name().to_owned();
        let Failure::Usage(message) =
            auto_failure(AutoError::NoLongWindow(fake.id()), &name, &|_| None)
        else {
            panic!("not a usage error");
        };
        assert_eq!(
            message,
            "--strategy consume-first needs a long usage window to rank by, and FakeAgent has none"
        );
    }

    #[test]
    fn auto_names_the_daemon_when_it_holds_the_engine_lock() {
        // D8.3: the holder's record matches daemon.lock's, so the refusal names the daemon and
        // how to pause it.
        let cc = ProviderId::new(CLAUDE_CODE);
        let name = |_: &ProviderId| "Claude Code".to_owned();
        let refusal = |daemon: Option<u32>| match auto_failure(
            AutoError::AlreadyRuns(vec![cc.clone()]),
            &name,
            &|_| daemon,
        ) {
            Failure::Message(kind, message) => {
                assert_eq!(kind, KIND_ENGINE_RUNNING);
                message
            }
            _ => panic!("not engine-running"),
        };
        assert_eq!(
            refusal(Some(4242)),
            "auto-switch already runs for Claude Code (the daemon, pid 4242; pause it with tagteam config set daemon.autoswitch false)"
        );
        assert_eq!(refusal(None), "auto-switch already runs for Claude Code");
    }
}
