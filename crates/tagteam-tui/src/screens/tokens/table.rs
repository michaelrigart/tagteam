//! The Tokens screen drawn (spec §5.3): its header, the report's table at the terminal's width,
//! the total line, the selected row's detail, and the footnotes. It reads nothing but the state.

use std::cell::OnceCell;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use tagteam_engine::tokens::{
    AccountSel, Cost, PeriodKey, RowAccount, TokenCounts, TokenGrouping, TokenPeriod, TokenReport,
    TokenRow,
};
use tagteam_engine::words::{self, Severity};

use super::TokensState;
use crate::state::State;
use crate::view::{bold, fit, selected, severity_style, width_of, wrap};

/// From this width the table adds the cache, the response count and a cost bar (spec §5.3).
pub const WIDE: usize = 120;
/// Drawn while a query for a report not yet read runs.
pub const LOADING: &str = "loading…";
/// What an empty report says while the index has never completed a refresh: the usage is not
/// missing from the range, it is not read in yet.
const NOT_INDEXED: &str = "No token usage indexed yet.";
/// The cost bar's cells.
const BAR: usize = 8;
/// The table keeps this many rows at least; the detail and the notes below it scroll (B25).
const TABLE_MIN: usize = 3;
/// The filter gives way to the drill-down on their row down to this many columns (B29).
const FILTER_LEAST: usize = 20;
/// The account column is cut no narrower than this while a more compact period can make room
/// (B24)...
const TEXT_MIN: usize = 6;
/// ... and the project column no narrower than this, else it is left out.
const PROJECT_MIN: usize = 4;
/// The model column keeps its whole value up to this many columns before it is cut: a realistic
/// Claude model ID (`claude-sonnet-4-5-20250929` is 26) with a suffix such as `[1m]`.
const MODEL_KEEP: usize = 30;
/// Where nothing else fits, the account keeps its position and an ellipsis (`1 …`) before the
/// model is cut.
const ACCOUNT_LEAST: usize = 3;
/// How many ways a session's or a block's span can be written (`span`).
const SPAN_LEVELS: usize = 4;
/// The active block's figures start this far in.
const FIGURES_INDENT: usize = 3;
/// A lower bound (spec §5.3): output with unfinalized responses, a partial cost.
const AT_LEAST: &str = "≥";

/// The table's columns, in the order they are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Col {
    /// The day, month, session's times or block's span.
    Period,
    /// A block's `observed` or `inferred`.
    Kind,
    Project,
    Account,
    Model,
    Input,
    Output,
    CacheWrite,
    CacheWrite5m,
    CacheWrite1h,
    CacheRead,
    Total,
    Responses,
    Cost,
    Bar,
}

impl Col {
    const ALL: [Col; 15] = [
        Col::Period,
        Col::Kind,
        Col::Project,
        Col::Account,
        Col::Model,
        Col::Input,
        Col::Output,
        Col::CacheWrite,
        Col::CacheWrite5m,
        Col::CacheWrite1h,
        Col::CacheRead,
        Col::Total,
        Col::Responses,
        Col::Cost,
        Col::Bar,
    ];

    fn head(self, period: TokenPeriod) -> &'static str {
        match self {
            Col::Period => match period {
                TokenPeriod::Day => "DAY",
                TokenPeriod::Month => "MONTH",
                TokenPeriod::Session => "SESSION",
                TokenPeriod::Block => "BLOCK",
            },
            Col::Kind | Col::Bar => "",
            Col::Project => "PROJECT",
            Col::Account => "ACCOUNT",
            Col::Model => "MODEL",
            Col::Input => "INPUT",
            Col::Output => "OUTPUT",
            Col::CacheWrite => "CACHE W",
            Col::CacheWrite5m => "CW 5M",
            Col::CacheWrite1h => "CW 1H",
            Col::CacheRead => "CACHE R",
            Col::Total => "TOTAL",
            Col::Responses => "RESP",
            Col::Cost => "COST",
        }
    }

    /// Text columns share what the fixed ones leave, and are cut to it.
    fn text(self) -> bool {
        matches!(self, Col::Project | Col::Account | Col::Model)
    }

    /// Numbers are right-aligned.
    fn number(self) -> bool {
        !matches!(
            self,
            Col::Period | Col::Kind | Col::Project | Col::Account | Col::Model | Col::Bar
        )
    }
}

/// A cell's text, and a style for when it says `≥`.
struct Cell {
    text: String,
    style: Style,
}

impl Cell {
    fn plain(text: impl Into<String>) -> Cell {
        Cell {
            text: text.into(),
            style: Style::new(),
        }
    }
}

/// `n` bytes as the header shows a refresh's progress: decimal units to one decimal place
/// (`1.2 GB`), rounded half up.
pub fn bytes(n: u64) -> String {
    const UNITS: [(u128, &str); 4] = [
        (1_000, "kB"),
        (1_000_000, "MB"),
        (1_000_000_000, "GB"),
        (1_000_000_000_000, "TB"),
    ];
    if n < 1_000 {
        return format!("{n} B");
    }
    let tenths = |unit: u128| (u128::from(n) * 10 + unit / 2) / unit;
    let (unit, suffix) = UNITS
        .iter()
        .copied()
        .find(|&(unit, _)| tenths(unit) < 10_000)
        .unwrap_or(UNITS[3]);
    let t = tenths(unit);
    format!("{}.{} {suffix}", t / 10, t % 10)
}

/// `ms` in `tz`.
fn zoned(ms: i64, tz: &TimeZone) -> Option<jiff::Zoned> {
    Some(Timestamp::from_millisecond(ms).ok()?.to_zoned(tz.clone()))
}

/// `ms` in `tz` as `2026-10-14 09:12`.
fn when(ms: i64, tz: &TimeZone) -> String {
    zoned(ms, tz).map_or_else(
        || words::MISSING.to_owned(),
        |z| z.strftime("%Y-%m-%d %H:%M").to_string(),
    )
}

/// A span in `tz` (B16, B24), written at `level`, each more compact than the one before:
/// 0. `10-14 09:12–11:40`, the year added when it is not `now_ms`'s, and the second date when
///    it ends on another day;
/// 1. the same without years;
/// 2. the end's time alone, with `+Nd` when it ends `N` days later: `10-14 23:50–00:30+1d`;
/// 3. the start alone, which the selected row's detail completes (B25).
fn span(start_ms: i64, end_ms: i64, tz: &TimeZone, now_ms: i64, level: usize) -> String {
    let (Some(a), Some(b), Some(now)) = (zoned(start_ms, tz), zoned(end_ms, tz), zoned(now_ms, tz))
    else {
        return words::MISSING.to_owned();
    };
    let date = |z: &jiff::Zoned| {
        if level == 0 && z.year() != now.year() {
            z.strftime("%Y-%m-%d").to_string()
        } else {
            z.strftime("%m-%d").to_string()
        }
    };
    let start = format!("{} {}", date(&a), a.strftime("%H:%M"));
    let days = (b.date() - a.date()).get_days();
    match level {
        _ if level >= 3 => start,
        _ if days == 0 => format!("{start}–{}", b.strftime("%H:%M")),
        2 => format!("{start}–{}+{days}d", b.strftime("%H:%M")),
        _ => format!("{start}–{} {}", date(&b), b.strftime("%H:%M")),
    }
}

/// A row's period as its first column says it, a span written at `level` (`span`).
fn period_text(period: &PeriodKey, tz: &TimeZone, now_ms: i64, level: usize) -> String {
    match period {
        PeriodKey::Day { date } => {
            format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day())
        }
        PeriodKey::Month { year, month } => format!("{year:04}-{month:02}"),
        PeriodKey::Session {
            first_ms, last_ms, ..
        } => span(*first_ms, *last_ms, tz, now_ms, level),
        PeriodKey::Block {
            start_ms, end_ms, ..
        } => span(*start_ms, *end_ms, tz, now_ms, level),
    }
}

/// An account as the table names it (spec §5.3): its position and alias, else its label;
/// `removed`; `unattributed`.
fn account_text(account: &RowAccount) -> String {
    match account {
        RowAccount::Account {
            position,
            label,
            alias,
            ..
        } => format!("{position} {}", alias.as_deref().unwrap_or(label)),
        RowAccount::Removed { .. } => "removed".to_owned(),
        RowAccount::Unattributed => "unattributed".to_owned(),
    }
}

/// An account in full, as the detail and the filter name it: `1 work (a@x.co)`, `removed <id>`.
pub fn account_name(account: &RowAccount) -> String {
    match account {
        RowAccount::Account {
            position,
            label,
            alias: Some(alias),
            ..
        } => format!("{position} {alias} ({label})"),
        RowAccount::Account {
            position, label, ..
        } => format!("{position} {label}"),
        RowAccount::Removed { id } => format!("removed {id}"),
        RowAccount::Unattributed => "unattributed".to_owned(),
    }
}

/// A cost as a cell says it: dollars and cents, `≥` before a partial one, `—` for none.
fn cost_text(cost: Option<&Cost>) -> String {
    match cost {
        Some(c) if c.partial => format!("{AT_LEAST}{}", words::dollars(c.usd_nanos)),
        Some(c) => words::dollars(c.usd_nanos),
        None => words::MISSING.to_owned(),
    }
}

/// Each column's widest cell over one report's rows and total, and the period's at each level
/// of `span`.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Found {
    cols: Vec<usize>,
    periods: [usize; SPAN_LEVELS],
}

/// A report's `Found`, found the first time the report is drawn: a year by session is thousands
/// of rows, which a frame then only lays out.
#[derive(Debug, Default)]
pub(super) struct Widths(OnceCell<Found>);

/// The table laid out for one report at one width.
struct Table<'a> {
    report: &'a TokenReport,
    widths: &'a Widths,
    cols: Vec<(Col, usize)>,
    /// The column whose total line cell says `TOTAL`: the first.
    lead: Col,
    /// How compact the period's spans are (`span`).
    level: usize,
    /// The largest row cost, for the bar.
    most: u64,
    color: bool,
    now_ms: i64,
}

impl<'a> Table<'a> {
    /// The columns `report` takes at `width` (spec §5.3): period, account, input, output, total
    /// and cost always, with the project for sessions, the kind for blocks and the model when
    /// grouped by it; but not the period and the project for one session's models, whose rows all
    /// share them, and which the header's drill-down and the detail say. From `WIDE` columns, the cache write (split into 5-minute and 1-hour when
    /// that still fits), the cache read, the responses and the bar. Text columns share the rest.
    fn new(
        report: &'a TokenReport,
        widths: &'a Widths,
        width: usize,
        color: bool,
        now_ms: i64,
    ) -> Table<'a> {
        let q = &report.query;
        let one_session = q.session.is_some();
        let mut cols = Vec::new();
        if !one_session {
            cols.push(Col::Period);
            match q.period {
                TokenPeriod::Block => cols.push(Col::Kind),
                TokenPeriod::Session => cols.push(Col::Project),
                TokenPeriod::Day | TokenPeriod::Month => {}
            }
        }
        let lead = cols.first().copied().unwrap_or(Col::Account);
        cols.push(Col::Account);
        if matches!(
            q.applied_grouping(),
            TokenGrouping::Model | TokenGrouping::AccountModel
        ) {
            cols.push(Col::Model);
        }
        cols.extend([Col::Input, Col::Output]);
        let wide = width >= WIDE;
        if wide {
            cols.extend([Col::CacheWrite, Col::CacheRead]);
        }
        cols.push(Col::Total);
        if wide {
            cols.push(Col::Responses);
        }
        cols.push(Col::Cost);
        if wide {
            cols.push(Col::Bar);
        }
        let most = report
            .rows
            .iter()
            .filter_map(|r| r.cost.map(|c| c.usd_nanos))
            .max()
            .unwrap_or(0);
        let mut table = Table {
            report,
            widths,
            cols: cols.into_iter().map(|c| (c, 0)).collect(),
            lead,
            level: 0,
            most,
            color,
            now_ms,
        };
        let split = [Col::CacheWrite5m, Col::CacheWrite1h];
        let natural = table.natural();
        let at = table.cols.iter().position(|(c, _)| *c == Col::CacheWrite);
        if let Some(at) = at {
            let extra: usize = split
                .iter()
                .map(|c| table.widest(*c) + 2)
                .sum::<usize>()
                .saturating_sub(table.widest(Col::CacheWrite) + 2);
            if natural + extra <= width {
                table.cols.splice(at..=at, split.map(|c| (c, 0)));
            }
        }
        table.fit(width);
        table
    }

    /// The width every column wants, its gaps and the one-column margin included.
    fn natural(&self) -> usize {
        1 + self
            .cols
            .iter()
            .map(|(c, _)| self.widest(*c))
            .sum::<usize>()
            + 2 * self.cols.len().saturating_sub(1)
    }

    /// What `Widths` holds, found once per report.
    fn found(&self) -> &Found {
        self.widths.0.get_or_init(|| Found {
            cols: Col::ALL.iter().map(|c| self.widest_cell(*c, 0)).collect(),
            periods: std::array::from_fn(|level| self.widest_cell(Col::Period, level)),
        })
    }

    /// The widest of `col`'s heading and cells, the total's among them, the period's at the
    /// table's level.
    fn widest(&self, col: Col) -> usize {
        if col == Col::Period {
            return self.found().periods[self.level];
        }
        self.found().cols[Col::ALL.iter().position(|c| *c == col).unwrap_or(0)]
    }

    fn widest_cell(&self, col: Col, level: usize) -> usize {
        if col == Col::Bar {
            return BAR;
        }
        let tz = &self.report.query.tz;
        let cell = |r: &TokenRow| match col {
            Col::Period => period_text(&r.period, tz, self.now_ms, level),
            _ => self.cell(col, r, tz).text,
        };
        self.report
            .rows
            .iter()
            .map(|r| width_of(&cell(r)))
            .chain([width_of(&self.total_cell(col).text)])
            .fold(width_of(col.head(self.report.query.period)), usize::max)
    }

    /// Lays the columns out in `width` (B17, B24). The numbers take their widest and are never
    /// cut. The period takes the fullest form of its spans that leaves each text column its
    /// least: the model its whole value up to `MODEL_KEEP`, the account `TEXT_MIN` and the
    /// project `PROJECT_MIN` (or its own width when that is less). Where no form does, the
    /// project gives way first and is left out. Where still none does, the most compact period
    /// is used, and the account gives way to `ACCOUNT_LEAST` and the model to one column, and
    /// the period itself is cut when that leaves the text columns no more. What is left over then
    /// goes, in turn, to the model up to `MODEL_KEEP`, the account, the project, and the model
    /// beyond `MODEL_KEEP`, and the text columns are cut by display width to what they got. A
    /// number is never cut.
    fn fit(&mut self, width: usize) {
        let periods = self.found().periods;
        let period_on = self.cols.iter().any(|(c, _)| *c == Col::Period);
        let least = |t: &Table, col: Col| match col {
            Col::Model => t.widest(col).min(MODEL_KEEP),
            Col::Project => t.widest(col).min(PROJECT_MIN),
            _ => t.widest(col).min(TEXT_MIN),
        };
        let squeezed = |t: &Table, col: Col| match col {
            Col::Model => t.widest(col).min(1),
            _ => t.widest(col).min(ACCOUNT_LEAST),
        };
        let (level, tight, gaps, fixed, texts, floors) = loop {
            let gaps = 1 + 2 * self.cols.len().saturating_sub(1);
            let fixed: usize = self
                .cols
                .iter()
                .filter(|(c, _)| !c.text() && *c != Col::Period)
                .map(|(c, _)| self.widest(*c))
                .sum();
            let texts: Vec<usize> = (0..self.cols.len())
                .filter(|&i| self.cols[i].0.text())
                .collect();
            let wanted: usize = texts.iter().map(|&i| least(self, self.cols[i].0)).sum();
            let level = (0..SPAN_LEVELS).find(|&l| {
                let period = if period_on { periods[l] } else { 0 };
                fixed + gaps + period + wanted <= width
            });
            match (
                level,
                self.cols.iter().position(|(c, _)| *c == Col::Project),
            ) {
                (None, Some(at)) => {
                    self.cols.remove(at);
                }
                (Some(level), _) => break (level, false, gaps, fixed, texts, wanted),
                (None, None) => {
                    let squeezed = texts.iter().map(|&i| squeezed(self, self.cols[i].0)).sum();
                    break (SPAN_LEVELS - 1, true, gaps, fixed, texts, squeezed);
                }
            }
        };
        self.level = level;
        let period = if period_on {
            periods[level].min(width.saturating_sub(fixed + gaps + floors))
        } else {
            0
        };
        let mut room = width.saturating_sub(fixed + gaps + period);
        let mut got: Vec<usize> = texts
            .iter()
            .map(|&i| {
                let col = self.cols[i].0;
                if tight {
                    squeezed(self, col)
                } else {
                    least(self, col)
                }
            })
            .collect();
        room = room.saturating_sub(got.iter().sum());
        let steps = [
            (Col::Model, MODEL_KEEP),
            (Col::Account, usize::MAX),
            (Col::Project, usize::MAX),
            (Col::Model, usize::MAX),
        ];
        for (col, up_to) in steps {
            if let Some(n) = texts.iter().position(|&i| self.cols[i].0 == col) {
                let want = self.widest(col).min(up_to);
                let add = want.saturating_sub(got[n]).min(room);
                got[n] += add;
                room -= add;
            }
        }
        for (n, &i) in texts.iter().enumerate() {
            self.cols[i].1 = got[n];
        }
        for i in 0..self.cols.len() {
            let col = self.cols[i].0;
            if col == Col::Period {
                self.cols[i].1 = period;
            } else if !col.text() {
                self.cols[i].1 = self.widest(col);
            }
        }
    }

    fn counts_cell(&self, col: Col, c: &TokenCounts, unfinalized: u64, responses: u64) -> Cell {
        let warning = severity_style(self.color, Severity::Warning);
        match col {
            Col::Input => Cell::plain(words::scaled(c.input)),
            Col::Output if unfinalized > 0 => Cell {
                text: format!("{AT_LEAST}{}", words::scaled(c.output)),
                style: warning,
            },
            Col::Output => Cell::plain(words::scaled(c.output)),
            Col::CacheWrite => Cell::plain(words::scaled(c.cache_write())),
            Col::CacheWrite5m => Cell::plain(words::scaled(c.cache_write_5m)),
            Col::CacheWrite1h => Cell::plain(words::scaled(c.cache_write_1h)),
            Col::CacheRead => Cell::plain(words::scaled(c.cache_read)),
            Col::Total => Cell::plain(words::scaled(c.total())),
            Col::Responses => Cell::plain(responses.to_string()),
            _ => Cell::plain(""),
        }
    }

    fn cost_cell(&self, cost: Option<&Cost>) -> Cell {
        let style = match cost {
            Some(c) if c.partial => severity_style(self.color, Severity::Warning),
            _ => Style::new(),
        };
        Cell {
            text: cost_text(cost),
            style,
        }
    }

    /// `row`'s cell in `col`.
    fn cell(&self, col: Col, row: &TokenRow, tz: &TimeZone) -> Cell {
        match col {
            Col::Period => Cell::plain(period_text(&row.period, tz, self.now_ms, self.level)),
            Col::Kind => Cell::plain(match row.period {
                PeriodKey::Block { observed: true, .. } => "observed",
                PeriodKey::Block { .. } => "inferred",
                _ => "",
            }),
            Col::Project => Cell::plain(match &row.period {
                PeriodKey::Session {
                    project: Some(p), ..
                } => p.clone(),
                _ => words::MISSING.to_owned(),
            }),
            Col::Account => Cell::plain(match &row.account {
                Some(a) => account_text(a),
                None => row
                    .accounts
                    .iter()
                    .map(account_text)
                    .collect::<Vec<_>>()
                    .join(", "),
            }),
            Col::Model => Cell::plain(row.model.clone().unwrap_or_else(|| row.models.join(", "))),
            Col::Cost => self.cost_cell(row.cost.as_ref()),
            Col::Bar => Cell::plain(match row.cost {
                Some(c) if self.most > 0 && c.usd_nanos > 0 => {
                    let cells =
                        (u128::from(c.usd_nanos) * BAR as u128).div_ceil(u128::from(self.most));
                    "█".repeat((cells as usize).clamp(1, BAR))
                }
                _ => String::new(),
            }),
            _ => self.counts_cell(col, &row.counts, row.unfinalized, row.responses),
        }
    }

    /// The total line's cell in `col`.
    fn total_cell(&self, col: Col) -> Cell {
        let t = &self.report.total;
        match col {
            _ if col == self.lead => Cell::plain("TOTAL"),
            Col::Kind | Col::Project | Col::Account | Col::Model | Col::Bar => Cell::plain(""),
            Col::Cost => self.cost_cell(t.cost.as_ref()),
            _ => self.counts_cell(col, &t.counts, t.unfinalized, t.responses),
        }
    }

    /// One line of cells: each padded or cut to its column, numbers on the right, two columns
    /// apart, after a one-column margin. `blank` leaves the period's own columns empty. A row's
    /// project keeps its end when it is cut; every other cell, its start.
    fn line(&self, cells: Vec<Cell>, blank: bool, row: bool) -> Line<'static> {
        let mut spans = vec![Span::raw(" ")];
        for (i, ((col, w), cell)) in self.cols.iter().zip(cells).enumerate() {
            if i > 0 {
                spans.push(Span::raw("  "));
            }
            let text = if blank && matches!(col, Col::Period | Col::Kind | Col::Project) {
                String::new()
            } else if *col == Col::Project && row {
                fit_left(&cell.text, *w)
            } else {
                fit(&cell.text, *w)
            };
            let gap = " ".repeat(w.saturating_sub(width_of(&text)));
            if col.number() {
                spans.push(Span::raw(gap));
                spans.push(Span::styled(text, cell.style));
            } else {
                spans.push(Span::styled(text, cell.style));
                spans.push(Span::raw(gap));
            }
        }
        trim(Line::from(spans))
    }

    fn header(&self) -> Line<'static> {
        let period = self.report.query.period;
        let cells = self
            .cols
            .iter()
            .map(|(c, _)| Cell::plain(c.head(period)))
            .collect();
        self.line(cells, false, false).style(bold())
    }

    fn row(&self, row: &TokenRow, blank: bool) -> Line<'static> {
        let tz = &self.report.query.tz;
        let cells = self
            .cols
            .iter()
            .map(|(c, _)| self.cell(*c, row, tz))
            .collect();
        self.line(cells, blank, true)
    }

    fn total(&self) -> Line<'static> {
        let cells = self.cols.iter().map(|(c, _)| self.total_cell(*c)).collect();
        self.line(cells, false, false).style(bold())
    }
}

/// `s` cut to `w` columns from the left, so a path keeps its end: `…/Code/tagteam`.
fn fit_left(s: &str, w: usize) -> String {
    if width_of(s) <= w {
        return s.to_owned();
    }
    if w == 0 {
        return String::new();
    }
    let mut kept: Vec<char> = Vec::new();
    for c in s.chars().rev() {
        kept.push(c);
        let tail: String = kept.iter().rev().collect();
        if width_of(&tail) > w - 1 {
            kept.pop();
            break;
        }
    }
    let tail: String = kept.iter().rev().collect();
    format!("…{tail}")
}

/// `line` without the spaces its last cell was padded with.
fn trim(line: Line<'static>) -> Line<'static> {
    let style = line.style;
    let mut spans = line.spans;
    while spans.last().is_some_and(|s| s.content.trim().is_empty()) {
        spans.pop();
    }
    Line::from(spans).style(style)
}

/// `parts` laid out after `head` in rows of at most `width` columns, `sep` between two parts on
/// a row, each row after the first `indent` columns in (B26). A part that fits no row whole
/// wraps at its spaces, and a word wider than a row breaks by width (`view::wrap`): nothing is
/// cut, for what lies below the total scrolls (B29).
fn laid_out(
    head: Vec<Span<'static>>,
    parts: &[String],
    sep: &str,
    indent: usize,
    width: usize,
) -> Vec<Line<'static>> {
    let room = width.saturating_sub(indent).max(1);
    let pad = || Span::raw(" ".repeat(indent));
    let mut lines = Vec::new();
    let mut used: usize = head.iter().map(|s| width_of(&s.content)).sum();
    let mut spans = head;
    for part in parts {
        let fresh = used <= indent;
        let w = width_of(part);
        if fresh && used + w <= width {
            spans.push(Span::raw(part.clone()));
            used += w;
            continue;
        }
        if !fresh && used + width_of(sep) + w <= width {
            spans.push(Span::raw(format!("{sep}{part}")));
            used += width_of(sep) + w;
            continue;
        }
        if !fresh {
            lines.push(Line::from(std::mem::replace(&mut spans, vec![pad()])));
            used = indent;
        }
        for (i, chunk) in wrap(part, room).into_iter().enumerate() {
            if i > 0 {
                lines.push(Line::from(std::mem::replace(&mut spans, vec![pad()])));
                used = indent;
            }
            used += width_of(&chunk);
            spans.push(Span::raw(chunk));
        }
    }
    lines.push(Line::from(spans));
    lines
}

/// The active block's figures (spec §5.3, §6.4) at `now_ms`: its elapsed and remaining time,
/// which count on between queries from its start and end, its burn rate and its projection.
/// `None` once it has ended: the row is no longer active.
fn figures(row: &TokenRow, now_ms: i64) -> Option<Vec<String>> {
    let PeriodKey::Block {
        start_ms,
        end_ms,
        active: Some(a),
        ..
    } = &row.period
    else {
        return None;
    };
    if now_ms >= *end_ms {
        return None;
    }
    let elapsed = (now_ms - start_ms).max(0) / 1000;
    let left = (end_ms - now_ms) / 1000;
    Some(vec![
        "active".to_owned(),
        format!("{} elapsed", words::duration(elapsed)),
        format!("{} left", words::duration(left)),
        format!(
            "{} tokens/min",
            words::scaled(a.tokens_per_min.max(0.0).round() as u64)
        ),
        format!("{}/h", cost_text(a.cost_per_hour.as_ref())),
        format!(
            "projected {} tokens, {}",
            words::scaled(a.projected_tokens),
            cost_text(a.projected_cost.as_ref())
        ),
    ])
}

/// One line of the table's scrolling part.
enum Item {
    /// A report row; `repeat` when its period is the row's before it.
    Row { repeat: bool },
    /// A line of the active block's figures under its row; a block's rows by model have them
    /// under the first only.
    Figures(Line<'static>),
}

/// The table's lines at `width`, and the line each row starts at.
fn items(report: &TokenReport, width: usize, now_ms: i64) -> (Vec<(usize, Item)>, Vec<usize>) {
    let mut items = Vec::new();
    let mut starts = Vec::with_capacity(report.rows.len());
    for (index, row) in report.rows.iter().enumerate() {
        starts.push(items.len());
        let repeat = index > 0 && report.rows[index - 1].period == row.period;
        items.push((index, Item::Row { repeat }));
        let parts = figures(row, now_ms).filter(|_| !repeat);
        if let Some(parts) = parts {
            let head = vec![Span::raw(" ".repeat(FIGURES_INDENT))];
            for line in laid_out(head, &parts, " · ", FIGURES_INDENT, width) {
                items.push((index, Item::Figures(line)));
            }
        }
    }
    (items, starts)
}

/// The first line to show so that lines `first..=last` show, moving as little as it can, and
/// never past the last full page (R22).
fn keep_visible(offset: usize, first: usize, last: usize, rows: usize, lines: usize) -> usize {
    let rows = rows.max(1);
    let offset = if first < offset {
        first
    } else if last >= offset + rows {
        (last + 1 - rows).min(first)
    } else {
        offset
    };
    offset.min(lines.saturating_sub(rows))
}

/// The grouping's words in the header.
fn grouping_text(g: TokenGrouping) -> &'static str {
    match g {
        TokenGrouping::None => "none",
        TokenGrouping::Account => "account",
        TokenGrouping::Model => "model",
        TokenGrouping::AccountModel => "account, model",
    }
}

/// The filter's words in the header: `all accounts · all models`, else what it keeps.
fn filter_text(state: &State) -> String {
    let t = &state.tokens;
    let accounts = match t.accounts.as_deref() {
        None => "all accounts".to_owned(),
        Some([AccountSel::Unattributed]) => "unattributed".to_owned(),
        Some([AccountSel::Account(id)]) => state
            .accounts
            .snapshot()
            .and_then(|s| {
                s.providers
                    .iter()
                    .flat_map(|p| &p.accounts)
                    .find(|v| &v.row.id == id)
            })
            .map_or_else(
                || "1 account".to_owned(),
                |v| format!("{} {}", v.row.position, words::name(&v.row)),
            ),
        Some(list) => words::plural(list.len() as u64, "account"),
    };
    let models = match t.models.as_deref() {
        None => "all models".to_owned(),
        Some([one]) => one.clone(),
        Some(list) => words::plural(list.len() as u64, "model"),
    };
    format!("filter: {accounts} · {models}")
}

/// The rows the drill-down came through, from the top: each one's period and its account and
/// model when it has them.
fn crumbs(t: &TokensState, now_ms: i64) -> Option<String> {
    let parents: Vec<String> = t.levels[..t.levels.len() - 1]
        .iter()
        .filter_map(|level| {
            let report = level.report.as_ref()?;
            let key = level.selected.as_ref()?;
            let row = report.rows.iter().find(|r| &r.key == key)?;
            let period = match &row.period {
                PeriodKey::Session { session_id, .. } => format!("session {session_id}"),
                p => period_text(p, &report.query.tz, now_ms, 0),
            };
            let mut parts = vec![period];
            parts.extend(row.account.as_ref().map(account_text));
            parts.extend(row.model.clone());
            Some(parts.join(" "))
        })
        .collect();
    (!parents.is_empty()).then(|| format!("› {}", parents.join(" › ")))
}

/// The screen's own header (spec §5.3), three rows at most (B29):
/// 1. the periods with the top level's bracketed, its range, and the grouping its report applies
///    (blocks are per account), whatever level is shown: 60 columns at the widest;
/// 2. the filter and the rows the drill-down came through, on the first row when they fit
///    there whole; else on a row of their own, the filter cut first, to `FILTER_LEAST`, then
///    the drill-down, each with `…`. The filter's dialog shows it whole, and the detail the
///    rows;
/// 3. the zone and the index's state, `catching up` while the index is partial and no refresh
///    shows its progress instead, and `updating…` while the report on screen is read again, cut
///    with `…` past the row.
fn header(state: &State, width: usize, now_ms: i64) -> Vec<Line<'static>> {
    let t = &state.tokens;
    let mut tabs = vec![Span::raw(" ")];
    for (i, (period, name)) in [
        (TokenPeriod::Day, "Day"),
        (TokenPeriod::Month, "Month"),
        (TokenPeriod::Session, "Session"),
        (TokenPeriod::Block, "Block"),
    ]
    .into_iter()
    .enumerate()
    {
        if i > 0 {
            tabs.push(Span::raw(" "));
        }
        if period == t.period {
            tabs.push(Span::styled(format!("[{name}]"), bold()));
        } else {
            tabs.push(Span::raw(name));
        }
    }
    let applied = t.top_query(now_ms).applied_grouping();
    tabs.push(Span::raw(format!(
        "  {}  group: {}",
        t.range.label(t.period),
        grouping_text(applied)
    )));
    let filter = filter_text(state);
    let crumb = crumbs(t, now_ms);
    let whole = match &crumb {
        Some(crumb) => format!("{filter}  {crumb}"),
        None => filter.clone(),
    };
    let used: usize = tabs.iter().map(|s| width_of(&s.content)).sum();
    let room = width.saturating_sub(1);
    let mut lines = if used + 2 + width_of(&whole) <= width {
        tabs.push(Span::raw(format!("  {whole}")));
        vec![Line::from(tabs)]
    } else {
        let second = match &crumb {
            None => fit(&filter, room),
            Some(crumb) => {
                let rest = room.saturating_sub(2);
                let least = rest.saturating_sub(width_of(crumb)).max(FILTER_LEAST);
                let filter = fit(&filter, width_of(&filter).min(least));
                let crumb = fit(crumb, rest.saturating_sub(width_of(&filter)));
                format!("{filter}  {crumb}")
            }
        };
        vec![Line::from(tabs), Line::from(format!(" {second}"))]
    };
    let zone = t
        .report()
        .map(|r| r.time_zone.clone())
        .unwrap_or_else(|| t.tz.iana_name().unwrap_or("UTC").to_owned());
    let mut status = vec![zone];
    let index = t.watch.state();
    let tz = &t.tz;
    if let Some(p) = t.watch.refreshing() {
        status.push(if p.bytes_total > 0 {
            format!(
                "indexing transcripts {} / {}",
                bytes(p.bytes_done),
                bytes(p.bytes_total)
            )
        } else {
            "indexing transcripts…".to_owned()
        });
    } else if t.watch.busy() {
        status.push("catching up elsewhere".to_owned());
    } else if let Some(at) = index.and_then(|i| i.refreshed_at_ms) {
        status.push(format!("refreshed {}", clock(at, tz, now_ms)));
    }
    if t.watch.refreshing().is_none() && index.is_some_and(|i| i.refreshed_at_ms.is_none()) {
        status.push("catching up".to_owned());
    }
    if t.pending && t.report().is_some() {
        status.push("updating…".to_owned());
    }
    lines.push(Line::from(format!(" {}", fit(&status.join(" · "), room))));
    lines
}

/// A time in `tz` as the header says it: `14:12` today, else with its date.
fn clock(ms: i64, tz: &TimeZone, now_ms: i64) -> String {
    let (Some(at), Some(now)) = (zoned(ms, tz), zoned(now_ms, tz)) else {
        return words::MISSING.to_owned();
    };
    if at.date() == now.date() {
        at.strftime("%H:%M").to_string()
    } else {
        at.strftime("%Y-%m-%d %H:%M").to_string()
    }
}

/// The selected row in full (B11, B25), in rows at `width`: its numbers first, laid out by
/// their parts and never cut; then what tells it apart and what it holds, wrapped: a session's
/// ID, project and times, a block's span and kind, every account and model.
fn detail_rows(row: &TokenRow, tz: &TimeZone, now_ms: i64, width: usize) -> Vec<Line<'static>> {
    let c = &row.counts;
    let numbers = [
        words::plural(row.responses, "response"),
        format!(
            "cache write {} (5m {}, 1h {})",
            words::scaled(c.cache_write()),
            words::scaled(c.cache_write_5m),
            words::scaled(c.cache_write_1h)
        ),
        format!("cache read {}", words::scaled(c.cache_read)),
    ];
    let mut text = match &row.period {
        PeriodKey::Session {
            session_id,
            project,
            first_ms,
            last_ms,
        } => vec![
            format!("session {session_id}"),
            project.clone().unwrap_or_else(|| "no project".to_owned()),
            format!("{} – {}", when(*first_ms, tz), when(*last_ms, tz)),
        ],
        PeriodKey::Block {
            start_ms,
            end_ms,
            observed,
            ..
        } => vec![
            format!("{} – {}", when(*start_ms, tz), when(*end_ms, tz)),
            if *observed { "observed" } else { "inferred" }.to_owned(),
        ],
        p => vec![period_text(p, tz, now_ms, 0)],
    };
    if !row.accounts.is_empty() {
        let names: Vec<String> = row.accounts.iter().map(account_name).collect();
        text.push(names.join(", "));
    }
    if !row.models.is_empty() {
        text.push(row.models.join(", "));
    }
    let mut rows = laid_out(vec![Span::raw(" ")], &numbers, " · ", 1, width);
    rows.extend(laid_out(vec![Span::raw(" ")], &text, " · ", 1, width));
    rows
}

/// What lies below the total (B25): the selected row's detail, then the footnotes (TR §11.2,
/// B15), each wrapped at `width` and never cut, for the region scrolls (B29).
fn below(state: &State, width: usize, now_ms: i64) -> Vec<Line<'static>> {
    let t = &state.tokens;
    let mut lines = Vec::new();
    let Some(report) = t.report().filter(|r| !r.rows.is_empty()) else {
        return lines;
    };
    if let Some(row) = t.selected_row() {
        lines.extend(detail_rows(row, &report.query.tz, now_ms, width));
    }
    for note in words::token_footnotes(report, AT_LEAST, true) {
        lines.extend(laid_out(vec![Span::raw(" ")], &[note], " ", 1, width));
    }
    lines
}

/// What the body holds below the header, at its size: the table's rows, then the total, then
/// what lies below it (`below`) in as many rows as it needs, half the body at most and leaving
/// the table `TABLE_MIN` rows. What does not fit there scrolls with `J`/`K` (B25), and its last
/// row says which lines show. The total follows the last row shown, so it is at the foot of a
/// table that fills the body.
struct Plan {
    header: Vec<Line<'static>>,
    rows: usize,
    below: Vec<Line<'static>>,
}

/// How many rows what lies below the total takes: as many as its `lines`, half of `height` at
/// most, and never the table's last `TABLE_MIN`.
fn below_rows(lines: usize, header: usize, height: usize) -> usize {
    let fixed = header + 2;
    lines.min(height.saturating_sub(fixed + TABLE_MIN).min(height / 2))
}

/// The lines that show of `lines` in `rows` rows from line `from` (clamped, R22), with a last row
/// that says which when they do not all fit; and the furthest `from` goes.
fn window(lines: Vec<Line<'static>>, from: usize, rows: usize) -> (Vec<Line<'static>>, usize) {
    let total = lines.len();
    if total <= rows {
        return (lines, 0);
    }
    let shown = if rows >= 2 { rows - 1 } else { rows };
    let furthest = total - shown;
    let from = from.min(furthest);
    let mut out: Vec<Line<'static>> = lines.into_iter().skip(from).take(shown).collect();
    if rows >= 2 {
        let hint = format!(" J K: lines {}–{} of {total}", from + 1, from + shown);
        out.push(Line::from(Span::styled(hint, bold())));
    }
    (out, furthest)
}

/// Where what lies below the total is scrolled to: from its top for a row other than the one it
/// was scrolled for.
fn below_from(t: &TokensState) -> usize {
    match (&t.below_for, t.selected()) {
        (Some(key), Some(selected)) if key == selected => t.below_from,
        _ => 0,
    }
}

fn plan(state: &State, area: Rect, now_ms: i64) -> Plan {
    let width = usize::from(area.width);
    let height = usize::from(area.height);
    let header = header(state, width, now_ms);
    let lines = below(state, width, now_ms);
    let rows = below_rows(lines.len(), header.len(), height);
    let (below, _) = window(lines, below_from(&state.tokens), rows);
    let table = height.saturating_sub(header.len() + 2 + below.len());
    Plan {
        header,
        rows: table,
        below,
    }
}

/// `J`/`K` (B25): what lies below the total moves `by` a line, stopping at either end.
pub(super) fn scroll_below(state: &mut State, area: Rect, now_ms: i64, by: isize) {
    let width = usize::from(area.width);
    let header = header(state, width, now_ms).len();
    let lines = below(state, width, now_ms);
    let rows = below_rows(lines.len(), header, usize::from(area.height));
    let from = below_from(&state.tokens);
    let (_, furthest) = window(lines, from, rows);
    let from = from.min(furthest) as isize;
    let t = &mut state.tokens;
    t.below_from = (from + by).clamp(0, furthest as isize) as usize;
    t.below_for = t.selected().cloned();
}

/// How many table rows show in `area`, for the keys that page (spec §5.6).
pub(super) fn page(state: &State, area: Rect, now_ms: i64) -> usize {
    plan(state, area, now_ms).rows
}

/// Keeps the selected row in view after a key moved it (R22: from what shows).
pub(super) fn follow(state: &mut State, area: Rect, now_ms: i64) {
    let rows = page(state, area, now_ms);
    let t = &state.tokens;
    let (Some(report), Some(at)) = (t.report(), t.selected_index()) else {
        return;
    };
    let (items, starts) = items(report, usize::from(area.width), now_ms);
    let last = starts.get(at + 1).copied().unwrap_or(items.len()) - 1;
    let offset = keep_visible(t.level().offset, starts[at], last, rows, items.len());
    state.tokens.level_mut().offset = offset;
}

/// The selected row in reverse video across the whole width.
fn highlight(row: Line<'static>, width: usize) -> Line<'static> {
    let used: usize = row.spans.iter().map(|s| width_of(&s.content)).sum();
    let mut spans = row.spans;
    spans.push(Span::raw(" ".repeat(width.saturating_sub(used))));
    Line::from(spans).style(selected())
}

pub fn render(state: &State, frame: &mut Frame, area: Rect, now_ms: i64) {
    let width = usize::from(area.width);
    let t = &state.tokens;
    let plan = plan(state, area, now_ms);
    let mut lines = plan.header;
    let Some(report) = t.report() else {
        if t.pending {
            lines.push(Line::from(format!(" {LOADING}")));
        }
        frame.render_widget(Paragraph::new(lines), area);
        return;
    };
    if report.rows.is_empty() {
        let unbuilt = t.watch.state().is_some_and(|i| i.refreshed_at_ms.is_none());
        let none = if unbuilt {
            NOT_INDEXED
        } else {
            words::NO_TOKEN_USAGE
        };
        lines.push(Line::from(format!(" {none}")));
        frame.render_widget(Paragraph::new(lines), area);
        return;
    }
    let table = Table::new(report, &t.level().widths, width, state.color, now_ms);
    lines.push(table.header());
    let (items, starts) = items(report, width, now_ms);
    let selected_at = t.selected_index();
    let offset = match selected_at {
        Some(at) => {
            let last = starts.get(at + 1).copied().unwrap_or(items.len()) - 1;
            keep_visible(t.level().offset, starts[at], last, plan.rows, items.len())
        }
        None => t.level().offset.min(items.len().saturating_sub(plan.rows)),
    };
    for (n, (index, item)) in items.into_iter().skip(offset).take(plan.rows).enumerate() {
        lines.push(match item {
            Item::Row { repeat } => {
                let row = table.row(&report.rows[index], repeat && n > 0);
                if Some(index) == selected_at {
                    highlight(row, width)
                } else {
                    row
                }
            }
            Item::Figures(line) => line,
        });
    }
    lines.push(table.total());
    lines.extend(plan.below);
    frame.render_widget(Paragraph::new(lines), area);
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyCode;
    use ratatui::style::{Color, Modifier};
    use tagteam_engine::tokens::{ActiveBlock, IndexProgress, IndexState, RefreshOutcome};

    use super::*;
    use crate::msg::{Cmd, Msg};
    use crate::state::Screen;
    use crate::testutil::*;
    use crate::update::update;

    /// 2026-09-20T00:00:00Z, the day before `NOW_MS`'s.
    const YESTERDAY: i64 = 1_789_862_400_000;
    const HOUR_MS: i64 = 3_600_000;
    const DAY_MS: i64 = 24 * HOUR_MS;

    /// Tokens at 80×24, its report for the query it asked for, `rows`, then `edit`ed.
    fn showing(rows: Vec<TokenRow>, edit: impl FnOnce(&mut TokenReport)) -> State {
        let mut state = fresh(Screen::Tokens);
        let cmds = state.start();
        let [_, Cmd::LoadReport { r#gen, query }] = cmds.as_slice() else {
            panic!("{cmds:?}");
        };
        let mut report = token_report(query.clone(), rows);
        edit(&mut report);
        let result = Ok(report);
        update(
            &mut state,
            Msg::TokenReport {
                r#gen: *r#gen,
                result,
            },
            NOW_MS,
        );
        state
    }

    /// The same, after `keys` set the query up: each key's query is answered with `rows`.
    fn showing_after(keys: &str, rows: Vec<TokenRow>) -> State {
        let mut state = fresh(Screen::Tokens);
        state.start();
        let mut last = vec![];
        for c in keys.chars() {
            last = update(&mut state, ch(c), NOW_MS);
        }
        let [Cmd::LoadReport { r#gen, query }] = last.as_slice() else {
            panic!("{last:?}");
        };
        let result = Ok(token_report(query.clone(), rows));
        update(
            &mut state,
            Msg::TokenReport {
                r#gen: *r#gen,
                result,
            },
            NOW_MS,
        );
        state
    }

    fn spare() -> RowAccount {
        RowAccount::Account {
            id: id(2),
            position: 2,
            label: "a2@x.co".into(),
            alias: Some("spare".into()),
        }
    }

    /// Yesterday for accounts 1 (with lower-bound output and its cache split) and 2 (`spare`),
    /// and the day before, unattributed and unpriced.
    fn days() -> Vec<TokenRow> {
        let mut one = token_row(
            day(1),
            Some(row_account(1)),
            None,
            counts(1_200_000, 310_000, 88_000_000),
            Some(41_200_000_000),
        );
        one.counts.cache_write_5m = 3_000_000;
        one.counts.cache_write_1h = 1_100_000;
        one.unfinalized = 2;
        one.responses = 412;
        let two = token_row(
            day(1),
            Some(spare()),
            None,
            counts(300_000, 80_000, 20_100_000),
            Some(9_750_000_000),
        );
        let three = token_row(
            day(2),
            Some(RowAccount::Unattributed),
            None,
            counts(5_000, 900, 100_000),
            None,
        );
        vec![one, two, three]
    }

    #[test]
    fn a_day_report_at_80_columns() {
        // Spec §5.3: period, account, input, output, total and cost; the period once per day.
        let state = showing(days(), |_| {});
        let lines = draw(&state, 80, 24);
        assert_eq!(
            lines[1..12],
            [
                " [Day] Month Session Block  30 days  group: account",
                " filter: all accounts · all models",
                " UTC · refreshed 14:12",
                " DAY         ACCOUNT        INPUT   OUTPUT   TOTAL    COST",
                " 2026-09-20  1 a1@x.co       1.2M  ≥310.0K   93.6M  $41.20",
                "             2 spare       300.0K    80.0K   20.5M   $9.75",
                " 2026-09-19  unattributed    5.0K      900  105.9K       —",
                " TOTAL                       1.5M  ≥390.9K  114.2M  $50.95",
                " 412 responses · cache write 4.1M (5m 3.0M, 1h 1.1M) · cache read 88.0M",
                " 2026-09-20 · 1 a1@x.co",
                " Output is a lower bound for 2 subagent responses.",
            ]
        );
        assert_eq!(lines[12], " API cost at list prices as of 2026-10-01.");
        assert_eq!(
            lines[23],
            "Enter sessions  p period  g group  f filter  [ ] range  r refresh"
        );
    }

    #[test]
    fn from_120_columns_the_cache_the_responses_and_a_cost_bar_join_them() {
        let state = showing(days(), |_| {});
        let lines = draw(&state, 120, 40);
        assert_eq!(
            lines[1],
            " [Day] Month Session Block  30 days  group: account  filter: all accounts · all models"
        );
        assert_eq!(
            lines[3..8],
            [
                " DAY         ACCOUNT        INPUT   OUTPUT  CW 5M  CW 1H  CACHE R   TOTAL  RESP    COST",
                " 2026-09-20  1 a1@x.co       1.2M  ≥310.0K   3.0M   1.1M    88.0M   93.6M   412  $41.20  ████████",
                "             2 spare       300.0K    80.0K      0      0    20.1M   20.5M     1   $9.75  ██",
                " 2026-09-19  unattributed    5.0K      900      0      0   100.0K  105.9K     1       —",
                " TOTAL                       1.5M  ≥390.9K   3.0M   1.1M   108.2M  114.2M   414  $50.95",
            ]
        );
    }

    /// A session of yesterday, 09:12 to 11:40 UTC, in the tagteam checkout.
    fn session(id: &str, account: RowAccount) -> TokenRow {
        let period = PeriodKey::Session {
            session_id: id.into(),
            project: Some("/Users/michael/Code/tagteam".into()),
            first_ms: YESTERDAY + 9 * HOUR_MS + 12 * 60_000,
            last_ms: YESTERDAY + 11 * HOUR_MS + 40 * 60_000,
        };
        let counts = counts(20_000, 3_000, 900_000);
        token_row(period, Some(account), None, counts, Some(1_230_000_000))
    }

    /// 12:00 to 17:00 UTC today.
    const NOON: i64 = 1_789_992_000_000;

    /// Account 1's block of five hours from `start_ms`: active at `NOW_MS` (14:13:20) and
    /// observed when `active`, else inferred.
    fn block(start_ms: i64, active: bool) -> TokenRow {
        let end_ms = start_ms + 5 * HOUR_MS;
        let period = PeriodKey::Block {
            start_ms,
            end_ms,
            observed: active,
            active: active.then_some(ActiveBlock {
                elapsed_s: (NOW_MS - start_ms) / 1000,
                remaining_s: (end_ms - NOW_MS) / 1000,
                tokens_per_min: 41_234.6,
                cost_per_hour: Some(Cost {
                    usd_nanos: 3_200_000_000,
                    partial: false,
                }),
                projected_tokens: 9_800_000,
                projected_cost: None,
            }),
        };
        let counts = counts(400_000, 50_000, 5_000_000);
        token_row(
            period,
            Some(row_account(1)),
            None,
            counts,
            Some(5_000_000_000),
        )
    }

    #[test]
    fn a_session_shows_its_project_and_times_and_a_month_its_month() {
        let sessions = showing_after(
            "pp",
            vec![
                session("3f2a9c1d-7e44-4b1a-9c55-0d1e2f3a4b5c", row_account(1)),
                session("88aa", spare()),
            ],
        );
        let lines = draw(&sessions, 80, 24);
        assert_eq!(
            lines[1..12],
            [
                " Day Month [Session] Block  30 days  group: account",
                " filter: all accounts · all models",
                " UTC · refreshed 14:12",
                " SESSION            PROJECT              ACCOUNT    INPUT  OUTPUT   TOTAL   COST",
                " 09-20 09:12–11:40  …chael/Code/tagteam  1 a1@x.co  20.0K    3.0K  923.0K  $1.23",
                " 09-20 09:12–11:40  …chael/Code/tagteam  2 spare    20.0K    3.0K  923.0K  $1.23",
                " TOTAL                                              40.0K    6.0K    1.8M  $2.46",
                " 1 response · cache write 0 (5m 0, 1h 0) · cache read 900.0K",
                " session 3f2a9c1d-7e44-4b1a-9c55-0d1e2f3a4b5c · /Users/michael/Code/tagteam",
                " 2026-09-20 09:12 – 2026-09-20 11:40 · 1 a1@x.co",
                " API cost at list prices as of 2026-10-01.",
            ]
        );
        let month = PeriodKey::Month {
            year: 2026,
            month: 9,
        };
        let months = showing_after(
            "p",
            vec![token_row(
                month,
                Some(row_account(1)),
                None,
                counts(1, 2, 3),
                None,
            )],
        );
        let lines = draw(&months, 80, 24);
        assert_eq!(
            lines[1],
            " Day [Month] Session Block  12 months  group: account"
        );
        assert_eq!(lines[4], " MONTH    ACCOUNT    INPUT  OUTPUT  TOTAL  COST");
        assert_eq!(lines[5], " 2026-09  1 a1@x.co      1       2      6     —");
    }

    #[test]
    fn a_block_shows_its_span_and_whether_it_was_observed() {
        // Spec §5.3, token report §9; blocks are per account, so the header shows the
        // grouping the report applies.
        let blocks = showing_after(
            "pppggg",
            vec![block(NOON, true), block(NOON - 5 * HOUR_MS, false)],
        );
        let lines = draw(&blocks, 80, 24);
        assert_eq!(
            lines[1..10],
            [
                " Day Month Session [Block]  30 days  group: account",
                " filter: all accounts · all models",
                " UTC · refreshed 14:12",
                " BLOCK                        ACCOUNT     INPUT  OUTPUT  TOTAL    COST",
                " 09-21 12:00–17:00  observed  1 a1@x.co  400.0K   50.0K   5.5M   $5.00",
                "   active · 2h13m elapsed · 2h46m left · 41.2K tokens/min · $3.20/h",
                "   projected 9.8M tokens, —",
                " 09-21 07:00–12:00  inferred  1 a1@x.co  400.0K   50.0K   5.5M   $5.00",
                " TOTAL                                   800.0K  100.0K  10.9M  $10.00",
            ]
        );
        assert_eq!(blocks.tokens.grouping(), TokenGrouping::None);
    }

    #[test]
    fn the_active_block_counts_down_between_queries_and_is_no_longer_active_at_its_end() {
        let blocks = showing_after("ppp", vec![block(NOON, true)]);
        let figures = |at: i64| draw_at(&blocks, 80, 24, at)[6].clone();
        assert!(figures(NOW_MS).starts_with("   active · 2h13m elapsed · 2h46m left"));
        assert!(
            figures(NOW_MS + 61_000).starts_with("   active · 2h14m elapsed · 2h45m left"),
            "from its start and end, with no query between"
        );
        let end = NOON + 5 * HOUR_MS;
        assert!(figures(end - 60_000).starts_with("   active · 4h59m elapsed · 1m left"));
        assert_eq!(
            figures(end),
            " TOTAL                                   400.0K   50.0K   5.5M  $5.00",
            "no longer active, until the query its end asks for returns"
        );
    }

    #[test]
    fn the_active_blocks_figures_show_once_under_the_first_of_its_model_rows() {
        let mut rows = vec![block(NOON, true), block(NOON, true)];
        rows[0].model = Some("claude-opus-4-5-20251101".into());
        rows[1].model = Some("claude-sonnet-4-5-20250929".into());
        let state = showing_after("pppg", rows);
        let lines = draw(&state, 80, 24);
        let at = |f: fn(&String) -> bool| -> Vec<usize> {
            (0..lines.len()).filter(|&i| f(&lines[i])).collect()
        };
        let figures = at(|l| l.starts_with("   active"));
        let rows = at(|l| l.contains("400.0K"));
        assert_eq!(rows.len(), 2, "{lines:?}");
        assert_eq!(figures.len(), 1, "once for the block: {lines:?}");
        assert_eq!(figures[0], rows[0] + 1, "under the first row: {lines:?}");
        assert!(rows[1] > figures[0], "{lines:?}");
    }

    #[test]
    fn the_account_and_model_columns_follow_the_grouping() {
        // Spec §5.3: the model column when the grouping has the model; the account column
        // always, listing a row's accounts when the rows are not split by account.
        let mut opus = token_row(day(1), None, Some("claude-opus-5-5"), counts(1, 2, 3), None);
        opus.accounts = vec![row_account(1), spare()];
        let models = showing_after("g", vec![opus.clone()]);
        let lines = draw(&models, 80, 24);
        assert_eq!(
            lines[1],
            " [Day] Month Session Block  30 days  group: model"
        );
        assert_eq!(
            lines[4],
            " DAY         ACCOUNT             MODEL            INPUT  OUTPUT  TOTAL  COST"
        );
        assert_eq!(
            lines[5],
            " 2026-09-20  1 a1@x.co, 2 spare  claude-opus-5-5      1       2      6     —"
        );
        opus.model = None;
        let none = showing_after("ggg", vec![opus]);
        let lines = draw(&none, 80, 24);
        assert_eq!(lines[1], " [Day] Month Session Block  30 days  group: none");
        assert_eq!(
            lines[4],
            " DAY         ACCOUNT             INPUT  OUTPUT  TOTAL  COST"
        );
    }

    #[test]
    fn lower_bounds_are_marked_and_stand_out_and_a_missing_cost_is_a_dash() {
        // Spec §5.3: `≥` for output with unfinalized responses and for a partial cost, `—` for
        // none; with colour the marked cells are yellow, without it bold.
        let mut rows = days();
        rows[1].cost = Some(Cost {
            usd_nanos: 9_750_000_000,
            partial: true,
        });
        for color in [true, false] {
            let mut state = showing(rows.clone(), |_| {});
            state.color = color;
            let buf = draw_buffer(&state, 80, 24, NOW_MS);
            let lines = text_of(&buf);
            assert_eq!(
                lines[6],
                "             2 spare       300.0K    80.0K   20.5M   ≥$9.75"
            );
            assert!(lines[7].ends_with("       —"), "{}", lines[7]);
            for needle in ["≥310.0K", "≥$9.75"] {
                let (x, y) = locate(&buf, needle).unwrap();
                let cell = &buf[(x, y)];
                if color {
                    assert_eq!(cell.fg, Color::Yellow, "{needle}");
                } else {
                    assert_eq!(cell.fg, Color::Reset, "{needle}");
                    assert!(cell.modifier.contains(Modifier::BOLD), "{needle}");
                }
            }
            let (x, y) = locate(&buf, "$41.20").unwrap();
            assert_eq!(buf[(x, y)].fg, Color::Reset, "a whole cost is plain");
        }
    }

    #[test]
    fn the_footnotes_say_what_the_report_leaves_out() {
        // Spec §5.3, token report §11.2: with the TUI's `≥` and the unreadable files.
        let state = showing(days(), |r| {
            r.unpriced_models = vec!["claude-test-9".into()];
            r.index.skipped_lines = 12;
            r.index.unreadable_files = 3;
            r.price_overrides = vec!["claude-opus-5-5".into()];
        });
        let lines = draw(&state, 80, 24);
        assert_eq!(
            lines[11..17],
            [
                " No price for claude-test-9: counted without cost (≥ marks a partial cost).",
                " Output is a lower bound for 2 subagent responses.",
                " 12 transcript lines could not be read, so these totals may be low.",
                " 3 transcript files could not be read; the next refresh tries again.",
                " API cost at list prices as of 2026-10-01, with config.toml's for",
                " claude-opus-5-5.",
            ]
        );
    }

    #[test]
    fn the_header_says_how_current_the_index_is() {
        // Spec §5.3 "Indexing".
        let mut state = showing(days(), |_| {});
        assert_eq!(state.tokens.watch().refreshing(), None);
        assert!(crate::update::drawn(&mut state).contains(&Cmd::RefreshIndex { provider: cc() }));
        assert_eq!(draw(&state, 80, 24)[3], " UTC · indexing transcripts…");
        let progress = IndexProgress {
            bytes_done: 1_200_000_000,
            bytes_total: 3_300_000_000,
            ..IndexProgress::default()
        };
        update(&mut state, Msg::IndexProgress(progress), NOW_MS);
        assert_eq!(
            draw(&state, 80, 24)[3],
            " UTC · indexing transcripts 1.2 GB / 3.3 GB"
        );
        update(
            &mut state,
            Msg::Refreshed {
                result: Ok(RefreshOutcome::Busy),
            },
            NOW_MS,
        );
        assert_eq!(draw(&state, 80, 24)[3], " UTC · catching up elsewhere");
        // A report of an index that has never completed a refresh (token report §14).
        let partial = IndexState {
            refreshed_at_ms: None,
            ..IndexState::default()
        };
        let mut state = showing(days(), |r| r.index = partial);
        assert_eq!(draw(&state, 80, 24)[3], " UTC · catching up");
        crate::screens::tokens::requery(&mut state, NOW_MS);
        assert_eq!(draw(&state, 80, 24)[3], " UTC · catching up · updating…");
        // Refreshed on another day: its date.
        let state = showing(days(), |r| {
            r.index.refreshed_at_ms = Some(YESTERDAY + HOUR_MS)
        });
        assert_eq!(draw(&state, 80, 24)[3], " UTC · refreshed 2026-09-20 01:00");
    }

    #[test]
    fn a_cold_catch_up_shows_its_progress_whole_at_80_columns_without_saying_catching_up() {
        let mut state = fresh(Screen::Tokens);
        state.tokens = crate::screens::tokens::TokensState::new(
            cc(),
            jiff::tz::TimeZone::get("Europe/Brussels").unwrap(),
        );
        let cmds = state.start();
        let [_, Cmd::LoadReport { r#gen, query }] = cmds.as_slice() else {
            panic!("{cmds:?}");
        };
        let mut report = token_report(query.clone(), days());
        report.time_zone = "Europe/Brussels".into();
        report.index.refreshed_at_ms = None;
        let result = Ok(report);
        update(
            &mut state,
            Msg::TokenReport {
                r#gen: *r#gen,
                result,
            },
            NOW_MS,
        );
        assert_eq!(draw(&state, 80, 24)[3], " Europe/Brussels · catching up");
        crate::update::drawn(&mut state);
        let progress = IndexProgress {
            bytes_done: 12_300_000_000,
            bytes_total: 33_300_000_000,
            ..IndexProgress::default()
        };
        update(&mut state, Msg::IndexProgress(progress), NOW_MS);
        crate::screens::tokens::requery(&mut state, NOW_MS);
        assert_eq!(
            draw(&state, 80, 24)[3],
            " Europe/Brussels · indexing transcripts 12.3 GB / 33.3 GB · updating…"
        );
    }

    #[test]
    fn loading_and_an_empty_report_say_so() {
        let mut state = fresh(Screen::Tokens);
        state.start();
        assert_eq!(draw(&state, 80, 24)[4], " loading…");
        let empty = showing(vec![], |_| {});
        let lines = draw(&empty, 80, 24);
        assert_eq!(lines[4], " No token usage in this range.");
        assert_eq!(lines[5], "");
    }

    #[test]
    fn an_empty_report_of_an_index_not_built_yet_says_so() {
        let unbuilt = IndexState {
            refreshed_at_ms: None,
            ..IndexState::default()
        };
        let state = showing(vec![], |r| r.index = unbuilt);
        let lines = draw(&state, 80, 24);
        assert_eq!(lines[3], " UTC · catching up");
        assert_eq!(lines[4], " No token usage indexed yet.");
        assert_eq!(lines[5], "");
        let built = showing(vec![], |_| {});
        assert_eq!(draw(&built, 80, 24)[4], " No token usage in this range.");
    }

    #[test]
    fn the_drill_down_is_named_in_the_header() {
        let mut state = showing(days(), |_| {});
        let cmds = update(&mut state, key(KeyCode::Enter), NOW_MS);
        let [Cmd::LoadReport { r#gen, query }] = cmds.as_slice() else {
            panic!("{cmds:?}");
        };
        let rows = vec![session("3f2a9c1d", row_account(1))];
        let result = Ok(token_report(query.clone(), rows));
        update(
            &mut state,
            Msg::TokenReport {
                r#gen: *r#gen,
                result,
            },
            NOW_MS,
        );
        let lines = draw(&state, 80, 24);
        assert_eq!(
            lines[1],
            " [Day] Month Session Block  30 days  group: account"
        );
        assert_eq!(
            lines[2],
            " filter: all accounts · all models  › 2026-09-20 1 a1@x.co"
        );
    }

    #[test]
    fn the_header_names_the_top_levels_grouping_at_every_depth() {
        // `g` changes the top level's grouping; a drilled level's own is not what it sets.
        let mut state = showing_after("g", days());
        assert_eq!(
            draw(&state, 80, 24)[1],
            " [Day] Month Session Block  30 days  group: model"
        );
        for rows in [
            vec![session("3f2a9c1d", row_account(1))],
            vec![session("3f2a9c1d", row_account(1))],
        ] {
            let down = update(&mut state, key(KeyCode::Enter), NOW_MS);
            let [Cmd::LoadReport { r#gen, query }] = down.as_slice() else {
                panic!("{down:?}");
            };
            let result = Ok(token_report(query.clone(), rows));
            update(
                &mut state,
                Msg::TokenReport {
                    r#gen: *r#gen,
                    result,
                },
                NOW_MS,
            );
        }
        assert_eq!(state.tokens.depth(), 2);
        assert_eq!(
            state.tokens.report().map(|r| r.query.grouping),
            Some(TokenGrouping::AccountModel)
        );
        assert_eq!(
            draw(&state, 80, 24)[1],
            " [Day] Month Session Block  30 days  group: model"
        );
    }

    /// Answers the report query among `cmds` with `rows`.
    fn answer_with(state: &mut State, cmds: &[Cmd], rows: Vec<TokenRow>) {
        let [Cmd::LoadReport { r#gen, query }] = cmds else {
            panic!("{cmds:?}");
        };
        let result = Ok(token_report(query.clone(), rows));
        update(
            state,
            Msg::TokenReport {
                r#gen: *r#gen,
                result,
            },
            NOW_MS,
        );
    }

    /// Real-format model IDs, the longest 26 columns.
    const MODELS: [&str; 3] = [
        "claude-opus-4-5-20251101",
        "claude-sonnet-4-5-20250929",
        "claude-haiku-4-5-20251001",
    ];

    fn me() -> RowAccount {
        RowAccount::Account {
            id: id(1),
            position: 1,
            label: "michael@example.com".into(),
            alias: None,
        }
    }

    fn work_session() -> PeriodKey {
        PeriodKey::Session {
            session_id: "3f2a9c1d-7e44-4b1a-9c55-0d1e2f3a4b5c".into(),
            project: Some("/Users/michael/Code/tagteam".into()),
            first_ms: YESTERDAY + 9 * HOUR_MS + 12 * 60_000,
            last_ms: YESTERDAY + 11 * HOUR_MS + 40 * 60_000,
        }
    }

    /// One row of `work_session` for each of `MODELS`.
    fn work_models() -> Vec<TokenRow> {
        MODELS
            .iter()
            .map(|m| {
                let counts = counts(400_000, 100_000, 30_000_000);
                token_row(
                    work_session(),
                    Some(me()),
                    Some(m),
                    counts,
                    Some(13_000_000_000),
                )
            })
            .collect()
    }

    #[test]
    fn one_sessions_models_read_whole_at_80_columns_without_the_period_and_the_project() {
        let counts = counts(1_200_000, 310_000, 88_000_000);
        let day_row = token_row(day(1), Some(me()), None, counts, Some(41_200_000_000));
        let mut state = showing(vec![day_row], |_| {});
        for rows in [
            vec![token_row(work_session(), Some(me()), None, counts, None)],
            work_models(),
        ] {
            let down = update(&mut state, key(KeyCode::Enter), NOW_MS);
            answer_with(&mut state, &down, rows);
        }
        assert_eq!(state.tokens.depth(), 2);
        let lines = draw(&state, 80, 24);
        assert!(
            lines[4].starts_with(" ACCOUNT"),
            "the period and the project are the same on every row: {lines:?}"
        );
        for model in MODELS {
            assert!(
                lines.iter().any(|l| l.contains(&format!("  {model}  "))),
                "{model} whole: {lines:?}"
            );
        }
        assert!(lines[8].starts_with(" TOTAL"), "{lines:?}");
        assert!(lines[8].contains("1.2M"), "{lines:?}");
    }

    #[test]
    fn the_sessions_by_account_and_model_keep_their_models_apart_at_80_columns() {
        // The project gives way before the account, and the account before the model is cut.
        let mut state = fresh(Screen::Tokens);
        state.start();
        let mut last = vec![];
        for c in "ppgg".chars() {
            last = update(&mut state, ch(c), NOW_MS);
        }
        answer_with(&mut state, &last, work_models());
        let lines = draw(&state, 80, 24);
        assert!(
            !lines[4].contains("PROJECT"),
            "the project gave way first: {lines:?}"
        );
        for model in MODELS {
            assert!(
                lines[5..8].iter().any(|l| l.contains(model)),
                "{model} whole: {lines:?}"
            );
        }
        assert!(lines[5].contains(" 1 mich…  "), "{lines:?}");
    }

    #[test]
    fn the_cache_write_is_one_column_where_its_halves_do_not_fit() {
        let mut rows = days();
        if let Some(RowAccount::Account { alias, .. }) = &mut rows[1].account {
            *alias = Some("an-alias-thirty-five-characters-long".into());
        }
        let state = showing(rows, |_| {});
        let lines = draw(&state, 120, 40);
        assert!(lines[3].contains("  CACHE W  CACHE R  "), "{}", lines[3]);
        assert!(
            lines[5].contains("2 an-alias-thirty-five-characters-long  "),
            "{}",
            lines[5]
        );
    }

    /// `n` days of account 1, newest first.
    fn many_days(n: i64) -> Vec<TokenRow> {
        (0..n)
            .map(|d| {
                token_row(
                    day(d),
                    Some(row_account(1)),
                    None,
                    counts(1, 2, 3),
                    Some(10_000_000),
                )
            })
            .collect()
    }

    #[test]
    fn a_long_report_scrolls_to_keep_the_selection_in_view_and_a_resize_keeps_it_there() {
        // R22: the stored scroll is clamped where it is used.
        let mut state = showing(many_days(30), |_| {});
        state.size = (120, 40);
        update(&mut state, key(KeyCode::End), NOW_MS);
        let lines = draw(&state, 120, 40);
        assert!(
            lines.iter().any(|l| l.starts_with(" 2026-08-23")),
            "the last row shows"
        );
        for (w, h) in [(80, 24), (100, 30)] {
            state.size = (w, h);
            let lines = draw(&state, w, h);
            let last = lines.iter().position(|l| l.starts_with(" 2026-08-23"));
            assert!(last.is_some(), "{w}x{h}: {lines:?}");
            assert!(lines[last.unwrap() + 1].starts_with(" TOTAL"), "{w}x{h}");
        }
        update(&mut state, key(KeyCode::Home), NOW_MS);
        assert_eq!(
            draw(&state, 100, 30)[4],
            " 2026-09-21  1 a1@x.co      1       2      6  $0.01"
        );
        update(&mut state, key(KeyCode::PageDown), NOW_MS);
        let page = page(&state, crate::view::body_at(&state), NOW_MS);
        assert_eq!(state.tokens.selected_index(), Some(page));
        update(&mut state, key(KeyCode::PageUp), NOW_MS);
        assert_eq!(state.tokens.selected_index(), Some(0));
    }

    /// `text` with its white space taken out: what a person reads across wrapped rows.
    fn squash(text: &str) -> String {
        text.chars().filter(|c| !c.is_whitespace()).collect()
    }

    /// Whether `lines`, read across their rows, hold `text` whole.
    fn reads(lines: &[String], text: &str) -> bool {
        squash(&lines.concat()).contains(&squash(text))
    }

    /// Account 3, whose alias and label are both wider than any column at 80.
    fn wide() -> RowAccount {
        RowAccount::Account {
            id: id(3),
            position: 3,
            label: "averyveryverylongname.with.a.long.tail@example.com".into(),
            alias: Some("山田太郎のアカウント".into()),
        }
    }

    /// The widest a report gets (spec §5.3): all time, by session, account and model; a span
    /// from one year into another; names wider than their columns; ten-digit counts and a
    /// partial six-figure cost.
    fn widest() -> State {
        let long = PeriodKey::Session {
            session_id: "3f2a9c1d-7e44-4b1a-9c55-0d1e2f3a4b5c".into(),
            project: Some("/Users/michael/Code/ViuMore/VM.Portal-feature-user-manuals".into()),
            first_ms: YESTERDAY - 365 * DAY_MS + 9 * HOUR_MS + 12 * 60_000,
            last_ms: YESTERDAY - 364 * DAY_MS + 11 * HOUR_MS + 40 * 60_000,
        };
        let model = "claude-opus-5-5-with-a-very-long-suffix";
        let ten_digits = counts(9_876_543_210, 1_234_567_890, 5_555_555_555);
        let mut big = token_row(long, Some(wide()), Some(model), ten_digits, None);
        big.unfinalized = 3;
        big.cost = Some(Cost {
            usd_nanos: 123_456_789_000_000,
            partial: true,
        });
        let short = PeriodKey::Session {
            session_id: "88aa".into(),
            project: Some("/w".into()),
            first_ms: YESTERDAY + 9 * HOUR_MS,
            last_ms: YESTERDAY + 10 * HOUR_MS,
        };
        let small = token_row(
            short,
            Some(row_account(1)),
            Some("claude-sonnet-5-5"),
            counts(1, 2, 3),
            Some(10_000_000),
        );
        showing_after("ppgg]]", vec![big, small])
    }

    #[test]
    fn every_required_value_shows_whole_at_80_columns_in_the_widest_report() {
        // Spec §5.3 at 80 columns: the period, account, model, input, output, total and cost
        // show. The numbers are never cut; the span compacts to its start and the project goes
        // (the detail has both), the account gives way to its position, and the model, which
        // is cut last, by display width (B24).
        let state = widest();
        let lines = draw(&state, 80, 24);
        let head = lines
            .iter()
            .position(|l| l.starts_with(" SESSION"))
            .unwrap();
        assert_eq!(
            lines[head],
            " SESSION      AC…  MODEL                       INPUT  OUTPUT  TOTAL         COST"
        );
        let big = &lines[head + 1];
        for value in ["09-20 09:12", "9.9B", "≥1.2B", "16.7B", "≥$123456.79"] {
            assert!(big.contains(value), "{value} in {big:?}");
        }
        assert!(
            big.contains(" 3 …"),
            "the account by position, cut: {big:?}"
        );
        assert!(
            big.contains("claude-opus-5-5-with-a-ve…"),
            "the model, cut last: {big:?}"
        );
        assert!(!lines[head].contains("PROJ"), "the project gave way first");
        let small = &lines[head + 2];
        assert!(small.contains("09-20 09:00"), "{small:?}");
        let total = &lines[head + 3];
        for value in ["9.9B", "≥1.2B", "16.7B", "≥$123456.80"] {
            assert!(total.contains(value), "{value} in {total:?}");
        }
        // At 120 the cache, the responses and the bar take room, and the span is cut to its
        // start, with the model whole to 30 columns; wider still, it shows the span and the years.
        let at_120 = draw(&state, 120, 40);
        assert!(
            at_120
                .iter()
                .any(|l| l.contains(" 09-20 09:12  3 ")
                    && l.contains("claude-opus-5-5-with-a-very-l…")),
            "{at_120:?}"
        );
        let at_160 = draw(&state, 160, 40);
        assert!(
            at_160
                .iter()
                .any(|l| l.contains("2025-09-20 09:12–2025-09-21 11:40")),
            "{at_160:?}"
        );
        // Day by account and model: the CJK alias is cut on a character, never in one.
        let day = showing_after(
            "g",
            vec![token_row(
                day(1),
                Some(wide()),
                Some("claude-opus-5-5-with-a-very-long-suffix"),
                counts(1, 2, 3),
                None,
            )],
        );
        let lines = draw(&day, 80, 24);
        assert!(lines[5].starts_with(" 2026-09-20  3 山田"), "{}", lines[5]);
    }

    /// A session as the engine gives one: a UUID, a long project, two models and the cache.
    fn realistic() -> TokenRow {
        let period = PeriodKey::Session {
            session_id: "3f2a9c1d-7e44-4b1a-9c55-0d1e2f3a4b5c".into(),
            project: Some("/Users/michael/Code/ViuMore/VM.Portal-feature-user-manuals".into()),
            first_ms: YESTERDAY + 9 * HOUR_MS + 12 * 60_000,
            last_ms: YESTERDAY + 11 * HOUR_MS + 40 * 60_000,
        };
        let mut row = token_row(
            period,
            Some(row_account(1)),
            None,
            counts(1_200_000, 310_000, 88_000_000),
            Some(41_200_000_000),
        );
        row.counts.cache_write_5m = 3_000_000;
        row.counts.cache_write_1h = 1_100_000;
        row.responses = 412;
        row.models = vec!["claude-opus-5-5".into(), "claude-sonnet-5-5".into()];
        row
    }

    /// What the selected row's detail must let a person read (B25).
    const DETAIL: [&str; 7] = [
        "412 responses · cache write 4.1M (5m 3.0M, 1h 1.1M) · cache read 88.0M",
        "session 3f2a9c1d-7e44-4b1a-9c55-0d1e2f3a4b5c",
        "/Users/michael/Code/ViuMore/VM.Portal-feature-user-manuals",
        "2026-09-20 09:12 – 2026-09-20 11:40",
        "1 a1@x.co",
        "claude-opus-5-5, claude-sonnet-5-5",
        "API cost at list prices as of 2026-10-01.",
    ];

    #[test]
    fn a_sessions_detail_shows_its_numbers_first_and_every_value_whole() {
        // B25: the numbers first, never cut; then the session's text, wrapped.
        let state = showing_after("pp", vec![realistic()]);
        let lines = draw(&state, 80, 24);
        let total = lines.iter().position(|l| l.starts_with(" TOTAL")).unwrap();
        assert_eq!(
            lines[total + 1],
            " 412 responses · cache write 4.1M (5m 3.0M, 1h 1.1M) · cache read 88.0M"
        );
        let below = &lines[total + 1..];
        for value in DETAIL {
            assert!(reads(below, value), "{value}: {below:?}");
        }
        assert!(
            !lines.iter().any(|l| l.contains("J K:")),
            "it all fits: {lines:?}"
        );
    }

    #[test]
    fn under_a_full_banner_and_toast_j_and_k_reach_every_value_of_the_detail_and_the_notes() {
        // R21 at 80×24: a three-row banner and a three-row toast leave the body 17 rows. The
        // table keeps three rows; the detail and the notes below it scroll (B25).
        let mut rows = vec![realistic()];
        rows.extend((1..=9).map(|n| session(&format!("s-{n}"), row_account(1))));
        rows[0].unfinalized = 1;
        let mut state = showing_after("pp", rows);
        if let Some(report) = state.tokens.levels[0].report.as_mut() {
            report.unpriced_models = (1..=12).map(|n| format!("claude-test-{n}")).collect();
            report.index.skipped_lines = 12;
            report.index.unreadable_files = 3;
            report.price_overrides = vec!["claude-opus-5-5".into(), "claude-sonnet-5-5".into()];
        }
        let words = "the store could not be opened because another tagteam holds its lock";
        state.banner = Some([words; 3].join("; "));
        state.toast([words; 3].join("; "), NOW_MS);
        let notes = [
            "No price for claude-test-1, claude-test-2, claude-test-3, claude-test-4, claude-test-5, claude-test-6, claude-test-7, claude-test-8, claude-test-9, claude-test-10, claude-test-11, claude-test-12: counted without cost (≥ marks a partial cost).",
            "Output is a lower bound for 1 subagent response.",
            "12 transcript lines could not be read, so these totals may be low.",
            "3 transcript files could not be read; the next refresh tries again.",
            "API cost at list prices as of 2026-10-01, with config.toml's for claude-opus-5-5, claude-sonnet-5-5.",
        ];
        let lines = draw(&state, 80, 24);
        let total = lines.iter().position(|l| l.starts_with(" TOTAL")).unwrap();
        assert!(total >= 4 + 1 + 3, "the table keeps three rows: {lines:?}");
        assert_eq!(
            lines[total + 1],
            format!(" {}", DETAIL[0]),
            "the numbers first"
        );
        assert!(lines[20].starts_with(" J K: lines 1–"), "{lines:?}");
        let mut seen = lines[total + 1..21].to_vec();
        for _ in 0..40 {
            update(&mut state, ch('J'), NOW_MS);
            let lines = draw(&state, 80, 24);
            let total = lines.iter().position(|l| l.starts_with(" TOTAL")).unwrap();
            seen.extend(lines[total + 1..21].iter().cloned());
        }
        for value in DETAIL[..6].iter().chain(&notes) {
            assert!(reads(&seen, value), "{value}: {seen:?}");
        }
        for _ in 0..40 {
            update(&mut state, ch('K'), NOW_MS);
        }
        let lines = draw(&state, 80, 24);
        let total = lines.iter().position(|l| l.starts_with(" TOTAL")).unwrap();
        assert_eq!(
            lines[total + 1],
            format!(" {}", DETAIL[0]),
            "K goes back to the top"
        );
        assert!(
            lines[21].starts_with("the store could not"),
            "the toast: {lines:?}"
        );
    }

    #[test]
    fn the_header_keeps_three_rows_and_a_long_filter_is_cut() {
        // B29: the periods, the filter and the status line take a row each at 80 columns. The
        // filter, which its dialog shows whole, is cut with `…`; the status line shows whole.
        let alias = "a-rather-long-alias-for-the-work-account";
        let label = "averyveryverylongname.with.a.long.tail@example.com";
        let model = "claude-opus-5-5-with-a-very-long-suffix-and-then-some-more-of-it-yet";
        let mut accounts = many(2);
        accounts[1].row.alias = Some(alias.into());
        accounts[1].row.label = label.into();
        accounts[1].row.email = Some(label.into());
        let mut state = showing(days(), |_| {});
        state.accounts.apply(snapshot(accounts));
        state.tokens.accounts = Some(vec![tagteam_engine::tokens::AccountSel::Account(id(2))]);
        state.tokens.models = Some(vec![model.into()]);
        crate::update::drawn(&mut state);
        let progress = IndexProgress {
            bytes_done: 1_200_000_000,
            bytes_total: 3_300_000_000,
            ..IndexProgress::default()
        };
        update(&mut state, Msg::IndexProgress(progress), NOW_MS);
        let lines = draw(&state, 80, 24);
        assert_eq!(
            lines[1],
            " [Day] Month Session Block  30 days  group: account"
        );
        assert!(
            lines[2].starts_with(&format!(" filter: 2 {alias} (avery")),
            "{lines:?}"
        );
        assert!(lines[2].ends_with('…'), "{lines:?}");
        assert!(width_of(&lines[2]) <= 80, "{lines:?}");
        assert_eq!(lines[3], " UTC · indexing transcripts 1.2 GB / 3.3 GB");
        assert!(lines[4].starts_with(" DAY"), "{lines:?}");
    }

    #[test]
    fn a_long_drill_down_keeps_its_start_on_the_filters_row() {
        // B29: the rows a drill-down came through share the filter's row. The filter gives way
        // first, down to `FILTER_LEAST` columns, then they are cut; the header keeps three rows.
        let mut state = widest();
        let rows = state.tokens.report().map(|r| r.rows[..1].to_vec());
        let cmds = update(&mut state, key(KeyCode::Enter), NOW_MS);
        let [Cmd::LoadReport { r#gen, query }] = cmds.as_slice() else {
            panic!("{cmds:?}");
        };
        let rows = rows.unwrap_or_default();
        let result = Ok(token_report(query.clone(), rows));
        update(
            &mut state,
            Msg::TokenReport {
                r#gen: *r#gen,
                result,
            },
            NOW_MS,
        );
        let lines = draw(&state, 80, 24);
        assert_eq!(
            lines[1..4],
            [
                " Day Month [Session] Block  all  group: account, model",
                " filter: all account…  › session 3f2a9c1d-7e44-4b1a-9c55-0d1e2f3a4b5c 3 山田太…",
                " UTC · refreshed 14:12",
            ]
        );
        assert!(
            lines[4].starts_with(" ACCOUNT"),
            "one session's models share the period and the project, which the rows leave out: {lines:?}"
        );
    }

    #[test]
    fn at_80x24_a_2000_column_model_leaves_the_table_three_rows_and_the_detail_reachable() {
        // B29, by construction: under a full banner and toast the body is 17 rows. The header
        // takes three at most; the table's heading and total two; what lies below the total
        // half the body at most and never the table's last three. Below the total nothing is
        // cut: it scrolls, and J reaches every line of it.
        let long = format!("claude-{}", "x".repeat(1993));
        let project = format!("/Users/michael/{}", "p".repeat(600));
        let mut big = realistic();
        if let PeriodKey::Session { project: p, .. } = &mut big.period {
            *p = Some(project.clone());
        }
        big.models = vec![long.clone(), "claude-sonnet-5-5".into()];
        let mut rows = vec![big];
        rows.extend((1..=9).map(|n| session(&format!("s-{n}"), row_account(1))));
        let mut state = showing_after("pp", rows);
        state.tokens.models = Some(vec![long.clone()]);
        if let Some(report) = state.tokens.levels[0].report.as_mut() {
            report.unpriced_models = vec![long.clone()];
        }
        let words = "the store could not be opened because another tagteam holds its lock";
        state.banner = Some([words; 3].join("; "));
        state.toast([words; 3].join("; "), NOW_MS);
        let lines = draw(&state, 80, 24);
        assert!(lines.iter().all(|l| width_of(l) <= 80), "{lines:?}");
        assert_eq!(
            lines[4],
            " Day Month [Session] Block  30 days  group: account"
        );
        assert!(
            lines[5].starts_with(" filter: all accounts · claude-xxxx"),
            "{lines:?}"
        );
        assert!(lines[5].ends_with('…'), "{lines:?}");
        assert!(lines[6].starts_with(" UTC · "), "{lines:?}");
        assert!(lines[7].starts_with(" SESSION"), "{lines:?}");
        let total = lines.iter().position(|l| l.starts_with(" TOTAL")).unwrap();
        assert!(total >= 8 + 3, "the table keeps three rows: {lines:?}");
        assert_eq!(lines[total + 1], format!(" {}", DETAIL[0]), "{lines:?}");
        assert!(lines[20].starts_with(" J K: lines 1–"), "{lines:?}");
        assert!(lines[21].starts_with("the store could not"), "{lines:?}");
        // Every line below the total, read by scrolling: each long piece whole.
        let seen = scrolled_below(&mut state);
        for value in [
            project.clone(),
            format!("{long}, claude-sonnet-5-5"),
            format!("No price for {long}: counted without cost (≥ marks a partial cost)."),
            "API cost at list prices as of 2026-10-01.".to_owned(),
        ] {
            assert!(reads(&seen, &value), "{seen:?}");
        }
        assert!(!seen.iter().any(|l| l.contains('…')), "{seen:?}");
        assert!(seen.iter().all(|l| width_of(l) <= 80), "{seen:?}");
        let lines = draw(&state, 80, 24);
        let total = lines.iter().position(|l| l.starts_with(" TOTAL")).unwrap();
        assert!(
            total >= 8 + 3,
            "scrolled, the table keeps three rows: {lines:?}"
        );
    }

    #[test]
    fn j_scrolls_through_a_footnote_longer_than_three_rows_to_its_last_name() {
        // B29: text below the total scrolls, so it wraps in full. A footnote naming twenty
        // unpriced models runs past three rows, and J reaches its last name and its reason.
        let mut state = showing_after("pp", vec![realistic()]);
        let names: Vec<String> = (1..=20).map(|n| format!("claude-model-{n:02}")).collect();
        if let Some(report) = state.tokens.levels[0].report.as_mut() {
            report.unpriced_models = names.clone();
        }
        let note = format!(
            "No price for {}: counted without cost (≥ marks a partial cost).",
            names.join(", ")
        );
        assert!(width_of(&note) > 3 * 79, "longer than three rows");
        let seen = scrolled_below(&mut state);
        assert!(reads(&seen, &note), "{seen:?}");
        let lines = draw(&state, 80, 24);
        let tail = "claude-model-20: counted without cost (≥ marks a partial cost).";
        assert!(reads(&lines, tail), "the last window: {lines:?}");
    }

    #[test]
    fn j_reaches_every_account_and_model_of_a_session_whose_lists_wrap_past_three_rows() {
        // B29: the detail's lists wrap in full below the total, and J reaches every name.
        let mut row = realistic();
        row.accounts = (1..=12)
            .map(|n| RowAccount::Account {
                id: id(n),
                position: n,
                label: format!("account-{n:02}@example.com"),
                alias: None,
            })
            .collect();
        row.models = (1..=15)
            .map(|n| format!("claude-model-family-{n:02}"))
            .collect();
        let accounts: Vec<String> = row.accounts.iter().map(account_name).collect();
        let accounts = accounts.join(", ");
        let models = row.models.join(", ");
        assert!(width_of(&accounts) > 3 * 79, "longer than three rows");
        assert!(width_of(&models) > 3 * 79, "longer than three rows");
        let mut state = showing_after("pp", vec![row]);
        let seen = scrolled_below(&mut state);
        assert!(reads(&seen, &accounts), "{seen:?}");
        assert!(reads(&seen, &models), "{seen:?}");
    }

    /// Every line below the total at 80×24, read as a person reads it: the first window, then
    /// the line each `J` brings in, until the hint says the last shows.
    fn scrolled_below(state: &mut State) -> Vec<String> {
        let mut seen: Vec<String> = Vec::new();
        loop {
            let lines = draw(state, 80, 24);
            let total = lines.iter().position(|l| l.starts_with(" TOTAL")).unwrap();
            let hint = lines
                .iter()
                .position(|l| l.starts_with(" J K: lines "))
                .unwrap_or_else(|| panic!("it scrolls: {lines:?}"));
            let [from, to, of] = lines[hint]
                .split(|c: char| !c.is_ascii_digit())
                .filter(|n| !n.is_empty())
                .map(|n| n.parse::<usize>().unwrap())
                .collect::<Vec<_>>()[..]
            else {
                panic!("{}", lines[hint]);
            };
            for (i, line) in lines[total + 1..hint].iter().enumerate() {
                if from - 1 + i == seen.len() {
                    seen.push(line.clone());
                }
            }
            if to == of {
                assert_eq!(seen.len(), of, "{seen:?}");
                return seen;
            }
            update(state, ch('J'), NOW_MS);
        }
    }

    #[test]
    fn a_reports_widths_are_found_once_and_anew_for_the_next() {
        // B17: a year by session is thousands of rows; a frame lays them out, and does not
        // measure every one of them again.
        let mut state = showing(many_days(30), |_| {});
        let found = |state: &State| state.tokens.level().widths.0.get().cloned();
        assert_eq!(found(&state), None, "not drawn yet");
        draw(&state, 80, 24);
        let first = found(&state).expect("found by the first frame");
        draw(&state, 120, 40);
        assert_eq!(found(&state).as_ref(), Some(&first), "whatever the width");
        let again = crate::screens::tokens::requery(&mut state, NOW_MS);
        let [Cmd::LoadReport { r#gen, query }] = again.as_slice() else {
            panic!("{again:?}");
        };
        let result = Ok(token_report(query.clone(), days()));
        update(
            &mut state,
            Msg::TokenReport {
                r#gen: *r#gen,
                result,
            },
            NOW_MS,
        );
        assert_eq!(found(&state), None, "a new report is measured anew");
        draw(&state, 80, 24);
        assert!(found(&state).is_some_and(|w| w != first));
    }

    #[test]
    fn byte_counts_read_in_decimal_units() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(999), "999 B");
        assert_eq!(bytes(1_000), "1.0 kB");
        assert_eq!(bytes(1_234_567), "1.2 MB");
        assert_eq!(bytes(3_420_000_000), "3.4 GB");
        assert_eq!(bytes(u64::MAX), "18446744.1 TB");
    }
}
