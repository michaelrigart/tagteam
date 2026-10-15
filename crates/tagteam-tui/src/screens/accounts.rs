//! The Accounts screen (spec §5.2). Task 6 draws it; this holds the view it will draw.

use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::Rect;

use crate::msg::Cmd;
use crate::source::AccountsSnapshot;
use crate::state::State;

#[derive(Default)]
pub struct AccountsState {
    snapshot: Option<AccountsSnapshot>,
}

impl AccountsState {
    pub fn snapshot(&self) -> Option<&AccountsSnapshot> {
        self.snapshot.as_ref()
    }

    pub fn apply(&mut self, snap: AccountsSnapshot) {
        self.snapshot = Some(snap);
    }
}

/// What the accounts view has to say above the body; Task 6 raises its banners.
pub fn banner(_state: &AccountsState) -> Option<String> {
    None
}

pub fn handle_key(_state: &mut State, _key: KeyEvent, _now_ms: i64) -> Vec<Cmd> {
    vec![]
}

pub fn render(_state: &State, _frame: &mut Frame, _area: Rect, _now_ms: i64) {}
