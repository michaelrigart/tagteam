use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Line;

use super::{BOX_COLS, BoxSpec, KeyOutcome, ProceedThen, boxed, question_rows};
use crate::actions::start;
use crate::msg::Cmd;
use crate::state::State;
use crate::view::wrap_rows;

/// What a proceed dialog's border says `⏎` does: `go` is `starts` or `continue`.
fn keys(go: &str) -> &'static str {
    match go {
        "starts" => "⏎ starts · Esc cancel",
        _ => "⏎ continue · Esc cancel",
    }
}

pub(super) fn key(state: &mut State, then: &ProceedThen, key: KeyEvent, now_ms: i64) -> KeyOutcome {
    match key.code {
        KeyCode::Enter => match then {
            ProceedThen::Start(action) => {
                KeyOutcome::CloseWith(start(state, action.clone(), now_ms))
            }
            ProceedThen::Suspend(action) => {
                // The terminal is handed over now, and not before (T2-19).
                state.on_terminal = true;
                KeyOutcome::CloseWith(vec![Cmd::Suspend(action.clone())])
            }
        },
        KeyCode::Esc => {
            // The command changed nothing, so declining says nothing (TUI §4.5).
            if matches!(then, ProceedThen::Suspend(_)) {
                state.running = None;
                state.dirty = true;
            }
            KeyOutcome::Close
        }
        _ => KeyOutcome::Keep,
    }
}

pub(super) fn render(
    state: &State,
    frame: &mut Frame,
    area: Rect,
    question: &str,
    detail: &str,
    go: &str,
) {
    let w = 64u16.min(area.width.saturating_sub(4));
    let inner = usize::from(w).saturating_sub(BOX_COLS).max(1);
    let mut lines: Vec<Line<'static>> = question_rows(question, inner, area.height)
        .into_iter()
        .map(Line::from)
        .collect();
    lines.push(Line::default());
    lines.extend(
        wrap_rows(detail, inner, usize::from(area.height / 3))
            .into_iter()
            .map(Line::from),
    );
    let spec = BoxSpec {
        title: "Confirm".to_owned(),
        keys: keys(go),
        destructive: false,
        width: w,
    };
    boxed(frame, state.theme, spec, lines);
}
