use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use super::{Overlay, TextThen, boxed, question_rows};
use crate::actions::start;
use crate::host::Action;
use crate::msg::Cmd;
use crate::state::State;
use crate::view::width_of;

/// The longest alias and position the text dialogs take.
const ALIAS_MAX: usize = 64;
const POSITION_MAX: usize = 9;

pub(super) fn key(
    state: &mut State,
    prompt: String,
    mut input: String,
    then: TextThen,
    key: KeyEvent,
    now_ms: i64,
) -> (Option<Overlay>, Vec<Cmd>) {
    match key.code {
        KeyCode::Esc => (None, vec![]),
        KeyCode::Enter => match submitted(&then, &input) {
            Some(action) => (None, start(state, action, now_ms)),
            None => (
                Some(Overlay::Text {
                    prompt,
                    input,
                    then,
                }),
                vec![],
            ),
        },
        KeyCode::Backspace => {
            input.pop();
            (
                Some(Overlay::Text {
                    prompt,
                    input,
                    then,
                }),
                vec![],
            )
        }
        KeyCode::Char(c) if !c.is_control() => {
            let fits = match then {
                TextThen::Alias(_) => input.chars().count() < ALIAS_MAX,
                TextThen::Move(_) => c.is_ascii_digit() && input.len() < POSITION_MAX,
            };
            if fits {
                input.push(c);
            }
            (
                Some(Overlay::Text {
                    prompt,
                    input,
                    then,
                }),
                vec![],
            )
        }
        _ => (
            Some(Overlay::Text {
                prompt,
                input,
                then,
            }),
            vec![],
        ),
    }
}

pub(super) fn render(state: &State, frame: &mut Frame, area: Rect, prompt: &str, input: &str) {
    let w = 64u16.min(area.width.saturating_sub(4));
    let inner = usize::from(w) - 4;
    let mut lines: Vec<Line<'static>> = question_rows(prompt, inner, area.height)
        .into_iter()
        .map(Line::from)
        .collect();
    let shown = tail(input, inner.saturating_sub(1));
    lines.push(Line::from(vec![
        Span::raw(shown),
        Span::styled(" ", state.theme.selection()),
    ]));
    lines.push(Line::from("Enter · Esc cancel"));
    boxed(frame, area, " Input ", lines, w);
}

/// The action a text dialog's input asks for: an empty alias clears it (spec §7.1), and a
/// position must be a number.
fn submitted(then: &TextThen, input: &str) -> Option<Action> {
    match then {
        TextThen::Alias(account) if input.trim().is_empty() => {
            Some(Action::ClearAlias(account.clone()))
        }
        TextThen::Alias(account) => Some(Action::SetAlias {
            account: account.clone(),
            alias: input.to_owned(),
        }),
        TextThen::Move(account) => input.parse::<u32>().ok().map(|position| Action::Move {
            account: account.clone(),
            position,
        }),
    }
}

/// The end of `s` that fits in `w` columns: what was typed last stays in view.
fn tail(s: &str, w: usize) -> String {
    let mut start = s.len();
    for (i, _) in s.char_indices().rev() {
        if width_of(&s[i..]) > w {
            break;
        }
        start = i;
    }
    s[start..].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_input_shows_its_end() {
        assert_eq!(tail("abcdef", 3), "def");
        assert_eq!(tail("山田太郎", 5), "太郎");
        assert_eq!(tail("ab", 5), "ab");
    }
}
