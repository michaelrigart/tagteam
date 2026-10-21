use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use super::{BOX_COLS, BoxSpec, ConfirmThen, KeyOutcome, boxed, question_rows};
use crate::actions::start;
use crate::host::Action;
use crate::msg::{Answer, Cmd};
use crate::state::State;
use crate::theme::Role;

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

pub(super) fn render(
    state: &State,
    frame: &mut Frame,
    area: Rect,
    question: &str,
    choice: &bool,
    then: &ConfirmThen,
) {
    let theme = state.theme;
    let destructive = matches!(then, ConfirmThen::Run(Action::Remove(_)));
    let w = 64u16.min(area.width.saturating_sub(4));
    let inner = usize::from(w).saturating_sub(BOX_COLS).max(1);
    let mut lines: Vec<Line<'static>> = question_rows(question, inner, area.height)
        .into_iter()
        .map(Line::from)
        .collect();
    lines.push(Line::default());
    let button = |label: &str, on: bool, destructive: bool| {
        let text = format!("[ {label} ]");
        match (on, destructive) {
            (true, true) => Span::styled(text, theme.focus_crit()),
            (true, false) => Span::styled(text, theme.focus()),
            (false, true) => Span::styled(text, theme.role(Role::Crit)),
            (false, false) => Span::raw(text),
        }
    };
    lines.push(Line::from(vec![
        button("Yes", *choice, destructive),
        Span::raw("   "),
        button("No", !*choice, false),
    ]));
    let spec = BoxSpec {
        title: "Confirm".to_owned(),
        keys: "y yes · n no · Enter choose · Esc no",
        destructive,
        width: w,
    };
    boxed(frame, theme, spec, lines);
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
    use crossterm::event::KeyCode;
    use ratatui::style::{Color, Modifier};

    use crate::host::Action;
    use crate::msg::AnswerTo;
    use crate::overlay::{ConfirmThen, Overlay};
    use crate::state::Screen;
    use crate::testutil::*;
    use crate::theme::Role;
    use crate::update::update;

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
                let surface = theme.surface().bg.unwrap_or(Color::Reset);
                assert_eq!(
                    cell.bg, surface,
                    "unfilled, on the dialog's own ground: {theme:?} {i}"
                );
                assert!(!cell.modifier.contains(Modifier::REVERSED), "{theme:?} {i}");
            }
        }
    }

    #[test]
    fn a_destructive_confirm_titles_and_fills_in_crit() {
        for theme in TIERS {
            let mut state = fresh(Screen::Accounts);
            state.theme = theme;
            state.accounts.apply(snapshot(many(1)));
            update(&mut state, ch('x'), NOW_MS);
            assert!(matches!(
                &state.overlay,
                Some(Overlay::Confirm {
                    then: ConfirmThen::Run(Action::Remove(_)),
                    ..
                })
            ));
            let crit = theme.role(Role::Crit);
            let title = crit.patch(theme.strong());
            let buf = draw_buffer(&state, 80, 24, NOW_MS);
            let (x, y) = locate(&buf, "Confirm").unwrap();
            for i in 0..7 {
                let cell = &buf[(x + i, y)];
                assert_eq!(cell.fg, title.fg.unwrap_or(Color::Reset), "{theme:?} {i}");
                assert!(cell.modifier.contains(title.add_modifier), "{theme:?} {i}");
            }
            // Its Yes is in crit unfocused (the default is No) and filled in crit focused.
            let (yx, yy) = locate(&buf, "[ Yes ]").unwrap();
            assert_eq!(
                buf[(yx + 2, yy)].fg,
                crit.fg.unwrap_or(Color::Reset),
                "{theme:?}"
            );
            assert!(
                buf[(yx + 2, yy)].modifier.contains(crit.add_modifier),
                "{theme:?}"
            );
            assert_eq!(
                buf[(yx + 2, yy)].bg,
                theme.surface().bg.unwrap_or(Color::Reset)
            );
            update(&mut state, key(KeyCode::Left), NOW_MS);
            let buf = draw_buffer(&state, 80, 24, NOW_MS);
            let fill = theme.focus_crit();
            for i in 0..7 {
                let cell = &buf[(yx + i, yy)];
                assert_eq!(cell.bg, fill.bg.unwrap_or(Color::Reset), "{theme:?} {i}");
                assert_eq!(cell.fg, fill.fg.unwrap_or(Color::Reset), "{theme:?} {i}");
                assert_eq!(
                    cell.modifier.contains(Modifier::REVERSED),
                    theme == TIERS[0],
                    "{theme:?} {i}"
                );
            }
            // Not the accent's fill, except where tier 0 has only REVERSED for both.
            assert!(
                theme == TIERS[0] || theme.focus_crit() != theme.focus(),
                "{theme:?}"
            );
            // An ordinary confirm keeps the accent.
            state.overlay = Some(Overlay::Confirm {
                question: "Pause?".into(),
                choice: true,
                then: ConfirmThen::Reply(AnswerTo(1)),
            });
            let buf = draw_buffer(&state, 80, 24, NOW_MS);
            let (x, y) = locate(&buf, "Confirm").unwrap();
            let accent = theme.role(Role::Accent);
            assert_eq!(
                buf[(x, y)].fg,
                accent.fg.unwrap_or(Color::Reset),
                "{theme:?}"
            );
            let (yx, yy) = locate(&buf, "[ Yes ]").unwrap();
            assert_eq!(buf[(yx, yy)].bg, theme.focus().bg.unwrap_or(Color::Reset));
        }
    }
}
