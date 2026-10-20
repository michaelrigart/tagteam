//! D5.4: the daemon's status, as a snapshot its jobs keep current (Decision 12). The
//! scheduler's jobs write it after each run; plan B's server reads it and fills in
//! `subscribers` and `socket`. Reading it never touches the store and never waits for a job.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tagteam_core::ProviderId;
use tagteam_engine::settings::NotifyEvent;

use crate::events::{Backoff, Mode, WaitingFor};

/// Everything `status` reports (D5.4), as the last job to run left it.
#[derive(Debug, Clone, PartialEq)]
pub struct StatusSnapshot {
    pub daemon: DaemonInfo,
    pub notifications: NotifyInfo,
    /// One entry per provider job, in provider order.
    pub providers: Vec<ProviderStatus>,
    /// Every job other than the provider jobs, in the order each first reported.
    pub jobs: Vec<JobStatus>,
    pub subscribers: usize,
}

/// The daemon itself. Times are epoch seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct DaemonInfo {
    pub pid: u32,
    pub version: String,
    pub started_at: i64,
    /// The socket it serves; `None` before one is bound, or when none could be (D5.1).
    pub socket: Option<PathBuf>,
    pub log: PathBuf,
}

/// D6: whether notifications are on, through which backend, and for which events.
#[derive(Debug, Clone, PartialEq)]
pub struct NotifyInfo {
    pub enabled: bool,
    /// `osascript`, `notify-send`, or `None` without a backend.
    pub backend: Option<&'static str>,
    pub events: Vec<NotifyEvent>,
}

/// One provider job's line (D4.3, D5.4). Times are epoch seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderStatus {
    pub provider: ProviderId,
    pub mode: Mode,
    /// The effective `daemon.autoswitch` (D7).
    pub autoswitch: bool,
    pub waiting_for: Option<WaitingFor>,
    /// The other engine's pid, while waiting for its lock and its record names it.
    pub holder_pid: Option<u32>,
    /// The keychain backoff (D4.1), while it holds.
    pub backoff: Option<Backoff>,
    /// When the last tick or `collect` round began, and when the next is due.
    pub last_tick_at: Option<i64>,
    pub next_tick_at: Option<i64>,
    pub last_outcome: Option<LastOutcome>,
    /// The last switch this daemon made.
    pub last_switch: Option<LastSwitch>,
}

/// What a provider job's last run came to (D5.4 `lastOutcome.kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeKind {
    Switch,
    NoSwitch,
    Collect,
    Error,
}

impl OutcomeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            OutcomeKind::Switch => "switch",
            OutcomeKind::NoSwitch => "no-switch",
            OutcomeKind::Collect => "collect",
            OutcomeKind::Error => "error",
        }
    }

    pub const ALL: [Self; 4] = [
        OutcomeKind::Switch,
        OutcomeKind::NoSwitch,
        OutcomeKind::Collect,
        OutcomeKind::Error,
    ];

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.as_str() == s)
    }
}

/// A run's outcome. `reason` is the kebab-case reason of a `no-switch`, or the message of an
/// `error`; `None` otherwise.
#[derive(Debug, Clone, PartialEq)]
pub struct LastOutcome {
    pub kind: OutcomeKind,
    pub reason: Option<String>,
    pub at: i64,
}

/// A switch this daemon made: positions, and its trigger's spelling (§11.4).
#[derive(Debug, Clone, PartialEq)]
pub struct LastSwitch {
    pub at: i64,
    pub from: u32,
    pub to: u32,
    pub trigger: String,
}

/// A job other than a provider job (D5.4 `jobs`). Times are epoch seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct JobStatus {
    pub name: String,
    pub last_run_at: Option<i64>,
    pub next_run_at: Option<i64>,
    pub last_error: Option<String>,
}

/// Decision 12: the snapshot, behind a lock no job holds across anything slow.
pub struct StatusBoard {
    snap: Mutex<StatusSnapshot>,
}

impl StatusBoard {
    pub fn new(daemon: DaemonInfo, notifications: NotifyInfo) -> Arc<Self> {
        Arc::new(StatusBoard {
            snap: Mutex::new(StatusSnapshot {
                daemon,
                notifications,
                providers: Vec::new(),
                jobs: Vec::new(),
                subscribers: 0,
            }),
        })
    }

    fn lock(&self) -> MutexGuard<'_, StatusSnapshot> {
        self.snap.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn snapshot(&self) -> StatusSnapshot {
        self.lock().clone()
    }

    /// Replaces the provider's line, or adds it in provider order.
    pub fn set_provider(&self, s: ProviderStatus) {
        let mut snap = self.lock();
        match snap
            .providers
            .binary_search_by(|p| p.provider.cmp(&s.provider))
        {
            Ok(i) => snap.providers[i] = s,
            Err(i) => snap.providers.insert(i, s),
        }
    }

    /// Replaces the job's line by its name, or adds it last.
    pub fn set_job(&self, s: JobStatus) {
        let mut snap = self.lock();
        match snap.jobs.iter_mut().find(|j| j.name == s.name) {
            Some(j) => *j = s,
            None => snap.jobs.push(s),
        }
    }

    pub fn set_subscribers(&self, n: usize) {
        self.lock().subscribers = n;
    }

    pub fn set_socket(&self, path: Option<PathBuf>) {
        self.lock().daemon.socket = path;
    }

    pub fn set_notifications(&self, n: NotifyInfo) {
        self.lock().notifications = n;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn daemon() -> DaemonInfo {
        DaemonInfo {
            pid: 4242,
            version: "20261014.2045.0".into(),
            started_at: 1_790_000_000,
            socket: None,
            log: PathBuf::from("/home/u/.local/state/tagteam/tagteam.log"),
        }
    }

    fn notify() -> NotifyInfo {
        NotifyInfo {
            enabled: true,
            backend: Some("osascript"),
            events: vec![
                NotifyEvent::Switch,
                NotifyEvent::Quarantine,
                NotifyEvent::Limit,
            ],
        }
    }

    fn provider(id: &str, mode: Mode) -> ProviderStatus {
        ProviderStatus {
            provider: ProviderId::new(id),
            mode,
            autoswitch: true,
            waiting_for: None,
            holder_pid: None,
            backoff: None,
            last_tick_at: None,
            next_tick_at: None,
            last_outcome: None,
            last_switch: None,
        }
    }

    fn job(name: &str, at: i64) -> JobStatus {
        JobStatus {
            name: name.into(),
            last_run_at: Some(at),
            next_run_at: Some(at + 2),
            last_error: None,
        }
    }

    #[test]
    fn every_outcome_kind_parses_from_its_own_spelling() {
        for kind in OutcomeKind::ALL {
            assert_eq!(OutcomeKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(OutcomeKind::parse("No-Switch"), None, "case-sensitive");
        assert_eq!(OutcomeKind::parse("noswitch"), None);
    }

    #[test]
    fn a_new_board_holds_the_daemon_and_its_notifications_and_nothing_else() {
        let board = StatusBoard::new(daemon(), notify());
        assert_eq!(
            board.snapshot(),
            StatusSnapshot {
                daemon: daemon(),
                notifications: notify(),
                providers: Vec::new(),
                jobs: Vec::new(),
                subscribers: 0,
            }
        );
    }

    #[test]
    fn providers_stay_in_provider_order_and_jobs_in_the_order_they_first_reported() {
        // D5.4: `providers` lists the provider jobs, `jobs` every other job. A job's line is
        // replaced in place, whichever order the jobs ran in.
        let board = StatusBoard::new(daemon(), notify());
        board.set_provider(provider("fake-agent", Mode::Collect));
        board.set_provider(provider("claude-code", Mode::Idle));
        board.set_provider(provider("claude-code", Mode::Auto));
        board.set_job(job("upgrade-watch", 10));
        board.set_job(job("store-watch", 10));
        board.set_job(job("upgrade-watch", 70));
        let snap = board.snapshot();
        let providers: Vec<(&str, Mode)> = snap
            .providers
            .iter()
            .map(|p| (p.provider.as_str(), p.mode))
            .collect();
        assert_eq!(
            providers,
            [("claude-code", Mode::Auto), ("fake-agent", Mode::Collect)]
        );
        assert_eq!(
            snap.jobs,
            [job("upgrade-watch", 70), job("store-watch", 10)]
        );
    }

    #[test]
    fn the_server_s_fields_are_kept_beside_the_jobs_lines() {
        // Decision 12: plan B's server sets the subscribers and the socket; the notifier's
        // state may change. None of it touches what the jobs wrote.
        let board = StatusBoard::new(daemon(), notify());
        board.set_provider(provider("claude-code", Mode::Auto));
        let socket = PathBuf::from("/home/u/.local/share/tagteam/daemon.sock");
        board.set_subscribers(3);
        board.set_socket(Some(socket.clone()));
        let off = NotifyInfo {
            enabled: false,
            backend: None,
            events: Vec::new(),
        };
        board.set_notifications(off.clone());
        let snap = board.snapshot();
        assert_eq!(snap.subscribers, 3);
        assert_eq!(snap.daemon.socket, Some(socket));
        assert_eq!(snap.notifications, off);
        assert_eq!(snap.providers, [provider("claude-code", Mode::Auto)]);
    }

    #[test]
    fn outcome_kinds_are_spelled_as_status_spells_them() {
        let kinds = [
            OutcomeKind::Switch,
            OutcomeKind::NoSwitch,
            OutcomeKind::Collect,
            OutcomeKind::Error,
        ]
        .map(OutcomeKind::as_str);
        assert_eq!(kinds, ["switch", "no-switch", "collect", "error"]);
    }
}
