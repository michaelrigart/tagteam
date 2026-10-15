//! Everything on screen (spec §4.2). `update` changes it, `view` draws it.

use tagteam_core::{AccountId, ProviderId};

use crate::actions::Overlay;
use crate::host::Action;
use crate::msg::Cmd;
use crate::screens::accounts::AccountsState;
use crate::screens::history::HistoryState;

/// Timer mode re-reads and collects this often (spec §6.2).
pub const REFRESH_MS: i64 = 30_000;
/// A toast replaces the footer this long (spec §5.1).
pub const TOAST_MS: i64 = 5_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Accounts,
    History,
} // part B inserts Tokens between them

impl Screen {
    /// `Tab`'s order.
    pub(crate) fn next(self) -> Screen {
        match self {
            Screen::Accounts => Screen::History,
            Screen::History => Screen::Accounts,
        }
    }

    /// `Shift-Tab`'s order.
    pub(crate) fn prev(self) -> Screen {
        match self {
            Screen::Accounts => Screen::History,
            Screen::History => Screen::Accounts,
        }
    }
}

pub struct State {
    pub screen: Screen,
    pub size: (u16, u16),
    pub color: bool,
    pub provider: Option<ProviderId>,
    pub accounts: AccountsState,
    pub history: HistoryState,
    pub overlay: Option<Overlay>,
    pub toast: Option<Toast>,
    /// A failed read's error, in the CLI's words (spec §10.4).
    pub banner: Option<String>,
    /// Which read raised `banner`: only a success of that same kind clears it.
    pub(crate) banner_from: Option<BannerKind>,
    /// The action running now, in the TUI or suspended (spec §7.2).
    pub running: Option<Action>,
    pub gens: Gens,
    pub refresh: Refresh,
    /// Set once the shutdown has begun: the exit code it will end with.
    pub quitting: Option<i32>,
    /// Whether the next loop iteration must redraw.
    pub dirty: bool,
    /// The second the screen was last drawn for: its countdowns change at most once a second
    /// (spec §5.2, "Countdowns tick").
    pub(crate) shown_s: i64,
}

/// What raised the banner (spec §10.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BannerKind {
    Accounts,
    History,
    Collect,
    Lane,
}

pub struct Toast {
    pub text: String,
    pub until_ms: i64,
}

#[derive(Default)]
pub struct Gens {
    pub accounts: u64,
    pub history: u64,
    pub collect: u64,
}

pub struct Refresh {
    pub next_ms: i64,
}

impl State {
    pub fn new(screen: Screen, color: bool, provider: Option<ProviderId>, now_ms: i64) -> State {
        State {
            screen,
            size: (0, 0),
            color,
            provider,
            accounts: AccountsState::default(),
            history: HistoryState::default(),
            overlay: None,
            toast: None,
            banner: None,
            banner_from: None,
            running: None,
            gens: Gens::default(),
            refresh: Refresh {
                next_ms: now_ms + REFRESH_MS,
            },
            quitting: None,
            dirty: true,
            shown_s: now_ms.div_euclid(1000),
        }
    }

    /// The first `Cmd`s: load `accounts` (gen 1). History needs an account, which only the
    /// first `accounts` result names, so a TUI started on History loads it once that arrives.
    pub fn start(&mut self) -> Vec<Cmd> {
        vec![self.load_accounts()]
    }

    /// A read of `accounts` under the next generation: any read still in flight is stale.
    pub(crate) fn load_accounts(&mut self) -> Cmd {
        self.gens.accounts += 1;
        Cmd::LoadAccounts {
            r#gen: self.gens.accounts,
        }
    }

    /// A read of `account`'s history since `since_s`, under the next generation.
    pub(crate) fn load_history(&mut self, account: AccountId, since_s: i64) -> Cmd {
        self.gens.history += 1;
        Cmd::LoadHistory {
            r#gen: self.gens.history,
            account,
            since_s,
        }
    }

    /// A collection of `accounts` (spec §6.2), under the next generation.
    pub(crate) fn collect_due(&mut self, accounts: Vec<AccountId>) -> Cmd {
        self.gens.collect += 1;
        Cmd::CollectDue {
            r#gen: self.gens.collect,
            accounts,
        }
    }

    /// Shows `text` in place of the footer for `TOAST_MS`.
    pub(crate) fn toast(&mut self, text: impl Into<String>, now_ms: i64) {
        self.toast = Some(Toast {
            text: text.into(),
            until_ms: now_ms + TOAST_MS,
        });
        self.dirty = true;
    }
}
