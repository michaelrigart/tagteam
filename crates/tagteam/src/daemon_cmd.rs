//! `tagteam daemon` (D8.1): the lines a person reads, the JSONL a script reads, and what the
//! command hands the daemon. `tagteam-daemon` schedules and decides; this module only renders.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use tagteam_core::ProviderId;
use tagteam_daemon::events::{DaemonEvent, DaemonSink, Mode, StopReason, WaitingFor};
use tagteam_daemon::notify::Notice;
use tagteam_daemon::protocol::daemon_event_json;
use tagteam_daemon::watch::program_path;
use tagteam_engine::auto::EventSink;
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
            DaemonEvent::Changed => {}
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
