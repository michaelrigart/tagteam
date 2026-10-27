//! The full list (S§6): every account as a block, scrolled by block.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use tagteam_core::AccountId;
use tagteam_engine::views::AccountView;

use super::card;
use super::table::IN_USE;
use crate::chrome::fit_spans;
use crate::state::State;
use crate::theme::Role;
use crate::view::select;

/// A block's rows start this far in: its facts, windows and problems under the name (S§6.1).
const INDENT: usize = 7;
/// The index of the title's first name span, for `select`: `▌`, the position, the mark and a
/// space come before it.
const NAME: usize = 4;
/// The blank, rule and blank under the chrome stay pinned; the same three separate two blocks.
const PINNED: u16 = 3;
const SEP: usize = PINNED as usize;
/// The rule between blocks stops this far from each side.
const INSET: usize = 3;

/// One scroll unit: `head` (Task 7: the provider heading), the account's own `lines` (title
/// to its last window or problem row), `tail` (Task 7: the blank row and the one-account
/// hint). Its height counts all three; `▌` and DIM apply to `lines` only (S§6.1).
pub(super) struct Block {
    pub(super) id: AccountId,
    pub(super) head: Vec<Line<'static>>,
    pub(super) lines: Vec<Line<'static>>,
    pub(super) tail: Vec<Line<'static>>,
}

impl Block {
    fn height(&self) -> usize {
        self.head.len() + self.lines.len() + self.tail.len()
    }
}

/// Every account's block at `width`, in table order.
pub(super) fn blocks(state: &State, width: u16, now_ms: i64) -> Vec<Block> {
    let theme = state.theme;
    let now_s = now_ms.div_euclid(1000);
    let rows = state.accounts.rows();
    let at = card::row_layout(
        rows.iter()
            .flat_map(|v| card::windows_of(v))
            .map(|(w, _)| w),
        INDENT,
        width,
    );
    rows.into_iter()
        .map(|v| {
            let chosen = state.accounts.selected.as_ref() == Some(&v.row.id);
            let mut lines = vec![
                title(v, chosen, width, state),
                card::facts_row(state, v, INDENT, width, now_ms),
            ];
            lines.extend(
                card::windows_of(v)
                    .iter()
                    .map(|(w, p)| card::window_line(w, p, &at, v.active, now_s, theme)),
            );
            lines.extend(card::no_windows_row(v, INDENT, width, now_s, theme));
            lines.extend(card::problem_rows(state, v, INDENT, width, now_ms));
            let lines = lines
                .into_iter()
                .enumerate()
                .map(|(i, line)| {
                    let line = if v.row.disabled {
                        line.style(theme.dim())
                    } else {
                        line
                    };
                    if chosen {
                        select(line, (i == 0).then_some(NAME), usize::from(width), theme)
                    } else {
                        line
                    }
                })
                .collect();
            Block {
                id: v.row.id.clone(),
                head: vec![],
                lines,
                tail: vec![],
            }
        })
        .collect()
}

/// The title (S§6.1): the position right-aligned in columns 1-3, `●` for the account in use, the
/// name from column 7, bold when selected.
fn title(v: &AccountView, chosen: bool, width: u16, state: &State) -> Line<'static> {
    let theme = state.theme;
    let name = card::name_spans(&v.row, width, chosen, theme);
    let mark = if v.active {
        Span::styled(IN_USE, theme.role(Role::Accent))
    } else {
        Span::raw(" ")
    };
    let mut spans = vec![
        Span::raw(" "),
        Span::raw(format!("{:>3} ", v.row.position)),
        mark,
        Span::raw(" "),
    ];
    spans.extend(name);
    Line::from(fit_spans(spans, usize::from(width)))
}

/// A blank row, a faint rule three columns in from each side, and a blank row (S§6.1).
fn separator(state: &State, width: u16) -> [Line<'static>; 3] {
    let dot = if state.theme.colourless() {
        "·"
    } else {
        "┈"
    };
    let rule = Line::from(vec![
        Span::raw(" ".repeat(INSET)),
        Span::styled(
            dot.repeat(usize::from(width).saturating_sub(2 * INSET)),
            state.theme.role(Role::Faint),
        ),
    ]);
    [Line::default(), rule, Line::default()]
}

/// The first block to show so that block `at` is whole in `rows` rows, moving `from` as little
/// as needed; a block taller than `rows` is shown alone. `heights` count the separator above
/// every block but the first; the first one shown loses it.
pub(super) fn keep_whole(heights: &[usize], from: usize, at: usize, rows: usize) -> usize {
    if heights.is_empty() {
        return 0;
    }
    let at = at.min(heights.len() - 1);
    let mut from = from.min(at);
    let used = |from: usize| heights[from..=at].iter().sum::<usize>() - SEP * usize::from(from > 0);
    while from < at && used(from) > rows {
        from += 1;
    }
    from
}

/// The list at `body`: its blocks, their heights, the selected block and the rows they scroll in.
struct Geometry {
    blocks: Vec<Block>,
    heights: Vec<usize>,
    at: usize,
    rows: usize,
}

fn geometry(state: &State, body: Rect, now_ms: i64) -> Geometry {
    let blocks = blocks(state, body.width, now_ms);
    let heights = blocks
        .iter()
        .enumerate()
        .map(|(i, b)| b.height() + SEP * usize::from(i > 0))
        .collect();
    let at = blocks
        .iter()
        .position(|b| state.accounts.selected.as_ref() == Some(&b.id))
        .unwrap_or(0);
    Geometry {
        blocks,
        heights,
        at,
        rows: usize::from(body.height.saturating_sub(PINNED)),
    }
}

/// Moves the first block shown the least that keeps the selected one whole.
pub(super) fn follow(state: &mut State, body: Rect, now_ms: i64) {
    let g = geometry(state, body, now_ms);
    state.accounts.list_from = keep_whole(&g.heights, state.accounts.list_from, g.at, g.rows);
}

/// How many whole blocks show from the first one shown: what `PgUp` and `PgDn` move by.
pub(super) fn page(state: &State, body: Rect, now_ms: i64) -> usize {
    let g = geometry(state, body, now_ms);
    let from = state.accounts.list_from;
    let mut used = 0;
    let whole = g
        .heights
        .iter()
        .enumerate()
        .skip(from)
        .take_while(|(i, h)| {
            used += **h - SEP * usize::from(*i == from && from > 0);
            used <= g.rows
        })
        .count();
    whole.max(1)
}

/// Draws the pinned blank, rule and blank, then the blocks from the first shown while they fit,
/// the next one cut at the body's end.
pub(super) fn render(state: &State, frame: &mut Frame, body: Rect, now_ms: i64) {
    let pinned = body.height.min(PINNED);
    let top = Rect::new(body.x, body.y, body.width, pinned);
    frame.render_widget(
        Paragraph::new(super::separator(state, body.width, pinned)),
        top,
    );
    let area = Rect::new(body.x, body.y + pinned, body.width, body.height - pinned);
    let g = geometry(state, body, now_ms);
    let from = keep_whole(&g.heights, state.accounts.list_from, g.at, g.rows);
    let mut shown: Vec<Line<'static>> = Vec::new();
    for (i, block) in g.blocks.into_iter().enumerate().skip(from) {
        if shown.len() >= g.rows {
            break;
        }
        if i > from {
            shown.extend(separator(state, body.width));
        }
        shown.extend(block.head);
        shown.extend(block.lines);
        shown.extend(block.tail);
    }
    frame.render_widget(Paragraph::new(shown), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keep_whole_moves_as_little_as_needed() {
        // Heights include the separator rows above every block but the first (3 each); the first
        // block shown loses its separator, so a block of height h shows in h - 3 rows when first.
        assert_eq!(keep_whole(&[5, 8, 8, 8], 0, 1, 17), 0);
        assert_eq!(keep_whole(&[5, 8, 8, 8], 0, 2, 17), 1);
        assert_eq!(keep_whole(&[5, 8, 8, 8], 2, 0, 17), 0);
        assert_eq!(keep_whole(&[5, 20, 8], 0, 1, 17), 1); // exactly fits once first (17 rows)
        assert_eq!(keep_whole(&[5, 21, 8], 0, 1, 17), 1); // taller than the viewport: alone
        assert_eq!(keep_whole(&[5, 8], 9, 1, 17), 1); // a stale `from` clamps
    }
}
