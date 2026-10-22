use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Line;

use super::{
    BOX_COLS, BoxSpec, CHOICE_ROWS, ChooseThen, GUTTER, KeyOutcome, boxed, gutter_rows,
    question_rows,
};
use crate::msg::{Answer, Cmd};
use crate::screens::history;
use crate::state::State;
use crate::view::{scrolled_windows, width_of, wrap_rows};

/// `⏎` or a digit chooses, `Esc` declines: a CLI prompt is answered, and History takes the
/// scope chosen (T4-17). A prompt's digit is its option's number; a scope's is the account's
/// position, `0` all accounts.
pub(super) fn key(
    state: &mut State,
    options: &[String],
    selected: &mut usize,
    then: &ChooseThen,
    key: KeyEvent,
    now_ms: i64,
) -> KeyOutcome {
    let last = options.len().saturating_sub(1);
    let digit = match key.code {
        KeyCode::Char(c) => c.to_digit(10).and_then(|d| match then {
            ChooseThen::Reply(_) => (d >= 1).then(|| d as usize - 1).filter(|i| *i <= last),
            ChooseThen::Scope(scopes) => history::scope_of_digit(state, scopes, d),
        }),
        _ => None,
    };
    let answer = match key.code {
        KeyCode::Enter if !options.is_empty() => Some(Some(*selected)),
        KeyCode::Esc => Some(None),
        _ => digit.map(Some),
    };
    if let Some(answer) = answer {
        return KeyOutcome::CloseWith(match then {
            ChooseThen::Reply(to) => vec![Cmd::Answer {
                to: *to,
                answer: Answer::Choose(answer),
            }],
            ChooseThen::Scope(scopes) => answer
                .and_then(|i| scopes.get(i).cloned())
                .map_or_else(Vec::new, |scope| history::choose(state, scope, now_ms)),
        });
    }
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => *selected = selected.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('j') => *selected = (*selected + 1).min(last),
        _ => {}
    }
    KeyOutcome::Keep
}

pub(super) fn render(
    state: &State,
    frame: &mut Frame,
    area: Rect,
    question: &str,
    options: &[String],
    at: &usize,
    then: &ChooseThen,
) {
    let w = 64u16.min(area.width.saturating_sub(4));
    let inner = usize::from(w).saturating_sub(BOX_COLS).max(1);
    let asked = question_rows(question, inner, area.height);
    // The box: the question, a blank row, the choices, the border and padding, and a row's
    // margin above and below. The keys are in the border.
    let room = usize::from(area.height)
        .saturating_sub(asked.len() + 7)
        .max(CHOICE_ROWS);
    let mut lines: Vec<Line<'static>> = asked.into_iter().map(Line::from).collect();
    lines.push(Line::default());
    // A scope's options lead with the account's position, its digit: no number of their own.
    let numbered = matches!(then, ChooseThen::Reply(_));
    let blocks: Vec<Vec<String>> = options
        .iter()
        .enumerate()
        .map(|(i, option)| choice_rows(numbered.then_some(i), option, inner - GUTTER))
        .collect();
    let heights: Vec<usize> = blocks.iter().map(Vec::len).collect();
    // From the first choice that keeps the selected one in view, whole.
    let mut first = *at;
    let mut used = heights.get(*at).copied().unwrap_or(0);
    while first > 0 && used + heights[first - 1] <= room {
        first -= 1;
        used += heights[first];
    }
    let shown = scrolled_windows(&heights, first, room);
    for (i, block) in blocks.into_iter().enumerate() {
        if !shown.contains(&i) {
            continue;
        }
        lines.extend(gutter_rows(block, i == *at, inner, state.theme));
    }
    let spec = BoxSpec {
        title: "Choose".to_owned(),
        keys: "↑ ↓ choose · Enter · Esc cancel",
        destructive: false,
        width: w,
    };
    boxed(frame, state.theme, spec, lines);
}

/// Choice `i`'s rows, `w` columns at most each: `N. ` when it is numbered and its text, which
/// wraps at word boundaries under itself, onto `CHOICE_ROWS` at most; the rest is cut
/// (`wrap_rows`).
fn choice_rows(i: Option<usize>, option: &str, w: usize) -> Vec<String> {
    let number = i.map_or_else(String::new, |i| format!("{}. ", i + 1));
    let hang = width_of(&number);
    wrap_rows(option, w.saturating_sub(hang), CHOICE_ROWS)
        .into_iter()
        .enumerate()
        .map(|(row, text)| {
            let lead = if row == 0 {
                number.clone()
            } else {
                " ".repeat(hang)
            };
            format!("{lead}{text}")
        })
        .collect()
}
