//! What reaches the state machine (`Msg`), what it asks for (`Cmd`), and the lanes' requests
//! (spec §4.2, §4.3).

use tagteam_core::{AccountId, ProviderId};
use tagteam_daemon::protocol::Status;
use tagteam_engine::tokens::{IndexProgress, IndexState, RefreshOutcome, TokenQuery, TokenReport};
use tagteam_engine::views::HistoryView;

use crate::host::{Action, ActionOutcome};
use crate::lanes::Coalesce;
use crate::live::{Live, StatusError};
use crate::source::{AccountsSnapshot, CollectSummary, SourceError};

/// The views lane's requests: store-served reads, no network (spec §4.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewsReq {
    Accounts {
        r#gen: u64,
    },
    History {
        r#gen: u64,
        account: AccountId,
        since_s: i64,
    },
}

/// The actions lane's requests: the collector and every `Host` action, one at a time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionsReq {
    Collect {
        r#gen: u64,
        accounts: Vec<AccountId>,
    },
    Act {
        action: Action,
    },
}

/// The report lane's requests: the token report, which reads the index only, and the index's
/// state (spec §4.3, §6.4).
#[derive(Debug, Clone, PartialEq)]
pub enum ReportReq {
    Report {
        r#gen: u64,
        query: TokenQuery,
    },
    IndexState {
        r#gen: u64,
        provider: ProviderId,
    },
    /// The filter's inventory: an unfiltered report (spec §5.3).
    Inventory {
        r#gen: u64,
        query: TokenQuery,
    },
}

/// The index lane's requests: the token index's catch-up, which can run for minutes on a cold
/// index (spec §4.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexReq {
    Refresh { provider: ProviderId },
}

/// The daemon lane's requests: the daemon's `status`, over a connection of its own (spec §4.3,
/// §8). `link` is the link it is asked for (C-R21).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonReq {
    Status { link: u64 },
}

impl Coalesce for ViewsReq {
    /// Any two of the same kind: only the newest read is worth making.
    fn same_as(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (ViewsReq::Accounts { .. }, ViewsReq::Accounts { .. })
                | (ViewsReq::History { .. }, ViewsReq::History { .. })
        )
    }
}

impl Coalesce for ReportReq {
    /// Any two of the same kind: only the newest query is worth making.
    fn same_as(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (ReportReq::Report { .. }, ReportReq::Report { .. })
                | (ReportReq::IndexState { .. }, ReportReq::IndexState { .. })
                | (ReportReq::Inventory { .. }, ReportReq::Inventory { .. })
        )
    }
}

impl Coalesce for IndexReq {
    /// A refresh queued behind the running one is one refresh.
    fn same_as(&self, other: &Self) -> bool {
        self == other
    }
}

impl Coalesce for DaemonReq {
    /// Any two: a burst of events costs one request, and the newest link's is the one made
    /// (spec §8).
    fn same_as(&self, _other: &Self) -> bool {
        true
    }
}

impl Coalesce for ActionsReq {
    /// Two collections merge; an action never does.
    fn same_as(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (ActionsReq::Collect { .. }, ActionsReq::Collect { .. })
        )
    }
}

/// One input to `update`. The data variants are large, as the engine's views are, and a
/// message is moved once, so they are not boxed.
#[allow(clippy::large_enum_variant)]
pub enum Msg {
    Key(crossterm::event::KeyEvent),
    Resize {
        width: u16,
        height: u16,
    },
    Tick,
    Accounts {
        r#gen: u64,
        result: Result<AccountsSnapshot, SourceError>,
    },
    History {
        r#gen: u64,
        result: Result<HistoryView, SourceError>,
    },
    Collected {
        r#gen: u64,
        result: Result<CollectSummary, SourceError>,
    },
    TokenReport {
        r#gen: u64,
        result: Result<TokenReport, SourceError>,
    },
    IndexState {
        r#gen: u64,
        result: Result<IndexState, SourceError>,
    },
    /// How far the TUI's refresh of the index has come, at most every 250 ms.
    IndexProgress(IndexProgress),
    /// The TUI's refresh of the index returned.
    Refreshed {
        result: Result<RefreshOutcome, SourceError>,
    },
    /// The filter's inventory (spec §5.3).
    Inventory {
        r#gen: u64,
        result: Result<TokenReport, SourceError>,
    },
    Prompt(Prompt),
    ActionDone {
        action: Action,
        outcome: ActionOutcome,
    },
    LaneDied {
        lane: &'static str,
    },
    /// What the live thread learned (spec §8). The runtime makes these from the live queue.
    Live(Live),
    /// The live queue has something: the runtime takes it all, and applies each as `Live`
    /// (C-R17).
    LiveReady,
    /// The daemon's `status`, from the daemon lane, for link `link` (spec §8, C-R21).
    DaemonStatus {
        link: u64,
        result: Result<Status, StatusError>,
    },
    /// The runtime began the shutdown (spec §10.2 step 1); it exits with `code`.
    ShutdownBegan {
        code: i32,
    },
    /// The input thread did not pause, so the terminal was not handed over (§7.3): `action` did
    /// not start, or, for `None`, Ctrl-Z did not suspend.
    SuspendRefused {
        action: Option<Action>,
        why: &'static str,
    },
}

/// Which waiting CLI prompt an answer is for (spec §7.2): the runtime holds the prompt's sender
/// and `State` holds only this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnswerTo(pub u64);

/// A question from the CLI path, and where its answer goes (spec §7.2).
pub struct Prompt {
    pub question: String,
    pub kind: PromptKind,
    pub to: AnswerTo,
}

pub enum PromptKind {
    Confirm { default_yes: bool },
    Choose { options: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    Confirm(bool),
    Choose(Option<usize>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Cmd {
    LoadAccounts {
        r#gen: u64,
    },
    LoadHistory {
        r#gen: u64,
        account: AccountId,
        since_s: i64,
    },
    CollectDue {
        r#gen: u64,
        accounts: Vec<AccountId>,
    },
    /// The token report `query` asks for, on the report lane (spec §4.3).
    LoadReport {
        r#gen: u64,
        query: TokenQuery,
    },
    /// The index's state, on the report lane (spec §6.4).
    LoadIndexState {
        r#gen: u64,
        provider: ProviderId,
    },
    /// A refresh of the index, on the index lane (spec §6.4).
    RefreshIndex {
        provider: ProviderId,
    },
    /// The filter's inventory, on the report lane (spec §5.3).
    LoadInventory {
        r#gen: u64,
        query: TokenQuery,
    },
    Act(Action),
    /// Run `action` with the terminal handed back (spec §7.3).
    Suspend(Action),
    /// Begin the shutdown sequence (spec §10.2) with this exit code.
    Quit {
        code: i32,
    },
    /// Ctrl-Z (spec §10.1).
    Stop,
    /// The daemon's `status` for link `link`, on the daemon lane (spec §8, C-R21).
    FetchStatus {
        link: u64,
    },
    /// End link `link`, which no longer works, if the live thread still holds it; it connects
    /// again (C-R19, C-R21).
    DropLink {
        link: u64,
    },
    /// Answers the CLI prompt `to` names (spec §7.2); the runtime holds its sender.
    Answer {
        to: AnswerTo,
        answer: Answer,
    },
}

#[cfg(test)]
mod tests {
    use tagteam_core::ProviderId;

    use super::*;

    fn id(s: &str) -> AccountId {
        AccountId::from_string(s)
    }

    #[test]
    fn views_requests_of_one_kind_coalesce_whatever_they_ask() {
        let accounts = |g| ViewsReq::Accounts { r#gen: g };
        let history = |g, a: &str| ViewsReq::History {
            r#gen: g,
            account: id(a),
            since_s: 0,
        };
        assert!(accounts(1).same_as(&accounts(2)));
        assert!(history(1, "a").same_as(&history(2, "b")));
        assert!(!accounts(1).same_as(&history(1, "a")));
        assert!(!history(1, "a").same_as(&accounts(1)));
    }

    #[test]
    fn report_queries_coalesce_whatever_they_ask() {
        let query = |days: i64| TokenQuery {
            provider: ProviderId::new("claude-code"),
            period: tagteam_engine::tokens::TokenPeriod::Day,
            grouping: tagteam_engine::tokens::TokenGrouping::Account,
            since_ms: Some(-days),
            until_ms: None,
            accounts: None,
            models: None,
            session: None,
            tz: jiff::tz::TimeZone::UTC,
        };
        let report = |g, days| ReportReq::Report {
            r#gen: g,
            query: query(days),
        };
        assert!(report(1, 7).same_as(&report(2, 30)));
        let state = |g| ReportReq::IndexState {
            r#gen: g,
            provider: ProviderId::new("claude-code"),
        };
        assert!(state(1).same_as(&state(2)));
        assert!(!state(1).same_as(&report(1, 7)));
        let inventory = |g| ReportReq::Inventory {
            r#gen: g,
            query: query(0),
        };
        assert!(inventory(1).same_as(&inventory(2)));
        assert!(!inventory(1).same_as(&report(1, 0)));
        let refresh = |p: &str| IndexReq::Refresh {
            provider: ProviderId::new(p),
        };
        assert!(refresh("claude-code").same_as(&refresh("claude-code")));
    }

    #[test]
    fn collections_coalesce_and_actions_never_do() {
        let collect = |g| ActionsReq::Collect {
            r#gen: g,
            accounts: vec![id("a")],
        };
        let act = || ActionsReq::Act {
            action: Action::AddLive {
                provider: ProviderId::new("claude-code"),
            },
        };
        assert!(collect(1).same_as(&collect(2)));
        assert!(!act().same_as(&act()));
        assert!(!collect(1).same_as(&act()));
        assert!(!act().same_as(&collect(1)));
    }

    #[test]
    fn status_requests_always_coalesce() {
        // Spec §8: a burst of events costs one request; the lane keeps the newest link's.
        let status = |link| DaemonReq::Status { link };
        assert!(status(1).same_as(&status(1)));
        assert!(status(1).same_as(&status(2)));
    }
}
