//! What reaches the state machine (`Msg`), what it asks for (`Cmd`), and the lanes' requests
//! (spec §4.2, §4.3).

use std::sync::mpsc::Sender;

use tagteam_core::AccountId;
use tagteam_engine::views::HistoryView;

use crate::host::{Action, ActionOutcome};
use crate::lanes::Coalesce;
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
    Prompt(Prompt),
    ActionDone {
        action: Action,
        outcome: ActionOutcome,
    },
    LaneDied {
        lane: &'static str,
    },
}

/// A question from the CLI path, and where its answer goes (spec §7.2).
pub struct Prompt {
    pub question: String,
    pub kind: PromptKind,
    pub reply: Sender<Answer>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
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
    Act(Action),
    /// Run `action` with the terminal handed back (spec §7.3).
    Suspend(Action),
    /// Begin the shutdown sequence (spec §10.2) with this exit code.
    Quit {
        code: i32,
    },
    /// Ctrl-Z (spec §10.1).
    Stop,
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
}
