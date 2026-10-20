use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, Paragraph};

use super::Overlay;
use crate::actions::start;
use crate::host::Action;
use crate::msg::Cmd;
use crate::state::State;
use crate::view::wrap;

pub(super) fn result_key(
    state: &State,
    title: String,
    text: String,
    scroll: u16,
    key: KeyEvent,
) -> (Option<Overlay>, Vec<Cmd>) {
    match key.code {
        KeyCode::Esc | KeyCode::Enter => (None, vec![]),
        code => {
            let scroll = scrolled(scroll, code, &text, state.size);
            (
                Some(Overlay::Result {
                    title,
                    text,
                    scroll,
                }),
                vec![],
            )
        }
    }
}

pub(super) fn doctor_key(
    state: &mut State,
    text: String,
    scroll: u16,
    online: bool,
    key: KeyEvent,
    now_ms: i64,
) -> (Option<Overlay>, Vec<Cmd>) {
    match key.code {
        KeyCode::Esc | KeyCode::Enter => (None, vec![]),
        KeyCode::Char('o') => {
            let provider = state.provider.clone();
            let action = Action::Doctor {
                provider,
                online: true,
            };
            let cmds = start(state, action, now_ms);
            let kept = Overlay::Doctor {
                text,
                scroll,
                online,
            };
            (Some(kept), cmds)
        }
        code => {
            let scroll = scrolled(scroll, code, &text, state.size);
            (
                Some(Overlay::Doctor {
                    text,
                    scroll,
                    online,
                }),
                vec![],
            )
        }
    }
}

pub(super) fn render_result(frame: &mut Frame, area: Rect, title: &str, text: &str, scroll: &u16) {
    panel(
        frame,
        area,
        &format!(" {title} "),
        text,
        *scroll,
        "Esc close · ↑ ↓ scroll",
    );
}

pub(super) fn render_doctor(
    frame: &mut Frame,
    area: Rect,
    text: &str,
    scroll: &u16,
    online: &bool,
) {
    let title = if *online {
        " doctor --online "
    } else {
        " doctor "
    };
    panel(
        frame,
        area,
        title,
        text,
        *scroll,
        "o online · Esc close · ↑ ↓ scroll",
    );
}

/// A panel's scroll after `code`, never past its last full page on a `size` terminal: rows,
/// not lines, as the panel wraps them. It moves from the row that shows, which a resize may
/// have brought nearer than the stored scroll.
fn scrolled(scroll: u16, code: KeyCode, text: &str, (width, height): (u16, u16)) -> u16 {
    let last = furthest(text, width, height);
    let scroll = scroll.min(last);
    let to = match code {
        KeyCode::Down | KeyCode::Char('j') => scroll.saturating_add(1),
        KeyCode::Up | KeyCode::Char('k') => scroll.saturating_sub(1),
        KeyCode::PageDown | KeyCode::Char(' ') => scroll.saturating_add(10),
        KeyCode::PageUp => scroll.saturating_sub(10),
        KeyCode::Home | KeyCode::Char('g') => 0,
        KeyCode::End | KeyCode::Char('G') => last,
        _ => scroll,
    };
    to.min(last)
}

/// The rows of text a panel shows at once on a terminal `height` rows tall: the panel leaves a
/// row above and below it, and its borders take two more.
fn panel_height(height: u16) -> usize {
    usize::from(height.saturating_sub(4)).max(1)
}

/// The furthest a panel's text can be scrolled on a `width` by `height` terminal: to the top of
/// its last full page, so the last row ends the panel and never sits alone at its top. Text
/// shorter than the panel does not scroll.
fn furthest(text: &str, width: u16, height: u16) -> u16 {
    let rows = panel_rows(text, panel_width(width)).len();
    u16::try_from(rows.saturating_sub(panel_height(height))).unwrap_or(u16::MAX)
}

/// The width of a panel's text on a terminal `width` wide: two columns of margin, a border and
/// a column of padding on either side.
fn panel_width(width: u16) -> usize {
    usize::from(width.saturating_sub(8))
}

/// `text` in rows of at most `width` columns: each line broken at word boundaries (`wrap`; a
/// word wider than a row breaks by width), its continuation rows indented as the line is. An
/// indent is a third of the width at most, so the words keep room.
fn panel_rows(text: &str, width: usize) -> Vec<String> {
    let mut rows = Vec::new();
    for line in text.lines() {
        let body = line.trim_start_matches(' ');
        let indent = " ".repeat((line.len() - body.len()).min(width / 3));
        let room = width - indent.len();
        if body.is_empty() {
            rows.push(String::new());
            continue;
        }
        rows.extend(
            wrap(body, room)
                .into_iter()
                .map(|row| format!("{indent}{row}")),
        );
    }
    rows
}

/// A scrollable panel over most of `area`, its keys on its last line. Its text wraps at the
/// panel's width (`panel_rows`), and its scroll stops at its last full page at that width and height.
fn panel(frame: &mut Frame, area: Rect, title: &str, text: &str, scroll: u16, keys: &str) {
    let scroll = scroll.min(furthest(text, area.width, area.height));
    let rect = Rect::new(
        area.x + 2,
        area.y + 1,
        area.width.saturating_sub(4),
        area.height.saturating_sub(2),
    );
    frame.render_widget(Clear, rect);
    let block = Block::bordered()
        .title(title.to_owned())
        .title_bottom(Line::from(format!(" {keys} ")))
        .padding(ratatui::widgets::Padding::horizontal(1));
    let lines: Vec<Line<'static>> = panel_rows(text, panel_width(area.width))
        .into_iter()
        .map(Line::from)
        .collect();
    frame.render_widget(Paragraph::new(lines).block(block).scroll((scroll, 0)), rect);
}
