use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::text::{Line, Span};

use super::{BOX_COLS, BOX_ROWS, BoxSpec, KeyOutcome, framed};
use crate::actions::start;
use crate::host::Action;
use crate::state::State;
use crate::theme::Role;
use crate::view::{areas, body_at, wrap};

pub(super) fn result_key(state: &State, text: &str, scroll: &mut u16, key: KeyEvent) -> KeyOutcome {
    match key.code {
        KeyCode::Esc | KeyCode::Enter => KeyOutcome::Close,
        code => {
            *scroll = scrolled(
                *scroll,
                code,
                text_rows(text, state.size.0),
                body_at(state).height,
            );
            KeyOutcome::Keep
        }
    }
}

pub(super) fn doctor_key(
    state: &mut State,
    text: &str,
    scroll: &mut u16,
    key: KeyEvent,
    now_ms: i64,
) -> KeyOutcome {
    match key.code {
        KeyCode::Esc | KeyCode::Enter => KeyOutcome::Close,
        KeyCode::Char('o') => {
            let provider = state.provider.clone();
            let action = Action::Doctor {
                provider,
                online: true,
            };
            KeyOutcome::KeepWith(start(state, action, now_ms))
        }
        code => {
            *scroll = scrolled(
                *scroll,
                code,
                text_rows(text, state.size.0),
                body_at(state).height,
            );
            KeyOutcome::Keep
        }
    }
}

pub(super) fn render_result(
    state: &State,
    frame: &mut Frame,
    title: &str,
    text: &str,
    scroll: &u16,
) {
    let body = areas(state, frame.area()).body;
    let lines = text_lines(text, body.width);
    let spec = BoxSpec {
        title: title.to_owned(),
        keys: "Esc close · ↑ ↓ scroll",
        destructive: false,
        width: body.width,
    };
    panel(state, frame, spec, lines, *scroll);
}

pub(super) fn render_doctor(
    state: &State,
    frame: &mut Frame,
    text: &str,
    scroll: &u16,
    online: &bool,
) {
    let body = areas(state, frame.area()).body;
    let theme = state.theme;
    let lines = wrapped(text, panel_width(body.width))
        .into_iter()
        .map(|(source, row)| {
            let style = match source.trim_start().chars().next() {
                Some('✓') => theme.role(Role::Ok),
                Some('!') => theme.role(Role::Warn),
                Some('✗') => theme.role(Role::Crit),
                _ => theme.role(Role::Text),
            };
            Line::from(Span::styled(row, style))
        })
        .collect();
    let spec = BoxSpec {
        title: if *online { "doctor --online" } else { "doctor" }.to_owned(),
        keys: "o online · Esc close · ↑ ↓ scroll",
        destructive: false,
        width: body.width,
    };
    panel(state, frame, spec, lines, *scroll);
}

/// A panel's scroll after `code`, never past its last full page when the body is `body_height`
/// rows tall: `rows` is how many rows the panel has, as it draws them. It moves from the row
/// that shows, which a resize may have brought nearer than the stored scroll.
pub(super) fn scrolled(scroll: u16, code: KeyCode, rows: usize, body_height: u16) -> u16 {
    let last = furthest(rows, body_height);
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

/// The rows of text a panel shows at once: a panel covers the body, and its frame (border and
/// padding) takes four rows of it.
fn page_rows(body_height: u16) -> usize {
    usize::from(body_height).saturating_sub(BOX_ROWS).max(1)
}

/// The furthest a panel of `rows` rows can be scrolled when the body is `body_height` rows
/// tall: to the top of its last full page, so the last row ends the panel and never sits alone
/// at its top. A panel shorter than the body does not scroll.
fn furthest(rows: usize, body_height: u16) -> u16 {
    u16::try_from(rows.saturating_sub(page_rows(body_height))).unwrap_or(u16::MAX)
}

/// The width of a panel's text when the body is `width` wide: the panel's border and padding
/// take `BOX_COLS` of it.
pub(super) fn panel_width(width: u16) -> usize {
    usize::from(width).saturating_sub(BOX_COLS)
}

/// `text` in rows of at most `width` columns, each with the line it came from: each line broken
/// at word boundaries (`wrap`; a word wider than a row breaks by width), its continuation rows
/// indented as the line is. An indent is a third of the width at most, so the words keep room.
fn wrapped(text: &str, width: usize) -> Vec<(&str, String)> {
    let width = width.max(1);
    let mut rows = Vec::new();
    for line in text.lines() {
        let body = line.trim_start_matches(' ');
        let indent = " ".repeat((line.len() - body.len()).min(width / 3));
        let room = width - indent.len();
        if body.is_empty() {
            rows.push((line, String::new()));
            continue;
        }
        rows.extend(
            wrap(body, room)
                .into_iter()
                .map(|row| (line, format!("{indent}{row}"))),
        );
    }
    rows
}

/// How many rows `text` takes in a panel over a body `width` wide.
fn text_rows(text: &str, width: u16) -> usize {
    wrapped(text, panel_width(width)).len()
}

/// `text` as the lines of a panel over a body `width` wide (`wrapped`).
fn text_lines(text: &str, width: u16) -> Vec<Line<'static>> {
    wrapped(text, panel_width(width))
        .into_iter()
        .map(|(_, row)| Line::from(row))
        .collect()
}

/// A scrollable panel over the whole body (§9.1), its keys in its bottom border. Its `lines` are
/// already as wide as the panel's text (`panel_width`), and its scroll stops at its last full
/// page at that height.
pub(super) fn panel(
    state: &State,
    frame: &mut Frame,
    spec: BoxSpec,
    lines: Vec<Line<'static>>,
    scroll: u16,
) {
    let body = areas(state, frame.area()).body;
    let scroll = scroll.min(furthest(lines.len(), body.height));
    framed(frame, state.theme, body, &spec, lines, scroll);
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Rect;
    use ratatui::style::Color;

    use crate::overlay::Overlay;
    use crate::state::Screen;
    use crate::testutil::*;
    use crate::theme::Role;
    use crate::view::areas;

    #[test]
    fn a_panel_covers_the_body_whole() {
        for (w, h) in [(80, 24), (120, 40)] {
            for overlay in [
                Overlay::Result {
                    title: "switch".into(),
                    text: "it failed\nsecond line".into(),
                    scroll: 0,
                },
                Overlay::Doctor {
                    text: "✓ ok".into(),
                    scroll: 0,
                    online: false,
                },
                Overlay::Help { scroll: 0 },
            ] {
                let mut state = fresh(Screen::Accounts);
                state.size = (w, h);
                let name = format!("{overlay:?}");
                state.overlay = Some(overlay);
                let body = areas(&state, Rect::new(0, 0, w, h)).body;
                let buf = draw_buffer(&state, w, h, NOW_MS);
                let (top, bottom) = (body.y, body.y + body.height - 1);
                let corners = [
                    ((0, top), "╭"),
                    ((w - 1, top), "╮"),
                    ((0, bottom), "╰"),
                    ((w - 1, bottom), "╯"),
                ];
                for ((x, y), symbol) in corners {
                    assert_eq!(buf[(x, y)].symbol(), symbol, "{name} at {w}×{h}: {x},{y}");
                }
                for y in top + 1..bottom {
                    assert_eq!(buf[(0, y)].symbol(), "│", "{name} at {w}×{h}: row {y}");
                    assert_eq!(buf[(w - 1, y)].symbol(), "│", "{name} at {w}×{h}: row {y}");
                }
                // And only the body: the rows above and below are the chrome's.
                for y in [top - 1, bottom + 1] {
                    for x in [0, w - 1] {
                        assert!(
                            !["│", "╭", "╮", "╰", "╯"].contains(&buf[(x, y)].symbol()),
                            "{name} at {w}×{h}: {x},{y}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn doctor_colours_each_line_by_its_glyph() {
        let long = format!("✓ {}", "word ".repeat(30).trim_end());
        let text = format!("✓ ok one\n! careful\n✗ broke\nplain\n  ✗ indented\n{long}");
        for theme in TIERS {
            let mut state = fresh(Screen::Accounts);
            state.theme = theme;
            state.overlay = Some(Overlay::Doctor {
                text: text.clone(),
                scroll: 0,
                online: false,
            });
            let buf = draw_buffer(&state, 80, 24, NOW_MS);
            let want = |role: Option<Role>| role.map_or(Default::default(), |r| theme.role(r));
            let rows = [
                ("✓ ok one", Some(Role::Ok)),
                ("! careful", Some(Role::Warn)),
                ("✗ broke", Some(Role::Crit)),
                ("plain", None),
                ("✗ indented", Some(Role::Crit)),
            ];
            for (line, role) in rows {
                let (x, y) = locate(&buf, line).unwrap();
                let style = want(role);
                let width = u16::try_from(line.chars().count()).unwrap();
                for i in 0..width {
                    let cell = &buf[(x + i, y)];
                    assert_eq!(
                        cell.fg,
                        style.fg.unwrap_or(Color::Reset),
                        "{theme:?} {line} {i}"
                    );
                    assert!(
                        cell.modifier.contains(style.add_modifier),
                        "{theme:?} {line} {i}"
                    );
                    assert_eq!(
                        cell.modifier.is_empty(),
                        style.add_modifier.is_empty(),
                        "{theme:?} {line} {i}"
                    );
                }
            }
            // A line that wraps keeps its colour on its continuation rows.
            let (x, y) = locate(&buf, "✓ word").unwrap();
            let style = theme.role(Role::Ok);
            let next = &buf[(x, y + 1)];
            assert_eq!(next.symbol(), "w", "{theme:?}: the second row of the line");
            assert_eq!(next.fg, style.fg.unwrap_or(Color::Reset), "{theme:?}");
        }
    }
}
