use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use super::{Overlay, boxed};
use crate::msg::Cmd;
use crate::state::{Screen, State};
use crate::view::pad;

/// The help panel's keys (spec §5.6): everywhere, then the current screen's.
const HELP_GLOBAL: [(&str, &str); 8] = [
    ("1 2 3  Tab Shift-Tab", "screens"),
    ("j k  ↑ ↓", "move"),
    ("A", "add the current login"),
    ("D", "doctor"),
    ("P", "pause or resume auto-switch"),
    ("?", "help"),
    ("q  Ctrl-C", "quit"),
    ("Ctrl-Z", "suspend"),
];
const HELP_ACCOUNTS: [(&str, &str); 11] = [
    ("s", "switch to the selected account"),
    ("b", "switch to the best account"),
    ("n", "switch to the next available account"),
    ("e", "enable or disable"),
    ("a", "set or clear the alias"),
    ("m", "move to another position"),
    ("x", "remove"),
    ("R", "run a session on the account"),
    ("r", "refresh"),
    ("Enter", "history of the account"),
    ("J K", "scroll the detail's windows"),
];
const HELP_TOKENS: [(&str, &str); 8] = [
    ("p", "period: day, month, session or block"),
    ("g", "group: none, account, model, or both"),
    ("f", "filter accounts and models"),
    ("[ ]", "range: 7, 30 or 90 days, or all"),
    ("Enter", "drill down: sessions, then models"),
    ("Esc  Backspace", "back up a level"),
    ("r", "refresh the token index"),
    ("J K", "scroll the detail and the notes"),
];
const HELP_HISTORY: [(&str, &str); 3] = [
    ("[ ]", "range: 1, 7, 14 or 30 days"),
    ("r", "refresh"),
    ("J K", "scroll the windows"),
];

pub(super) fn key(key: KeyEvent) -> (Option<Overlay>, Vec<Cmd>) {
    match key.code {
        KeyCode::Char('?') | KeyCode::Esc | KeyCode::Enter => (None, vec![]),
        _ => (Some(Overlay::Help), vec![]),
    }
}

pub(super) fn render(state: &State, frame: &mut Frame, area: Rect) {
    let keys: &[(&str, &str)] = match state.screen {
        Screen::Accounts => &HELP_ACCOUNTS,
        Screen::Tokens => &HELP_TOKENS,
        Screen::History => &HELP_HISTORY,
    };
    let mut lines = vec![Line::from(Span::styled("Everywhere", state.theme.strong()))];
    lines.extend(HELP_GLOBAL.iter().map(|(k, what)| help_line(k, what)));
    let title = match state.screen {
        Screen::Accounts => "Accounts",
        Screen::Tokens => "Tokens",
        Screen::History => "History",
    };
    lines.push(Line::from(Span::styled(title, state.theme.strong())));
    lines.extend(keys.iter().map(|(k, what)| help_line(k, what)));
    boxed(frame, area, " Help ", lines, 64);
}

fn help_line(key: &str, what: &str) -> Line<'static> {
    Line::from(format!("  {}  {what}", pad(key, 20)))
}

#[cfg(test)]
mod tests {
    use super::{HELP_ACCOUNTS, HELP_GLOBAL, HELP_HISTORY, HELP_TOKENS};
    use crate::state::Screen;
    use crate::testutil::*;
    use crate::update::update;

    #[test]
    fn every_help_line_shows_whole_on_every_screen_at_80_columns() {
        // R21: the key column and the description both fit the help box.
        for (screen, keys) in [
            (Screen::Accounts, &HELP_ACCOUNTS[..]),
            (Screen::Tokens, &HELP_TOKENS[..]),
            (Screen::History, &HELP_HISTORY[..]),
        ] {
            let mut state = fresh(screen);
            update(&mut state, ch('?'), NOW_MS);
            let lines = draw(&state, 80, 24);
            for (key, what) in HELP_GLOBAL.iter().chain(keys) {
                assert!(
                    lines.iter().any(|l| l.contains(key) && l.contains(what)),
                    "{screen:?}: {key} {what}: {lines:?}"
                );
            }
        }
    }
}
