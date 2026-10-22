//! History's rows (T4-8, §7.1, §7.8): the scope row, the charts, the events lane, the lanes,
//! the axis, then the event list, sized to the body.

use std::cmp::Reverse;

use ratatui::layout::Rect;
use tagteam_engine::timeline::{Scope, TimelineView};

use crate::chrome::{RULE_FROM, WIDE};

/// T4-10: the marker column and the 8-column gutter come before a chart's first column.
pub(crate) const CHART_X: u16 = 9;
/// The event list keeps at least this many rows, and a chart at least this many.
pub(crate) const LIST_LEAST: u16 = 4;
pub(crate) const CHART_LEAST: u16 = 5;
/// The all-accounts chart's rows at most, and each of one account's charts'.
pub(crate) const CHART_MOST_ALL: u16 = 12;
pub(crate) const CHART_MOST_EACH: u16 = 8;
/// At most this many swimlanes show (§7.4); the rest are counted on the more row.
pub(crate) const LANES_MOST: usize = 10;

/// Where each part of History goes in the body.
#[derive(Debug)]
pub(crate) struct Layout {
    #[cfg(test)]
    pub(crate) scope: Rect,
    /// Each chart shown: its index in `view.charts`, and its title row and rows.
    pub(crate) charts: Vec<(usize, Rect)>,
    #[cfg(test)]
    pub(crate) events: Rect,
    /// The lanes shown, by index in `view.lanes`, in position order (T4-7, T4-8).
    pub(crate) lanes: Vec<(usize, Rect)>,
    /// The `+N more · f picks one` row, and N.
    pub(crate) more: Option<(Rect, usize)>,
    pub(crate) axis: Rect,
    #[cfg(test)]
    pub(crate) head: Option<Rect>,
    #[cfg(test)]
    pub(crate) list: Rect,
}

/// The body's rows from the top, each part taking what is left when the body runs out.
struct Rows {
    area: Rect,
    y: u16,
}

impl Rows {
    fn take(&mut self, n: u16) -> Rect {
        let n = n.min(self.area.bottom().saturating_sub(self.y));
        let rect = Rect::new(self.area.x, self.y, self.area.width, n);
        self.y += n;
        rect
    }

    #[cfg(test)]
    fn rest(&mut self) -> Rect {
        self.take(u16::MAX)
    }
}

/// T4-8: `view`'s rows in `body` on a terminal `terminal_rows` tall, one account's charts
/// from `chart_from` on (as far as keeps the last one shown).
pub(crate) fn layout(
    view: &TimelineView,
    scope: &Scope,
    body: Rect,
    terminal_rows: u16,
    chart_from: usize,
) -> Layout {
    let spaced = terminal_rows >= RULE_FROM;
    let headed = body.width >= WIDE;
    // The scope row, the events lane, the axis, and the chart's title or the in-use lane.
    let fixed = 4 + 2 * u16::from(spaced) + u16::from(headed);
    let avail = body.height.saturating_sub(fixed);
    let mut rows = Rows {
        area: body,
        y: body.y,
    };
    // The scope row.
    rows.take(1);
    if spaced {
        rows.take(1);
    }
    let (charts, lanes, more, events);
    match scope {
        Scope::All => {
            let room = avail.saturating_sub(LIST_LEAST + CHART_LEAST);
            let n = view.lanes.len();
            let shown = n.min(LANES_MOST.min(usize::from(room.saturating_sub(1))));
            let hidden = n - shown;
            let lane_rows = u16::try_from(shown).unwrap_or(u16::MAX) + u16::from(hidden > 0);
            let chart = avail
                .saturating_sub(LIST_LEAST + lane_rows)
                .clamp(CHART_LEAST, CHART_MOST_ALL);
            let chart = rows.take(1 + chart);
            charts = if view.charts.is_empty() {
                vec![]
            } else {
                vec![(0, chart)]
            };
            events = rows.take(1);
            lanes = kept(view, shown)
                .into_iter()
                .map(|i| (i, rows.take(1)))
                .collect();
            more = (hidden > 0).then(|| (rows.take(1), hidden));
        }
        Scope::Account(_) => {
            let n = view.charts.len();
            let room = avail.saturating_sub(LIST_LEAST);
            let shown = n.min(usize::from(room / (CHART_LEAST + 1)));
            let each = u16::try_from(shown)
                .ok()
                .filter(|s| *s > 0)
                .map_or(0, |s| (room / s).saturating_sub(1).min(CHART_MOST_EACH));
            let from = chart_from.min(n - shown);
            charts = (from..from + shown)
                .map(|i| (i, rows.take(1 + each)))
                .collect();
            events = rows.take(1);
            let lane = rows.take(1);
            lanes = if view.lanes.is_empty() {
                vec![]
            } else {
                vec![(0, lane)]
            };
            more = None;
        }
    }
    let axis = rows.take(1);
    #[cfg(not(test))]
    let _ = events;
    #[cfg(test)]
    let (head, list) = {
        if spaced {
            rows.take(1);
        }
        let head = headed.then(|| rows.take(1));
        (head, rows.rest())
    };
    Layout {
        #[cfg(test)]
        scope: Rect {
            height: body.height.min(1),
            ..body
        },
        charts,
        #[cfg(test)]
        events,
        lanes,
        more,
        axis,
        #[cfg(test)]
        head,
        #[cfg(test)]
        list,
    }
}

/// The `shown` lanes in use in the most columns, in position order; of two in use as long,
/// the one at the lower position (T4-8).
fn kept(view: &TimelineView, shown: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..view.lanes.len()).collect();
    order.sort_by_key(|&i| Reverse(view.lanes[i].in_use.iter().filter(|u| **u).count()));
    order.truncate(shown);
    order.sort_unstable();
    order
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Rect;
    use tagteam_engine::timeline::{IN_USE, Scope, Series, TimelineView};

    use super::*;
    use crate::testutil::*;

    fn chart(label: &str) -> Series {
        Series {
            label: label.into(),
            peaks: vec![Some(10.0); 70],
        }
    }

    /// Every account's view over 70 columns: the in-use chart, and the lane of each position
    /// `p` in use in its first `in_use[p − 1]` columns.
    fn all_of(in_use: &[usize]) -> TimelineView {
        let lanes = (1..)
            .zip(in_use)
            .map(|(p, &n)| lane_of(p, vec![Some(10.0); 70], (0..70).map(|c| c < n).collect()))
            .collect();
        timeline_view(70, vec![chart(IN_USE)], lanes)
    }

    /// Account 1's view over 70 columns, a chart per window in `labels`.
    fn one_of(labels: &[&str]) -> TimelineView {
        let lane = lane_of(1, vec![Some(10.0); 70], vec![true; 70]);
        timeline_view(70, labels.iter().map(|l| chart(l)).collect(), vec![lane])
    }

    /// `r`'s first and last row, counted from the body's first; it spans the body's width.
    fn rows(body: Rect, r: Rect) -> (u16, u16) {
        assert_eq!((r.x, r.width), (body.x, body.width), "{r:?}");
        assert!(r.height > 0, "{r:?}");
        (r.y - body.y, r.y + r.height - 1 - body.y)
    }

    fn row(body: Rect, r: Rect) -> u16 {
        let (first, last) = rows(body, r);
        assert_eq!(first, last, "{r:?}");
        first
    }

    fn charts(body: Rect, l: &Layout) -> Vec<(usize, (u16, u16))> {
        l.charts.iter().map(|(i, r)| (*i, rows(body, *r))).collect()
    }

    fn lanes(body: Rect, l: &Layout) -> Vec<(usize, u16)> {
        l.lanes.iter().map(|(i, r)| (*i, row(body, *r))).collect()
    }

    #[test]
    fn the_80_by_24_budget_fits_3_6_and_12_accounts() {
        let body = Rect::new(0, 3, 80, 20);
        let l = layout(&all_of(&[70; 3]), &Scope::All, body, 24, 0);
        assert_eq!(row(body, l.scope), 0);
        assert_eq!(charts(body, &l), [(0, (1, 10))], "the title and 9 rows");
        assert_eq!(row(body, l.events), 11);
        assert_eq!(lanes(body, &l), [(0, 12), (1, 13), (2, 14)]);
        assert_eq!(l.more, None);
        assert_eq!(row(body, l.axis), 15);
        assert_eq!(l.head, None);
        assert_eq!(rows(body, l.list), (16, 19));

        let l = layout(&all_of(&[70; 6]), &Scope::All, body, 24, 0);
        assert_eq!(charts(body, &l), [(0, (1, 7))], "the title and 6 rows");
        assert_eq!(row(body, l.events), 8);
        assert_eq!(
            lanes(body, &l),
            [(0, 9), (1, 10), (2, 11), (3, 12), (4, 13), (5, 14)]
        );
        assert_eq!(l.more, None);
        assert_eq!(row(body, l.axis), 15);
        assert_eq!(rows(body, l.list), (16, 19));

        // Positions 4, 8, 10, 2, 12 and 6 were in use in the most columns.
        let used = [3, 9, 1, 12, 5, 7, 2, 11, 4, 10, 6, 8];
        let l = layout(&all_of(&used), &Scope::All, body, 24, 0);
        assert_eq!(charts(body, &l), [(0, (1, 6))], "the title and 5 rows");
        assert_eq!(row(body, l.events), 7);
        assert_eq!(
            lanes(body, &l),
            [(1, 8), (3, 9), (5, 10), (7, 11), (9, 12), (11, 13)],
            "the six in use the most, in position order"
        );
        let (more, n) = l.more.expect("a more row");
        assert_eq!((row(body, more), n), (14, 6));
        assert_eq!(row(body, l.axis), 15);
        assert_eq!(rows(body, l.list), (16, 19));
    }

    #[test]
    fn at_120_by_40_the_chart_takes_12_rows_and_the_list_the_rest() {
        let body = Rect::new(0, 4, 120, 35);
        let l = layout(&all_of(&[70; 3]), &Scope::All, body, 40, 0);
        assert_eq!(row(body, l.scope), 0);
        assert_eq!(
            charts(body, &l),
            [(0, (2, 14))],
            "a blank, the title and 12 rows"
        );
        assert_eq!(row(body, l.events), 15);
        assert_eq!(lanes(body, &l), [(0, 16), (1, 17), (2, 18)]);
        assert_eq!(l.more, None);
        assert_eq!(row(body, l.axis), 19);
        assert_eq!(l.head.map(|h| row(body, h)), Some(21), "after a blank");
        assert_eq!(rows(body, l.list), (22, 34));

        // Every lane in use as long: the two of the highest positions are hidden.
        let l = layout(&all_of(&[70; 12]), &Scope::All, body, 40, 0);
        assert_eq!(charts(body, &l), [(0, (2, 14))]);
        assert_eq!(
            lanes(body, &l),
            (0..10).map(|i| (i, 16 + i as u16)).collect::<Vec<_>>()
        );
        let (more, n) = l.more.expect("a more row");
        assert_eq!((row(body, more), n), (26, 2));
        assert_eq!(row(body, l.axis), 27);
        assert_eq!(l.head.map(|h| row(body, h)), Some(29));
        assert_eq!(rows(body, l.list), (30, 34));
    }

    #[test]
    fn one_account_s_charts_share_the_rows_and_scroll() {
        let body = Rect::new(0, 3, 80, 20);
        let scope = Scope::Account(id(1));
        let l = layout(&one_of(&["5h", "7d"]), &scope, body, 24, 0);
        assert_eq!(row(body, l.scope), 0);
        assert_eq!(
            charts(body, &l),
            [(0, (1, 6)), (1, (7, 12))],
            "each its title and 5 rows"
        );
        assert_eq!(row(body, l.events), 13);
        assert_eq!(lanes(body, &l), [(0, 14)], "the in-use lane");
        assert_eq!(l.more, None);
        assert_eq!(row(body, l.axis), 15);
        assert_eq!(rows(body, l.list), (16, 19));

        let four = one_of(&["5h", "7d", "Fable", "Sonnet"]);
        let l = layout(&four, &scope, body, 24, 0);
        assert_eq!(charts(body, &l), [(0, (1, 6)), (1, (7, 12))]);
        let l = layout(&four, &scope, body, 24, 2);
        assert_eq!(charts(body, &l), [(2, (1, 6)), (3, (7, 12))]);
        let l = layout(&four, &scope, body, 24, 3);
        assert_eq!(
            charts(body, &l),
            [(2, (1, 6)), (3, (7, 12))],
            "the last window stays shown"
        );

        let body = Rect::new(0, 4, 120, 35);
        let l = layout(&one_of(&["5h", "7d", "Fable"]), &scope, body, 40, 0);
        assert_eq!(
            charts(body, &l),
            [(0, (2, 9)), (1, (10, 17)), (2, (18, 25))],
            "each its title and 7 rows"
        );
        assert_eq!(row(body, l.events), 26);
        assert_eq!(lanes(body, &l), [(0, 27)]);
        assert_eq!(row(body, l.axis), 28);
        assert_eq!(l.head.map(|h| row(body, h)), Some(30));
        assert_eq!(rows(body, l.list), (31, 34));

        // No relevant window: no chart, and the list takes its rows.
        let l = layout(&one_of(&[]), &scope, Rect::new(0, 3, 80, 20), 24, 0);
        assert!(l.charts.is_empty());
        assert_eq!(row(Rect::new(0, 3, 80, 20), l.events), 1);
    }

    #[test]
    fn every_body_gets_its_parts_in_order_without_overlap() {
        for (width, terminal_rows) in [(80, 24), (120, 40)] {
            for height in 0..=60 {
                let body = Rect::new(0, 3, width, height);
                for scope in [Scope::All, Scope::Account(id(1))] {
                    let view = match scope {
                        Scope::All => all_of(&[70; 12]),
                        Scope::Account(_) => one_of(&["5h", "7d", "Fable"]),
                    };
                    let l = layout(&view, &scope, body, terminal_rows, 0);
                    // Top to bottom, as T4-8 orders them.
                    let mut parts = vec![l.scope];
                    parts.extend(l.charts.iter().map(|(_, r)| *r));
                    parts.push(l.events);
                    parts.extend(l.lanes.iter().map(|(_, r)| *r));
                    parts.extend(l.more.map(|(r, _)| r));
                    parts.push(l.axis);
                    parts.extend(l.head);
                    parts.push(l.list);
                    let at = format!("{width}×{height} on {terminal_rows} rows, {scope:?}");
                    for r in &parts {
                        assert!(
                            r.y >= body.y && r.bottom() <= body.bottom(),
                            "{at}: {r:?} outside {body:?}"
                        );
                    }
                    for pair in parts.windows(2) {
                        assert!(
                            pair[0].bottom() <= pair[1].y,
                            "{at}: {:?} overlaps or follows {:?}",
                            pair[0],
                            pair[1]
                        );
                    }
                }
            }
        }
    }
}
