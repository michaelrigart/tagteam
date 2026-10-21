//! Everything on screen (spec §4.2). `update` changes it, `view` draws it.

use std::time::SystemTime;

use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::views::{ProviderAccounts, ProviderNames};

use crate::autoline;
use crate::chrome;
use crate::daemon::DaemonState;
use crate::host::{AccountRef, Action};
use crate::msg::Cmd;
use crate::overlay::Overlay;
use crate::screens::accounts::AccountsState;
use crate::screens::history::HistoryState;
use crate::screens::tokens::TokensState;
use crate::source::CollectSummary;
use crate::theme::Theme;

/// Timer mode re-reads and collects this often (spec §6.2).
pub const REFRESH_MS: i64 = 30_000;
/// A `Done` or `Info` toast replaces the footer this long (redesign §5.5).
pub const TOAST_MS: i64 = 5_000;
/// A `Warn` or `Failed` toast, and a `Done` one that wraps, last this long (redesign §5.5).
pub const TOAST_LONG_MS: i64 = 8_000;
/// After a key closes an overlay, the action keys are ignored this long (redesign §9.5, T2-18).
pub const GUARD_MS: i64 = 300;

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
    /// The input thread is dropping every key until an unfinished colour reply ends (§4.2).
    pub(crate) keys_held: bool,
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
    /// The default provider's names (§12.2), for a provider the accounts view does not list.
    pub(crate) fallback_names: ProviderNames,
    /// The collect generation a person's `r` asked for, while it runs (spec §9.7): the spinner
    /// says `checking usage` meanwhile.
    pub(crate) checking: Option<u64>,
    /// What that collection did, until the accounts read after it, which the `r` toast needs.
    pub(crate) checked: Option<CollectSummary>,
    /// The action running now was handed the terminal: its outcome is worded by the exit code
    /// alone (T2-19).
    pub(crate) on_terminal: bool,
    /// The action keys are ignored until this instant: a key closed an overlay just before
    /// (T2-18).
    pub(crate) guard_until_ms: i64,
    /// The settings warnings the CLI printed before the TUI started (redesign §5.6, T2-21).
    pub(crate) startup: Startup,
}

/// The settings warnings of start-up, and the config file's mtime as read before the settings
/// were: an accounts read that sees another mtime means the file changed, and clears them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Startup {
    pub(crate) warnings: Vec<String>,
    pub(crate) mtime: Option<SystemTime>,
}

/// A banner's words, and what raised them: a success of that same kind clears it, and a refusal
/// is cleared by any success.
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

/// What a toast reports (redesign §5.5): its mark, its role and how long it lasts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Done,
    Info,
    Warn,
    Failed,
}

pub struct Toast {
    pub kind: ToastKind,
    pub text: String,
    pub until_ms: i64,
}

#[derive(Default)]
pub struct Gens {
    pub accounts: u64,
    pub history: u64,
    /// The selected account's card's history read (T3-21).
    pub card: u64,
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
        names: ProviderNames,
        startup: Startup,
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
            keys_held: false,
            running: None,
            gens: Gens::default(),
            refresh: Refresh {
                next_ms: now_ms + REFRESH_MS,
            },
            quitting: None,
            dirty: true,
            shown_s: now_ms.div_euclid(1000),
            started_ms: now_ms,
            fallback_names: names,
            checking: None,
            checked: None,
            on_terminal: false,
            guard_until_ms: 0,
            startup,
        }
    }

    /// `provider`'s names: from the accounts view, else the default provider's
    /// (`TuiOptions.names`).
    pub(crate) fn names_of(&self, provider: &ProviderId) -> ProviderNames {
        self.view_of(provider)
            .map_or(self.fallback_names, |p| p.names)
    }

    /// `provider`'s part of the last accounts view, when it lists the provider.
    pub(crate) fn view_of(&self, provider: &ProviderId) -> Option<&ProviderAccounts> {
        self.accounts
            .snapshot()?
            .providers
            .iter()
            .find(|p| &p.provider == provider)
    }

    /// The pid of a live `tagteam auto` other than the daemon, holding `provider`'s engine lock
    /// in the last accounts view (C-16).
    pub(crate) fn foreign_auto_pid(&self, provider: &ProviderId) -> Option<u32> {
        self.accounts
            .snapshot()?
            .auto_holders
            .iter()
            .find_map(|(p, pid)| (p == provider).then_some(*pid))
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

    /// A read of `account`'s history since `since_s` for the selected account's card, under the
    /// card's next generation (T3-21).
    pub(crate) fn load_card(&mut self, account: AccountId, since_s: i64) -> Cmd {
        self.gens.card += 1;
        Cmd::LoadCard {
            r#gen: self.gens.card,
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
    /// owns (spec §10.4). Never while quitting.
    pub(crate) fn clear_banner(&mut self, kind: BannerKind) {
        let owned = self
            .banner
            .as_ref()
            .is_some_and(|b| b.kind == kind || b.kind == BannerKind::Refusal);
        if self.quitting.is_none() && owned {
            self.banner = None;
        }
    }

    #[cfg(test)]
    pub(crate) fn banner_text(&self) -> Option<&str> {
        self.banner.as_ref().map(|b| b.text.as_str())
    }

    /// Shows `text` in place of the footer's left side: `Done` for 5 s (8 s when it takes more
    /// than one row at `self.size`'s width, as the footer lays it out), `Info` for 5 s, `Warn`
    /// and `Failed` for 8 s (redesign §5.5).
    pub(crate) fn toast(&mut self, kind: ToastKind, text: impl Into<String>, now_ms: i64) {
        let text = text.into();
        let long = match kind {
            ToastKind::Done => {
                chrome::toast_rows(self, kind, &text, self.size.0)
                    .rows
                    .len()
                    > 1
            }
            ToastKind::Info => false,
            ToastKind::Warn | ToastKind::Failed => true,
        };
        let lasts = if long { TOAST_LONG_MS } else { TOAST_MS };
        self.toast = Some(Toast {
            kind,
            text,
            until_ms: now_ms + lasts,
        });
        self.dirty = true;
    }

    /// `account` as the person knows it: `words::short_name` from the last accounts view, else
    /// its position alone. The account's ID finds it first, so a re-read that moved it (a move,
    /// a remove, `R`) under a running action still names it; the position is the fallback.
    pub(crate) fn short_name(&self, account: &AccountRef) -> String {
        let view = self.view_of(&account.provider);
        let position = view
            .and_then(|v| v.accounts.iter().find(|a| a.row.id == account.id))
            .map_or(account.position, |a| a.row.position);
        autoline::named(view, position)
    }

    /// The words the spinner and the busy toast give `action` (redesign §9.4), without the `…`.
    pub(crate) fn label_of(&self, action: &Action) -> String {
        let provider = match action {
            Action::Switch(a)
            | Action::Enable(a)
            | Action::Disable(a)
            | Action::ClearAlias(a)
            | Action::Remove(a)
            | Action::Run(a)
            | Action::SetAlias { account: a, .. }
            | Action::Move { account: a, .. } => Some(&a.provider),
            Action::SwitchBy { provider, .. }
            | Action::AddLive { provider }
            | Action::Autoswitch { provider, .. } => Some(provider),
            Action::Doctor { provider, .. } => provider.as_ref(),
        };
        let names = provider.map_or(self.fallback_names, |p| self.names_of(p));
        action.label(&|a| self.short_name(a), names.display_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil;

    #[test]
    fn a_short_name_follows_the_account_when_a_re_read_moves_its_position() {
        let mut state = testutil::fresh(Screen::Accounts);
        state.accounts.apply(testutil::snapshot(testutil::many(2)));
        let first = AccountRef {
            id: testutil::id(1),
            provider: testutil::cc(),
            position: 1,
        };
        assert_eq!(state.short_name(&first), "1 a1");
        // The re-read while an action runs: a1 is now second, and a2 is first.
        let mut moved = testutil::many(2);
        moved[0].row.position = 2;
        moved[1].row.position = 1;
        moved.reverse();
        state.accounts.apply(testutil::snapshot(moved));
        assert_eq!(state.short_name(&first), "2 a1");
        // Gone from the view: the position it had is all there is to go on.
        let mut removed = testutil::many(2);
        removed.remove(0);
        removed[0].row.position = 1;
        state.accounts.apply(testutil::snapshot(removed));
        let gone = AccountRef {
            id: testutil::id(9),
            provider: testutil::cc(),
            position: 7,
        };
        assert_eq!(state.short_name(&gone), "7");
    }

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

    #[test]
    fn each_toast_kind_lasts_as_long_as_it_says() {
        // Redesign §5.5: Done 5 s, and 8 s when it wraps; Info 5 s; Warn and Failed 8 s.
        let short = "Switched";
        let wraps = "word ".repeat(20);
        let lasts = |kind: ToastKind, text: &str| {
            let mut state = testutil::fresh(Screen::Accounts);
            assert_eq!(state.size, (80, 24));
            state.dirty = false;
            state.toast(kind, text, 1_000);
            assert!(state.dirty);
            let toast = state.toast.unwrap();
            assert_eq!((toast.kind, toast.text.as_str()), (kind, text));
            toast.until_ms - 1_000
        };
        assert_eq!(lasts(ToastKind::Done, short), TOAST_MS);
        assert_eq!(lasts(ToastKind::Done, &wraps), TOAST_LONG_MS);
        assert_eq!(lasts(ToastKind::Info, short), TOAST_MS);
        assert_eq!(
            lasts(ToastKind::Info, &wraps),
            TOAST_MS,
            "Info never takes longer"
        );
        assert_eq!(lasts(ToastKind::Warn, short), TOAST_LONG_MS);
        assert_eq!(lasts(ToastKind::Failed, short), TOAST_LONG_MS);
        assert_eq!((TOAST_MS, TOAST_LONG_MS), (5_000, 8_000));
    }
}
