use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use super::{BOX_COLS, BoxSpec, KeyOutcome, TextThen, boxed, question_rows};
use crate::actions::start;
use crate::host::Action;
use crate::state::State;
use crate::theme::Role;
use crate::view::{width_of, wrap_rows};

/// The longest alias and position the text dialogs take.
const ALIAS_MAX: usize = 64;
const POSITION_MAX: usize = 9;

pub(super) fn key(
    state: &mut State,
    input: &mut String,
    error: &mut Option<String>,
    then: &TextThen,
    key: KeyEvent,
    now_ms: i64,
) -> KeyOutcome {
    match key.code {
        KeyCode::Esc => KeyOutcome::Close,
        KeyCode::Enter => match submitted(then, input) {
            Submitted::Action(action) => KeyOutcome::CloseWith(start(state, action, now_ms)),
            Submitted::Refused(why) => {
                *error = Some(why);
                KeyOutcome::Keep
            }
            Submitted::Nothing => KeyOutcome::Keep,
        },
        KeyCode::Backspace => {
            *error = None;
            input.pop();
            KeyOutcome::Keep
        }
        KeyCode::Char(c) if !c.is_control() => {
            *error = None;
            let fits = match then {
                TextThen::Alias(_) => input.chars().count() < ALIAS_MAX,
                TextThen::Move { .. } => c.is_ascii_digit() && input.len() < POSITION_MAX,
            };
            if fits {
                input.push(c);
            }
            KeyOutcome::Keep
        }
        _ => KeyOutcome::Keep,
    }
}

pub(super) fn render(
    state: &State,
    frame: &mut Frame,
    area: Rect,
    prompt: &str,
    input: &str,
    error: Option<&str>,
) {
    let w = 64u16.min(area.width.saturating_sub(4));
    let inner = usize::from(w).saturating_sub(BOX_COLS).max(1);
    let mut lines: Vec<Line<'static>> = question_rows(prompt, inner, area.height)
        .into_iter()
        .map(Line::from)
        .collect();
    let shown = tail(input, inner.saturating_sub(1));
    lines.push(Line::from(vec![
        Span::raw(shown),
        Span::styled(" ", state.theme.focus()),
    ]));
    if let Some(error) = error {
        lines.extend(
            wrap_rows(error, inner, 2)
                .into_iter()
                .map(|row| Line::from(Span::styled(row, state.theme.role(Role::Crit)))),
        );
    }
    let spec = BoxSpec {
        title: "Input".to_owned(),
        keys: "Enter · Esc cancel",
        destructive: false,
        width: w,
    };
    boxed(frame, state.theme, spec, lines);
}

/// What `Enter` made of a text dialog's input.
enum Submitted {
    Action(Action),
    /// The input is refused, with the words to show under it.
    Refused(String),
    /// Nothing to submit yet.
    Nothing,
}

/// The action a text dialog's input asks for: an empty alias clears it (spec §7.1), and a
/// position must be a number from 1 to the provider's last (redesign §9.5, T2-20).
fn submitted(then: &TextThen, input: &str) -> Submitted {
    match then {
        TextThen::Alias(account) if input.trim().is_empty() => {
            Submitted::Action(Action::ClearAlias(account.clone()))
        }
        TextThen::Alias(account) => Submitted::Action(Action::SetAlias {
            account: account.clone(),
            alias: input.to_owned(),
        }),
        TextThen::Move { account, last } => match input.parse::<u32>() {
            Ok(position) if (1..=*last).contains(&position) => Submitted::Action(Action::Move {
                account: account.clone(),
                position,
            }),
            Ok(_) => Submitted::Refused(format!("Choose a position from 1 to {last}.")),
            Err(_) => Submitted::Nothing,
        },
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
