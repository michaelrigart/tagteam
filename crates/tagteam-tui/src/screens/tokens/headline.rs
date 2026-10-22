//! The Tokens screen's headline (redesign §8, VIS-5, VIS-9): what the report adds up to, over
//! its table.

use ratatui::text::{Line, Span};
use tagteam_engine::tokens::TokenReport;
use tagteam_engine::words;

use super::amounts;
use crate::chrome::fit_spans;
use crate::state::State;
use crate::theme::Role;

/// Between the headline's figures (§8's text).
const GAP: &str = "    ";

/// The headline's rows at `width` (T5-10): one, its left part, until Task 5.
pub(super) fn headline(
    state: &State,
    report: &TokenReport,
    width: usize,
    _now_ms: i64,
) -> Vec<Line<'static>> {
    let theme = state.theme;
    let (text, muted) = (theme.role(Role::Text), theme.role(Role::Muted));
    let total = &report.total;
    let cost = match total.cost {
        Some(c) if c.partial => Span::styled(
            format!("{}{}", words::AT_LEAST, amounts::dollars(c.usd_nanos)),
            theme.role(Role::Warn).patch(theme.strong()),
        ),
        Some(c) => Span::styled(amounts::dollars(c.usd_nanos), theme.strong()),
        None => Span::styled(words::MISSING, theme.strong()),
    };
    let tokens = total.counts.total();
    let spans = vec![
        Span::raw(" "),
        cost,
        Span::raw(" "),
        Span::styled("at API prices", muted),
        Span::raw(GAP),
        Span::styled(words::scaled(tokens), text),
        Span::raw(" "),
        Span::styled(if tokens == 1 { "token" } else { "tokens" }, muted),
        Span::raw(GAP),
        Span::styled(amounts::grouped(total.responses), text),
        Span::raw(" "),
        Span::styled(
            if total.responses == 1 {
                "response"
            } else {
                "responses"
            },
            muted,
        ),
    ];
    vec![Line::from(fit_spans(spans, width))]
}
