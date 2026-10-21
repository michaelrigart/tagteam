use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use super::{ConfirmThen, KeyOutcome, boxed, question_rows};
use crate::actions::start;
use crate::msg::{Answer, Cmd};
use crate::state::State;

pub(super) fn key(
    state: &mut State,
    choice: &mut bool,
    then: &ConfirmThen,
    key: KeyEvent,
    now_ms: i64,
) -> KeyOutcome {
    let answer = match key.code {
        KeyCode::Char('y' | 'Y') => Some(true),
        KeyCode::Char('n' | 'N') | KeyCode::Esc => Some(false),
        KeyCode::Enter => Some(*choice),
        _ => None,
    };
    match answer {
        Some(yes) => KeyOutcome::CloseWith(confirmed(state, then, yes, now_ms)),
        None => {
            if matches!(
                key.code,
                KeyCode::Left
                    | KeyCode::Right
                    | KeyCode::Tab
                    | KeyCode::BackTab
                    | KeyCode::Char('h' | 'l')
            ) {
                *choice = !*choice;
            }
            KeyOutcome::Keep
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
            Span::styled(text, state.theme.focus())
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
fn confirmed(state: &mut State, then: &ConfirmThen, yes: bool, now_ms: i64) -> Vec<Cmd> {
    match then {
        ConfirmThen::Reply(to) => vec![Cmd::Answer {
            to: *to,
            answer: Answer::Confirm(yes),
        }],
        ConfirmThen::Run(action) if yes => start(state, action.clone(), now_ms),
        ConfirmThen::Run(_) => vec![],
    }
}

#[cfg(test)]
mod tests {
    use ratatui::style::{Color, Modifier};

    use crate::msg::AnswerTo;
    use crate::overlay::{ConfirmThen, Overlay};
    use crate::state::Screen;
    use crate::testutil::*;

    #[test]
    fn a_focused_button_is_filled_and_reversed_only_without_colour() {
        for theme in TIERS {
            let mut state = fresh(Screen::Accounts);
            state.theme = theme;
            state.overlay = Some(Overlay::Confirm {
                question: "Remove a1@x.co?".into(),
                choice: true,
                then: ConfirmThen::Reply(AnswerTo(1)),
            });
            let buf = draw_buffer(&state, 80, 24, NOW_MS);
            let focus = theme.focus();
            let (x, y) = locate(&buf, "[ Yes ]").unwrap();
            let (nx, ny) = locate(&buf, "[ No ]").unwrap();
            for i in 0..7 {
                let cell = &buf[(x + i, y)];
                assert_eq!(cell.bg, focus.bg.unwrap_or(Color::Reset), "{theme:?} {i}");
                assert_eq!(cell.fg, focus.fg.unwrap_or(Color::Reset), "{theme:?} {i}");
                assert_eq!(
                    cell.modifier.contains(Modifier::REVERSED),
                    theme == TIERS[0],
                    "{theme:?} {i}"
                );
            }
            for i in 0..6 {
                let cell = &buf[(nx + i, ny)];
                assert_eq!(cell.bg, Color::Reset, "{theme:?} {i}");
                assert!(!cell.modifier.contains(Modifier::REVERSED), "{theme:?} {i}");
            }
        }
    }
}
