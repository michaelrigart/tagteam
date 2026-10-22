//! The Tokens screen's headline (redesign §8, VIS-5, VIS-9, VIS-14): what the report adds up
//! to, over its table; on a Day report today's cost and a sparkline of cost per day, and on a
//! 5h-block report the active block's progress line.

use std::collections::HashMap;

use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use ratatui::text::{Line, Span};
use tagteam_engine::tokens::{Cost, PeriodKey, RowAccount, TokenPeriod, TokenReport, TokenRow};
use tagteam_engine::words;

use super::amounts;
use super::table::figures_clock;
use crate::chrome::{WIDE, fit_spans};
use crate::gauge;
use crate::keys::RIGHT_INSET;
use crate::state::State;
use crate::theme::{Role, Theme};
use crate::view::width_of;

/// Between the headline's figures (§8's text).
const GAP: &str = "    ";
/// The least gap between the headline's figures and what sits on their right (T5-12, T5-13).
const RIGHT_GAP: usize = 3;
/// The per-day sparkline's label (T5-12).
const PER_DAY: &str = "per day ";
/// The most dates the per-day sparkline shows (`min(30, the days its query spans)`), and the
/// fewest it keeps (T5-12).
const DAYS_MOST: usize = 30;
const DAYS_LEAST: usize = 7;
/// The block's gauge, and the fewest cells it keeps on its own row (T5-13).
const BLOCK_GAUGE: usize = 22;
const BLOCK_GAUGE_LEAST: usize = 8;

/// The headline's rows at `width` (T5-10, T5-12, T5-13): one, or two while the active block's
/// line does not fit beside the figures.
pub(super) fn headline(
    state: &State,
    report: &TokenReport,
    width: usize,
    now_ms: i64,
) -> Vec<Line<'static>> {
    let theme = state.theme;
    let mut left = figures(theme, report);
    match report.query.period {
        TokenPeriod::Day if width >= usize::from(WIDE) => {
            let tz = &report.query.tz;
            let Some(today) = date_of(now_ms, tz) else {
                return vec![Line::from(fit_spans(left, width))];
            };
            left.extend(today_spans(theme, report, today));
            let days = days_spanned(report, today);
            let room = width.saturating_sub(used(&left) + RIGHT_GAP + PER_DAY.len() + RIGHT_INSET);
            let cells = days.min(room);
            let right = if cells >= DAYS_LEAST {
                vec![
                    Span::styled(PER_DAY, theme.role(Role::Muted)),
                    gauge::sparkline_of(&per_day(report, today, cells), theme),
                ]
            } else {
                vec![]
            };
            vec![beside(left, right, width)]
        }
        TokenPeriod::Block => match active_block(state, report, now_ms) {
            Some(block) => block_rows(theme, left, &block, width),
            None => vec![Line::from(fit_spans(left, width))],
        },
        _ => vec![Line::from(fit_spans(left, width))],
    }
}

/// The headline's left part (T5-10): the total cost, tokens and responses.
fn figures(theme: Theme, report: &TokenReport) -> Vec<Span<'static>> {
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
    vec![
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
    ]
}

/// The columns `spans` take.
fn used(spans: &[Span]) -> usize {
    spans.iter().map(|s| width_of(&s.content)).sum()
}

/// `left`, then `right` ending `RIGHT_INSET` in, cut to `width`.
fn beside(mut left: Vec<Span<'static>>, right: Vec<Span<'static>>, width: usize) -> Line<'static> {
    if !right.is_empty() {
        let gap = width.saturating_sub(used(&left) + used(&right) + RIGHT_INSET);
        left.push(Span::raw(" ".repeat(gap)));
        left.extend(right);
    }
    Line::from(fit_spans(left, width))
}

/// The date `ms` falls on in `tz`.
fn date_of(ms: i64, tz: &TimeZone) -> Option<Date> {
    Some(
        Timestamp::from_millisecond(ms)
            .ok()?
            .to_zoned(tz.clone())
            .date(),
    )
}

/// The row's day, when it is one.
fn day_of(row: &TokenRow) -> Option<Date> {
    match row.period {
        PeriodKey::Day { date } => Some(date),
        _ => None,
    }
}

/// `costs` added up (T5-12, T5-13): partial when any is partial or none, and none only when
/// every one is.
fn summed(costs: impl IntoIterator<Item = Option<Cost>>) -> Option<Cost> {
    let mut sum: Option<Cost> = None;
    let mut missing = false;
    for cost in costs {
        match cost {
            Some(c) => {
                let s = sum.get_or_insert(Cost {
                    usd_nanos: 0,
                    partial: false,
                });
                s.usd_nanos = s.usd_nanos.saturating_add(c.usd_nanos);
                s.partial |= c.partial;
            }
            None => missing = true,
        }
    }
    sum.map(|s| Cost {
        partial: s.partial || missing,
        ..s
    })
}

/// A cost as `money` writes it, in text: `≥` before a partial one, in warn; `—` for none.
fn cost_span(theme: Theme, cost: Option<Cost>, money: fn(u64) -> String) -> Span<'static> {
    match cost {
        Some(c) if c.partial => Span::styled(
            format!("{}{}", words::AT_LEAST, money(c.usd_nanos)),
            theme.role(Role::Warn),
        ),
        Some(c) => Span::styled(money(c.usd_nanos), theme.role(Role::Text)),
        None => Span::styled(words::MISSING, theme.role(Role::Text)),
    }
}

/// `    today {cost}` (T5-12): today's rows' costs added up, `$0.00` with no row today.
fn today_spans(theme: Theme, report: &TokenReport, today: Date) -> Vec<Span<'static>> {
    let mut rows = report
        .rows
        .iter()
        .filter(|r| day_of(r) == Some(today))
        .peekable();
    let cost = if rows.peek().is_none() {
        Some(Cost {
            usd_nanos: 0,
            partial: false,
        })
    } else {
        summed(rows.map(|r| r.cost))
    };
    vec![
        Span::raw(GAP),
        Span::styled("today", theme.role(Role::Muted)),
        Span::raw(" "),
        cost_span(theme, cost, amounts::dollars),
    ]
}

/// How many dates the per-day sparkline shows for `report` (T5-12): the days its own query
/// spans to `today`, so a report shown while another range is asked for keeps its own, and
/// `DAYS_MOST` at most or for a query with no start.
fn days_spanned(report: &TokenReport, today: Date) -> usize {
    report
        .query
        .since_ms
        .and_then(|since| date_of(since, &report.query.tz))
        .and_then(|since| since.until(today).ok())
        .map_or(DAYS_MOST, |span| {
            usize::try_from(span.get_days()).map_or(0, |days| days.min(DAYS_MOST))
        })
}

/// The last `cells` dates to `today`, oldest first, each its rows' costs added up: none for a
/// date with no row, 0 for one whose rows have no price (T5-12).
fn per_day(report: &TokenReport, today: Date, cells: usize) -> Vec<Option<u64>> {
    let mut by_date: HashMap<Date, u64> = HashMap::new();
    for row in &report.rows {
        if let Some(date) = day_of(row) {
            let sum = by_date.entry(date).or_default();
            *sum = sum.saturating_add(row.cost.map_or(0, |c| c.usd_nanos));
        }
    }
    let mut values = Vec::with_capacity(cells);
    let mut date = Some(today);
    for _ in 0..cells {
        values.push(date.and_then(|d| by_date.get(&d).copied()));
        date = date.and_then(|d| d.yesterday().ok());
    }
    values.reverse();
    values
}

/// The active block the headline shows (T5-13), its rows by model added up.
struct Block {
    /// The elapsed share, in percent.
    pct: f64,
    /// Seconds left.
    left: i64,
    tokens_per_min: f64,
    cost_per_hour: Option<Cost>,
    projected_cost: Option<Cost>,
}

/// The Tokens provider's account in use's active block, else the report's first: its rows
/// added up. `None` with no active block, or once it has ended.
fn active_block(state: &State, report: &TokenReport, now_ms: i64) -> Option<Block> {
    let in_use = state
        .view_of(state.tokens.provider())
        .and_then(|v| v.accounts.iter().find(|a| a.active))
        .map(|a| &a.row.id);
    let active: Vec<&TokenRow> = report
        .rows
        .iter()
        .filter(|r| figures_clock(r, now_ms).is_some())
        .collect();
    let first = active
        .iter()
        .find(|r| {
            r.accounts.iter().any(|a| match a {
                RowAccount::Account { id, .. } => Some(id) == in_use,
                _ => false,
            })
        })
        .or(active.first())?;
    let PeriodKey::Block {
        start_ms, end_ms, ..
    } = first.period
    else {
        return None;
    };
    let (_, left) = figures_clock(first, now_ms)?;
    let parts: Vec<_> = active
        .iter()
        .filter(|r| r.account == first.account)
        .filter_map(|r| match &r.period {
            PeriodKey::Block {
                start_ms: s,
                end_ms: e,
                active: Some(a),
                ..
            } if (*s, *e) == (start_ms, end_ms) => Some(a),
            _ => None,
        })
        .collect();
    let span = (end_ms - start_ms).max(1) as f64;
    Some(Block {
        pct: (now_ms - start_ms) as f64 / span * 100.0,
        left,
        tokens_per_min: parts.iter().map(|a| a.tokens_per_min).sum(),
        cost_per_hour: summed(parts.iter().map(|a| a.cost_per_hour)),
        projected_cost: summed(parts.iter().map(|a| a.projected_cost)),
    })
}

/// The block line (T5-13) with a gauge `cells` wide, its rate and its cost per hour while
/// `rate` and `per_hour` keep them.
fn block_line(
    theme: Theme,
    block: &Block,
    cells: usize,
    rate: bool,
    per_hour: bool,
) -> Vec<Span<'static>> {
    let (text, muted) = (theme.role(Role::Text), theme.role(Role::Muted));
    let sep = || Span::styled(" · ", muted);
    let mut spans = vec![
        Span::styled("▶", theme.role(Role::Accent)),
        Span::raw(" "),
        Span::styled("block", muted),
        Span::raw(" "),
    ];
    spans.extend(gauge::progress_line(block.pct, cells, theme));
    spans.extend([
        Span::raw(" "),
        Span::styled(words::duration(block.left), text),
        Span::raw(" "),
        Span::styled("left", muted),
    ]);
    if rate {
        let n = block.tokens_per_min.max(0.0).round() as u64;
        spans.extend([
            sep(),
            Span::styled(words::scaled(n), text),
            Span::raw(" "),
            Span::styled("tok/min", muted),
        ]);
    }
    if let Some(cost) = block.cost_per_hour.filter(|_| per_hour) {
        spans.extend([
            sep(),
            cost_span(theme, Some(cost), amounts::dollars),
            Span::styled("/h", muted),
        ]);
    }
    if let Some(cost) = block.projected_cost {
        spans.extend([
            sep(),
            Span::styled("≈", text),
            Span::raw(" "),
            cost_span(theme, Some(cost), amounts::about),
            Span::raw(" "),
            Span::styled("at its end", muted),
        ]);
    }
    spans
}

/// The headline's rows with the block line (T5-13): on the figures' row when it fits whole
/// there; else on its own row, its gauge shrunk, then its rate and its cost per hour left out,
/// then cut.
fn block_rows(
    theme: Theme,
    left: Vec<Span<'static>>,
    block: &Block,
    width: usize,
) -> Vec<Line<'static>> {
    let whole = block_line(theme, block, BLOCK_GAUGE, true, true);
    if used(&left) + RIGHT_GAP + used(&whole) + RIGHT_INSET <= width {
        return vec![beside(left, whole, width)];
    }
    let room = width.saturating_sub(1 + RIGHT_INSET);
    let fitted = [(true, true), (false, true), (false, false)]
        .into_iter()
        .find_map(|(rate, per_hour)| {
            let rest = used(&block_line(theme, block, 0, rate, per_hour));
            let cells = room.checked_sub(rest)?.min(BLOCK_GAUGE);
            (cells >= BLOCK_GAUGE_LEAST).then(|| block_line(theme, block, cells, rate, per_hour))
        });
    let line = fitted.unwrap_or_else(|| {
        let least = block_line(theme, block, BLOCK_GAUGE_LEAST, false, false);
        fit_spans(least, room)
    });
    let mut own = vec![Span::raw(" ")];
    own.extend(line);
    vec![
        Line::from(fit_spans(left, width)),
        Line::from(fit_spans(own, width)),
    ]
}

#[cfg(test)]
mod tests {
    use jiff::civil::date;
    use tagteam_engine::tokens::ActiveBlock;

    use super::super::table::{
        HOUR_MS, NOON, block, body_y, days, letters, showing, showing_after,
    };
    use super::*;
    use crate::msg::{Cmd, Msg};
    use crate::screens::tokens::TokensState;
    use crate::state::Screen;
    use crate::testutil::*;
    use crate::update::update;

    fn dollars(usd_nanos: u64) -> Cost {
        Cost {
            usd_nanos,
            partial: false,
        }
    }

    /// Body row `n` of `state` at `w`×40, as text and as `roles`' letters in tier 1.
    fn row_at(mut state: State, w: u16, n: u16) -> (String, String) {
        let theme = Theme::new(true);
        state.theme = theme;
        let buf = draw_buffer(&state, w, 40, NOW_MS);
        let y = body_y(&state, w, 40, n);
        let text = body_of(&state, w, 40, NOW_MS)[usize::from(n)].clone();
        (text, roles(&buf, theme, y).trim_end().to_owned())
    }

    /// The letters of the cells under `needle` in body row `n` of `state` at `w`×40, tier 1.
    fn roles_of(mut state: State, w: u16, n: u16, needle: &str) -> String {
        let theme = Theme::new(true);
        state.theme = theme;
        let buf = draw_buffer(&state, w, 40, NOW_MS);
        let y = body_y(&state, w, 40, n);
        let text = &body_of(&state, w, 40, NOW_MS)[usize::from(n)];
        let at = text
            .find(needle)
            .unwrap_or_else(|| panic!("{needle} in {text:?}"));
        let x = width_of(&text[..at]);
        roles(&buf, theme, y)
            .chars()
            .skip(x)
            .take(width_of(needle))
            .collect()
    }

    /// Account 1's `cost` row of the day `days_ago` days before `NOW_MS`'s.
    fn day_row(days_ago: i64, position: u32, cost: Option<u64>) -> TokenRow {
        token_row(
            day(days_ago),
            Some(row_account(position)),
            None,
            counts(1_000, 0, 0),
            cost,
        )
    }

    #[test]
    fn from_120_columns_a_day_report_says_today_and_a_sparkline_of_cost_per_day() {
        // T5-12: 2026-09-19 has rows but no price, 2026-09-20 has $50.95, today none.
        let left = " $50.95 at API prices    114.2M tokens    414 responses";
        let want = format!(
            "{left}    today $0.00{}per day {}▁█",
            " ".repeat(11),
            " ".repeat(27)
        );
        let (text, got) = row_at(showing(days(), |_| {}), 120, 1);
        assert_eq!(text, want);
        assert_eq!(
            width_of(&text) + 2,
            120,
            "the sparkline's today, then the inset"
        );
        let theme = Theme::new(true);
        let roles = letters(&[
            (" $50.95 ", 't'),
            ("at API prices", 'm'),
            ("    114.2M ", 't'),
            ("tokens", 'm'),
            ("    414 ", 't'),
            ("responses", 'm'),
            ("    ", 't'),
            ("today", 'm'),
            (" $0.00", 't'),
            (&" ".repeat(11), 't'),
            ("per day", 'm'),
            (&" ".repeat(28), 't'),
            ("▁█", 'a'),
        ]);
        assert_eq!(got, canon(theme, &roles));
        assert_eq!(body_of(&showing(days(), |_| {}), 80, 24, NOW_MS)[1], left);

        let rows = || {
            vec![
                day_row(0, 1, Some(8_000_000_000)),
                day_row(1, 1, Some(4_000_000_000)),
                day_row(3, 1, Some(2_000_000_000)),
            ]
        };
        let line = body_of(&showing(rows(), |_| {}), 120, 40, NOW_MS)[1].clone();
        assert!(line.contains("    today $8.00 "), "{line}");
        assert!(line.ends_with("▃ ▅█"), "{line}");
        assert_eq!(width_of(&line) + 1, 120, "{line}");
        let week = body_of(&showing_after("[", rows()), 120, 40, NOW_MS)[1].clone();
        assert!(week.ends_with("per day    ▃ ▅█"), "seven cells: {week}");
        assert_eq!(width_of(&week) + 1, 120, "{week}");
        // The cells are the shown report's: asked for 30 days, a 7-day report still draws 7.
        let mut asked = showing_after("[", rows());
        let cmds = update(&mut asked, ch(']'), NOW_MS);
        assert!(
            matches!(cmds.as_slice(), [Cmd::LoadReport { .. }]),
            "{cmds:?}"
        );
        assert!(asked.tokens.pending());
        let line = body_of(&asked, 120, 40, NOW_MS)[1].clone();
        assert_eq!(line, week, "seven cells until the new report answers");

        let mut unpriced = rows();
        unpriced.push(day_row(0, 2, None));
        let state = showing(unpriced, |_| {});
        let line = body_of(&state, 120, 40, NOW_MS)[1].clone();
        assert!(line.contains("    today ≥$8.00 "), "{line}");
        let theme = Theme::new(true);
        assert_eq!(roles_of(state, 120, 1, "≥$8.00"), canon(theme, "wwwwww"));

        let none = showing(vec![day_row(0, 1, None), day_row(1, 1, Some(1))], |_| {});
        let line = body_of(&none, 120, 40, NOW_MS)[1].clone();
        assert!(line.contains("    today — "), "no priced row today: {line}");
        assert!(line.ends_with("█▁"), "today's rows have no price: {line}");
    }

    #[test]
    fn today_and_the_per_day_cells_follow_the_report_s_zone() {
        // Review Focus 3: NOW_MS is 2026-09-22 04:13 at UTC+14.
        let rows = || {
            vec![
                token_row(
                    PeriodKey::Day {
                        date: date(2026, 9, 22),
                    },
                    Some(row_account(1)),
                    None,
                    counts(1_000, 0, 0),
                    Some(3_000_000_000),
                ),
                token_row(
                    PeriodKey::Day {
                        date: date(2026, 9, 21),
                    },
                    Some(row_account(1)),
                    None,
                    counts(1_000, 0, 0),
                    Some(5_000_000_000),
                ),
            ]
        };
        let mut state = fresh(Screen::Tokens);
        state.tokens = TokensState::new(cc(), TimeZone::get("Pacific/Kiritimati").unwrap());
        let cmds = state.start();
        let [_, Cmd::LoadReport { r#gen, query }] = cmds.as_slice() else {
            panic!("{cmds:?}");
        };
        let result = Ok(token_report(query.clone(), rows()));
        update(
            &mut state,
            Msg::TokenReport {
                r#gen: *r#gen,
                result,
            },
            NOW_MS,
        );
        let line = body_of(&state, 120, 40, NOW_MS)[1].clone();
        assert!(line.contains("    today $3.00 "), "{line}");
        assert!(line.ends_with("█▅"), "{line}");

        let utc = body_of(&showing(rows(), |_| {}), 120, 40, NOW_MS)[1].clone();
        assert!(
            utc.contains("    today $5.00 "),
            "in UTC today is the 21st: {utc}"
        );
        assert!(utc.ends_with(" █"), "{utc}");
    }

    /// `line` as tier 0 draws it, the gauge's track `·` (T5-13).
    fn tier0(line: &str) -> String {
        line.replace('─', "·")
    }

    const BLOCK_LEFT: &str = " $5.00 at API prices    5.5M tokens    1 response";
    const BLOCK_LINE: &str = "▶ block ━━━━━━━━━╸──────────── 2h46m left · 41.2K tok/min · $3.20/h";

    /// `block(NOON, true)` for account `position`, `model`, with these figures.
    fn active(
        position: u32,
        model: Option<&str>,
        tokens_per_min: f64,
        cost_per_hour: Option<Cost>,
        projected_cost: Option<Cost>,
    ) -> TokenRow {
        let row = block(NOON, true);
        let PeriodKey::Block {
            start_ms,
            end_ms,
            observed,
            active: Some(a),
        } = row.period
        else {
            panic!("{row:?}");
        };
        let period = PeriodKey::Block {
            start_ms,
            end_ms,
            observed,
            active: Some(ActiveBlock {
                tokens_per_min,
                cost_per_hour,
                projected_cost,
                ..a
            }),
        };
        let cost = row.cost.map(|c| c.usd_nanos);
        token_row(period, Some(row_account(position)), model, row.counts, cost)
    }

    #[test]
    fn a_block_report_shows_the_active_block_s_progress() {
        // T5-13: 44.4 % of the block gone at 14:13:20, 2h46m40s left.
        let state = || showing_after("ppp", vec![block(NOON, true)]);
        let wide = body_of(&state(), 120, 40, NOW_MS);
        assert_eq!(wide[1], tier0(&format!("{BLOCK_LEFT}   {BLOCK_LINE}")));
        assert_eq!(width_of(&wide[1]) + 1, 120, "ending one column in");
        assert_eq!(wide[2], "");

        let lines = body_of(&state(), 80, 24, NOW_MS);
        assert_eq!(lines[1], BLOCK_LEFT);
        assert_eq!(lines[2], tier0(&format!(" {BLOCK_LINE}")));
        assert_eq!(lines[3], "");
        assert!(lines[4].starts_with(" BLOCK"), "{lines:?}");

        let theme = Theme::new(true);
        let want = letters(&[
            (" ", 't'),
            ("▶", 'a'),
            (" block ", 'm'),
            ("━━━━━━━━━╸", 'a'),
            ("────────────", 'f'),
            (" 2h46m ", 't'),
            ("left", 'm'),
            (" · ", 'm'),
            ("41.2K ", 't'),
            ("tok/min", 'm'),
            (" · ", 'm'),
            ("$3.20", 't'),
            ("/h", 'm'),
        ]);
        let (text, got) = row_at(state(), 80, 2);
        assert_eq!(text, format!(" {BLOCK_LINE}"), "in colour, on a `─` track");
        assert_eq!(got, canon(theme, &want));
    }

    #[test]
    fn the_block_line_counts_down_and_goes_at_the_block_s_end() {
        let state = showing_after("ppp", vec![block(NOON, true)]);
        let at = |now: i64| body_of(&state, 80, 24, now);
        assert!(
            at(NOW_MS + 61_000)[2].contains(" 2h45m left · "),
            "between queries"
        );
        let end = NOON + 5 * HOUR_MS;
        let last = format!("{}╸ <1m left", "━".repeat(21));
        assert!(at(end - 1_000)[2].contains(&last), "{:?}", at(end - 1_000));
        let lines = at(end);
        assert_eq!(lines[1], BLOCK_LEFT);
        assert_eq!(lines[2], "", "no longer active, the line goes");
        assert!(lines[3].starts_with(" BLOCK"), "{lines:?}");
        assert!(!body_of(&state, 120, 40, end)[1].contains('▶'));
    }

    #[test]
    fn the_block_line_gives_way_on_its_own_row() {
        let projected = |cost: Cost| {
            showing_after(
                "ppp",
                vec![active(
                    1,
                    None,
                    41_234.6,
                    Some(dollars(3_200_000_000)),
                    Some(cost),
                )],
            )
        };
        let whole = " ▶ block ━━━━━━━━━╸──────────── 2h46m left · 41.2K tok/min · $3.20/h · ≈ $38 at its end";
        let lines = body_of(&projected(dollars(38_120_000_000)), 120, 40, NOW_MS);
        assert_eq!(lines[1], BLOCK_LEFT, "it does not fit beside the figures");
        assert_eq!(lines[2], tier0(whole));
        assert_eq!(row_at(projected(dollars(38_120_000_000)), 120, 2).0, whole);
        assert_eq!(lines[3], "");

        let narrow =
            " ▶ block ━━━━━━──────── 2h46m left · 41.2K tok/min · $3.20/h · ≈ $38 at its end";
        let lines = body_of(&projected(dollars(38_120_000_000)), 80, 24, NOW_MS);
        assert_eq!(lines[2], tier0(narrow), "a 14-cell gauge");
        assert_eq!(width_of(&lines[2]), 79);

        let partial = Cost {
            usd_nanos: 38_120_000_000,
            partial: true,
        };
        let line = body_of(&projected(partial), 80, 24, NOW_MS)[2].clone();
        assert!(line.ends_with(" · ≈ ≥$38 at its end"), "{line}");
        let theme = Theme::new(true);
        assert_eq!(
            roles_of(projected(partial), 80, 2, "≥$38"),
            canon(theme, "wwww")
        );

        // The gauge keeps 8 cells at least: the rate gives way first, and the gauge takes the
        // room it leaves (20 cells).
        let long = showing_after(
            "ppp",
            vec![active(
                1,
                None,
                41_234.6,
                Some(Cost {
                    usd_nanos: 1_234_560_000_000,
                    partial: true,
                }),
                Some(Cost {
                    usd_nanos: 12_345_000_000_000,
                    partial: true,
                }),
            )],
        );
        assert_eq!(
            body_of(&long, 80, 24, NOW_MS)[2],
            format!(
                " ▶ block {}╸{} 2h46m left · ≥$1,234.56/h · ≈ ≥$12,345 at its end",
                "━".repeat(8),
                "·".repeat(11)
            )
        );
    }

    #[test]
    fn the_block_line_is_the_account_in_use_s_and_adds_its_model_rows() {
        // Review Focus 1: one active block per account (TR §9, §10.1).
        let two = || {
            vec![
                active(
                    1,
                    None,
                    41_234.6,
                    Some(dollars(3_200_000_000)),
                    Some(dollars(20_000_000_000)),
                ),
                active(
                    2,
                    None,
                    10_000.0,
                    Some(dollars(1_000_000_000)),
                    Some(dollars(3_000_000_000)),
                ),
            ]
        };
        let line = |state: &State| body_of(state, 80, 24, NOW_MS)[2].clone();
        let mut in_use = showing_after("ppp", two());
        let mut accounts = many(2);
        accounts[1].active = true;
        in_use.accounts.apply(snapshot(accounts));
        assert!(
            line(&in_use).ends_with(" · 10.0K tok/min · $1.00/h · ≈ $3.00 at its end"),
            "account 2 is in use: {}",
            line(&in_use)
        );
        let nobody = showing_after("ppp", two());
        assert!(
            line(&nobody).ends_with(" · 41.2K tok/min · $3.20/h · ≈ $20 at its end"),
            "the first active block: {}",
            line(&nobody)
        );

        let opus = Some("claude-opus-4-5-20251101");
        let sonnet = Some("claude-sonnet-4-5-20250929");
        let mut models = two();
        models[0] = active(
            1,
            opus,
            41_234.6,
            Some(dollars(3_200_000_000)),
            Some(dollars(20_000_000_000)),
        );
        models.insert(
            1,
            active(
                1,
                sonnet,
                1_000.0,
                Some(dollars(800_000_000)),
                Some(dollars(5_000_000_000)),
            ),
        );
        let state = showing_after("pppg", models.clone());
        assert!(
            line(&state).ends_with(" · 42.2K tok/min · $4.00/h · ≈ $25 at its end"),
            "{}",
            line(&state)
        );
        let unpriced = || {
            let mut rows = models.clone();
            rows[1] = active(1, sonnet, 1_000.0, None, None);
            showing_after("pppg", rows)
        };
        assert!(
            line(&unpriced()).ends_with(" · 42.2K tok/min · ≥$3.20/h · ≈ ≥$20 at its end"),
            "{}",
            line(&unpriced())
        );
        let theme = Theme::new(true);
        assert_eq!(
            roles_of(unpriced(), 80, 2, "≥$3.20"),
            canon(theme, "wwwwww")
        );
        assert_eq!(roles_of(unpriced(), 80, 2, "≥$20"), canon(theme, "wwww"));
    }
}
