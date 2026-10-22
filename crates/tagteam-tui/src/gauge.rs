//! The bars the Accounts and Tokens screens draw (spec §4.4, T3-17): the lead card's lead bar,
//! the table's line gauge and the card's sparkline; the Tokens headline's block progress line
//! and its sparkline of cost per day; the Tokens table's share line.

use ratatui::text::Span;
use tagteam_core::Sample;
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

/// The cell `expected` falls in, the last one at most (T3-17).
fn pace_cell(expected: Option<f64>, cells: usize) -> Option<usize> {
    let expected = expected.filter(|e| !e.is_nan())?;
    let cell = filled(expected, cells).floor() as usize;
    cell.min(cells.checked_sub(1)?).into()
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
/// fill's role on a faint `─` track (tier 0 `·`), the pace mark `┿` inside the whole cells and a
/// muted `┼` elsewhere.
pub(crate) fn line_gauge(
    pct: f64,
    expected: Option<f64>,
    cells: usize,
    theme: Theme,
) -> Vec<Span<'static>> {
    gauge(pct, expected, fill_role(pct), cells, theme)
}

/// The block's progress (T5-13): `━` whole cells and a `╸` half in accent on a faint `─` track
/// (tier 0 `·`), `cells` wide.
pub(crate) fn progress_line(pct: f64, cells: usize, theme: Theme) -> Vec<Span<'static>> {
    gauge(pct, None, Role::Accent, cells, theme)
}

/// Tokens' share line (§4.4, T5-17): `━` whole cells and a `╸` half, at least `╸` above 0.
pub(crate) fn share_line(pct: f64, cells: usize) -> String {
    let x = filled(pct, cells);
    let whole = x.floor() as usize;
    let half = x - x.floor() >= 0.5 || (whole == 0 && x > 0.0);
    format!("{}{}", "━".repeat(whole), if half { "╸" } else { "" })
}

/// `line_gauge`'s drawing with its fill in `fill`.
fn gauge(
    pct: f64,
    expected: Option<f64>,
    fill: Role,
    cells: usize,
    theme: Theme,
) -> Vec<Span<'static>> {
    let x = filled(pct, cells);
    let whole = x.floor() as usize;
    let half = x - x.floor() >= 0.5;
    let track = if theme.colourless() { '·' } else { '─' };
    let pace = pace_cell(expected, cells);
    let drawn = (0..cells)
        .map(|i| match (Some(i) == pace, i < whole) {
            (true, true) => ('┿', fill),
            (true, false) => ('┼', Role::Muted),
            (false, true) => ('━', fill),
            (false, false) if i == whole && half => ('╸', fill),
            (false, false) => (track, Role::Faint),
        })
        .collect();
    spans(drawn, theme)
}

/// The lead card's bar (§4.4): `▆` in the fill's role on a faint `▁` baseline, the pace mark `│`
/// in text inside the fill and muted outside it.
pub(crate) fn lead_bar(
    pct: f64,
    expected: Option<f64>,
    cells: usize,
    theme: Theme,
) -> Vec<Span<'static>> {
    let fill = fill_role(pct);
    let whole = filled(pct, cells).round() as usize;
    let pace = pace_cell(expected, cells);
    let drawn = (0..cells)
        .map(|i| match (Some(i) == pace, i < whole) {
            (true, true) => ('│', Role::Text),
            (true, false) => ('│', Role::Muted),
            (false, true) => ('▆', fill),
            (false, false) => ('▁', Role::Faint),
        })
        .collect();
    spans(drawn, theme)
}

/// The card's sparkline (§4.4): `cells` equal buckets over `[from_s, to_s)`, each its samples'
/// peak on `▁…█` in accent; an empty bucket is a space, and a sample outside the range is left
/// out.
pub(crate) fn sparkline(
    samples: &[Sample],
    from_s: i64,
    to_s: i64,
    cells: usize,
    theme: Theme,
) -> Span<'static> {
    let span = (i128::from(to_s) - i128::from(from_s)).max(1);
    let mut peaks: Vec<Option<f64>> = vec![None; cells];
    for s in samples {
        if s.fetched_at < from_s || s.fetched_at >= to_s {
            continue;
        }
        let i = ((i128::from(s.fetched_at) - i128::from(from_s)) * cells as i128 / span) as usize;
        if let Some(slot) = peaks.get_mut(i) {
            *slot = Some(slot.map_or(s.pct, |p| p.max(s.pct)));
        }
    }
    let text: String = peaks
        .into_iter()
        .map(|peak| match peak {
            Some(pct) => SPARK[filled(pct, 7).round() as usize],
            None => ' ',
        })
        .collect();
    Span::styled(text, theme.role(Role::Accent))
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
                let (text, _) = drawn(line_gauge(pct, None, 9, theme), theme);
                assert_eq!(text, want, "{pct} in {theme:?}");
            }
            // The fill's cells, the half cell included, then the track's.
            for (pct, role, fill) in [
                (54.0, Role::Accent, 5),
                (75.0, Role::Warn, 7),
                (95.0, Role::Crit, 9),
            ] {
                assert_eq!(fill_role(pct), role, "{pct}");
                let (_, letters) = drawn(line_gauge(pct, None, 9, theme), theme);
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
    fn a_line_gauge_s_pace_mark_is_inside_or_outside_the_fill() {
        for theme in TIERS {
            let track = if theme.colourless() { "·" } else { "─" };
            let (text, letters) = drawn(line_gauge(54.0, Some(62.0), 9, theme), theme);
            assert_eq!(text, format!("━━━━╸┼{}", track.repeat(3)), "{theme:?}");
            assert_eq!(letters, canon(theme, "aaaaamfff"), "{theme:?}");

            let (text, letters) = drawn(line_gauge(31.0, Some(24.0), 14, theme), theme);
            assert_eq!(text, format!("━━━┿{}", track.repeat(10)), "{theme:?}");
            assert_eq!(letters, canon(theme, "aaaaffffffffff"), "{theme:?}");

            let (text, letters) = drawn(line_gauge(6.0, Some(10.0), 14, theme), theme);
            assert_eq!(text, format!("╸┼{}", track.repeat(12)), "{theme:?}");
            assert_eq!(letters, canon(theme, "amffffffffffff"), "{theme:?}");

            // On the half cell the mark takes its place.
            let (text, letters) = drawn(line_gauge(54.0, Some(50.0), 9, theme), theme);
            assert_eq!(text, format!("━━━━┼{}", track.repeat(4)), "{theme:?}");
            assert_eq!(letters, canon(theme, "aaaamffff"), "{theme:?}");
        }
    }

    #[test]
    fn a_lead_bar_fills_on_a_baseline_with_its_pace_mark() {
        for theme in TIERS {
            let (text, letters) = drawn(lead_bar(53.0, None, 52, theme), theme);
            assert_eq!(text, format!("{}{}", "▆".repeat(28), "▁".repeat(24)));
            assert_eq!(
                letters,
                canon(theme, &format!("{}{}", "a".repeat(28), "f".repeat(24))),
                "{theme:?}"
            );

            let (text, letters) = drawn(lead_bar(6.0, Some(10.0), 52, theme), theme);
            assert_eq!(text, format!("▆▆▆▁▁│{}", "▁".repeat(46)));
            assert_eq!(
                letters,
                canon(theme, &format!("aaaffm{}", "f".repeat(46))),
                "{theme:?}"
            );

            let (text, letters) = drawn(lead_bar(53.0, Some(30.0), 52, theme), theme);
            assert_eq!(text.chars().position(|c| c == '│'), Some(15));
            assert_eq!(text.chars().count(), 52);
            assert_eq!(
                letters,
                canon(
                    theme,
                    &format!("{}t{}{}", "a".repeat(15), "a".repeat(12), "f".repeat(24))
                ),
                "{theme:?}"
            );
        }
    }

    #[test]
    fn a_sparkline_shows_each_bucket_s_peak_and_leaves_an_empty_one_blank() {
        let at = |fetched_at, pct| Sample {
            fetched_at,
            pct,
            resets_at: None,
        };
        let samples = [at(10, 0.0), at(110, 50.0), at(120, 100.0), at(390, 20.0)];
        let outside = [at(-1, 90.0), at(400, 90.0)];
        let peak_first = [at(110, 100.0), at(120, 50.0)];
        for theme in TIERS {
            let (text, letters) = drawn(vec![sparkline(&samples, 0, 400, 4, theme)], theme);
            assert_eq!(text, "▁█ ▂", "{theme:?}");
            assert_eq!(letters, canon(theme, "aa a"), "{theme:?}");

            let all: Vec<Sample> = samples.iter().chain(&outside).copied().collect();
            let (text, _) = drawn(vec![sparkline(&all, 0, 400, 4, theme)], theme);
            assert_eq!(text, "▁█ ▂", "{theme:?}");

            let (text, _) = drawn(vec![sparkline(&[], 0, 400, 4, theme)], theme);
            assert_eq!(text, "    ", "{theme:?}");

            let (text, _) = drawn(vec![sparkline(&peak_first, 0, 400, 4, theme)], theme);
            assert_eq!(text, " █  ", "{theme:?}");
        }
    }

    #[test]
    fn the_gauges_draw_edge_inputs_exactly_their_cells_wide() {
        let samples = [Sample {
            fetched_at: 10,
            pct: 60.0,
            resets_at: None,
        }];
        for theme in TIERS {
            let track = if theme.colourless() { "·" } else { "─" };
            let line =
                |pct, expected, cells| drawn(line_gauge(pct, expected, cells, theme), theme).0;
            let lead = |pct, expected, cells| drawn(lead_bar(pct, expected, cells, theme), theme).0;
            let spark = |from_s, to_s, cells| {
                drawn(vec![sparkline(&samples, from_s, to_s, cells, theme)], theme).0
            };
            let cases = [
                (line(54.0, Some(50.0), 0), "", 0),
                (lead(54.0, Some(50.0), 0), "", 0),
                (spark(0, 400, 0), "", 0),
                (line(54.0, Some(50.0), 1), "┼", 1),
                (lead(54.0, Some(50.0), 1), "│", 1),
                (spark(0, 400, 1), "▅", 1),
                (line(-20.0, None, 9), "─────────", 9),
                (lead(-20.0, None, 9), "▁▁▁▁▁▁▁▁▁", 9),
                (line(54.0, Some(150.0), 9), "━━━━╸───┼", 9),
                (lead(54.0, Some(150.0), 9), "▆▆▆▆▆▁▁▁│", 9),
                (line(54.0, Some(-10.0), 9), "┿━━━╸────", 9),
                (lead(54.0, Some(-10.0), 9), "│▆▆▆▆▁▁▁▁", 9),
                (line(54.0, Some(f64::NAN), 9), "━━━━╸────", 9),
                (lead(54.0, Some(f64::NAN), 9), "▆▆▆▆▆▁▁▁▁", 9),
                (spark(400, 400, 4), "    ", 4),
                (spark(400, 0, 4), "    ", 4),
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
