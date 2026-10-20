//! D5.5's events. A tick's own (§11.4's auto kinds) and the daemon's (`usage`, `changed`, `mode`,
//! `stopping`) reach one `DaemonSink`. The adapters below join it to the engine's `EventSink`
//! both ways, so an engine emits into a daemon sink, and `tagteam auto` runs on the scheduler
//! with its own sink (D2).

use std::collections::BTreeMap;

use tagteam_core::ProviderId;
use tagteam_engine::auto::{AutoEvent, EventSink};

use crate::notify::Notice;

/// D4.3: what a provider job does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Auto,
    Collect,
    Waiting,
    Idle,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Auto => "auto",
            Mode::Collect => "collect",
            Mode::Waiting => "waiting",
            Mode::Idle => "idle",
        }
    }

    pub const ALL: [Self; 4] = [Mode::Auto, Mode::Collect, Mode::Waiting, Mode::Idle];

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.as_str() == s)
    }
}

/// D4.3: what a `waiting` job waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitingFor {
    /// Another live process is the provider's engine (§11.1).
    EngineLock,
    /// The store's schema is newer than this build (D4.1 step 4).
    Schema,
}

impl WaitingFor {
    pub fn as_str(self) -> &'static str {
        match self {
            WaitingFor::EngineLock => "engine-lock",
            WaitingFor::Schema => "schema",
        }
    }

    pub const ALL: [Self; 2] = [WaitingFor::EngineLock, WaitingFor::Schema];

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.as_str() == s)
    }
}

/// D4.1: a provider job's keychain backoff, while it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    /// When the job runs next, epoch seconds.
    pub until: i64,
}

/// Why the scheduler stopped: a signal (D4.6), a replaced binary or a newer schema (D4.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    Signal,
    Upgrade,
    Schema,
}

impl StopReason {
    pub fn as_str(self) -> &'static str {
        match self {
            StopReason::Signal => "signal",
            StopReason::Upgrade => "upgrade",
            StopReason::Schema => "schema",
        }
    }
}

/// One event the daemon emits (D5.5).
#[derive(Debug, Clone, PartialEq)]
pub enum DaemonEvent {
    /// One of §11.4's kinds, from a tick or the auto job around it.
    Auto(AutoEvent),
    /// A `collect` round that recorded or failed something: the positions whose reading was
    /// recorded, and each failed fetch's error kind by position.
    Usage {
        provider: ProviderId,
        updated: Vec<u32>,
        fetch_errors: BTreeMap<u32, String>,
    },
    /// Another process committed to the store (D4.4).
    Changed,
    /// A provider job changed mode, or its keychain backoff started or ended (D4.3).
    Mode {
        provider: ProviderId,
        mode: Mode,
        waiting_for: Option<WaitingFor>,
        /// The other engine's pid, while `waiting_for` is `EngineLock`.
        holder_pid: Option<u32>,
        backoff: Option<Backoff>,
    },
    /// The last event before the daemon closes.
    Stopping { reason: StopReason },
    /// D5.5's reserved `token-index` (token report spec §12): the index's state after a change,
    /// exactly as `token_index_state` stores it. `skipped_lines` is the engine's count over
    /// every call of the last completed pass; the daemon accumulates nothing.
    TokenIndex {
        provider: ProviderId,
        state: tagteam_engine::tokens::IndexState,
    },
}

/// Where the scheduler's jobs send their events: the CLI's human or JSON output, the socket's
/// subscribers (plan B), the notifier (D6).
pub trait DaemonSink {
    fn emit(&self, e: &DaemonEvent);
    /// The end of a tick of `provider`'s engine, as `EventSink::tick_done` has it.
    fn tick_done(&self, _provider: &ProviderId) {}
    /// A notice was queued for `provider` (D6). `tagteam daemon` prints a line for it (D8.1).
    /// It is not an event, so neither `--json` nor the socket carries it.
    fn notified(&self, _provider: &ProviderId, _notice: &Notice) {}
}

/// Every `AutoEvent` an engine emits, as `DaemonEvent::Auto`, into a `DaemonSink`; the end of
/// each tick goes with them.
pub struct AutoEvents<'a>(pub &'a dyn DaemonSink);

impl EventSink for AutoEvents<'_> {
    fn emit(&self, e: &AutoEvent) {
        self.0.emit(&DaemonEvent::Auto(e.clone()));
    }

    fn tick_done(&self, provider: &ProviderId) {
        self.0.tick_done(provider);
    }
}

/// A `DaemonSink` over a `tagteam auto` `EventSink`: the auto kinds and the end of each tick are
/// handed on, and the daemon's own kinds dropped, since `auto` prints none (D2).
pub struct AutoOnly<'a>(pub &'a dyn EventSink);

impl DaemonSink for AutoOnly<'_> {
    fn emit(&self, e: &DaemonEvent) {
        if let DaemonEvent::Auto(e) = e {
            self.0.emit(e);
        }
    }

    fn tick_done(&self, provider: &ProviderId) {
        self.0.tick_done(provider);
    }
}

/// Several sinks: each call goes to every one, in order.
pub struct Fanout<'a>(pub Vec<&'a dyn DaemonSink>);

impl DaemonSink for Fanout<'_> {
    fn emit(&self, e: &DaemonEvent) {
        for sink in &self.0 {
            sink.emit(e);
        }
    }

    fn tick_done(&self, provider: &ProviderId) {
        for sink in &self.0 {
            sink.tick_done(provider);
        }
    }

    fn notified(&self, provider: &ProviderId, notice: &Notice) {
        for sink in &self.0 {
            sink.notified(provider, notice);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use tagteam_cc::CLAUDE_CODE;

    use super::*;

    fn cc() -> ProviderId {
        ProviderId::new(CLAUDE_CODE)
    }

    fn warning() -> AutoEvent {
        AutoEvent::ConfigWarning {
            provider: cc(),
            message: "m".into(),
        }
    }

    /// What a sink was handed: an event, or the end of a tick.
    #[derive(Debug, Clone, PartialEq)]
    enum Call<E> {
        Emit(E),
        Done(ProviderId),
    }

    /// Every call a sink is handed, with the sink's name, in one order shared by several sinks.
    struct Tagged<'a, E> {
        name: &'static str,
        log: &'a RefCell<Vec<(&'static str, Call<E>)>>,
    }

    impl DaemonSink for Tagged<'_, DaemonEvent> {
        fn emit(&self, e: &DaemonEvent) {
            self.log
                .borrow_mut()
                .push((self.name, Call::Emit(e.clone())));
        }

        fn tick_done(&self, provider: &ProviderId) {
            self.log
                .borrow_mut()
                .push((self.name, Call::Done(provider.clone())));
        }
    }

    impl EventSink for Tagged<'_, AutoEvent> {
        fn emit(&self, e: &AutoEvent) {
            self.log
                .borrow_mut()
                .push((self.name, Call::Emit(e.clone())));
        }

        fn tick_done(&self, provider: &ProviderId) {
            self.log
                .borrow_mut()
                .push((self.name, Call::Done(provider.clone())));
        }
    }

    #[test]
    fn modes_waits_and_stop_reasons_are_spelled_as_d5_spells_them() {
        assert_eq!(
            [Mode::Auto, Mode::Collect, Mode::Waiting, Mode::Idle].map(Mode::as_str),
            ["auto", "collect", "waiting", "idle"]
        );
        assert_eq!(
            [WaitingFor::EngineLock, WaitingFor::Schema].map(WaitingFor::as_str),
            ["engine-lock", "schema"]
        );
        assert_eq!(
            [StopReason::Signal, StopReason::Upgrade, StopReason::Schema].map(StopReason::as_str),
            ["signal", "upgrade", "schema"]
        );
    }

    #[test]
    fn every_mode_parses_from_its_own_spelling() {
        for mode in Mode::ALL {
            assert_eq!(Mode::parse(mode.as_str()), Some(mode));
        }
        assert_eq!(Mode::parse("Auto"), None, "case-sensitive");
        assert_eq!(Mode::parse("paused"), None);
    }

    #[test]
    fn every_waiting_for_parses_from_its_own_spelling() {
        for waiting_for in WaitingFor::ALL {
            assert_eq!(WaitingFor::parse(waiting_for.as_str()), Some(waiting_for));
        }
        assert_eq!(WaitingFor::parse("Schema"), None, "case-sensitive");
        assert_eq!(WaitingFor::parse("upgrade"), None);
    }

    #[test]
    fn auto_events_hands_each_engine_event_on_as_an_auto_kind_with_the_tick_s_end() {
        let log = RefCell::new(Vec::new());
        let daemon = Tagged {
            name: "daemon",
            log: &log,
        };
        let sink = AutoEvents(&daemon);
        sink.emit(&warning());
        sink.tick_done(&cc());
        assert_eq!(
            *log.borrow(),
            [
                ("daemon", Call::Emit(DaemonEvent::Auto(warning()))),
                ("daemon", Call::Done(cc())),
            ]
        );
    }

    #[test]
    fn auto_only_hands_on_the_auto_kinds_and_the_tick_s_end_and_drops_the_daemon_s_own() {
        let log = RefCell::new(Vec::new());
        let auto = Tagged {
            name: "auto",
            log: &log,
        };
        let sink = AutoOnly(&auto);
        for e in [
            DaemonEvent::Usage {
                provider: cc(),
                updated: vec![1],
                fetch_errors: BTreeMap::new(),
            },
            DaemonEvent::Auto(warning()),
            DaemonEvent::Changed,
            DaemonEvent::Mode {
                provider: cc(),
                mode: Mode::Collect,
                waiting_for: None,
                holder_pid: None,
                backoff: None,
            },
            DaemonEvent::Stopping {
                reason: StopReason::Signal,
            },
        ] {
            sink.emit(&e);
        }
        sink.tick_done(&cc());
        assert_eq!(
            *log.borrow(),
            [("auto", Call::Emit(warning())), ("auto", Call::Done(cc()))]
        );
    }

    #[test]
    fn a_tick_through_both_adapters_reaches_the_command_s_sink_unchanged() {
        // `tagteam auto` on the scheduler: the engine emits into `AutoEvents` over `AutoOnly`
        // over the command's sink, and the human sink's end-of-tick bookkeeping still runs.
        let log = RefCell::new(Vec::new());
        let command = Tagged {
            name: "command",
            log: &log,
        };
        let only = AutoOnly(&command);
        let engine_side = AutoEvents(&only);
        engine_side.emit(&warning());
        engine_side.tick_done(&cc());
        assert_eq!(
            *log.borrow(),
            [
                ("command", Call::Emit(warning())),
                ("command", Call::Done(cc()))
            ]
        );
    }

    #[test]
    fn fanout_hands_every_call_to_each_sink_in_order() {
        let log = RefCell::new(Vec::new());
        let (a, b) = (
            Tagged {
                name: "a",
                log: &log,
            },
            Tagged {
                name: "b",
                log: &log,
            },
        );
        let fanout = Fanout(vec![&a, &b]);
        fanout.emit(&DaemonEvent::Changed);
        fanout.tick_done(&cc());
        assert_eq!(
            *log.borrow(),
            [
                ("a", Call::Emit(DaemonEvent::Changed)),
                ("b", Call::Emit(DaemonEvent::Changed)),
                ("a", Call::Done(cc())),
                ("b", Call::Done(cc())),
            ]
        );
    }
}
