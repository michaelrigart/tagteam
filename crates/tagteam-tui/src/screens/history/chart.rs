//! History's charts (T4-9, T4-11, §4.4, §7.3): eighths stacked over the chart's rows in each
//! column's own severity, the threshold's line and the cursor through the empty cells, a muted
//! y axis in the gutter, and the title row with its readouts.

use ratatui::text::{Line, Span};
use tagteam_core::Window;
use tagteam_core::autoswitch::AutoStrategy;
use tagteam_core::usage::{is_relevant, max_relevant_pct};
use tagteam_engine::timeline::{Scope, Series};
use tagteam_engine::views::{AccountView, ProviderAccounts};
use tagteam_engine::words;

use super::GAP;
use crate::chrome::{WIDE, fit_spans};
use crate::daemon;
use crate::gauge::fill_role;
use crate::keys::RIGHT_INSET;
use crate::state::State;
use crate::theme::{Role, Theme};
use crate::view::width_of;

/// A cell's eighths below a full block (§4.4).
const EIGHTHS: [&str; 7] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇"];
const FULL: &str = "█";
/// The threshold's line, and tier 0's in its place (§4.2).
const LINE: &str = "┈";
const LINE_PLAIN: &str = "·";
/// The cursor through a chart's empty cells (§7.3).
const CURSOR: &str = "│";
/// One account's charts draw their line at 90 % (§7.3).
pub(crate) const ONE_ACCOUNT_LINE: f64 = 90.0;
/// The title row's readout labels (§7.3).
const AT_CURSOR: &str = "at cursor";
const NOW: &str = "now";

/// T4-9: `peaks` over `rows` rows, the top first, each under the y axis's gutter: a value's
/// eighths in `fill_role`, and in an empty cell the cursor's `│` in column `cursor`, else the
/// line at `line_pct` on its row.
pub(crate) fn chart_rows(
    peaks: &[Option<f64>],
    rows: u16,
    line_pct: f64,
    cursor: usize,
    theme: Theme,
) -> Vec<Line<'static>> {
    let n = usize::from(rows);
    if n == 0 {
        return vec![];
    }
    let eighths: Vec<Option<usize>> = peaks.iter().map(|p| p.map(|v| eighths(v, n))).collect();
    let line = ((line_pct / 100.0 * n as f64).floor().max(0.0) as usize).min(n - 1);
    let half = (n >= 6).then_some((n / 2).min(n - 1));
    let (muted, faint, accent) = (
        theme.role(Role::Muted),
        theme.role(Role::Faint),
        theme.role(Role::Accent),
    );
    let line_glyph = if theme.colourless() { LINE_PLAIN } else { LINE };
    (0..n)
        .rev()
        .map(|r| {
            let label = match r {
                _ if r == n - 1 => "100",
                0 => "0",
                _ if Some(r) == half => "50",
                _ => "",
            };
            let mut spans = vec![Span::styled(format!("{label:>7}"), muted), Span::raw("  ")];
            for (c, (e, v)) in eighths.iter().zip(peaks).enumerate() {
                spans.push(match (e.and_then(|e| cell(e, r)), v) {
                    (Some(glyph), Some(v)) => Span::styled(glyph, theme.role(fill_role(*v))),
                    _ if c == cursor => Span::styled(CURSOR, accent),
                    _ if r == line => Span::styled(line_glyph, faint),
                    _ => Span::raw(" "),
                });
            }
            Line::from(spans)
        })
        .collect()
}

/// `v`'s eighths over `n` rows, at least one when it is there: a present 0 % is not an empty
/// bucket (T4-9).
fn eighths(v: f64, n: usize) -> usize {
    let v = if v.is_nan() { 0.0 } else { v.clamp(0.0, 100.0) };
    ((v / 100.0 * (n * 8) as f64).round() as usize).max(1)
}

/// Row `r`'s glyph (0 at the bottom) of a column `e` eighths tall; `None` above it.
fn cell(e: usize, r: usize) -> Option<&'static str> {
    if e >= 8 * (r + 1) {
        Some(FULL)
    } else if e > 8 * r {
        Some(EIGHTHS[e - 8 * r - 1])
    } else {
        None
    }
}

/// T4-11: ` {label}` in bold, from `WIDE` columns the in-use chart's subtitle, and on the right,
/// one column in and never cut, `at cursor` and `now` with their values.
pub(crate) fn title_row(state: &State, series: &Series, all: bool, width: u16) -> Line<'static> {
    let theme = state.theme;
    let muted = theme.role(Role::Muted);
    let provider = state.view_of(&daemon::focus(state));
    let at_cursor = state
        .history
        .view
        .as_ref()
        .and_then(|v| series.peaks.get(super::cursor_column(v)).copied().flatten());
    let now = provider.and_then(|p| now_pct(state, p, series, all));
    let value = |v: Option<f64>| match v {
        Some(v) => Span::styled(
            format!("{:>4}", format!("{}%", v.round() as i64)),
            theme.role(fill_role(v)),
        ),
        None => Span::styled(format!("{:>4}", words::MISSING), muted),
    };
    let right = vec![
        Span::styled(AT_CURSOR, muted),
        Span::raw(" "),
        value(at_cursor),
        Span::raw(GAP),
        Span::styled(NOW, muted),
        Span::raw(" "),
        value(now),
    ];
    let mut left = vec![
        Span::raw(" "),
        Span::styled(series.label.clone(), theme.strong()),
    ];
    let threshold = state.history.view.as_ref().map(|v| v.threshold);
    let subtitle = provider
        .filter(|_| all && width >= WIDE)
        .and_then(|p| subtitle(p, threshold));
    if let Some(subtitle) = subtitle {
        left.push(Span::raw(GAP));
        left.push(Span::styled(subtitle, muted));
    }
    let used = |spans: &[Span]| spans.iter().map(|s| width_of(&s.content)).sum::<usize>();
    let room = usize::from(width).saturating_sub(used(&right) + RIGHT_INSET + width_of(GAP));
    let mut spans = fit_spans(left, room);
    let gap = usize::from(width).saturating_sub(used(&spans) + used(&right) + RIGHT_INSET);
    spans.push(Span::raw(" ".repeat(gap)));
    spans.extend(right);
    Line::from(spans)
}

/// The account in use on focus's provider.
fn in_use(p: &ProviderAccounts) -> Option<&AccountView> {
    p.accounts.iter().find(|a| a.active)
}

/// The account's current windows.
fn windows(a: &AccountView) -> Vec<Window> {
    a.usage
        .windows
        .iter()
        .flatten()
        .map(|(w, _)| w.clone())
        .collect()
}

/// `now` (T4-11): the in-use account's highest relevant window on the in-use chart; on one
/// account's chart, its window of the chart's label.
fn now_pct(state: &State, p: &ProviderAccounts, series: &Series, all: bool) -> Option<f64> {
    if all {
        return max_relevant_pct(&windows(in_use(p)?), &p.models);
    }
    let Scope::Account(id) = &state.history.scope else {
        return None;
    };
    let account = p.accounts.iter().find(|a| &a.row.id == id)?;
    windows(account)
        .into_iter()
        .find(|w| w.label == series.label)
        .map(|w| w.pct)
}

/// The in-use chart's subtitle: what it charts of the in-use account's relevant windows, then,
/// unless the strategy is `consume-first`, what auto compares that with (T4-11, C-17).
fn subtitle(p: &ProviderAccounts, threshold: Option<f64>) -> Option<String> {
    let labels: Vec<String> = windows(in_use(p)?)
        .into_iter()
        .filter(|w| is_relevant(w, &p.models))
        .map(|w| w.label)
        .collect();
    let what = match labels.as_slice() {
        [] => return None,
        [one] => format!("its {one}"),
        [a, b] => format!("the higher of its {a} and {b}"),
        [rest @ .., last] => format!("the highest of its {} and {last}", rest.join(", ")),
    };
    Some(match threshold {
        Some(t) if p.strategy != AutoStrategy::ConsumeFirst => {
            format!("{what}: what auto compares with {}", words::threshold(t))
        }
        _ => what,
    })
}

#[cfg(test)]
mod tests {
    use ratatui::style::Modifier;
    use tagteam_core::autoswitch::AutoStrategy;
    use tagteam_engine::timeline::{IN_USE, Series};

    use super::*;
    use crate::screens::history::buffer_of;
    use crate::state::Screen;
    use crate::testutil::*;
    use crate::view::width_of;

    #[test]
    fn a_chart_stacks_eighths_in_its_value_s_severity_with_the_threshold_and_the_cursor() {
        let peaks = [
            Some(100.0),
            Some(50.0),
            Some(0.0),
            None,
            Some(95.0),
            Some(75.0),
        ];
        let theme = Theme::new(true);
        let lines = chart_rows(&peaks, 9, 90.0, 5, theme);
        assert_eq!(lines.len(), 9);
        let buf = buffer_of(lines, 15);
        let text = text_of(&buf);
        assert_eq!(text[0], "    100  █┈┈┈▄│");
        assert_eq!(text[4], "     50  █▄  ██");
        assert_eq!(text[8], "      0  ██▁ ██");
        assert_eq!(&roles(&buf, theme, 0)[..9], canon(theme, "    mmm  "));
        assert_eq!(&roles(&buf, theme, 0)[9..], canon(theme, "cfffca"));
        assert_eq!(&roles(&buf, theme, 4)[9..], canon(theme, "ca  cw"));
        assert_eq!(&roles(&buf, theme, 8)[9..], canon(theme, "caa cw"));
        let plain = text_of(&buffer_of(
            chart_rows(&peaks, 9, 90.0, 5, Theme::new(false)),
            15,
        ));
        assert_eq!(plain[0], "    100  █···▄│");
    }

    #[test]
    fn a_value_past_100_is_a_full_column_and_nan_reads_as_0() {
        let theme = Theme::new(true);
        let peaks = [Some(150.0), Some(f64::NAN)];
        let buf = buffer_of(chart_rows(&peaks, 9, 90.0, 5, theme), 11);
        let text = text_of(&buf);
        assert_eq!(text[0], "    100  █┈");
        for (y, row) in text.iter().enumerate().take(8).skip(1) {
            let want = if y == 4 {
                "     50  █"
            } else {
                "         █"
            };
            assert_eq!(row, want, "row {y}");
        }
        assert_eq!(text[8], "      0  █▁");
        for y in 0..8 {
            assert_eq!(&roles(&buf, theme, y)[9..10], canon(theme, "c"), "row {y}");
        }
        assert_eq!(&roles(&buf, theme, 8)[9..], canon(theme, "ca"));
    }

    #[test]
    fn the_y_axis_labels_100_50_from_6_rows_and_0() {
        let gutters = |rows| -> Vec<String> {
            chart_rows(&[None], rows, 90.0, 1, Theme::new(false))
                .iter()
                .map(|l| l.to_string().chars().take(9).collect())
                .collect()
        };
        let nine = gutters(9);
        assert_eq!(nine.len(), 9);
        for (i, g) in nine.iter().enumerate() {
            let want = match i {
                0 => "    100  ",
                4 => "     50  ",
                8 => "      0  ",
                _ => "         ",
            };
            assert_eq!(g, want, "row {i}");
        }
        assert_eq!(gutters(6)[2], "     50  ", "row 3 from the bottom");
        let five = gutters(5);
        assert_eq!(
            five,
            [
                "    100  ",
                "         ",
                "         ",
                "         ",
                "      0  "
            ]
        );
    }

    /// History at 110 columns, account 2 in use at 53 % and 6 % when `in_use`, under
    /// `strategy`: the in-use chart's last column 53.
    fn in_use_state(strategy: AutoStrategy, in_use: bool) -> (State, Series) {
        let mut accounts = many(3);
        accounts[1].usage = read(60, 53.0, 6.0, false);
        accounts[1].active = in_use;
        let mut snap = snapshot(accounts);
        snap.providers[0].strategy = strategy;
        let mut state = fresh(Screen::History);
        state.theme = Theme::new(true);
        state.accounts.apply(snap);
        let mut peaks = vec![None; 110];
        peaks[109] = Some(53.0);
        let series = Series {
            label: IN_USE.into(),
            peaks,
        };
        state.history.view = Some(timeline_view(110, vec![series.clone()], vec![]));
        (state, series)
    }

    /// The row's role letters: each part's characters in its letter, spaces as spaces.
    fn letters(parts: &[(&str, char)]) -> String {
        parts
            .iter()
            .flat_map(|(text, role)| {
                text.chars()
                    .map(move |c| if c == ' ' { ' ' } else { *role })
            })
            .collect()
    }

    #[test]
    fn the_in_use_chart_s_title_names_its_windows_and_the_threshold_from_120_columns() {
        let subtitle = "the higher of its 5h and 7d: what auto compares with 90%";
        let readouts = "at cursor  53%   now  53%";
        let row = |left: &str, right: &str, width: usize| {
            format!(
                "{left}{}{right}",
                " ".repeat(width - 1 - width_of(left) - width_of(right))
            )
        };
        let (state, series) = in_use_state(AutoStrategy::Best, true);
        let theme = state.theme;
        let buf = buffer_of(vec![title_row(&state, &series, true, 120)], 120);
        let left = format!(" in use   {subtitle}");
        assert_eq!(text_of(&buf)[0], row(&left, readouts, 120));
        let right = letters(&[
            ("at cursor", 'm'),
            ("  ", 't'),
            ("53%", 'a'),
            ("   ", 't'),
            ("now", 'm'),
            ("  ", 't'),
            ("53%", 'a'),
        ]);
        let left = letters(&[(" ", 't'), ("in use", 't'), ("   ", 't'), (subtitle, 'm')]);
        assert_eq!(
            roles(&buf, theme, 0),
            canon(theme, &format!("{} ", row(&left, &right, 120)))
        );
        let bold = |x: u16| buf[(x, 0)].modifier.contains(Modifier::BOLD);
        assert!((1..7).all(bold), "in use is bold");
        assert!(!bold(0) && !bold(10), "the rest is not");

        let buf = buffer_of(vec![title_row(&state, &series, true, 80)], 80);
        assert_eq!(text_of(&buf)[0], row(" in use", readouts, 80));

        let (state, series) = in_use_state(AutoStrategy::ConsumeFirst, true);
        let buf = buffer_of(vec![title_row(&state, &series, true, 120)], 120);
        assert_eq!(
            text_of(&buf)[0],
            row(" in use   the higher of its 5h and 7d", readouts, 120)
        );

        let (state, series) = in_use_state(AutoStrategy::Best, false);
        let buf = buffer_of(vec![title_row(&state, &series, true, 80)], 80);
        assert_eq!(
            text_of(&buf)[0],
            row(" in use", "at cursor  53%   now    —", 80)
        );
    }

    #[test]
    fn the_subtitle_names_one_window_or_three() {
        let (mut state, series) = in_use_state(AutoStrategy::Best, true);
        let title = |state: &State| {
            text_of(&buffer_of(vec![title_row(state, &series, true, 160)], 160))[0].clone()
        };
        let with = |state: &mut State, windows: &[(&str, tagteam_core::WindowKind)]| {
            let snap = state.accounts.snapshot().unwrap().clone();
            let mut snap = snap;
            let usage = snap.providers[0].accounts[1]
                .usage
                .windows
                .as_mut()
                .unwrap();
            usage.clear();
            for (label, kind) in windows {
                let w = window(label, label, *kind, 10.0, None);
                usage.push((w, tagteam_core::Pace::default()));
            }
            snap.providers[0].models = vec!["fable".into()];
            state.accounts.apply(snap);
        };
        use tagteam_core::WindowKind::{Long, Scoped, Short, Spend};
        with(&mut state, &[("5h", Short), ("Spend", Spend)]);
        assert!(
            title(&state).starts_with(" in use   its 5h: what auto compares with 90%"),
            "{}",
            title(&state)
        );
        with(
            &mut state,
            &[("5h", Short), ("7d", Long), ("Fable", Scoped)],
        );
        assert!(
            title(&state)
                .starts_with(" in use   the highest of its 5h, 7d and Fable: what auto compares"),
            "{}",
            title(&state)
        );
    }
}
