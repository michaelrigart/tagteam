use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Line;

use super::{CHOICE_ROWS, GUTTER, KeyOutcome, boxed, gutter_rows, question_rows};
use crate::msg::{Answer, AnswerTo, Cmd};
use crate::state::State;
use crate::view::{scrolled_windows, width_of, wrap_rows};

pub(super) fn key(
    options: &[String],
    selected: &mut usize,
    reply: AnswerTo,
    key: KeyEvent,
) -> KeyOutcome {
    let last = options.len().saturating_sub(1);
    let digit = match key.code {
        KeyCode::Char(c @ '1'..='9') => Some(c as usize - '1' as usize),
        _ => None,
    };
    let answer = match key.code {
        KeyCode::Enter if !options.is_empty() => Some(Some(*selected)),
        KeyCode::Esc => Some(None),
        _ => digit.filter(|d| *d <= last).map(Some),
    };
    if let Some(answer) = answer {
        let answer = Answer::Choose(answer);
        return KeyOutcome::CloseWith(vec![Cmd::Answer { to: reply, answer }]);
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
) {
    let w = 64u16.min(area.width.saturating_sub(4));
    let inner = usize::from(w) - 4;
    let asked = question_rows(question, inner, area.height);
    // The box: the question, a blank row, the choices, the keys, the border, and a
    // row's margin above and below.
    let room = usize::from(area.height)
        .saturating_sub(asked.len() + 7)
        .max(CHOICE_ROWS);
    let mut lines: Vec<Line<'static>> = asked.into_iter().map(Line::from).collect();
    lines.push(Line::default());
    let blocks: Vec<Vec<String>> = options
        .iter()
        .enumerate()
        .map(|(i, option)| choice_rows(i, option, inner - GUTTER))
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
    lines.push(Line::from("↑ ↓ choose · Enter · Esc cancel"));
    boxed(frame, area, " Choose ", lines, w);
}

/// Choice `i`'s rows, `w` columns at most each: `N. ` and its text, which wraps at word boundaries under
/// itself, onto `CHOICE_ROWS` at most; the rest is cut (`wrap_rows`).
fn choice_rows(i: usize, option: &str, w: usize) -> Vec<String> {
    let number = format!("{}. ", i + 1);
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
