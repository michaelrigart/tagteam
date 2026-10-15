//! `tagteam daemon` (D8.1): the lines a person reads, the JSONL a script reads, and what the
//! command hands the daemon. `tagteam-daemon` schedules and decides; this module only renders.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use tagteam_core::ProviderId;
use tagteam_daemon::daemon::{DaemonExit, Started};
use tagteam_daemon::events::{DaemonEvent, DaemonSink, Fanout, Mode, StopReason, WaitingFor};
use tagteam_daemon::notify::Notice;
use tagteam_daemon::protocol::daemon_event_json;
use tagteam_daemon::scheduler::{ThreadSleeper, uniform_jitter};
use tagteam_daemon::server::Server;
use tagteam_daemon::status::StatusBoard;
use tagteam_daemon::watch::program_path;
use tagteam_engine::auto::EventSink;
use tagteam_engine::daemon::daemon_socket_path;
use tagteam_engine::settings::Settings;
use tagteam_provider::keychain::ReadCounts;
use tagteam_provider::{Cancel, Env, Keychain};

use crate::auto::{HumanSink, written};

/// D4.5, D8.1: stopped so that a newer tagteam takes over, either a new binary or the one that
/// migrated the store. A service restarts it.
pub(crate) const EXIT_RESTART: i32 = 75;

/// Decision 7: the read counts of each Keychain instance in `keychains`, an instance named
/// twice counted once. The vault's Keychain is usually the command's own.
pub(crate) fn keychain_counts<'k>(
    keychains: impl IntoIterator<Item = &'k Arc<dyn Keychain>>,
) -> Vec<Arc<ReadCounts>> {
    let mut counts: Vec<Arc<ReadCounts>> = Vec::new();
    for c in keychains.into_iter().filter_map(|k| k.read_counts()) {
        if !counts.iter().any(|seen| Arc::ptr_eq(seen, &c)) {
            counts.push(c);
        }
    }
    counts
}

/// D4.5: the path the upgrade watch follows. It is `argv[0]` when that is absolute, as a
/// service unit passes it, and otherwise what `daemon install` would record (D9.1).
pub(crate) fn program(path_var: Option<&OsStr>) -> PathBuf {
    let argv0 = std::env::args_os().next().unwrap_or_default();
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from(&argv0));
    program_path(&argv0, path_var, &exe)
}

/// D4.1 step 5, D5.1: the socket, bound once start-up holds `daemon.lock` and before the
/// scheduler runs. A bind that fails is logged at ERROR, and the daemon runs without a socket:
/// scheduling never depends on it.
pub(crate) fn bind_socket(env: &Env, board: &Arc<StatusBoard>) -> Option<Server> {
    let path = daemon_socket_path(env);
    match Server::bind(&path, &env.home, Arc::clone(board), env.cancel.clone()) {
        Ok(server) => {
            board.set_socket(Some(path));
            Some(server)
        }
        Err(e) => {
            tracing::error!(error = %e, "the daemon runs without its socket");
            None
        }
    }
}

/// D5.5, D8.1: the daemon's run, every event going to `sink` (the command's own output) and
/// then, when the socket is bound, to its subscribers through the hub. The hub closes the
/// socket when `stopping` reaches it, before `serve` lets `daemon.lock` go (D4.6).
pub(crate) fn serve(
    started: Started<'_>,
    sink: &dyn DaemonSink,
    server: Option<&Server>,
) -> DaemonExit {
    let mut sinks: Vec<&dyn DaemonSink> = vec![sink];
    if let Some(server) = server {
        sinks.push(server.hub());
    }
    started.serve(&Fanout(sinks), &ThreadSleeper, &mut uniform_jitter)
}

/// A mode line's text (D4.3). `paused` means the provider's `daemon.autoswitch` reads false.
fn mode_text(
    mode: Mode,
    waiting_for: Option<WaitingFor>,
    holder_pid: Option<u32>,
    paused: bool,
) -> String {
    match (mode, waiting_for) {
        (Mode::Auto, _) => "mode auto".into(),
        (Mode::Collect, _) if paused => "mode collect (paused)".into(),
        (Mode::Collect, _) => "mode collect".into(),
        (Mode::Idle, _) => "mode idle".into(),
        (Mode::Waiting, Some(WaitingFor::EngineLock)) => match holder_pid {
            Some(pid) => format!("waiting for the engine lock held by pid {pid}"),
            None => "waiting for the engine lock held by another process".into(),
        },
        (Mode::Waiting, Some(WaitingFor::Schema)) => {
            "waiting for an upgrade: a newer tagteam migrated the store".into()
        }
        (Mode::Waiting, None) => "waiting".into(),
    }
}

/// A `collect` round's line (D5.5's `usage`): each recorded position, then each failed one.
fn usage_text(updated: &[u32], fetch_errors: &BTreeMap<u32, String>) -> String {
    let mut parts: Vec<String> = updated.iter().map(|p| format!("#{p} updated")).collect();
    parts.extend(
        fetch_errors
            .iter()
            .map(|(p, kind)| format!("#{p} failed ({kind})")),
    );
    if parts.is_empty() {
        return "usage: nothing collected".into();
    }
    format!("usage: {}", parts.join(", "))
}

/// The last line (D4.5, D4.6).
fn stopping_text(reason: StopReason) -> &'static str {
    match reason {
        StopReason::Signal => "stopped",
        StopReason::Upgrade => "tagteam was upgraded; start the daemon again",
        StopReason::Schema => "a newer tagteam migrated the store; start the daemon again",
    }
}

/// D8.1's human output: `auto`'s lines for the ticks, through its own `HumanSink` (§11.4), and
/// a line of the daemon's own for its start and stop, each mode change, each `collect` round
/// and each notification. Errors and warnings go to stderr, as `auto`'s do.
pub(crate) struct DaemonHuman<'a> {
    auto: HumanSink<'a>,
    /// Where `daemon.autoswitch` is read, to tell a pause from one switchable account.
    env: &'a Env,
}

impl<'a> DaemonHuman<'a> {
    pub(crate) fn new(auto: HumanSink<'a>, env: &'a Env) -> Self {
        DaemonHuman { auto, env }
    }

    /// The first line, once the daemon holds `daemon.lock`.
    pub(crate) fn started(&self, pid: u32) {
        let text = format!("daemon started (pid {pid}); Ctrl-C stops it");
        self.auto.plain(self.auto.now_s(), &text);
    }
}

impl DaemonSink for DaemonHuman<'_> {
    fn emit(&self, e: &DaemonEvent) {
        let now = self.auto.now_s();
        match e {
            DaemonEvent::Auto(e) => self.auto.emit(e),
            DaemonEvent::Mode {
                provider,
                mode,
                waiting_for,
                holder_pid,
                backoff,
            } => {
                let paused = *mode == Mode::Collect
                    && !Settings::load(self.env, provider).0.daemon_autoswitch;
                let text = mode_text(*mode, *waiting_for, *holder_pid, paused);
                let text = match backoff {
                    Some(b) => format!(
                        "{text}; keychain locked, retrying at {}",
                        self.auto.time_of(b.until)
                    ),
                    None => text,
                };
                self.auto.line(now, provider, &text);
            }
            DaemonEvent::Usage {
                provider,
                updated,
                fetch_errors,
            } => self
                .auto
                .line(now, provider, &usage_text(updated, fetch_errors)),
            DaemonEvent::Changed | DaemonEvent::TokenIndex { .. } => {}
            DaemonEvent::Stopping { reason } => self.auto.plain(now, stopping_text(*reason)),
        }
    }

    fn tick_done(&self, provider: &ProviderId) {
        self.auto.tick_done(provider);
    }

    fn notified(&self, provider: &ProviderId, notice: &Notice) {
        let text = format!("notified: {}", notice.body);
        self.auto.line(self.auto.now_s(), provider, &text);
    }
}

/// `--json` (D8.1): every event as one object on a line of its own, exactly as the socket's
/// `subscribe` delivers it (D5.5).
pub(crate) struct DaemonJson<'a> {
    out: RefCell<&'a mut dyn Write>,
    /// The engine's wall clock, epoch milliseconds: each event's `ts`.
    now_ms: &'a dyn Fn() -> i64,
    /// What a `BrokenPipe` on `out` stops the command through.
    cancel: Option<Cancel>,
}

impl<'a> DaemonJson<'a> {
    pub(crate) fn new(out: &'a mut dyn Write, now_ms: &'a dyn Fn() -> i64) -> Self {
        DaemonJson {
            out: RefCell::new(out),
            now_ms,
            cancel: None,
        }
    }

    /// A write that fails with `BrokenPipe` (the reader is gone) records SIGPIPE in `cancel`.
    pub(crate) fn stopping(mut self, cancel: &Cancel) -> Self {
        self.cancel = Some(cancel.clone());
        self
    }
}

impl DaemonSink for DaemonJson<'_> {
    fn emit(&self, e: &DaemonEvent) {
        let line = daemon_event_json(e, (self.now_ms)().div_euclid(1000));
        let mut out = self.out.borrow_mut();
        written(writeln!(out, "{line}"), &self.cancel);
    }
}

/// Plan B Decision 7: the multi-process tests' hooks, read only by a `test-support` build.
/// `TAGTEAM_TEST_UPGRADE_WATCH_MS` shortens the upgrade watch's period (D4.5), so an exit-75
/// test takes seconds. `TAGTEAM_TEST_DAEMON_PROGRAM` names the file the watch follows, so a
/// test can replace it without touching the binary it runs.
#[cfg(any(test, feature = "test-support"))]
const TEST_UPGRADE_WATCH_MS: &str = "TAGTEAM_TEST_UPGRADE_WATCH_MS";
#[cfg(any(test, feature = "test-support"))]
const TEST_DAEMON_PROGRAM: &str = "TAGTEAM_TEST_DAEMON_PROGRAM";

/// Decision 7: the path the upgrade watch follows. It is `program`, unless `var` gives
/// `TAGTEAM_TEST_DAEMON_PROGRAM` a value that is not empty.
#[cfg(feature = "test-support")]
pub(crate) fn test_program(
    program: PathBuf,
    var: &dyn Fn(&str) -> Option<std::ffi::OsString>,
) -> PathBuf {
    var(TEST_DAEMON_PROGRAM)
        .filter(|p| !p.is_empty())
        .map_or(program, PathBuf::from)
}

/// Decision 7: the upgrade watch's period. It is `ms`, unless `var` gives
/// `TAGTEAM_TEST_UPGRADE_WATCH_MS` a positive number of milliseconds.
#[cfg(feature = "test-support")]
pub(crate) fn test_upgrade_watch_ms(
    ms: i64,
    var: &dyn Fn(&str) -> Option<std::ffi::OsString>,
) -> i64 {
    var(TEST_UPGRADE_WATCH_MS)
        .and_then(|v| v.into_string().ok())
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|ms| *ms > 0)
        .unwrap_or(ms)
}

/// A release build has no test hooks, whatever the environment holds.
#[cfg(not(feature = "test-support"))]
pub(crate) fn test_program(
    program: PathBuf,
    _var: &dyn Fn(&str) -> Option<std::ffi::OsString>,
) -> PathBuf {
    program
}

/// A release build has no test hooks, whatever the environment holds.
#[cfg(not(feature = "test-support"))]
pub(crate) fn test_upgrade_watch_ms(
    ms: i64,
    _var: &dyn Fn(&str) -> Option<std::ffi::OsString>,
) -> i64 {
    ms
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tagteam_core::CLAUDE_CODE;
    use tagteam_daemon::events::Backoff;
    use tagteam_engine::auto::AutoEvent;
    use tagteam_provider::FakeKeychain;

    use super::*;

    const T0: i64 = 1_790_000_000;

    fn cc() -> ProviderId {
        ProviderId::new(CLAUDE_CODE)
    }

    fn mode(
        mode: Mode,
        waiting_for: Option<WaitingFor>,
        holder_pid: Option<u32>,
        backoff: Option<Backoff>,
    ) -> DaemonEvent {
        DaemonEvent::Mode {
            provider: cc(),
            mode,
            waiting_for,
            holder_pid,
            backoff,
        }
    }

    /// What the human sink prints for `events`, on a fixed clock whose times read as seconds
    /// after `T0`, with `config` as `config.toml`.
    fn human(config: &str, f: impl FnOnce(&DaemonHuman<'_>)) -> (String, String) {
        let d = tempfile::tempdir().unwrap();
        let env = Env::for_test(d.path());
        fs::create_dir_all(env.config_dir()).unwrap();
        fs::write(env.config_dir().join("config.toml"), config).unwrap();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let now = || T0 * 1000;
        let names = |p: &ProviderId| format!("<{p}>");
        {
            let auto = HumanSink::new(&mut out, &mut err, &now, &names, false, false)
                .with_clock(|s| format!("t+{}", s - T0));
            f(&DaemonHuman::new(auto, &env));
        }
        (
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn the_daemon_s_own_lines_follow_auto_s() {
        // D8.1: a line per start and stop, mode change, `collect` round and notification, each
        // starting as `auto`'s lines do. `changed` prints nothing.
        let (out, err) = human("[daemon]\nautoswitch = false\n", |sink| {
            sink.started(4242);
            sink.emit(&mode(Mode::Collect, None, None, None));
            sink.emit(&mode(
                Mode::Waiting,
                Some(WaitingFor::EngineLock),
                Some(4243),
                None,
            ));
            sink.emit(&mode(
                Mode::Waiting,
                Some(WaitingFor::EngineLock),
                None,
                None,
            ));
            sink.emit(&mode(
                Mode::Auto,
                None,
                None,
                Some(Backoff { until: T0 + 60 }),
            ));
            sink.emit(&mode(Mode::Waiting, Some(WaitingFor::Schema), None, None));
            sink.emit(&mode(Mode::Idle, None, None, None));
            sink.emit(&DaemonEvent::Usage {
                provider: cc(),
                updated: vec![1, 2],
                fetch_errors: BTreeMap::from([(3, "over-budget".to_owned())]),
            });
            sink.emit(&DaemonEvent::Changed);
            sink.notified(
                &cc(),
                &Notice {
                    title: "tagteam".into(),
                    body: "Switched 2 · spare → 1 · work (7d at 95%)".into(),
                },
            );
            for reason in [StopReason::Signal, StopReason::Upgrade, StopReason::Schema] {
                sink.emit(&DaemonEvent::Stopping { reason });
            }
        });
        assert_eq!(
            out,
            "t+0  daemon started (pid 4242); Ctrl-C stops it\n\
             t+0  mode collect (paused)\n\
             t+0  waiting for the engine lock held by pid 4243\n\
             t+0  waiting for the engine lock held by another process\n\
             t+0  mode auto; keychain locked, retrying at t+60\n\
             t+0  waiting for an upgrade: a newer tagteam migrated the store\n\
             t+0  mode idle\n\
             t+0  usage: #1 updated, #2 updated, #3 failed (over-budget)\n\
             t+0  notified: Switched 2 · spare → 1 · work (7d at 95%)\n\
             t+0  stopped\n\
             t+0  tagteam was upgraded; start the daemon again\n\
             t+0  a newer tagteam migrated the store; start the daemon again\n"
        );
        assert_eq!(err, "");
        // With `daemon.autoswitch` true, `collect` is no pause: one switchable account.
        let (out, _) = human("", |sink| sink.emit(&mode(Mode::Collect, None, None, None)));
        assert_eq!(out, "t+0  mode collect\n");
    }

    #[test]
    fn a_tick_s_events_print_as_auto_prints_them() {
        let (out, err) = human("", |sink| {
            sink.emit(&DaemonEvent::Auto(AutoEvent::Poll {
                provider: cc(),
                active: Some((2, "b@x.co".into())),
                headroom_pct: BTreeMap::from([(1, Some(80.0)), (2, Some(40.0))]),
                threshold: 90.0,
                fetch_errors: BTreeMap::new(),
                windows_pct: BTreeMap::new(),
            }));
            sink.emit(&DaemonEvent::Auto(AutoEvent::NoSwitch {
                provider: cc(),
                reason: "below-threshold".into(),
                detail: String::new(),
            }));
            sink.emit(&DaemonEvent::Auto(AutoEvent::ConfigWarning {
                provider: cc(),
                message: "a warning".into(),
            }));
            sink.tick_done(&cc());
        });
        assert_eq!(out, "t+0  #2 b@x.co  60%  no switch: below-threshold\n");
        assert_eq!(err, "t+0  warning: a warning\n");
    }

    #[test]
    fn json_prints_each_event_as_the_socket_delivers_it_and_a_gone_reader_stops_it() {
        // D8.1: `daemon_event_json`, one object per line. A reader that has gone stops the
        // command as SIGPIPE would (§14.1), as `auto --json` does.
        let events = [
            mode(Mode::Auto, None, None, None),
            DaemonEvent::Stopping {
                reason: StopReason::Signal,
            },
        ];
        let mut out = Vec::new();
        let now = || T0 * 1000;
        {
            let sink = DaemonJson::new(&mut out, &now);
            for e in &events {
                sink.emit(e);
            }
        }
        let want: String = events
            .iter()
            .map(|e| format!("{}\n", daemon_event_json(e, T0)))
            .collect();
        assert_eq!(String::from_utf8(out).unwrap(), want);

        struct Gone;
        impl Write for Gone {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let cancel = Cancel::new();
        let mut gone = Gone;
        DaemonJson::new(&mut gone, &now)
            .stopping(&cancel)
            .emit(&events[0]);
        assert_eq!(cancel.requested(), Some(libc::SIGPIPE));
    }

    #[test]
    fn each_keychain_instance_s_counts_are_handed_over_once() {
        // Decision 7: the vault's Keychain is usually the command's own instance.
        let one: Arc<dyn Keychain> = Arc::new(FakeKeychain::new());
        let other: Arc<dyn Keychain> = Arc::new(FakeKeychain::new());
        assert_eq!(keychain_counts([&one, &one]).len(), 1);
        assert_eq!(keychain_counts([&one, &other]).len(), 2);
        let both = keychain_counts([&one, &other]);
        assert!(Arc::ptr_eq(&both[0], &one.read_counts().unwrap()));
    }
}

#[cfg(test)]
mod socket_tests {
    use std::path::Path;

    use tagteam_daemon::status::{DaemonInfo, NotifyInfo, StatusBoard};
    use tagteam_provider::Env;

    use super::bind_socket;

    #[test]
    fn a_socket_that_cannot_be_bound_leaves_the_daemon_running_without_one() {
        // D5.1: scheduling never depends on the socket. A path past `sun_path` is not bound,
        // nothing is created for it, and the board names no socket.
        let mut env = Env::for_test(Path::new("/tmp/tagteam-unused"));
        env.xdg_data_home = Some(Path::new("/tmp").join("d".repeat(120)));
        let board = StatusBoard::new(
            DaemonInfo {
                pid: 1,
                version: "v".into(),
                started_at: 0,
                socket: None,
                log: env.log_file(),
            },
            NotifyInfo {
                enabled: false,
                backend: None,
                events: Vec::new(),
            },
        );
        assert!(bind_socket(&env, &board).is_none());
        assert_eq!(board.snapshot().daemon.socket, None);
        assert!(!env.data_dir().exists(), "nothing was created");
    }
}

#[cfg(test)]
mod test_hook_tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use super::*;

    /// The path `tagteam daemon` would follow, before the hooks.
    const INSTALLED: &str = "/opt/homebrew/bin/tagteam";

    fn all_set(k: &str) -> Option<OsString> {
        match k {
            TEST_UPGRADE_WATCH_MS => Some("200".into()),
            TEST_DAEMON_PROGRAM => Some("/tmp/tt/bin/tagteam".into()),
            _ => None,
        }
    }

    /// Runs under both builds: with `test-support` the hooks are honoured; without it (a
    /// release build) they are ignored, whatever the environment holds (Decision 7).
    #[test]
    fn the_daemon_test_hooks_exist_only_with_test_support() {
        let honoured = cfg!(feature = "test-support");
        assert_eq!(
            test_upgrade_watch_ms(60_000, &all_set),
            if honoured { 200 } else { 60_000 }
        );
        assert_eq!(
            test_program(PathBuf::from(INSTALLED), &all_set),
            PathBuf::from(if honoured {
                "/tmp/tt/bin/tagteam"
            } else {
                INSTALLED
            })
        );
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn a_hook_value_that_is_not_usable_changes_nothing() {
        let unusable = |k: &str| match k {
            TEST_UPGRADE_WATCH_MS => Some(OsString::from("soon")),
            TEST_DAEMON_PROGRAM => Some(OsString::new()),
            _ => None,
        };
        assert_eq!(test_upgrade_watch_ms(60_000, &unusable), 60_000);
        assert_eq!(
            test_program(PathBuf::from(INSTALLED), &unusable),
            PathBuf::from(INSTALLED)
        );
        for zero_or_less in ["0", "-5"] {
            let var = |k: &str| (k == TEST_UPGRADE_WATCH_MS).then(|| OsString::from(zero_or_less));
            assert_eq!(
                test_upgrade_watch_ms(60_000, &var),
                60_000,
                "{zero_or_less}"
            );
        }
    }
}
