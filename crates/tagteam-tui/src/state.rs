//! Everything on screen (spec §4.2). `update` changes it, `view` draws it.

use tagteam_core::{AccountId, ProviderId};

use crate::actions::Overlay;
use crate::daemon::DaemonState;
use crate::host::Action;
use crate::msg::Cmd;
use crate::screens::accounts::AccountsState;
use crate::screens::history::HistoryState;
use crate::screens::tokens::TokensState;
use crate::theme::Theme;

/// Timer mode re-reads and collects this often (spec §6.2).
pub const REFRESH_MS: i64 = 30_000;
/// A toast replaces the footer this long (spec §5.1).
pub const TOAST_MS: i64 = 5_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Accounts,
    Tokens,
    History,
}

impl Screen {
    /// `Tab`'s order: the screens' numbers (spec §5.6).
    pub(crate) fn next(self) -> Screen {
        match self {
            Screen::Accounts => Screen::Tokens,
            Screen::Tokens => Screen::History,
            Screen::History => Screen::Accounts,
        }
    }

    /// `Shift-Tab`'s order.
    pub(crate) fn prev(self) -> Screen {
        match self {
            Screen::Accounts => Screen::History,
            Screen::Tokens => Screen::Accounts,
            Screen::History => Screen::Tokens,
        }
    }
}

pub struct State {
    pub screen: Screen,
    pub size: (u16, u16),
    pub theme: Theme,
    pub provider: Option<ProviderId>,
    pub accounts: AccountsState,
    pub tokens: TokensState,
    pub history: HistoryState,
    /// The daemon's link, and what it says (spec §8).
    pub daemon: DaemonState,
    pub overlay: Option<Overlay>,
    pub toast: Option<Toast>,
    /// A failed read's error, in the CLI's words (spec §10.4).
    pub(crate) banner: Option<Banner>,
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
    /// When the state was made: the time the first reads are asked for at (`start`).
    pub(crate) started_ms: i64,
}

/// A banner's words, and what raised them: only a success of that same kind clears it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Banner {
    pub(crate) text: String,
    pub(crate) kind: BannerKind,
}

/// What raised the banner (spec §10.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BannerKind {
    Accounts,
    History,
    Collect,
    Lane,
    /// A hand-over the input thread did not allow (spec §7.3): any successful read clears it.
    Refusal,
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
    pub report: u64,
    pub index: u64,
    pub inventory: u64,
}

pub struct Refresh {
    pub next_ms: i64,
}

impl State {
    pub fn new(
        screen: Screen,
        theme: Theme,
        provider: Option<ProviderId>,
        tokens: TokensState,
        now_ms: i64,
    ) -> State {
        State {
            screen,
            size: (0, 0),
            theme,
            provider,
            accounts: AccountsState::default(),
            tokens,
            history: HistoryState::default(),
            daemon: DaemonState::default(),
            overlay: None,
            toast: None,
            banner: None,
            running: None,
            gens: Gens::default(),
            refresh: Refresh {
                next_ms: now_ms + REFRESH_MS,
            },
            quitting: None,
            dirty: true,
            shown_s: now_ms.div_euclid(1000),
            started_ms: now_ms,
        }
    }

    /// The first `Cmd`s: load `accounts` (gen 1), and on Tokens its report. History needs an
    /// account, which only the first `accounts` result names, so a TUI started on History loads
    /// it once that arrives.
    pub fn start(&mut self) -> Vec<Cmd> {
        let mut cmds = vec![self.load_accounts()];
        if self.screen == Screen::Tokens {
            cmds.extend(crate::screens::tokens::on_enter(self, self.started_ms));
        }
        cmds
    }

    /// The views re-read: `accounts`, and on History the account shown, each under a new
    /// generation.
    pub(crate) fn reload_views(&mut self, now_ms: i64) -> Vec<Cmd> {
        let mut cmds = vec![self.load_accounts()];
        if self.screen == Screen::History {
            cmds.extend(crate::screens::history::reload(self, now_ms));
        }
        cmds
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

    /// A failed read's banner, remembering which kind raised it.
    pub(crate) fn raise_banner(&mut self, kind: BannerKind, text: impl Into<String>) {
        self.banner = Some(Banner {
            text: text.into(),
            kind,
        });
    }

    /// A read of `kind` succeeded: clears a banner of that kind, or a refusal, which no read
    /// owns. Never while quitting.
    pub(crate) fn clear_banner(&mut self, kind: BannerKind) {
        let owned = self
            .banner
            .as_ref()
            .is_some_and(|b| b.kind == kind || b.kind == BannerKind::Refusal);
        if self.quitting.is_none() && owned {
            self.banner = None;
        }
    }

    pub(crate) fn banner_text(&self) -> Option<&str> {
        self.banner.as_ref().map(|b| b.text.as_str())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil;

    #[test]
    fn a_banner_clears_for_its_own_kind_or_a_refusal_and_never_while_quitting() {
        let mut state = testutil::fresh(Screen::Accounts);

        state.raise_banner(BannerKind::Collect, "usage was not collected");
        state.clear_banner(BannerKind::Accounts);
        assert_eq!(state.banner_text(), Some("usage was not collected"));
        state.clear_banner(BannerKind::Collect);
        assert_eq!(state.banner_text(), None);

        state.raise_banner(BannerKind::Refusal, "no");
        state.clear_banner(BannerKind::History);
        assert!(state.banner.is_none());

        state.raise_banner(BannerKind::Accounts, "locked");
        state.quitting = Some(0);
        state.clear_banner(BannerKind::Accounts);
        assert_eq!(state.banner_text(), Some("locked"));
    }
}
