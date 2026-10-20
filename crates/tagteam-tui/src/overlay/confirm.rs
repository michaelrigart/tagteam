use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use super::{ConfirmThen, Overlay, boxed, question_rows};
use crate::actions::start;
use crate::msg::{Answer, Cmd};
use crate::state::State;

pub(super) fn key(
    state: &mut State,
    question: String,
    default_yes: bool,
    choice: bool,
    then: ConfirmThen,
    key: KeyEvent,
    now_ms: i64,
) -> (Option<Overlay>, Vec<Cmd>) {
    let answer = match key.code {
        KeyCode::Char('y' | 'Y') => Some(true),
        KeyCode::Char('n' | 'N') | KeyCode::Esc => Some(false),
        KeyCode::Enter => Some(choice),
        _ => None,
    };
    match answer {
        Some(yes) => (None, confirmed(state, then, yes, now_ms)),
        None => {
            let choice = match key.code {
                KeyCode::Left
                | KeyCode::Right
                | KeyCode::Tab
                | KeyCode::BackTab
                | KeyCode::Char('h' | 'l') => !choice,
                _ => choice,
            };
            let kept = Overlay::Confirm {
                question,
                default_yes,
                choice,
                then,
            };
            (Some(kept), vec![])
        }
    }
}

pub(super) fn render(state: &State, frame: &mut Frame, area: Rect, question: &str, choice: &bool) {
    let w = 64u16.min(area.width.saturating_sub(4));
    let mut lines: Vec<Line<'static>> = question_rows(question, usize::from(w) - 4, area.height)
        .into_iter()
        .map(Line::from)
        .collect();
    lines.push(Line::default());
    let button = |label: &str, on: bool| {
        let text = format!("[ {label} ]");
        if on {
            Span::styled(text, state.theme.selection())
        } else {
            Span::raw(text)
        }
    };
    lines.push(Line::from(vec![
        button("Yes", *choice),
        Span::raw("   "),
        button("No", !*choice),
    ]));
    lines.push(Line::from("y yes · n no · Enter choose · Esc no"));
    boxed(frame, area, " Confirm ", lines, w);
}

/// A confirm dialog's answer: sent to the prompt waiting for it, or a yes starts the action.
fn confirmed(state: &mut State, then: ConfirmThen, yes: bool, now_ms: i64) -> Vec<Cmd> {
    match then {
        ConfirmThen::Reply(to) => vec![Cmd::Answer {
            to,
            answer: Answer::Confirm(yes),
        }],
        ConfirmThen::Run(action) if yes => start(state, action, now_ms),
        ConfirmThen::Run(_) => vec![],
    }
}
