//! History's events lane, its swimlanes, its in-use lane and its day axis (T4-7, T4-10,
//! T4-12, T4-13, §7.4).

use jiff::Timestamp;
use jiff::civil::{Date, Weekday};
use jiff::tz::TimeZone;
use ratatui::text::{Line, Span};
use tagteam_engine::EventKind;
use tagteam_engine::timeline::{Grid, Lane, TimelineView};
use tagteam_engine::words;

use super::layout::CHART_X;
use super::marks::{Mark, mark};
use crate::gauge::fill_role;
use crate::theme::{Role, Theme};
use crate::view::pad;

/// A swimlane's levels, low to high (§4.4).
const LEVELS: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
/// Where an account was not in use, and the in-use lane where it was (§7.4).
const TRACK: &str = "─";
const IN_USE: &str = "━";
/// The cursor through a lane's empty cells and where the account was not in use (§7.1).
const CURSOR: &str = "│";
/// A pause's span until the next resume (§7.4), and tier 0's in its place.
const PAUSED: &str = "┄";
const PAUSED_PLAIN: &str = "·";
/// A lane's name in the gutter, after the marker column and before a space (T4-10).
const NAME: usize = 7;
/// Each axis label's tick, and the axis's last word (§7.4).
const TICK: &str = "╵";
const NOW: &str = "now";
/// T4-12: the 1-day axis marks the local hours divisible by this.
const HOURS: usize = 3;

/// T4-13: the events lane, after an empty gutter: each column's highest-rank event's glyph in
/// its role (the newest of equal rank), a pause's span `┄` in warn in each column after it and
/// before the next resume (from column 0 when the range starts paused), and in column `cursor`
/// its event's glyph `Theme::marked`, or the cursor's `│` in accent with none there.
pub(crate) fn events_row(view: &TimelineView, cursor: usize, theme: Theme) -> Line<'static> {
    let grid = view.grid();
    let mut marks: Vec<Option<Mark>> = vec![None; view.columns];
    let mut paused = vec![false; view.columns];
    // The column a pause's span starts at, while paused.
    let mut from = view.paused_at_start.then_some(0);
    for e in &view.events {
        let Some(c) = grid.column_of(grid.placed(e.at_ms)) else {
            continue;
        };
        let m = mark(e);
        if marks[c].is_none_or(|old| m.rank >= old.rank) {
            marks[c] = Some(m);
        }
        match e.kind {
            EventKind::AutoswitchPaused => {
                from.get_or_insert(c + 1);
            }
            EventKind::AutoswitchResumed => {
                if let Some(start) = from.take() {
                    paused
                        .iter_mut()
                        .take(c)
                        .skip(start)
                        .for_each(|p| *p = true);
                }
            }
            _ => {}
        }
    }
    if let Some(start) = from {
        paused.iter_mut().skip(start).for_each(|p| *p = true);
    }
    let span = if theme.colourless() {
        PAUSED_PLAIN
    } else {
        PAUSED
    };
    let (warn, accent) = (theme.role(Role::Warn), theme.role(Role::Accent));
    let mut spans = vec![Span::raw(" ".repeat(usize::from(CHART_X)))];
    spans.extend(
        marks
            .iter()
            .zip(&paused)
            .enumerate()
            .map(|(c, (m, p))| match (m, c == cursor) {
                (Some(m), true) => Span::styled(m.glyph, theme.marked(m.role)),
                (Some(m), false) => Span::styled(m.glyph, theme.role(m.role)),
                (None, true) => Span::styled(CURSOR, accent),
                (None, false) if *p => Span::styled(span, warn),
                (None, false) => Span::raw(" "),
            }),
    );
    Line::from(spans)
}

/// One account's swimlane: its name in the gutter, then each column's binding usage in
/// `fill_role` where it was in use (blank without a reading), a faint `─` where it was not, and
/// in column `cursor` the cursor's `│` in accent through either of the last two.
pub(crate) fn lane_row(lane: &Lane, cursor: usize, theme: Theme) -> Line<'static> {
    let name = pad(&words::account_name(&lane.account), NAME);
    let mut spans = vec![Span::raw(format!(" {name} "))];
    let (faint, accent) = (theme.role(Role::Faint), theme.role(Role::Accent));
    spans.extend(
        lane.peaks
            .iter()
            .zip(&lane.in_use)
            .enumerate()
            .map(|(c, (v, used))| match (used, v) {
                (true, Some(v)) => Span::styled(LEVELS[level(*v)], theme.role(fill_role(*v))),
                (true, None) if c == cursor => Span::styled(CURSOR, accent),
                (true, None) => Span::raw(" "),
                (false, _) if c == cursor => Span::styled(CURSOR, accent),
                (false, _) => Span::styled(TRACK, faint),
            }),
    );
    Line::from(spans)
}

/// `round(min(v, 100) / 100 × 7)`: a lane cell's level.
fn level(v: f64) -> usize {
    let v = if v.is_nan() { 0.0 } else { v.clamp(0.0, 100.0) };
    (v / 100.0 * 7.0).round() as usize
}

/// One account's in-use lane under its charts: `━` in accent where it was in use, a faint `─`
/// elsewhere and the cursor's `│` in accent in column `cursor`, after an empty gutter.
pub(crate) fn in_use_row(lane: &Lane, cursor: usize, theme: Theme) -> Line<'static> {
    let (accent, faint) = (theme.role(Role::Accent), theme.role(Role::Faint));
    let mut spans = vec![Span::raw(" ".repeat(usize::from(CHART_X)))];
    spans.extend(lane.in_use.iter().enumerate().map(|(c, used)| match used {
        true => Span::styled(IN_USE, accent),
        false if c == cursor => Span::styled(CURSOR, accent),
        false => Span::styled(TRACK, faint),
    }));
    Line::from(spans)
}

/// ` +{n} more · f picks one`, muted: the lanes past the cap (§7.4).
pub(crate) fn more_row(n: usize, theme: Theme) -> Line<'static> {
    Line::from(Span::styled(
        format!(" +{n} more · f picks one"),
        theme.role(Role::Muted),
    ))
}

/// T4-12: the day axis in `tz`, muted. Over 1 day `╵HH` at the local hours divisible by 3, over
/// 7 and 14 days `╵Tue` at local midnights, over 30 `╵Sep 7` at local Monday midnights, each
/// from its boundary's column; `now` ends at the last column. A label is left out unless a
/// blank column parts it from the one drawn before it and from `now`.
pub(crate) fn axis_row(grid: Grid, days: i64, tz: &TimeZone, theme: Theme) -> Line<'static> {
    let width = usize::from(CHART_X) + grid.columns;
    let mut row = vec![' '; width];
    let now_at = width.saturating_sub(NOW.len());
    let mut end = None;
    for (at_ms, label) in boundaries(grid, days, tz) {
        let Some(column) = grid.column_of(at_ms) else {
            continue;
        };
        let x = usize::from(CHART_X) + column;
        let w = label.chars().count();
        if end.is_some_and(|e| x <= e) || x + w >= now_at {
            continue;
        }
        row.splice(x..x + w, label.chars());
        end = Some(x + w);
    }
    row.splice(now_at..width, NOW.chars().take(width - now_at));
    Line::from(Span::styled(
        row.into_iter().collect::<String>(),
        theme.role(Role::Muted),
    ))
}

/// Each boundary T4-12 labels in `grid`'s range, oldest first, with its label.
fn boundaries(grid: Grid, days: i64, tz: &TimeZone) -> Vec<(i64, String)> {
    let date_of = |ms| Timestamp::from_millisecond(ms).map(|t| t.to_zoned(tz.clone()).date());
    let (Ok(first), Ok(last)) = (date_of(grid.since_ms), date_of(grid.now_ms)) else {
        return vec![];
    };
    let mut out: Vec<(i64, String)> = Vec::new();
    let mut date: Date = first;
    while date <= last {
        let hours: Vec<i8> = match days {
            1 => (0..24).step_by(HOURS).collect(),
            d if d >= 30 && date.weekday() != Weekday::Monday => vec![],
            _ => vec![0],
        };
        for hour in hours {
            let at = date.at(hour, 0, 0, 0);
            let Ok(zoned) = at.to_zoned(tz.clone()) else {
                continue;
            };
            let ms = zoned.timestamp().as_millisecond();
            let fresh = out.last().is_none_or(|(before, _)| *before < ms);
            if ms < grid.since_ms || ms > grid.now_ms || !fresh {
                continue;
            }
            let form = match days {
                1 => "%H",
                d if d >= 30 => "%b %-d",
                _ => "%a",
            };
            out.push((ms, format!("{TICK}{}", at.strftime(form))));
        }
        let Ok(next) = date.tomorrow() else {
            break;
        };
        date = next;
    }
    out
}

#[cfg(test)]
mod tests {
    use jiff::tz::{TimeZone, offset};
    use ratatui::style::Modifier;
    use tagteam_engine::timeline::TimelineView;
    use tagteam_engine::{EventKind, EventSource};

    use super::*;
    use crate::screens::history::{buffer_of, event_at};
    use crate::testutil::*;

    #[test]
    fn a_lane_draws_its_usage_where_in_use_and_a_track_elsewhere() {
        let theme = Theme::new(true);
        let lane = lane_of(
            1,
            vec![Some(40.0), Some(95.0), None, Some(10.0)],
            vec![true, true, true, false],
        );
        let buf = buffer_of(vec![lane_row(&lane, 0, theme)], 13);
        assert_eq!(text_of(&buf)[0], " 1 a1    ▄█ ─");
        assert_eq!(&roles(&buf, theme, 0)[9..], canon(theme, "ac f"));

        let lane = lane_of(1, vec![Some(40.0); 3], vec![true, false, true]);
        let buf = buffer_of(vec![in_use_row(&lane, 0, theme)], 12);
        assert_eq!(text_of(&buf)[0], "         ━─━");
        assert_eq!(&roles(&buf, theme, 0)[9..], canon(theme, "afa"));
    }

    #[test]
    fn the_cursor_runs_through_a_lane_where_the_account_was_not_in_use() {
        // §7.1's mock-up: a lane's track gives way to the cursor; a value in use stays.
        let theme = Theme::new(true);
        let lane = lane_of(1, vec![Some(40.0); 3], vec![true, false, false]);
        let row = |line| {
            let buf = buffer_of(vec![line], 12);
            (
                text_of(&buf)[0].clone(),
                roles(&buf, theme, 0)[9..].to_owned(),
            )
        };
        let want = |text: &str, roles: &str| (text.to_owned(), canon(theme, roles));
        assert_eq!(row(lane_row(&lane, 1, theme)), want(" 1 a1    ▄│─", "aaf"));
        assert_eq!(row(lane_row(&lane, 0, theme)), want(" 1 a1    ▄──", "aff"));
        assert_eq!(
            row(in_use_row(&lane, 2, theme)),
            want("         ━─│", "afa")
        );
        assert_eq!(
            row(in_use_row(&lane, 0, theme)),
            want("         ━──", "aff")
        );
        // An in-use cell with no reading is empty: the cursor runs through it too.
        let lane = lane_of(1, vec![None, Some(40.0), None], vec![true, true, false]);
        assert_eq!(row(lane_row(&lane, 0, theme)), want(" 1 a1    │▄─", "aaf"));
        assert_eq!(row(lane_row(&lane, 1, theme)), want(" 1 a1     ▄─", " af"));
    }

    #[test]
    fn a_lane_value_past_100_is_full_and_nan_reads_as_0() {
        let theme = Theme::new(true);
        let lane = lane_of(1, vec![Some(150.0), Some(f64::NAN)], vec![true, true]);
        let buf = buffer_of(vec![lane_row(&lane, 0, theme)], 11);
        assert_eq!(text_of(&buf)[0], " 1 a1    █▁");
        assert_eq!(&roles(&buf, theme, 0)[9..], canon(theme, "ca"));
    }

    /// Ten columns of the 7 days to now, `events` in them, each `(column, kind, source)`.
    fn ten_columns(events: &[(usize, EventKind, EventSource)], paused: bool) -> TimelineView {
        let mut view = timeline_view(10, vec![], vec![]);
        let grid = view.grid();
        let width = (grid.now_ms - grid.since_ms) / 10;
        view.events = events
            .iter()
            .map(|(c, kind, source)| {
                event_at(grid.since_ms + *c as i64 * width + 1, *kind, *source)
            })
            .collect();
        view.paused_at_start = paused;
        view
    }

    #[test]
    fn the_events_lane_shows_each_column_s_most_severe_event_and_a_pause_s_span() {
        use EventKind::{Add, AutoswitchPaused, AutoswitchResumed, Quarantine, Switch};
        use EventSource::{Auto, Daemon};
        let theme = Theme::new(true);
        let view = ten_columns(
            &[
                (2, Switch, Auto),
                (2, Quarantine, Auto),
                (5, AutoswitchPaused, Daemon),
                (8, AutoswitchResumed, Daemon),
                (9, Add, EventSource::Cli),
            ],
            false,
        );
        let after_gutter = |buf: &ratatui::buffer::Buffer| -> String {
            text_of(buf)[0].chars().skip(usize::from(CHART_X)).collect()
        };
        let buf = buffer_of(vec![events_row(&view, 0, theme)], 19);
        assert!(
            text_of(&buf)[0].starts_with(&" ".repeat(9)),
            "an empty gutter"
        );
        assert_eq!(after_gutter(&buf), "│ ✕  ‖┄┄‖+");
        assert_eq!(&roles(&buf, theme, 0)[9..], canon(theme, "a c  wwwmm"));
        let reversed = |buf: &ratatui::buffer::Buffer, x: u16| {
            buf[(x, 0)]
                .modifier
                .contains(Modifier::BOLD | Modifier::REVERSED)
        };
        assert!(
            !(9..19).any(|x| reversed(&buf, x)),
            "no event at the cursor"
        );

        // Paused from the range's start: the span runs to the resume, which the cursor is on.
        let view = ten_columns(&[(3, AutoswitchResumed, Daemon)], true);
        let buf = buffer_of(vec![events_row(&view, 3, theme)], 19);
        assert_eq!(after_gutter(&buf), "┄┄┄‖");
        assert_eq!(&roles(&buf, theme, 0)[9..13], canon(theme, "wwwm"));
        assert!(
            reversed(&buf, 12),
            "the cursor's event is bold and REVERSED"
        );
        assert!(!(9..12).any(|x| reversed(&buf, x)));

        // A pause with no resume runs to the last column.
        let view = ten_columns(&[(6, AutoswitchPaused, Daemon)], false);
        let buf = buffer_of(vec![events_row(&view, 0, theme)], 19);
        assert_eq!(after_gutter(&buf), "│     ‖┄┄┄");

        // Tier 0 draws `·` for the span; the cursor on column 2 marks its `✕`.
        let plain = Theme::new(false);
        let view = ten_columns(
            &[
                (2, Switch, Auto),
                (2, Quarantine, Auto),
                (5, AutoswitchPaused, Daemon),
                (8, AutoswitchResumed, Daemon),
                (9, Add, EventSource::Cli),
            ],
            false,
        );
        let buf = buffer_of(vec![events_row(&view, 2, plain)], 19);
        assert_eq!(after_gutter(&buf), "  ✕  ‖··‖+");
        assert!(reversed(&buf, 11), "{:?}", buf[(11, 0)]);
        assert!(!(9..19).filter(|x| *x != 11).any(|x| reversed(&buf, x)));
    }

    #[test]
    fn the_axis_keeps_one_label_per_boundary_across_daylight_saving() {
        let brussels = TimeZone::get("Europe/Brussels").unwrap();
        let ms = |at: &str| at.parse::<jiff::Timestamp>().unwrap().as_millisecond();
        let hours = ["╵15", "╵18", "╵21", "╵00", "╵03", "╵06", "╵09", "╵12"];
        // Now; the range; each boundary's label; the hours between one boundary and the next.
        let cases: [(&str, i64, &[&str], &[i64]); 4] = [
            // 2026-10-25: 03:00 CEST falls back to 02:00 CET.
            ("2026-10-25T12:00:00Z", 1, &hours, &[3, 3, 3, 4, 3, 3, 3]),
            (
                "2026-10-28T12:00:00Z",
                7,
                &["╵Thu", "╵Fri", "╵Sat", "╵Sun", "╵Mon", "╵Tue", "╵Wed"],
                &[24, 24, 24, 25, 24, 24],
            ),
            // 2026-03-29: 02:00 CET springs forward to 03:00 CEST.
            ("2026-03-29T12:00:00Z", 1, &hours, &[3, 3, 3, 2, 3, 3, 3]),
            (
                "2026-04-01T12:00:00Z",
                7,
                &["╵Thu", "╵Fri", "╵Sat", "╵Sun", "╵Mon", "╵Tue", "╵Wed"],
                &[24, 24, 24, 23, 24, 24],
            ),
        ];
        for (now, days, labels, steps) in cases {
            let grid = Grid {
                since_ms: ms(now) - days * 86_400_000,
                now_ms: ms(now),
                columns: 70,
            };
            let found = boundaries(grid, days, &brussels);
            let got: Vec<&str> = found.iter().map(|(_, l)| l.as_str()).collect();
            assert_eq!(got, labels, "{now}, {days}d");
            let between: Vec<i64> = found
                .windows(2)
                .map(|w| (w[1].0 - w[0].0) / 3_600_000)
                .collect();
            assert_eq!(between, steps, "{now}, {days}d");
            let row = axis_row(grid, days, &brussels, Theme::new(true)).to_string();
            let drawn: usize = labels.iter().map(|l| row.matches(l).count()).sum();
            assert!(
                labels.iter().all(|l| row.matches(l).count() <= 1),
                "no label twice: {row:?}"
            );
            assert_eq!(
                row.matches('╵').count(),
                drawn,
                "each tick a boundary's: {row:?}"
            );
            assert!(drawn >= labels.len() - 1, "{row:?}");
        }
    }

    #[test]
    fn hidden_lanes_are_counted_on_a_muted_more_row() {
        let theme = Theme::new(true);
        let buf = buffer_of(vec![more_row(6, theme)], 30);
        let text = &text_of(&buf)[0];
        assert_eq!(text, " +6 more · f picks one");
        assert_eq!(
            roles(&buf, theme, 0).trim_end(),
            canon(theme, " mm mmmm m m mmmmm mmm")
        );
    }

    /// The axis over the `days` days to `NOW_MS` in 70 columns, in `tz`.
    fn axis(days: i64, tz: &TimeZone) -> String {
        let grid = Grid {
            since_ms: NOW_MS - days * 86_400_000,
            now_ms: NOW_MS,
            columns: 70,
        };
        axis_row(grid, days, tz, Theme::new(true)).to_string()
    }

    /// The column `needle` starts at in `row`.
    fn at(row: &str, needle: &str) -> Option<usize> {
        row.find(needle).map(|b| row[..b].chars().count())
    }

    #[test]
    fn the_axis_marks_each_day_and_now() {
        let utc = TimeZone::UTC;
        let week = axis(7, &utc);
        for (label, column) in [
            ("╵Tue", 13),
            ("╵Wed", 23),
            ("╵Thu", 33),
            ("╵Fri", 43),
            ("╵Sat", 53),
            ("╵Sun", 63),
        ] {
            assert_eq!(at(&week, label), Some(column), "{label} in {week:?}");
        }
        assert_eq!(at(&week, "╵Mon"), None, "too near now: {week:?}");
        assert_eq!(week.matches('╵').count(), 6, "{week:?}");
        assert_eq!(at(&week, "now"), Some(76));
        assert_eq!(week.trim_end().chars().count(), 79, "now ends at column 78");

        let day = axis(1, &utc);
        for (label, column) in [
            ("╵15", 11),
            ("╵18", 20),
            ("╵21", 28),
            ("╵00", 37),
            ("╵03", 46),
            ("╵06", 55),
            ("╵09", 63),
            ("╵12", 72),
        ] {
            assert_eq!(at(&day, label), Some(column), "{label} in {day:?}");
        }
        assert_eq!(day.matches('╵').count(), 8, "{day:?}");

        let month = axis(30, &utc);
        for (label, column) in [
            ("╵Aug 24", 12),
            ("╵Aug 31", 28),
            ("╵Sep 7", 44),
            ("╵Sep 14", 61),
        ] {
            assert_eq!(at(&month, label), Some(column), "{label} in {month:?}");
        }
        assert_eq!(at(&month, "╵Sep 21"), None, "too near now: {month:?}");
        assert_eq!(month.matches('╵').count(), 4, "{month:?}");

        // In the TUI's time zone: Tuesday starts at 22:00 UTC two hours east.
        let east = axis(7, &TimeZone::fixed(offset(2)));
        assert_eq!(at(&east, "╵Tue"), Some(12), "{east:?}");

        let theme = Theme::new(true);
        let grid = Grid {
            since_ms: NOW_MS - 7 * 86_400_000,
            now_ms: NOW_MS,
            columns: 70,
        };
        let buf = buffer_of(vec![axis_row(grid, 7, &utc, theme)], 80);
        assert!(
            roles(&buf, theme, 0).chars().all(|c| c == ' ' || c == 'm'),
            "all muted"
        );
    }
}
