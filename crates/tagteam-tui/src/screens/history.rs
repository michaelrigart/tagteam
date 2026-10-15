//! The History screen (spec §5.4). Task 7 draws it; this holds the account it shows.

use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::Rect;
use tagteam_core::AccountId;

use crate::msg::Cmd;
use crate::state::State;

#[derive(Default)]
pub struct HistoryState {
    account: Option<AccountId>,
}

impl HistoryState {
    pub fn account(&self) -> Option<&AccountId> {
        self.account.as_ref()
    }

    /// Shows `account`'s history.
    pub fn select(&mut self, account: AccountId) {
        self.account = Some(account);
    }
}

/// What History reads as it opens; Task 7 reads the account's history.
pub(crate) fn on_enter(_state: &mut State, _now_ms: i64) -> Vec<Cmd> {
    vec![]
}

pub fn handle_key(_state: &mut State, _key: KeyEvent, _now_ms: i64) -> Vec<Cmd> {
    vec![]
}

pub fn render(_state: &State, _frame: &mut Frame, _area: Rect, _now_ms: i64) {}
