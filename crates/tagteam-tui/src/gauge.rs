//! The bars the Accounts and Tokens screens draw (spec §4.4, T3-17): the thick bar of the
//! account in use, the table's line gauge; the Tokens headline's block progress line and its
//! sparkline of cost per day; the Tokens table's share line.

use ratatui::text::Span;
use tagteam_engine::words::{self, Severity};

use crate::theme::{Role, Theme};

const SPARK: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// The fill's role for `pct` (T3-18): accent, warn from 70, crit from 90.
pub(crate) fn fill_role(pct: f64) -> Role {
    match words::severity(pct.round() as i64) {
        Severity::Normal => Role::Accent,
        Severity::Warning => Role::Warn,
        Severity::Critical => Role::Crit,
    }
}

/// `pct` of `cells` cells, clamped to the bar; a NaN percentage is 0 (T3-17).
fn filled(pct: f64, cells: usize) -> f64 {
    let pct = if pct.is_nan() {
        0.0
    } else {
        pct.clamp(0.0, 100.0)
    };
    pct / 100.0 * cells as f64
}

/// One span per run of cells in the same role.
fn spans(cells: Vec<(char, Role)>, theme: Theme) -> Vec<Span<'static>> {
    let mut runs: Vec<(String, Role)> = Vec::new();
    for (glyph, role) in cells {
        match runs.last_mut() {
            Some((text, last)) if *last == role => text.push(glyph),
            _ => runs.push((glyph.to_string(), role)),
        }
    }
    runs.into_iter()
        .map(|(text, role)| Span::styled(text, theme.role(role)))
        .collect()
}

/// The table's and the Now strip's gauge (§4.4): `━` whole cells and a `╸` half cell in the
/// fill's role on a faint `─` track (tier 0 `·`).
pub(crate) fn line_gauge(pct: f64, cells: usize, theme: Theme) -> Vec<Span<'static>> {
    gauge(pct, fill_role(pct), cells, theme)
}

/// The block's progress (T5-13): `━` whole cells and a `╸` half in accent on a faint `─` track
/// (tier 0 `·`), `cells` wide.
pub(crate) fn progress_line(pct: f64, cells: usize, theme: Theme) -> Vec<Span<'static>> {
    gauge(pct, Role::Accent, cells, theme)
}

/// Tokens' share line (§4.4, T5-17): `━` whole cells and a `╸` half, at least `╸` above 0.
pub(crate) fn share_line(pct: f64, cells: usize) -> String {
    let x = filled(pct, cells);
    let whole = x.floor() as usize;
    let half = x - x.floor() >= 0.5 || (whole == 0 && x > 0.0);
    format!("{}{}", "━".repeat(whole), if half { "╸" } else { "" })
}

/// `line_gauge`'s drawing with its fill in `fill`.
fn gauge(pct: f64, fill: Role, cells: usize, theme: Theme) -> Vec<Span<'static>> {
    let x = filled(pct, cells);
    let whole = x.floor() as usize;
    let half = x - x.floor() >= 0.5;
    let track = if theme.colourless() { '·' } else { '─' };
    let drawn = (0..cells)
        .map(|i| match (i < whole, i == whole && half) {
            (true, _) => ('━', fill),
            (false, true) => ('╸', fill),
            (false, false) => (track, Role::Faint),
        })
        .collect();
    spans(drawn, theme)
}

/// The thick bar of the account in use in its card (§4.4): `▆` in the fill's role on a faint `▁` baseline.
pub(crate) fn lead_bar(pct: f64, cells: usize, theme: Theme) -> Vec<Span<'static>> {
    let fill = fill_role(pct);
    let whole = filled(pct, cells).round() as usize;
    let drawn = (0..cells)
        .map(|i| {
            if i < whole {
                ('▆', fill)
            } else {
                ('▁', Role::Faint)
            }
        })
        .collect();
    spans(drawn, theme)
}

/// T5-12's sparkline: each value on `▁…█` against the largest (`▁` when it is 0), a space for
/// none, in accent.
pub(crate) fn sparkline_of(values: &[Option<u64>], theme: Theme) -> Span<'static> {
    let largest = values.iter().flatten().copied().max().unwrap_or(0);
    let text: String = values
        .iter()
        .map(|value| match value {
            Some(_) if largest == 0 => SPARK[0],
            Some(v) => SPARK[filled(*v as f64 / largest as f64 * 100.0, 7).round() as usize],
            None => ' ',
        })
        .collect();
    Span::styled(text, theme.role(Role::Accent))
}

#[cfg(test)]
mod tests {
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::text::Line;
    use ratatui::widgets::{Paragraph, Widget};

    use super::*;
    use crate::testutil::{TIERS, canon, roles};

    /// `spans` as text and as `roles`' letters, drawn in a one-row buffer as wide as they are.
    fn drawn(spans: Vec<Span<'static>>, theme: Theme) -> (String, String) {
        let line = Line::from(spans);
        let text = line.to_string();
        let mut buf = Buffer::empty(Rect::new(0, 0, line.width() as u16, 1));
        Paragraph::new(line).render(buf.area, &mut buf);
        (text, roles(&buf, theme, 0))
    }

    fn letter(role: Role) -> char {
        match role {
            Role::Accent => 'a',
            Role::Warn => 'w',
            Role::Crit => 'c',
            other => panic!("{other:?} is no fill"),
        }
    }

    #[test]
    fn a_line_gauge_fills_whole_and_half_cells_on_a_track() {
        for theme in TIERS {
            let track = if theme.colourless() { "·" } else { "─" };
            let cases = [
                (54.0, format!("━━━━╸{}", track.repeat(4))),
                (0.0, track.repeat(9)),
                (100.0, "━".repeat(9)),
                (150.0, "━".repeat(9)),
                (f64::NAN, track.repeat(9)),
            ];
            for (pct, want) in cases {
                let (text, _) = drawn(line_gauge(pct, 9, theme), theme);
                assert_eq!(text, want, "{pct} in {theme:?}");
            }
            // The fill's cells, the half cell included, then the track's.
            for (pct, role, fill) in [
                (54.0, Role::Accent, 5),
                (75.0, Role::Warn, 7),
                (95.0, Role::Crit, 9),
            ] {
                assert_eq!(fill_role(pct), role, "{pct}");
                let (_, letters) = drawn(line_gauge(pct, 9, theme), theme);
                let want = format!(
                    "{}{}",
                    letter(role).to_string().repeat(fill),
                    "f".repeat(9 - fill)
                );
                assert_eq!(letters, canon(theme, &want), "{pct} in {theme:?}");
            }
        }
    }

    #[test]
    fn a_line_gauge_draws_no_pace_mark() {
        for theme in TIERS {
            let track = if theme.colourless() { "·" } else { "─" };
            let (text, _) = drawn(line_gauge(54.0, 9, theme), theme);
            assert_eq!(text, format!("━━━━╸{}", track.repeat(4)), "{theme:?}");
            assert!(!text.contains(['┿', '┼', '│']), "{text}");
        }
    }

    #[test]
    fn a_lead_bar_draws_no_pace_mark() {
        for theme in TIERS {
            let (text, letters) = drawn(lead_bar(53.0, 52, theme), theme);
            assert_eq!(text, format!("{}{}", "▆".repeat(28), "▁".repeat(24)));
            assert!(!text.contains(['┿', '┼', '│']), "{text}");
            assert_eq!(
                letters,
                canon(theme, &format!("{}{}", "a".repeat(28), "f".repeat(24))),
                "{theme:?}"
            );
        }
    }

    #[test]
    fn the_gauges_draw_edge_inputs_exactly_their_cells_wide() {
        for theme in TIERS {
            let track = if theme.colourless() { "·" } else { "─" };
            let line = |pct, cells| drawn(line_gauge(pct, cells, theme), theme).0;
            let lead = |pct, cells| drawn(lead_bar(pct, cells, theme), theme).0;
            let cases = [
                (line(54.0, 0), "", 0),
                (lead(54.0, 0), "", 0),
                (line(54.0, 1), "╸", 1),
                (lead(54.0, 1), "▆", 1),
                (line(54.0, 9), "━━━━╸────", 9),
                (lead(54.0, 9), "▆▆▆▆▆▁▁▁▁", 9),
                (line(-20.0, 9), "─────────", 9),
                (lead(-20.0, 9), "▁▁▁▁▁▁▁▁▁", 9),
            ];
            for (n, (text, want, cells)) in cases.into_iter().enumerate() {
                assert_eq!(text, want.replace('─', track), "case {n} in {theme:?}");
                assert_eq!(text.chars().count(), cells, "case {n} in {theme:?}");
            }
        }
    }

    #[test]
    fn the_progress_line_fills_its_share_in_accent_on_a_track() {
        for theme in TIERS {
            let track = if theme.colourless() { "·" } else { "─" };
            let (text, letters) = drawn(progress_line(44.44, 22, theme), theme);
            assert_eq!(
                text,
                format!("{}╸{}", "━".repeat(9), track.repeat(12)),
                "{theme:?}"
            );
            let want = format!("{}{}", "a".repeat(10), "f".repeat(12));
            assert_eq!(letters, canon(theme, &want), "{theme:?}");

            let (text, letters) = drawn(progress_line(44.44, 14, theme), theme);
            assert_eq!(
                text,
                format!("{}{}", "━".repeat(6), track.repeat(8)),
                "{theme:?}"
            );
            let want = format!("{}{}", "a".repeat(6), "f".repeat(8));
            assert_eq!(letters, canon(theme, &want), "{theme:?}");

            // Accent whatever the share: a block's progress is no severity.
            let (_, letters) = drawn(progress_line(95.0, 10, theme), theme);
            assert_eq!(letters, canon(theme, "aaaaaaaaaa"), "{theme:?}");
        }
    }

    #[test]
    fn a_share_line_is_its_share_of_eight_cells() {
        for (pct, want) in [(100.0, "━━━━━━━━"), (23.67, "━╸"), (1.0, "╸"), (0.0, "")]
        {
            assert_eq!(share_line(pct, 8), want, "{pct}");
        }
    }

    #[test]
    fn a_sparkline_of_values_scales_to_the_largest() {
        for theme in TIERS {
            let values = [None, Some(0), Some(2), Some(4), Some(8)];
            let (text, letters) = drawn(vec![sparkline_of(&values, theme)], theme);
            assert_eq!(text, " ▁▃▅█", "{theme:?}");
            assert_eq!(letters, canon(theme, " aaaa"), "{theme:?}");

            let (text, _) = drawn(vec![sparkline_of(&[Some(0); 3], theme)], theme);
            assert_eq!(text, "▁▁▁", "{theme:?}");
            let (text, _) = drawn(vec![sparkline_of(&[None, None], theme)], theme);
            assert_eq!(text, "  ", "{theme:?}");
        }
    }
}
