//! The Tokens screen drawn (spec §5.3): the rows above the table (redesign §8), the report's
//! table at the terminal's width, the total line, the selected row's detail, and the
//! footnotes. It reads nothing but the state.

use std::cell::{OnceCell, RefCell};
use std::rc::Rc;

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
use tagteam_engine::words;

use super::models::ShortModels;
use super::{TokensState, amounts, headline};
use crate::autoline;
use crate::chrome::fit_spans;
use crate::gauge;
use crate::keys::RIGHT_INSET;
use crate::state::State;
use crate::theme::{Role, Theme};
use crate::view::{centred_block, fit, keep_visible, select, width_of, wrap};

/// From this width the table adds the cache, the response count and a share line (spec §5.3).
pub const WIDE: usize = 120;
/// From this width the cache write splits into its 5-minute and 1-hour halves (§8).
const CACHE_SPLIT_FROM: usize = 160;
/// Drawn while a query for a report not yet read runs.
pub const LOADING: &str = "loading…";
/// What an empty report says while the index has never completed a refresh (redesign §5.7): the
/// usage is not missing from the range, it is not read in yet.
const NOT_INDEXED: &str = "No token usage yet";
/// The share line's cells (T5-17).
const SHARE: usize = 8;
/// The table keeps this many rows at least; the detail and the notes below it scroll (B25).
const TABLE_MIN: usize = 3;
/// The control bar's filter gives way to the index's words down to this many columns (T5-7).
const FILTER_MIN: usize = 10;
/// The account column is cut no narrower than this while a more compact period can make room
/// (B24)...
const TEXT_MIN: usize = 6;
/// ... and the project column no narrower than this, else it is left out.
const PROJECT_MIN: usize = 4;
/// The project keeps up to this many columns before the period's spans compact (T5-20).
const PROJECT_KEEP: usize = 20;
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
/// What leads an estimated (inferred) block's span (T5-21).
const ESTIMATED: &str = "~";
/// What labels the selected row's detail (T5-23)...
const SELECTED: &str = "SELECTED";
/// ... whose rows start this far in: ` SELECTED  `.
const DETAIL_INDENT: usize = 11;
/// A footnote's text starts this far in, after its mark or none (T5-24).
const NOTE_INDENT: usize = 3;

/// The table's columns, in the order they are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Col {
    /// The day, month, session's times or block's span.
    Period,
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
    /// The row's share of the largest row cost (T5-17), headless.
    Share,
}

impl Col {
    const ALL: [Col; 14] = [
        Col::Period,
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
        Col::Share,
    ];

    fn head(self, period: TokenPeriod) -> &'static str {
        match self {
            Col::Period => match period {
                TokenPeriod::Day => "DAY",
                TokenPeriod::Month => "MONTH",
                TokenPeriod::Session => "SESSION",
                TokenPeriod::Block => "BLOCK",
            },
            Col::Share => "",
            Col::Project => "PROJECT",
            Col::Account => "ACCOUNT",
            Col::Model => "MODEL",
            Col::Input => "INPUT",
            Col::Output => "OUTPUT",
            Col::CacheWrite => "CACHE W",
            Col::CacheWrite5m => "CACHE W 5M",
            Col::CacheWrite1h => "1H",
            Col::CacheRead => "CACHE R",
            Col::Total => "TOTAL",
            Col::Responses => "RESPONSES",
            Col::Cost => "API $",
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
            Col::Period | Col::Project | Col::Account | Col::Model | Col::Share
        )
    }

    /// Its cells' role (T5-15): the counts but TOTAL muted, the share line accent, the rest
    /// text.
    fn role(self) -> Role {
        match self {
            Col::Input
            | Col::Output
            | Col::CacheWrite
            | Col::CacheWrite5m
            | Col::CacheWrite1h
            | Col::CacheRead
            | Col::Responses => Role::Muted,
            Col::Share => Role::Accent,
            _ => Role::Text,
        }
    }
}

/// A cell's text and its style.
struct Cell {
    text: String,
    style: Style,
}

/// `n` bytes as the control bar shows a refresh's progress: decimal units to one decimal place
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

/// What an unattributed row says when its whole period ends before the token index's epoch: its
/// usage predates tracking, so no account could have been recorded for it.
pub(crate) const BEFORE_TRACKING: &str = "before tracking";

/// An account as the table names it (spec §5.3, T5-19): its position and alias, else its label;
/// `removed account`; `unknown account`, or with `before` (its period ends before the index's
/// epoch) `before tracking`.
fn account_text(account: &RowAccount, before: bool) -> String {
    match account {
        RowAccount::Account {
            position,
            label,
            alias,
            ..
        } => format!("{position} {}", alias.as_deref().unwrap_or(label)),
        RowAccount::Removed { .. } => "removed account".to_owned(),
        RowAccount::Unattributed if before => BEFORE_TRACKING.to_owned(),
        RowAccount::Unattributed => "unknown account".to_owned(),
    }
}

/// An account in full, as the detail and the filter name it (T5-19): `1 work (a@x.co)`,
/// `removed account 3f2a9c1d`, `unknown account` (`before tracking` with `before`).
pub fn account_name(account: &RowAccount, before: bool) -> String {
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
        RowAccount::Removed { id } => format!("removed account {}", short_id(id.as_str())),
        RowAccount::Unattributed if before => BEFORE_TRACKING.to_owned(),
        RowAccount::Unattributed => "unknown account".to_owned(),
    }
}

/// An ID as the breadcrumbs and the detail show it (T5-19): its first 8 characters.
fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

/// A session's project as its cell names it (T5-20): the last non-empty component of `path`,
/// split on `/` or `\`, else `path` as stored.
fn project_name(path: &str) -> &str {
    path.rsplit(['/', '\\'])
        .find(|c| !c.is_empty())
        .unwrap_or(path)
}

/// A cost as a cell says it: dollars and cents, grouped (T5-11), `≥` before a partial one, `—`
/// for none.
fn cost_text(cost: Option<&Cost>) -> String {
    match cost {
        Some(c) if c.partial => format!("{}{}", words::AT_LEAST, amounts::dollars(c.usd_nanos)),
        Some(c) => amounts::dollars(c.usd_nanos),
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
    /// The report's models as the Model column names them (T5-22).
    models: &'a ShortModels,
    cols: Vec<(Col, usize)>,
    /// The column whose total line cell says `TOTAL`: the first.
    lead: Col,
    /// How compact the period's spans are (`span`).
    level: usize,
    /// Whether the report holds an estimated block, whose span `ESTIMATED` leads (T5-21).
    estimated: bool,
    /// The largest row cost, for the share line.
    most: u64,
    theme: Theme,
    now_ms: i64,
}

impl<'a> Table<'a> {
    /// The columns `report` takes at `width` (spec §5.3): period, account, input, output, total
    /// and cost always, with the project for sessions and the model when grouped by it; but not
    /// the period and the project for one session's models, whose rows all share them, and
    /// which the header's drill-down and the detail say. From `WIDE` columns, the cache write
    /// (split into 5-minute and 1-hour from `CACHE_SPLIT_FROM` columns, when that still fits),
    /// the cache read, the responses and the share line. Text columns share the rest. A model is
    /// named by its short form (T5-22).
    fn new(
        report: &'a TokenReport,
        widths: &'a Widths,
        models: &'a ShortModels,
        width: usize,
        theme: Theme,
        now_ms: i64,
    ) -> Table<'a> {
        let q = &report.query;
        let one_session = q.session.is_some();
        let mut cols = Vec::new();
        if !one_session {
            cols.push(Col::Period);
            if q.period == TokenPeriod::Session {
                cols.push(Col::Project);
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
            cols.push(Col::Share);
        }
        let most = report
            .rows
            .iter()
            .filter_map(|r| r.cost.map(|c| c.usd_nanos))
            .max()
            .unwrap_or(0);
        let estimated = report.rows.iter().any(|r| {
            matches!(
                r.period,
                PeriodKey::Block {
                    observed: false,
                    ..
                }
            )
        });
        let mut table = Table {
            report,
            widths,
            models,
            cols: cols.into_iter().map(|c| (c, 0)).collect(),
            lead,
            level: 0,
            estimated,
            most,
            theme,
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
            if width >= CACHE_SPLIT_FROM && natural + extra <= width {
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
        if col == Col::Share {
            return SHARE;
        }
        let cell = |r: &TokenRow| match col {
            Col::Period => self.period_cell(&r.period, level),
            _ => self.cell(col, r).text,
        };
        self.report
            .rows
            .iter()
            .map(|r| width_of(&cell(r)))
            .chain([width_of(&self.total_cell(col).text)])
            .fold(width_of(col.head(self.report.query.period)), usize::max)
    }

    /// Lays the columns out in `width` (B17, B24, T5-20). The numbers take their widest and are
    /// never cut. The period takes the fullest form of its spans that leaves each text column
    /// its least: the model its whole value up to `MODEL_KEEP`, the account its whole name and
    /// the project `PROJECT_KEEP` (or its own width when that is less). Where no form does, the
    /// account gives way to `TEXT_MIN`, with the most compact period; then the project: to
    /// `PROJECT_MIN`, then it is left out, each time with the account whole again first. Where
    /// still none does, the most compact period is used, and the account gives way to
    /// `ACCOUNT_LEAST` and the model to one column, and the period itself is cut when that
    /// leaves the text columns no more. What is left over then goes, in turn, to the model up
    /// to `MODEL_KEEP`, the account, the project, and the model beyond `MODEL_KEEP`, and the
    /// text columns are cut by display width to what they got. A number is never cut.
    fn fit(&mut self, width: usize) {
        let periods = self.found().periods;
        let period_on = self.cols.iter().any(|(c, _)| *c == Col::Period);
        let least = |t: &Table, col: Col, project: usize, account: bool| match col {
            Col::Model => t.widest(col).min(MODEL_KEEP),
            Col::Project => t.widest(col).min(project),
            Col::Account if account => t.widest(col),
            _ => t.widest(col).min(TEXT_MIN),
        };
        let mut project = PROJECT_KEEP;
        let mut account = true;
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
            let wanted: usize = texts
                .iter()
                .map(|&i| least(self, self.cols[i].0, project, account))
                .sum();
            let shortest = if account { 0 } else { SPAN_LEVELS - 1 };
            let level = (shortest..SPAN_LEVELS).find(|&l| {
                let period = if period_on { periods[l] } else { 0 };
                fixed + gaps + period + wanted <= width
            });
            match (
                level,
                self.cols.iter().position(|(c, _)| *c == Col::Project),
            ) {
                (None, _) if account => account = false,
                (None, Some(_)) if project > PROJECT_MIN => {
                    project = PROJECT_MIN;
                    account = true;
                }
                (None, Some(at)) => {
                    self.cols.remove(at);
                    account = true;
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
                    least(self, col, project, account)
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

    /// `period` as its column says it at `level`: led, while the report holds an estimated
    /// block, by `ESTIMATED` for an inferred block and a space for an observed one (T5-21).
    fn period_cell(&self, period: &PeriodKey, level: usize) -> String {
        let text = period_text(period, &self.report.query.tz, self.now_ms, level);
        match period {
            PeriodKey::Block { observed, .. } if self.estimated => {
                format!("{}{text}", if *observed { " " } else { ESTIMATED })
            }
            _ => text,
        }
    }

    /// `text` in `col`'s role (T5-15): a `≥` cell warn, a `—` cell faint.
    fn styled(&self, col: Col, text: String) -> Cell {
        let role = if text.starts_with(words::AT_LEAST) {
            Role::Warn
        } else if text == words::MISSING {
            Role::Faint
        } else {
            col.role()
        };
        Cell {
            text,
            style: self.theme.role(role),
        }
    }

    fn counts_text(col: Col, c: &TokenCounts, unfinalized: u64, responses: u64) -> String {
        match col {
            Col::Input => words::scaled(c.input),
            Col::Output if unfinalized > 0 => {
                format!("{}{}", words::AT_LEAST, words::scaled(c.output))
            }
            Col::Output => words::scaled(c.output),
            Col::CacheWrite => words::scaled(c.cache_write()),
            Col::CacheWrite5m => words::scaled(c.cache_write_5m),
            Col::CacheWrite1h => words::scaled(c.cache_write_1h),
            Col::CacheRead => words::scaled(c.cache_read),
            Col::Total => words::scaled(c.total()),
            Col::Responses => amounts::grouped(responses),
            _ => String::new(),
        }
    }

    /// `row`'s cell in `col`.
    fn cell(&self, col: Col, row: &TokenRow) -> Cell {
        let text = match col {
            Col::Period => self.period_cell(&row.period, self.level),
            Col::Project => match &row.period {
                PeriodKey::Session {
                    project: Some(p), ..
                } => project_name(p).to_owned(),
                _ => words::MISSING.to_owned(),
            },
            Col::Account => {
                let before = row.before_tracking(self.report.tracking_since_ms);
                match &row.account {
                    Some(a) => account_text(a, before),
                    None => row
                        .accounts
                        .iter()
                        .map(|a| account_text(a, before))
                        .collect::<Vec<_>>()
                        .join(", "),
                }
            }
            Col::Model => match &row.model {
                Some(m) => self.models.get(m).to_owned(),
                None => row
                    .models
                    .iter()
                    .map(|m| self.models.get(m))
                    .collect::<Vec<_>>()
                    .join(", "),
            },
            Col::Cost => cost_text(row.cost.as_ref()),
            Col::Share => match row.cost {
                Some(c) if self.most > 0 => {
                    let pct = c.usd_nanos as f64 / self.most as f64 * 100.0;
                    gauge::share_line(pct, SHARE)
                }
                _ => String::new(),
            },
            _ => Self::counts_text(col, &row.counts, row.unfinalized, row.responses),
        };
        self.styled(col, text)
    }

    /// The total line's cell in `col`: `TOTAL` alone bold (T5-18).
    fn total_cell(&self, col: Col) -> Cell {
        let t = &self.report.total;
        let text = match col {
            _ if col == self.lead => {
                return Cell {
                    text: "TOTAL".to_owned(),
                    style: self.theme.strong(),
                };
            }
            Col::Project | Col::Account | Col::Model | Col::Share => String::new(),
            Col::Cost => cost_text(t.cost.as_ref()),
            _ => Self::counts_text(col, &t.counts, t.unfinalized, t.responses),
        };
        self.styled(col, text)
    }

    /// One line of cells: each padded or cut from its end to its column, numbers on the right,
    /// two columns apart, after a one-column margin. `blank` leaves the period's own columns
    /// empty.
    fn line(&self, cells: Vec<Cell>, blank: bool) -> Line<'static> {
        let mut spans = vec![Span::raw(" ")];
        for (i, ((col, w), cell)) in self.cols.iter().zip(cells).enumerate() {
            if i > 0 {
                spans.push(Span::raw("  "));
            }
            let text = if blank && matches!(col, Col::Period | Col::Project) {
                String::new()
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

    /// The heads, muted capitals, never bold (T5-15).
    fn header(&self) -> Line<'static> {
        let period = self.report.query.period;
        let muted = self.theme.role(Role::Muted);
        let cells = self
            .cols
            .iter()
            .map(|(c, _)| Cell {
                text: c.head(period).to_owned(),
                style: muted,
            })
            .collect();
        self.line(cells, false)
    }

    fn row(&self, row: &TokenRow, blank: bool) -> Line<'static> {
        let cells = self.cols.iter().map(|(c, _)| self.cell(*c, row)).collect();
        self.line(cells, blank)
    }

    /// The faint rule above the total (T5-18), from column 1 across the table's columns.
    fn rule(&self) -> Line<'static> {
        let width =
            self.cols.iter().map(|(_, w)| w).sum::<usize>() + 2 * self.cols.len().saturating_sub(1);
        Line::from(vec![
            Span::raw(" "),
            Span::styled("─".repeat(width), self.theme.role(Role::Faint)),
        ])
    }

    fn total(&self) -> Line<'static> {
        let cells = self.cols.iter().map(|(c, _)| self.total_cell(*c)).collect();
        self.line(cells, false)
    }
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

/// `parts` in `style` laid out after `head` in rows of at most `width` columns, `sep` between
/// two parts on a row, each row after the first `indent` columns in (B26). A part that fits no
/// row whole wraps at its spaces, and a word wider than a row breaks by width (`view::wrap`):
/// nothing is cut, for what lies below the total scrolls (B29).
fn laid_out(
    head: Vec<Span<'static>>,
    parts: &[String],
    style: Style,
    sep: Span<'static>,
    indent: usize,
    width: usize,
) -> Vec<Line<'static>> {
    let room = width.saturating_sub(indent).max(1);
    let pad = || Span::raw(" ".repeat(indent));
    let sep_width = width_of(&sep.content);
    let mut lines = Vec::new();
    let mut used: usize = head.iter().map(|s| width_of(&s.content)).sum();
    let mut spans = head;
    for part in parts {
        let fresh = used <= indent;
        let w = width_of(part);
        if fresh && used + w <= width {
            spans.push(Span::styled(part.clone(), style));
            used += w;
            continue;
        }
        if !fresh && used + sep_width + w <= width {
            spans.push(sep.clone());
            spans.push(Span::styled(part.clone(), style));
            used += sep_width + w;
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
            spans.push(Span::styled(chunk, style));
        }
    }
    lines.push(Line::from(spans));
    lines
}

/// What of `row`'s figures depends on the time: the elapsed and remaining seconds, which count
/// on between queries from the block's start and end, while it is active. `None` when `row` is
/// not a block, has no `active`, or has ended.
pub(super) fn figures_clock(row: &TokenRow, now_ms: i64) -> Option<(i64, i64)> {
    let PeriodKey::Block {
        start_ms,
        end_ms,
        active: Some(_),
        ..
    } = &row.period
    else {
        return None;
    };
    if now_ms >= *end_ms {
        return None;
    }
    Some(((now_ms - start_ms).max(0) / 1000, (end_ms - now_ms) / 1000))
}

/// The active block's figures (spec §5.3, §6.4) at `now_ms`: its elapsed and remaining time,
/// its burn rate and its projection. `None` once it has ended: the row is no longer active.
fn figures(row: &TokenRow, now_ms: i64) -> Option<Vec<String>> {
    let (elapsed, left) = figures_clock(row, now_ms)?;
    let PeriodKey::Block {
        active: Some(a), ..
    } = &row.period
    else {
        return None;
    };
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

/// The table's lines and the line each row starts at (PERF-R-10b).
pub(super) struct Rows {
    items: Vec<(usize, Item)>,
    starts: Vec<usize>,
}

impl Rows {
    /// The table's lines at `width`.
    fn build(report: &TokenReport, width: usize, now_ms: i64) -> Rows {
        let mut items = Vec::new();
        let mut starts = Vec::with_capacity(report.rows.len());
        for (index, row) in report.rows.iter().enumerate() {
            starts.push(items.len());
            let repeat = index > 0 && report.rows[index - 1].period == row.period;
            items.push((index, Item::Row { repeat }));
            let parts = figures(row, now_ms).filter(|_| !repeat);
            if let Some(parts) = parts {
                let head = vec![Span::raw(" ".repeat(FIGURES_INDENT))];
                for line in laid_out(
                    head,
                    &parts,
                    Style::new(),
                    Span::raw(" · "),
                    FIGURES_INDENT,
                    width,
                ) {
                    items.push((index, Item::Figures(line)));
                }
            }
        }
        Rows { items, starts }
    }
}

/// What `Layout` holds: the rows, for the width and the clocks of the active blocks' rows.
struct Laid {
    width: usize,
    /// The report's rows that are a block with `active`, and their `figures_clock` when laid out.
    active: Vec<(usize, Option<(i64, i64)>)>,
    rows: Rc<Rows>,
}

/// `Rows` for the level's report, kept while the width, the number of rows and the active
/// blocks' clocks stay: the time reaches the rows only through `figures`, so a report with no
/// active block is laid out once per width, and one with an active block whenever a clock of
/// its figures changes.
#[derive(Default)]
pub(super) struct Layout {
    laid: RefCell<Option<Laid>>,
    #[cfg(test)]
    builds: std::cell::Cell<usize>,
}

impl Layout {
    /// The rows of `report` at `width` and `now_ms`: the kept ones while they read the same.
    pub(super) fn rows(&self, report: &TokenReport, width: usize, now_ms: i64) -> Rc<Rows> {
        let mut laid = self.laid.borrow_mut();
        if let Some(l) = laid.as_ref().filter(|l| {
            l.width == width
                && l.rows.starts.len() == report.rows.len()
                && l.active.iter().all(|&(i, clock)| {
                    report
                        .rows
                        .get(i)
                        .is_some_and(|row| figures_clock(row, now_ms) == clock)
                })
        }) {
            return Rc::clone(&l.rows);
        }
        let active = report
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                matches!(
                    row.period,
                    PeriodKey::Block {
                        active: Some(_),
                        ..
                    }
                )
            })
            .map(|(i, row)| (i, figures_clock(row, now_ms)))
            .collect();
        let rows = Rc::new(Rows::build(report, width, now_ms));
        #[cfg(test)]
        self.builds.set(self.builds.get() + 1);
        *laid = Some(Laid {
            width,
            active,
            rows: Rc::clone(&rows),
        });
        rows
    }

    /// How many times the rows were laid out.
    #[cfg(test)]
    pub(super) fn builds(&self) -> usize {
        self.builds.get()
    }
}

/// The grouping's words on the control bar (T5-6).
fn grouping_text(g: TokenGrouping) -> &'static str {
    match g {
        TokenGrouping::None => "no grouping",
        TokenGrouping::Account => "by account",
        TokenGrouping::Model => "by model",
        TokenGrouping::AccountModel => "by account and model",
    }
}

/// The filter's words on the control bar (T5-6): `all accounts · all models`, else what it
/// keeps, an account by its short name; one model in full, or with `short` by the shown level's
/// short form (T5-22).
fn filter_text(state: &State, short: bool) -> String {
    let t = &state.tokens;
    let accounts = match t.accounts.as_deref() {
        None => "all accounts".to_owned(),
        Some([AccountSel::Unattributed]) => "unknown account".to_owned(),
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
                |v| {
                    let email = words::email(&v.row);
                    words::short_name(v.row.position, v.row.alias.as_deref(), &email)
                },
            ),
        Some(list) => words::plural(list.len() as u64, "account"),
    };
    let models = match t.models.as_deref() {
        None => "all models".to_owned(),
        Some([one]) if short => t
            .level()
            .short_models()
            .map_or(one.as_str(), |m| m.get(one))
            .to_owned(),
        Some([one]) => one.clone(),
        Some(list) => words::plural(list.len() as u64, "model"),
    };
    format!("{accounts} · {models}")
}

/// The rows the drill-down came through, from the top (T5-9): ` › {crumb} › {crumb}`, muted,
/// each its row's period (a session by its ID's first 8 characters, an estimated block led by
/// `ESTIMATED`) and its account and short model (T5-22) when it has them, cut to the width less
/// one. `None` at the top.
fn crumbs(state: &State, width: usize, now_ms: i64) -> Option<Line<'static>> {
    let t = &state.tokens;
    let parents: Vec<String> = t.levels[..t.levels.len() - 1]
        .iter()
        .filter_map(|level| {
            let report = level.report.as_ref()?;
            let key = level.selected.as_ref()?;
            let row = report.rows.iter().find(|r| &r.key == key)?;
            let models = level.short_models()?;
            let period = match &row.period {
                PeriodKey::Session { session_id, .. } => {
                    format!("session {}", short_id(session_id))
                }
                p @ PeriodKey::Block {
                    observed: false, ..
                } => format!("{ESTIMATED}{}", period_text(p, &report.query.tz, now_ms, 0)),
                p => period_text(p, &report.query.tz, now_ms, 0),
            };
            let mut parts = vec![period];
            let before = row.before_tracking(report.tracking_since_ms);
            parts.extend(row.account.as_ref().map(|a| account_text(a, before)));
            parts.extend(row.model.as_deref().map(|m| models.get(m).to_owned()));
            Some(parts.join(" · "))
        })
        .collect();
    if parents.is_empty() {
        return None;
    }
    let crumb = format!("› {}", parents.join(" › "));
    Some(Line::from(vec![
        Span::raw(" "),
        Span::styled(
            fit(&crumb, width.saturating_sub(2)),
            state.theme.role(Role::Muted),
        ),
    ]))
}

/// The bar's periods, in `p`'s order.
const PERIODS: [(TokenPeriod, &str); 4] = [
    (TokenPeriod::Day, "Day"),
    (TokenPeriod::Month, "Month"),
    (TokenPeriod::Session, "Session"),
    (TokenPeriod::Block, "5h block"),
];
/// Between the bar's groups (§8's text), and where that does not fit (§4.3's group gap).
const BAR_GAP: &str = "     ";
const BAR_GAP_NARROW: &str = "   ";
/// The least gap before the bar's right part (T5-7).
const BAR_RIGHT_GAP: usize = 3;

/// The index's state as the bar says it (T5-8), joined ` · `: the TUI's own refresh's
/// progress, another process catching up, or when the index was last refreshed; then
/// `catching up` while the index has never completed a refresh, and with `updating`,
/// `updating…` while the shown report is read again.
fn index_words(t: &TokensState, now_ms: i64, updating: bool) -> String {
    let index = t.watch.state();
    let mut parts = Vec::new();
    if let Some(p) = t.watch.refreshing() {
        parts.push(if p.bytes_total > 0 {
            let (done, total) = (bytes(p.bytes_done), bytes(p.bytes_total));
            let done = match (done.split_once(' '), total.split_once(' ')) {
                (Some((n, unit)), Some((_, of))) if unit == of => n.to_owned(),
                _ => done,
            };
            format!("indexing {done} / {total}")
        } else {
            "indexing…".to_owned()
        });
    } else if t.watch.busy() {
        parts.push("catching up elsewhere".to_owned());
    } else if let Some(at) = index.and_then(|i| i.refreshed_at_ms) {
        parts.push(format!("indexed {}", autoline::when(at, now_ms, &t.tz)));
    }
    if t.watch.refreshing().is_none() && index.is_some_and(|i| i.refreshed_at_ms.is_none()) {
        parts.push("catching up".to_owned());
    }
    if updating && t.pending && t.report().is_some() {
        parts.push("updating…".to_owned());
    }
    parts.join(" · ")
}

/// The control bar (T5-6): ` p` and the periods, `[ ]` and the range, `g` and the top level's
/// applied grouping, `f` and the filter, and the index's state on the right, `RIGHT_INSET` in.
/// It gives way as T5-7 orders, in the first of its forms that fits; a filter it must cut names
/// one model by its short form, and gives way to the index's words down to `FILTER_MIN`.
fn bar(state: &State, width: usize, now_ms: i64) -> Line<'static> {
    let t = &state.tokens;
    let theme = state.theme;
    let (accent, text, muted) = (
        theme.role(Role::Accent),
        theme.role(Role::Text),
        theme.role(Role::Muted),
    );
    let grouping = grouping_text(t.top_query(now_ms).applied_grouping());
    let left = |gap: &'static str, every_period: bool, filter: String| {
        let mut spans = vec![Span::raw(" "), Span::styled("p", accent)];
        let mut first = true;
        for (period, name) in PERIODS {
            let current = period == t.period;
            if !current && !every_period {
                continue;
            }
            spans.push(Span::raw(if first { " " } else { "  " }));
            spans.push(Span::styled(name, if current { text } else { muted }));
            first = false;
        }
        spans.extend([
            Span::raw(gap),
            Span::styled("[ ]", accent),
            Span::raw(" "),
            Span::styled(t.range.label(t.period), text),
            Span::raw(gap),
            Span::styled("g", accent),
            Span::raw(" "),
            Span::styled(grouping, text),
            Span::raw(gap),
            Span::styled("f", accent),
            Span::raw(" "),
            Span::styled(filter, text),
        ]);
        spans
    };
    let used = |spans: &[Span]| spans.iter().map(|s| width_of(&s.content)).sum::<usize>();
    let fits = |left: &[Span], right: &str| {
        used(left) + BAR_RIGHT_GAP + width_of(right) + RIGHT_INSET <= width
    };
    let filter = filter_text(state, false);
    let unfiltered = t.accounts.is_none() && t.models.is_none();
    let short = if unfiltered {
        "all".to_owned()
    } else {
        filter.clone()
    };
    let (whole, quiet) = (index_words(t, now_ms, true), index_words(t, now_ms, false));
    let forms = [
        (left(BAR_GAP, true, filter.clone()), &whole),
        (left(BAR_GAP_NARROW, true, filter.clone()), &whole),
        (left(BAR_GAP_NARROW, true, filter), &quiet),
        (left(BAR_GAP_NARROW, false, short.clone()), &whole),
        (left(BAR_GAP_NARROW, false, short.clone()), &quiet),
    ];
    let (mut spans, right) = match forms.into_iter().find(|(l, r)| fits(l, r)) {
        Some((left, right)) => (left, vec![Span::styled(right.clone(), muted)]),
        None => {
            let cut = if unfiltered {
                short
            } else {
                filter_text(state, true)
            };
            let bare = used(&left(BAR_GAP_NARROW, false, String::new()));
            let room = width.saturating_sub(bare + BAR_RIGHT_GAP + width_of(&quiet) + RIGHT_INSET);
            let least = FILTER_MIN.min(width_of(&cut));
            let left = left(BAR_GAP_NARROW, false, fit(&cut, room.max(least)));
            let room = width.saturating_sub(used(&left) + BAR_RIGHT_GAP + RIGHT_INSET);
            let right = if room < 2 {
                vec![]
            } else {
                fit_spans(vec![Span::styled(quiet.clone(), muted)], room)
            };
            (left, right)
        }
    };
    if used(&right) > 0 {
        let gap = width.saturating_sub(used(&spans) + used(&right) + RIGHT_INSET);
        spans.push(Span::raw(" ".repeat(gap)));
        spans.extend(right);
    }
    Line::from(fit_spans(spans, width))
}

/// The rows above the table (T5-5): the control bar; the rows the drill-down came through,
/// while drilled; and while the report has rows, its headline, DIM while it is the last one
/// read (T2-22), and a blank row. The bar and the crumbs are the controls, so they stay plain.
fn top(state: &State, width: usize, now_ms: i64) -> Vec<Line<'static>> {
    let t = &state.tokens;
    let mut lines = vec![bar(state, width, now_ms)];
    lines.extend(crumbs(state, width, now_ms));
    if let Some(report) = t.report().filter(|r| !r.rows.is_empty()) {
        for line in headline::headline(state, report, width, now_ms) {
            lines.push(if t.stale() {
                line.patch_style(state.theme.dim())
            } else {
                line
            });
        }
        lines.push(Line::default());
    }
    lines
}

/// The selected row in full (B11, B25, T5-23), in rows at `width`, `DETAIL_INDENT` in: under
/// `SELECTED`, what tells it apart (its period and its account), with what `Enter` opens from it
/// when `enter` says; then its numbers, muted; then the rest of what it holds: a session's
/// project's path and its times, whether a block was observed or estimated, every account when
/// it has none of its own, and every model in full. Laid out by their parts and never cut.
/// `before` says the row's period ends before the index's epoch, which names its unattributed
/// usage `before tracking`.
fn detail_rows(
    row: &TokenRow,
    tz: &TimeZone,
    before: bool,
    now_ms: i64,
    width: usize,
    enter: Option<&str>,
    theme: Theme,
) -> Vec<Line<'static>> {
    let (text, muted) = (theme.role(Role::Text), theme.role(Role::Muted));
    let sep = || Span::styled(" · ", muted);
    let pad = || vec![Span::raw(" ".repeat(DETAIL_INDENT))];
    let c = &row.counts;
    let numbers = [
        format!(
            "{} {}",
            amounts::grouped(row.responses),
            if row.responses == 1 {
                "response"
            } else {
                "responses"
            }
        ),
        format!(
            "cache write {} (5m {}, 1h {})",
            words::scaled(c.cache_write()),
            words::scaled(c.cache_write_5m),
            words::scaled(c.cache_write_1h)
        ),
        format!("cache read {}", words::scaled(c.cache_read)),
    ];
    let (first, mut rest) = match &row.period {
        PeriodKey::Session {
            session_id,
            project,
            first_ms,
            last_ms,
        } => (
            format!("session {}", short_id(session_id)),
            vec![
                project.clone().unwrap_or_else(|| "no project".to_owned()),
                format!("{} – {}", when(*first_ms, tz), when(*last_ms, tz)),
            ],
        ),
        PeriodKey::Block {
            start_ms,
            end_ms,
            observed,
            ..
        } => (
            format!("{} – {}", when(*start_ms, tz), when(*end_ms, tz)),
            vec![if *observed { "observed" } else { "estimated" }.to_owned()],
        ),
        p => (period_text(p, tz, now_ms, 0), vec![]),
    };
    let mut identity = vec![first];
    match &row.account {
        Some(account) => identity.push(account_name(account, before)),
        None if !row.accounts.is_empty() => {
            let names: Vec<String> = row
                .accounts
                .iter()
                .map(|a| account_name(a, before))
                .collect();
            rest.push(names.join(", "));
        }
        None => {}
    }
    if !row.models.is_empty() {
        rest.push(row.models.join(", "));
    }
    let head = vec![
        Span::styled(format!(" {SELECTED}"), muted),
        Span::raw(" ".repeat(DETAIL_INDENT - 1 - SELECTED.len())),
    ];
    let mut rows = laid_out(head, &identity, text, sep(), DETAIL_INDENT, width);
    if let Some(to) = enter {
        let opens = [
            Span::styled("⏎", theme.role(Role::Accent)),
            Span::styled(format!(" its {to}"), muted),
        ];
        let gap = "   ";
        let used = rows.last().map_or(0, Line::width);
        let wanted = width_of(gap) + opens.iter().map(Span::width).sum::<usize>();
        match rows.last_mut() {
            Some(last) if used + wanted <= width => {
                last.spans.push(Span::raw(gap));
                last.spans.extend(opens);
            }
            _ => rows.push(Line::from([pad(), opens.to_vec()].concat())),
        }
    }
    rows.extend(laid_out(
        pad(),
        &numbers,
        muted,
        sep(),
        DETAIL_INDENT,
        width,
    ));
    if !rest.is_empty() {
        rows.extend(laid_out(pad(), &rest, muted, sep(), DETAIL_INDENT, width));
    }
    rows
}

/// What lies below the total (B25): a blank row and the selected row's detail, then a blank row
/// and the footnotes (TR §11.2, B15, T5-24), each led by its mark, if it has one, and wrapped
/// at `width` `NOTE_INDENT` in; never cut, for the region scrolls (B29).
fn below(state: &State, width: usize, now_ms: i64) -> Vec<Line<'static>> {
    let t = &state.tokens;
    let theme = state.theme;
    let mut lines = Vec::new();
    let Some(report) = t.report().filter(|r| !r.rows.is_empty()) else {
        return lines;
    };
    if let Some(row) = t.selected_row() {
        lines.push(Line::default());
        let before = row.before_tracking(report.tracking_since_ms);
        lines.extend(detail_rows(
            row,
            &report.query.tz,
            before,
            now_ms,
            width,
            t.drill_to(),
            theme,
        ));
    }
    let notes = words::token_footnotes(report, words::AT_LEAST, true);
    if !notes.is_empty() {
        lines.push(Line::default());
    }
    let muted = theme.role(Role::Muted);
    for note in notes {
        let head = match note.mark {
            Some(mark) => {
                let role = if mark == words::AT_LEAST {
                    Role::Warn
                } else {
                    Role::Muted
                };
                vec![
                    Span::raw(" "),
                    Span::styled(mark, theme.role(role)),
                    Span::raw(" ".repeat(NOTE_INDENT.saturating_sub(1 + width_of(mark)))),
                ]
            }
            None => vec![Span::raw(" ".repeat(NOTE_INDENT))],
        };
        lines.extend(laid_out(
            head,
            &[note.text],
            muted,
            Span::raw(" "),
            NOTE_INDENT,
            width,
        ));
    }
    lines
}

/// What the body holds under the rows above the table (`top`), at its size: the table's head and
/// rows, then the rule and the total, then what lies below it (`below`) in as many rows as it
/// needs, half the body at most and leaving the table `TABLE_MIN` rows. What does not fit there
/// scrolls with `J`/`K` (B25), and its last row says which lines show. The total follows the last
/// row shown, so it is at the foot of a table that fills the body.
struct Plan {
    top: Vec<Line<'static>>,
    rows: usize,
    below: Vec<Line<'static>>,
}

/// The table's rows that are not report rows: the head, the rule and the total (T5-18).
const TABLE_FIXED: usize = 3;

/// How many rows what lies below the total takes: as many as its `lines`, half of `height` at
/// most, and never the table's last `TABLE_MIN`.
fn below_rows(lines: usize, top: usize, height: usize) -> usize {
    let fixed = top + TABLE_FIXED;
    lines.min(height.saturating_sub(fixed + TABLE_MIN).min(height / 2))
}

/// The lines that show of `lines` in `rows` rows from line `from` (clamped, R22), with a last row
/// that says which when they do not all fit; and the furthest `from` goes.
fn window(
    theme: Theme,
    lines: Vec<Line<'static>>,
    from: usize,
    rows: usize,
) -> (Vec<Line<'static>>, usize) {
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
        out.push(Line::from(Span::styled(hint, theme.role(Role::Muted))));
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
    let top = top(state, width, now_ms);
    let lines = below(state, width, now_ms);
    let rows = below_rows(lines.len(), top.len(), height);
    let (below, _) = window(state.theme, lines, below_from(&state.tokens), rows);
    let table = height.saturating_sub(top.len() + TABLE_FIXED + below.len());
    Plan {
        top,
        rows: table,
        below,
    }
}

/// `J`/`K` (B25): what lies below the total moves `by` a line, stopping at either end.
pub(super) fn scroll_below(state: &mut State, area: Rect, now_ms: i64, by: isize) {
    let width = usize::from(area.width);
    let top = top(state, width, now_ms).len();
    let lines = below(state, width, now_ms);
    let rows = below_rows(lines.len(), top, usize::from(area.height));
    let from = below_from(&state.tokens);
    let (_, furthest) = window(state.theme, lines, from, rows);
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
    let laid = t
        .level()
        .layout
        .rows(report, usize::from(area.width), now_ms);
    let (items, starts) = (&laid.items, &laid.starts);
    let last = starts.get(at + 1).copied().unwrap_or(items.len()) - 1;
    let offset = keep_visible(t.level().offset, starts[at], last, rows, items.len());
    state.tokens.level_mut().offset = offset;
}

/// What explains an empty report (TR §11.3): the lines and the files the index could not read,
/// in the footnotes' words, wrapped at `width`.
fn empty_warnings(index: &tagteam_engine::tokens::IndexState, width: usize) -> Vec<Line<'static>> {
    let mut notes = Vec::new();
    if index.skipped_lines > 0 {
        notes.push(words::skipped_transcript_lines(index.skipped_lines));
    }
    if index.unreadable_files > 0 {
        notes.push(format!(
            "{}.",
            words::unreadable_transcripts(index.unreadable_files)
        ));
    }
    notes
        .into_iter()
        .flat_map(|note| {
            laid_out(
                vec![Span::raw(" ")],
                &[note],
                Style::new(),
                Span::raw(" "),
                1,
                width,
            )
        })
        .collect()
}

pub fn render(state: &State, frame: &mut Frame, area: Rect, now_ms: i64) {
    let width = usize::from(area.width);
    let t = &state.tokens;
    let plan = plan(state, area, now_ms);
    let mut lines = plan.top;
    let level = t.level();
    let Some((report, models)) = level.report.as_ref().zip(level.short_models()) else {
        if t.pending {
            lines.push(Line::from(format!(" {LOADING}")));
        }
        frame.render_widget(Paragraph::new(lines), area);
        return;
    };
    if report.rows.is_empty() {
        let unbuilt = t.watch.state().is_some_and(|i| i.refreshed_at_ms.is_none());
        if !unbuilt {
            lines.push(Line::from(format!(" {}", words::NO_TOKEN_USAGE)));
        }
        lines.extend(empty_warnings(&report.index, width));
        let used = u16::try_from(lines.len())
            .unwrap_or(u16::MAX)
            .min(area.height);
        frame.render_widget(Paragraph::new(lines), area);
        if unbuilt {
            let display = state.names_of(t.provider()).display_name;
            let sentence =
                format!("tagteam reads {display}'s transcripts; they appear here as you use it.");
            let rest = Rect::new(area.x, area.y + used, area.width, area.height - used);
            centred_block(frame, rest, NOT_INDEXED, Some(&sentence), state.theme);
        }
        return;
    }
    let table = Table::new(report, &level.widths, models, width, state.theme, now_ms);
    // A failed query keeps the last report, DIM (T2-22).
    let dim = |line: Line<'static>| {
        if t.stale() {
            line.patch_style(state.theme.dim())
        } else {
            line
        }
    };
    lines.push(dim(table.header()));
    let laid = t.level().layout.rows(report, width, now_ms);
    let (items, starts) = (&laid.items, &laid.starts);
    let selected_at = t.selected_index();
    let offset = match selected_at {
        Some(at) => {
            let last = starts.get(at + 1).copied().unwrap_or(items.len()) - 1;
            keep_visible(t.level().offset, starts[at], last, plan.rows, items.len())
        }
        None => t.level().offset.min(items.len().saturating_sub(plan.rows)),
    };
    for (n, (index, item)) in items.iter().skip(offset).take(plan.rows).enumerate() {
        lines.push(dim(match item {
            Item::Row { repeat } => {
                let row = table.row(&report.rows[*index], *repeat && n > 0);
                if Some(*index) == selected_at {
                    select(row, Some(1), width, state.theme)
                } else {
                    row
                }
            }
            Item::Figures(line) => line.clone().style(state.theme.role(Role::Muted)),
        }));
    }
    lines.push(dim(table.rule()));
    lines.push(dim(table.total()));
    lines.extend(plan.below.into_iter().map(dim));
    frame.render_widget(Paragraph::new(lines), area);
}

/// The fixtures the headline's tests share with the table's.
#[cfg(test)]
pub(super) use tests::{HOUR_MS, NOON, block, body_y, days, letters, showing, showing_after};

#[cfg(test)]
mod tests {
    use crossterm::event::KeyCode;
    use ratatui::style::{Color, Modifier};
    use tagteam_core::AccountId;
    use tagteam_engine::tokens::{ActiveBlock, IndexProgress, IndexState, RefreshOutcome, RowKey};

    use super::*;
    use crate::msg::{Cmd, Msg};
    use crate::source::SourceError;
    use crate::state::BannerKind;
    use crate::state::Screen;
    use crate::testutil::*;
    use crate::theme::Theme;
    use crate::update::update;

    /// 2026-09-20T00:00:00Z, the day before `NOW_MS`'s.
    const YESTERDAY: i64 = 1_789_862_400_000;
    pub(crate) const HOUR_MS: i64 = 3_600_000;
    const DAY_MS: i64 = 24 * HOUR_MS;

    /// Tokens at 80×24, its report for the query it asked for, `rows`, then `edit`ed.
    pub(crate) fn showing(rows: Vec<TokenRow>, edit: impl FnOnce(&mut TokenReport)) -> State {
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
    pub(crate) fn showing_after(keys: &str, rows: Vec<TokenRow>) -> State {
        showing_after_with(keys, rows, |_| {})
    }

    /// The same, the report `edit`ed as `showing` does.
    fn showing_after_with(
        keys: &str,
        rows: Vec<TokenRow>,
        edit: impl FnOnce(&mut TokenReport),
    ) -> State {
        let mut state = fresh(Screen::Tokens);
        state.start();
        let mut last = vec![];
        for c in keys.chars() {
            last = update(&mut state, ch(c), NOW_MS);
        }
        let [Cmd::LoadReport { r#gen, query }] = last.as_slice() else {
            panic!("{last:?}");
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
    pub(crate) fn days() -> Vec<TokenRow> {
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

    /// The bar's left part at 80 for a Day report, unfiltered (T5-7's form 4).
    const DAY_BAR: &str = " p Day   [ ] 30 days   g by account   f all";

    /// Body row 0 as the bar draws it `width` wide: `left`, a gap, then `right` ending
    /// `RIGHT_INSET` in.
    fn bar_row(left: &str, right: &str, width: usize) -> String {
        let gap = width - RIGHT_INSET - width_of(left) - width_of(right);
        format!("{left}{}{right}", " ".repeat(gap))
    }

    /// `parts` as `roles` letters: each part's characters in its role, its spaces blank.
    pub(crate) fn letters(parts: &[(&str, char)]) -> String {
        parts
            .iter()
            .flat_map(|(text, role)| {
                text.chars()
                    .map(move |c| if c == ' ' { ' ' } else { *role })
            })
            .collect()
    }

    /// The body's row `n` at `w`×`h`, where `draw_buffer` puts it.
    pub(crate) fn body_y(state: &State, w: u16, h: u16, n: u16) -> u16 {
        crate::view::areas(state, Rect::new(0, 0, w, h)).body.y + n
    }

    #[test]
    fn a_day_report_at_80_columns() {
        // Spec §5.3: period, account, input, output, total and cost; the period once per day;
        // a faint rule above the total (T5-18); the selected row's labelled detail and the
        // notes, each after a blank row (T5-23, T5-24).
        let state = showing(days(), |_| {});
        let lines = body_of(&state, 80, 24, NOW_MS);
        assert_eq!(lines[0], bar_row(DAY_BAR, "indexed 14:12", 80));
        assert_eq!(
            lines[1..16],
            [
                " $50.95 at API prices    114.2M tokens    414 responses".to_owned(),
                String::new(),
                " DAY         ACCOUNT           INPUT   OUTPUT   TOTAL   API $".to_owned(),
                "▌2026-09-20  1 a1@x.co          1.2M  ≥310.0K   93.6M  $41.20".to_owned(),
                "             2 spare          300.0K    80.0K   20.5M   $9.75".to_owned(),
                " 2026-09-19  unknown account    5.0K      900  105.9K       —".to_owned(),
                format!(" {}", "─".repeat(60)),
                " TOTAL                          1.5M  ≥390.9K  114.2M  $50.95".to_owned(),
                String::new(),
                " SELECTED  2026-09-20 · 1 a1@x.co   ⏎ its sessions".to_owned(),
                "           412 responses · cache write 4.1M (5m 3.0M, 1h 1.1M)".to_owned(),
                "           cache read 88.0M".to_owned(),
                String::new(),
                " ≥ at least: 2 subagent responses are still being counted".to_owned(),
                " $ what this would cost on the API at list prices of 2026-10-01; not your bill"
                    .to_owned(),
            ]
        );
        assert_eq!(
            draw(&state, 80, 24)[23],
            footer_row(
                "⏎ sessions  p period  g group  f filter  [ ] range  r refresh",
                80
            )
        );
    }

    #[test]
    fn the_selected_row_s_detail_is_labelled_and_says_what_enter_opens() {
        // T5-23: below the total a blank row, then `SELECTED`, the row's identity and what
        // `Enter` opens, then its numbers, all `DETAIL_INDENT` in.
        let mut state = showing(days(), |_| {});
        let lines = body_of(&state, 80, 24, NOW_MS);
        assert_eq!(
            lines[9..16],
            [
                "",
                " SELECTED  2026-09-20 · 1 a1@x.co   ⏎ its sessions",
                "           412 responses · cache write 4.1M (5m 3.0M, 1h 1.1M)",
                "           cache read 88.0M",
                "",
                " ≥ at least: 2 subagent responses are still being counted",
                " $ what this would cost on the API at list prices of 2026-10-01; not your bill",
            ]
        );
        let numbers = "412 responses · cache write 4.1M (5m 3.0M, 1h 1.1M)";
        let rows = [
            (
                10,
                letters(&[
                    (" ", 't'),
                    ("SELECTED", 'm'),
                    ("  ", 't'),
                    ("2026-09-20", 't'),
                    (" · ", 'm'),
                    ("1 a1@x.co", 't'),
                    ("   ", 't'),
                    ("⏎", 'a'),
                    (" its sessions", 'm'),
                ]),
            ),
            (11, letters(&[("           ", 't'), (numbers, 'm')])),
            (
                12,
                letters(&[("           ", 't'), ("cache read 88.0M", 'm')]),
            ),
        ];
        for theme in TIERS {
            state.theme = theme;
            let buf = draw_buffer(&state, 80, 24, NOW_MS);
            for (n, want) in &rows {
                let y = body_y(&state, 80, 24, *n);
                assert_eq!(
                    roles(&buf, theme, y).trim_end(),
                    canon(theme, want),
                    "{theme:?}: body row {n}"
                );
            }
        }
        // An unattributed row names its bucket.
        for _ in 0..2 {
            update(&mut state, ch('j'), NOW_MS);
        }
        let lines = body_of(&state, 80, 24, NOW_MS);
        let total = lines.iter().position(|l| l.starts_with(" TOTAL")).unwrap();
        assert!(
            lines[total + 2].starts_with(" SELECTED  2026-09-19 · unknown account   "),
            "{lines:?}"
        );
        // A session's row opens its models; a session's models open nothing.
        let counts = counts(1_200_000, 310_000, 88_000_000);
        let day_row = token_row(day(1), Some(me()), None, counts, Some(41_200_000_000));
        let mut state = showing(vec![day_row], |_| {});
        let selected = |state: &State| {
            let lines = body_of(state, 80, 24, NOW_MS);
            let total = lines.iter().position(|l| l.starts_with(" TOTAL")).unwrap();
            lines[total + 2].clone()
        };
        for (rows, want) in [
            (
                vec![token_row(work_session(), Some(me()), None, counts, None)],
                " SELECTED  session 3f2a9c1d · 1 alex@example.com   ⏎ its models",
            ),
            (
                work_models(),
                " SELECTED  session 3f2a9c1d · 1 alex@example.com",
            ),
        ] {
            let down = update(&mut state, key(KeyCode::Enter), NOW_MS);
            answer_with(&mut state, &down, rows);
            assert_eq!(selected(&state), want);
        }
        assert_eq!(state.tokens.depth(), 2);
    }

    #[test]
    fn each_note_is_led_by_its_mark() {
        // T5-24: ` {mark} {text}`, `≥` in warn and `$` muted, the words muted; a note without a
        // mark starts where the words do, and so does a note's second row.
        let mut state = showing(days(), |r| {
            r.unpriced_models = vec!["claude-test-9".into()];
            r.index.skipped_lines = 12;
            r.index.unreadable_files = 3;
            r.price_overrides = vec!["claude-opus-5-5".into()];
        });
        let lines = body_of(&state, 80, 40, NOW_MS);
        let total = lines.iter().position(|l| l.starts_with(" TOTAL")).unwrap();
        let notes = [
            "   No price for claude-test-9: counted without cost (≥ marks a partial cost).",
            " ≥ at least: 2 subagent responses are still being counted",
            "   12 transcript lines could not be read, so these totals may be low.",
            "   3 transcript files could not be read; the next refresh tries again.",
            " $ what this would cost on the API at list prices of 2026-10-01, with",
            "   config.toml's for claude-opus-5-5; not your bill",
        ];
        // The detail is a blank row and three more, then a blank row.
        let at = total + 5;
        assert_eq!(lines[at], "", "{lines:?}");
        assert_eq!(lines[at + 1..at + 7], notes, "{lines:?}");
        assert!(!lines.iter().any(|l| l.contains("J K:")), "{lines:?}");
        let mark = |n: usize, role: char| {
            let mark: String = notes[n].chars().skip(1).take(1).collect();
            let text: String = notes[n].chars().skip(3).collect();
            letters(&[(" ", 't'), (&mark, role), (" ", 't'), (&text, 'm')])
        };
        let plain = |n: usize| letters(&[(notes[n], 'm')]);
        let rows = [
            (0, plain(0)),
            (1, mark(1, 'w')),
            (2, plain(2)),
            (3, plain(3)),
            (4, mark(4, 'm')),
            (5, plain(5)),
        ];
        for theme in TIERS {
            state.theme = theme;
            let buf = draw_buffer(&state, 80, 40, NOW_MS);
            for (n, want) in &rows {
                let y = body_y(&state, 80, 40, (at + 1 + n) as u16);
                assert_eq!(
                    roles(&buf, theme, y).trim_end(),
                    canon(theme, want),
                    "{theme:?}: note row {n}"
                );
            }
        }
    }

    #[test]
    fn from_120_columns_the_cache_the_responses_and_a_share_line_join_them() {
        // T5-16, T5-17: one `CACHE W` below 160 columns; the share line in place of the bar.
        let state = showing(days(), |_| {});
        let lines = body_of(&state, 120, 40, NOW_MS);
        assert!(
            lines[0].starts_with(" p Day  Month  Session  5h block     [ ] 30 days"),
            "{}",
            lines[0]
        );
        assert_eq!(
            lines[3..9],
            [
                " DAY         ACCOUNT           INPUT   OUTPUT  CACHE W  CACHE R   TOTAL  RESPONSES   API $".to_owned(),
                "▌2026-09-20  1 a1@x.co          1.2M  ≥310.0K     4.1M    88.0M   93.6M        412  $41.20  ━━━━━━━━".to_owned(),
                "             2 spare          300.0K    80.0K        0    20.1M   20.5M          1   $9.75  ━╸".to_owned(),
                " 2026-09-19  unknown account    5.0K      900        0   100.0K  105.9K          1       —".to_owned(),
                format!(" {}", "─".repeat(99)),
                " TOTAL                          1.5M  ≥390.9K     4.1M   108.2M  114.2M        414  $50.95".to_owned(),
            ]
        );
    }

    #[test]
    fn the_cache_write_splits_into_5m_and_1h_from_160_columns() {
        // T5-16: `CACHE W 5M` and `1H` only from `CACHE_SPLIT_FROM` columns.
        let state = showing(days(), |_| {});
        let head = |w: u16| body_of(&state, w, 40, NOW_MS)[3].clone();
        assert!(
            head(160).contains("  CACHE W 5M    1H  CACHE R  "),
            "{}",
            head(160)
        );
        assert!(head(159).contains("  CACHE W  CACHE R  "), "{}", head(159));
        assert!(!head(159).contains("5M"), "{}", head(159));
    }

    #[test]
    fn thousands_separators_go_on_dollars_and_responses() {
        // T5-11: the TUI groups dollars and responses; token counts stay scaled.
        let mut row = token_row(
            day(1),
            Some(row_account(1)),
            None,
            counts(1_000_000_000, 200_000_000, 500_000_000),
            Some(1_541_640_000_000),
        );
        row.responses = 9_891;
        let state = showing(vec![row], |_| {});
        let lines = body_of(&state, 120, 40, NOW_MS);
        let total = lines.iter().position(|l| l.starts_with(" TOTAL")).unwrap();
        for line in [&lines[4], &lines[total]] {
            for value in ["$1,541.64", "  9,891  ", "1.7B"] {
                assert!(line.contains(value), "{value} in {line:?}");
            }
        }
        assert!(
            reads(&lines[total + 1..], "9,891 responses · cache write"),
            "and the detail's: {lines:?}"
        );
        // And the active block's figures under its row.
        let mut big = block(NOON, true);
        if let PeriodKey::Block {
            active: Some(a), ..
        } = &mut big.period
        {
            a.cost_per_hour = Some(Cost {
                usd_nanos: 1_234_000_000_000,
                partial: false,
            });
        }
        let blocks = showing_after("ppp", vec![big]);
        let lines = body_of(&blocks, 80, 24, NOW_MS);
        assert!(
            under_the_block(&lines).ends_with(" · $1,234.00/h"),
            "{lines:?}"
        );
    }

    #[test]
    fn the_table_draws_its_roles_in_every_tier() {
        // §16, T5-15, T5-17, T5-18: muted heads and counts, text values, a warn `≥`, a faint
        // `—` and rule, the share line in accent, `TOTAL` alone bold, `sel-bg` on the selection.
        let at_80 = [
            letters(&[(
                " DAY         ACCOUNT           INPUT   OUTPUT   TOTAL   API $",
                'm',
            )]),
            letters(&[
                ("▌", 'a'),
                ("2026-09-20", 'B'),
                ("  ", 't'),
                ("1 a1@x.co", 't'),
                ("          ", 't'),
                ("1.2M", 'm'),
                ("  ", 't'),
                ("≥310.0K", 'w'),
                ("   ", 't'),
                ("93.6M", 't'),
                ("  ", 't'),
                ("$41.20", 't'),
            ]),
            letters(&[
                ("             ", 't'),
                ("2 spare", 't'),
                ("          ", 't'),
                ("300.0K", 'm'),
                ("    ", 't'),
                ("80.0K", 'm'),
                ("   ", 't'),
                ("20.5M", 't'),
                ("   ", 't'),
                ("$9.75", 't'),
            ]),
            letters(&[
                (" ", 't'),
                ("2026-09-19", 't'),
                ("  ", 't'),
                ("unknown account", 't'),
                ("    ", 't'),
                ("5.0K", 'm'),
                ("      ", 't'),
                ("900", 'm'),
                ("  ", 't'),
                ("105.9K", 't'),
                ("       ", 't'),
                ("—", 'f'),
            ]),
            letters(&[(" ", 't'), ("─".repeat(60).as_str(), 'f')]),
            letters(&[
                (" ", 't'),
                ("TOTAL", 'B'),
                ("                          ", 't'),
                ("1.5M", 'm'),
                ("  ", 't'),
                ("≥390.9K", 'w'),
                ("  ", 't'),
                ("114.2M", 't'),
                ("  ", 't'),
                ("$50.95", 't'),
            ]),
        ];
        let at_120 = [
            letters(&[(
                " DAY         ACCOUNT           INPUT   OUTPUT  CACHE W  CACHE R   TOTAL  RESPONSES   API $",
                'm',
            )]),
            letters(&[
                ("▌", 'a'),
                ("2026-09-20", 'B'),
                ("  ", 't'),
                ("1 a1@x.co", 't'),
                ("          ", 't'),
                ("1.2M", 'm'),
                ("  ", 't'),
                ("≥310.0K", 'w'),
                ("     ", 't'),
                ("4.1M", 'm'),
                ("    ", 't'),
                ("88.0M", 'm'),
                ("   ", 't'),
                ("93.6M", 't'),
                ("        ", 't'),
                ("412", 'm'),
                ("  ", 't'),
                ("$41.20", 't'),
                ("  ", 't'),
                ("━━━━━━━━", 'a'),
            ]),
            letters(&[
                ("             ", 't'),
                ("2 spare", 't'),
                ("          ", 't'),
                ("300.0K", 'm'),
                ("    ", 't'),
                ("80.0K", 'm'),
                ("        ", 't'),
                ("0", 'm'),
                ("    ", 't'),
                ("20.1M", 'm'),
                ("   ", 't'),
                ("20.5M", 't'),
                ("          ", 't'),
                ("1", 'm'),
                ("   ", 't'),
                ("$9.75", 't'),
                ("  ", 't'),
                ("━╸", 'a'),
            ]),
            letters(&[
                (" ", 't'),
                ("2026-09-19", 't'),
                ("  ", 't'),
                ("unknown account", 't'),
                ("    ", 't'),
                ("5.0K", 'm'),
                ("      ", 't'),
                ("900", 'm'),
                ("        ", 't'),
                ("0", 'm'),
                ("   ", 't'),
                ("100.0K", 'm'),
                ("  ", 't'),
                ("105.9K", 't'),
                ("          ", 't'),
                ("1", 'm'),
                ("       ", 't'),
                ("—", 'f'),
            ]),
            letters(&[(" ", 't'), ("─".repeat(99).as_str(), 'f')]),
            letters(&[
                (" ", 't'),
                ("TOTAL", 'B'),
                ("                          ", 't'),
                ("1.5M", 'm'),
                ("  ", 't'),
                ("≥390.9K", 'w'),
                ("     ", 't'),
                ("4.1M", 'm'),
                ("   ", 't'),
                ("108.2M", 'm'),
                ("  ", 't'),
                ("114.2M", 't'),
                ("        ", 't'),
                ("414", 'm'),
                ("  ", 't'),
                ("$50.95", 't'),
            ]),
        ];
        for theme in TIERS {
            let mut state = showing(days(), |_| {});
            state.theme = theme;
            for ((w, h), want) in [((80, 24), &at_80), ((120, 40), &at_120)] {
                let buf = draw_buffer(&state, w, h, NOW_MS);
                for (n, row) in want.iter().enumerate() {
                    let y = body_y(&state, w, h, 3 + n as u16);
                    assert_eq!(
                        roles(&buf, theme, y).trim_end(),
                        canon(theme, row),
                        "{theme:?} at {w}: body row {}",
                        3 + n
                    );
                    let bg = if n == 1 {
                        theme.sel_bg().bg.unwrap_or(Color::Reset)
                    } else {
                        Color::Reset
                    };
                    for x in 0..w {
                        assert_eq!(buf[(x, y)].bg, bg, "{theme:?} at {w}: ({x}, {y})");
                    }
                }
                let head = body_y(&state, w, h, 3);
                assert!(
                    (0..w).all(|x| !buf[(x, head)].modifier.contains(Modifier::BOLD)),
                    "{theme:?} at {w}: no head is bold"
                );
            }
        }
    }

    #[test]
    fn the_control_bar_names_each_choice_and_the_index() {
        // T5-6, T5-7: the periods, the range, the grouping and the filter, the index on the
        // right; five-space groups at 120, the narrow form at 80.
        let mut state = showing(days(), |_| {});
        assert_eq!(
            body_of(&state, 120, 40, NOW_MS)[0],
            format!(
                " p Day  Month  Session  5h block     [ ] 30 days     g by account     f all accounts · all models{}indexed 14:12",
                " ".repeat(9)
            )
        );
        assert_eq!(
            body_of(&state, 80, 24, NOW_MS)[0],
            format!(
                " p Day   [ ] 30 days   g by account   f all{}indexed 14:12",
                " ".repeat(23)
            )
        );
        let left = letters(&[
            (" ", 't'),
            ("p", 'a'),
            (" ", 't'),
            ("Day", 't'),
            ("  ", 't'),
            ("Month", 'm'),
            ("  ", 't'),
            ("Session", 'm'),
            ("  ", 't'),
            ("5h block", 'm'),
            ("     ", 't'),
            ("[ ]", 'a'),
            (" ", 't'),
            ("30 days", 't'),
            ("     ", 't'),
            ("g", 'a'),
            (" ", 't'),
            ("by account", 't'),
            ("     ", 't'),
            ("f", 'a'),
            (" ", 't'),
            ("all accounts · all models", 't'),
        ]);
        let right = letters(&[("indexed 14:12", 'm')]);
        let want = format!("{left}{}{right} ", " ".repeat(9));
        let text = body_of(&state, 120, 40, NOW_MS)[0].clone();
        let at = |key: &str| text.find(&format!("     {key} ")).unwrap() as u16 + 5;
        let keys = [
            vec![1],
            (at("[ ]")..at("[ ]") + 3).collect(),
            vec![at("g"), at("f")],
        ]
        .concat();
        for theme in TIERS {
            state.theme = theme;
            let buf = draw_buffer(&state, 120, 40, NOW_MS);
            let y = body_y(&state, 120, 40, 0);
            assert_eq!(roles(&buf, theme, y), canon(theme, &want), "{theme:?}");
            // Nothing bold (T5-6), but in tier 0, which draws accent bold (§4.1): the keys.
            let bold: Vec<u16> = (0..120)
                .filter(|&x| buf[(x, y)].modifier.contains(Modifier::BOLD))
                .collect();
            let want_bold = if theme.colourless() {
                keys.clone()
            } else {
                vec![]
            };
            assert_eq!(bold, want_bold, "{theme:?}: only tier 0's keys are bold");
        }
        let row = |keys: &str| body_of(&showing_after(keys, days()), 80, 24, NOW_MS)[0].clone();
        assert!(
            row("ppp").starts_with(" p 5h block   [ ] 30 days   g by account"),
            "{}",
            row("ppp")
        );
        for (keys, words) in [
            ("g", "   g by model   "),
            ("gg", "   g by account and model   "),
            ("ggg", "   g no grouping   "),
            ("]", "   [ ] 90 days   "),
            ("]]", "   [ ] all time   "),
        ] {
            assert!(row(keys).contains(words), "{keys}: {}", row(keys));
        }
    }

    #[test]
    fn the_headline_says_the_report_s_cost_tokens_and_responses() {
        // T5-10, T5-11: the cost bold, the labels muted, the dollars and the responses grouped.
        let headline = " $50.95 at API prices    114.2M tokens    414 responses";
        let mut state = showing(days(), |_| {});
        let lines = body_of(&state, 80, 24, NOW_MS);
        assert_eq!(lines[1], headline);
        assert_eq!(lines[2], "");
        assert!(lines[3].starts_with(" DAY"), "{lines:?}");
        assert!(body_of(&state, 120, 40, NOW_MS)[1].starts_with(headline));
        let want = letters(&[
            (" ", 't'),
            ("$50.95", 'B'),
            (" ", 't'),
            ("at API prices", 'm'),
            ("    ", 't'),
            ("114.2M", 't'),
            (" ", 't'),
            ("tokens", 'm'),
            ("    ", 't'),
            ("414", 't'),
            (" ", 't'),
            ("responses", 'm'),
        ]);
        for theme in TIERS {
            state.theme = theme;
            let buf = draw_buffer(&state, 80, 24, NOW_MS);
            let y = body_y(&state, 80, 24, 1);
            assert_eq!(
                roles(&buf, theme, y).trim_end(),
                canon(theme, &want),
                "{theme:?}"
            );
            let bold: Vec<u16> = (0..80)
                .filter(|x| buf[(*x, y)].modifier.contains(Modifier::BOLD))
                .collect();
            assert_eq!(
                bold,
                (1..7).collect::<Vec<u16>>(),
                "{theme:?}: the cost alone is bold"
            );
        }
        let theme = TIERS[1];

        let mut partial = showing(days(), |r| {
            r.total.cost = Some(Cost {
                usd_nanos: 50_950_000_000,
                partial: true,
            })
        });
        partial.theme = theme;
        assert!(
            body_of(&partial, 80, 24, NOW_MS)[1].starts_with(" ≥$50.95 at API prices    "),
            "{:?}",
            body_of(&partial, 80, 24, NOW_MS)
        );
        let buf = draw_buffer(&partial, 80, 24, NOW_MS);
        let y = body_y(&partial, 80, 24, 1);
        for x in 1..8 {
            assert_eq!(buf[(x, y)].fg, Color::Yellow, "{x}");
            assert!(buf[(x, y)].modifier.contains(Modifier::BOLD), "{x}");
        }
        let line =
            |edit: fn(&mut TokenReport)| body_of(&showing(days(), edit), 80, 24, NOW_MS)[1].clone();
        assert_eq!(
            line(|r| r.total.cost = None),
            " — at API prices    114.2M tokens    414 responses"
        );
        assert_eq!(
            line(|r| {
                r.total.cost = Some(Cost {
                    usd_nanos: 1_541_640_000_000,
                    partial: false,
                });
                r.total.responses = 9_891;
            }),
            " $1,541.64 at API prices    114.2M tokens    9,891 responses"
        );
        assert!(line(|r| r.total.responses = 1).ends_with("    1 response"));
        assert!(line(|r| r.total.counts = counts(1, 0, 0)).contains("    1 token    "));
    }

    /// A session of yesterday, 09:12 to 11:40 UTC, in the tagteam checkout.
    fn session(id: &str, account: RowAccount) -> TokenRow {
        let period = PeriodKey::Session {
            session_id: id.into(),
            project: Some("/Users/alex/Code/tagteam".into()),
            first_ms: YESTERDAY + 9 * HOUR_MS + 12 * 60_000,
            last_ms: YESTERDAY + 11 * HOUR_MS + 40 * 60_000,
        };
        let counts = counts(20_000, 3_000, 900_000);
        token_row(period, Some(account), None, counts, Some(1_230_000_000))
    }

    /// 12:00 to 17:00 UTC today.
    pub(crate) const NOON: i64 = 1_789_992_000_000;

    /// Account 1's block of five hours from `start_ms`: active at `NOW_MS` (14:13:20) and
    /// observed when `active`, else inferred.
    pub(crate) fn block(start_ms: i64, active: bool) -> TokenRow {
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
        let lines = body_of(&sessions, 80, 24, NOW_MS);
        assert_eq!(
            lines[0],
            bar_row(
                " p Session   [ ] 30 days   g by account   f all",
                "indexed 14:12",
                80
            )
        );
        assert_eq!(
            lines[1..14],
            [
                " $2.46 at API prices    1.8M tokens    2 responses".to_owned(),
                String::new(),
                " SESSION            PROJECT  ACCOUNT    INPUT  OUTPUT   TOTAL  API $".to_owned(),
                "▌09-20 09:12–11:40  tagteam  1 a1@x.co  20.0K    3.0K  923.0K  $1.23".to_owned(),
                " 09-20 09:12–11:40  tagteam  2 spare    20.0K    3.0K  923.0K  $1.23".to_owned(),
                format!(" {}", "─".repeat(67)),
                format!(" TOTAL{}40.0K    6.0K    1.8M  $2.46", " ".repeat(34)),
                String::new(),
                " SELECTED  session 3f2a9c1d · 1 a1@x.co   ⏎ its models".to_owned(),
                "           1 response · cache write 0 (5m 0, 1h 0) · cache read 900.0K".to_owned(),
                "           /Users/alex/Code/tagteam · 2026-09-20 09:12 – 2026-09-20 11:40"
                    .to_owned(),
                String::new(),
                " $ what this would cost on the API at list prices of 2026-10-01; not your bill"
                    .to_owned(),
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
        let lines = body_of(&months, 80, 24, NOW_MS);
        assert_eq!(
            lines[0],
            bar_row(
                " p Month   [ ] 12 months   g by account   f all",
                "indexed 14:12",
                80
            )
        );
        assert_eq!(lines[1], " — at API prices    6 tokens    1 response");
        assert_eq!(lines[3], " MONTH    ACCOUNT    INPUT  OUTPUT  TOTAL  API $");
        assert_eq!(lines[4], "▌2026-09  1 a1@x.co      1       2      6      —");
    }

    #[test]
    fn estimated_blocks_lead_with_a_tilde() {
        // T5-21: an inferred block's span is led by `~`, an observed one's by a space, in one
        // column that only a report holding an inferred block has; the detail says `estimated`.
        let rows = || vec![block(NOON, true), block(NOON - 5 * HOUR_MS, false)];
        let mut blocks = showing_after("ppp", rows());
        let lines = body_of(&blocks, 80, 24, NOW_MS);
        assert_eq!(
            lines[1..11],
            [
                " $10.00 at API prices    10.9M tokens    2 responses".to_owned(),
                " ▶ block ━━━━━━━━━╸············ 2h46m left · 41.2K tok/min · $3.20/h".to_owned(),
                String::new(),
                " BLOCK               ACCOUNT     INPUT  OUTPUT  TOTAL   API $".to_owned(),
                "▌ 09-21 12:00–17:00  1 a1@x.co  400.0K   50.0K   5.5M   $5.00".to_owned(),
                "   active · 2h13m elapsed · 2h46m left · 41.2K tokens/min · $3.20/h".to_owned(),
                "   projected 9.8M tokens, —".to_owned(),
                " ~09-21 07:00–12:00  1 a1@x.co  400.0K   50.0K   5.5M   $5.00".to_owned(),
                format!(" {}", "─".repeat(60)),
                format!(" TOTAL{}800.0K  100.0K  10.9M  $10.00", " ".repeat(26)),
            ]
        );
        let table = &lines[4..11];
        assert!(
            table
                .iter()
                .all(|l| !l.contains("observed") && !l.contains("inferred")),
            "{lines:?}"
        );
        // Blocks are per account, so the bar shows the grouping the report applies.
        let grouped = showing_after("pppggg", rows());
        let lines = body_of(&grouped, 80, 24, NOW_MS);
        assert_eq!(
            lines[0],
            bar_row(
                " p 5h block   [ ] 30 days   g by account   f all",
                "indexed 14:12",
                80
            )
        );
        assert_eq!(lines[4..11], *table);
        assert_eq!(grouped.tokens.grouping(), TokenGrouping::None);
        // With only observed blocks there is no leading column.
        let observed = showing_after("ppp", vec![block(NOON, true)]);
        let lines = body_of(&observed, 80, 24, NOW_MS);
        assert_eq!(
            lines[4..6],
            [
                " BLOCK              ACCOUNT     INPUT  OUTPUT  TOTAL  API $",
                "▌09-21 12:00–17:00  1 a1@x.co  400.0K   50.0K   5.5M  $5.00",
            ]
        );
        // Selected, the inferred block's detail says it is estimated.
        update(&mut blocks, ch('j'), NOW_MS);
        let lines = body_of(&blocks, 80, 24, NOW_MS);
        let total = lines.iter().position(|l| l.starts_with(" TOTAL")).unwrap();
        let below = &lines[total + 1..];
        assert!(reads(below, "estimated"), "{below:?}");
        assert!(!below.iter().any(|l| l.contains("inferred")), "{below:?}");
        // And the breadcrumbs lead it with `~`.
        let down = update(&mut blocks, key(KeyCode::Enter), NOW_MS);
        answer_with(
            &mut blocks,
            &down,
            vec![session("3f2a9c1d", row_account(1))],
        );
        assert_eq!(
            body_of(&blocks, 80, 24, NOW_MS)[1],
            " › ~09-21 07:00–12:00 · 1 a1@x.co"
        );
    }

    #[test]
    fn removed_and_unattributed_rows_read_removed_account_and_unknown_account() {
        // T5-19: a cell says `removed account` and `unknown account`; the detail and the filter
        // dialog name a removed account by its ID's first 8 characters.
        let lines = body_of(&showing(days(), |_| {}), 80, 24, NOW_MS);
        assert_eq!(
            lines[3],
            " DAY         ACCOUNT           INPUT   OUTPUT   TOTAL   API $"
        );
        assert_eq!(
            lines[6],
            " 2026-09-19  unknown account    5.0K      900  105.9K       —"
        );
        let gone = RowAccount::Removed {
            id: AccountId::from_string("3f2a9c1d-7e44-4b1a-9c55-0d1e2f3a4b5c"),
        };
        let row = token_row(day(1), Some(gone.clone()), None, counts(1, 2, 3), None);
        let state = showing(vec![row], |_| {});
        let lines = body_of(&state, 80, 24, NOW_MS);
        assert!(
            lines[4].starts_with("▌2026-09-20  removed account  "),
            "{lines:?}"
        );
        let total = lines.iter().position(|l| l.starts_with(" TOTAL")).unwrap();
        let below = &lines[total + 1..];
        assert!(reads(below, "removed account 3f2a9c1d"), "{below:?}");
        assert!(!lines.iter().any(|l| l.contains("7e44")), "{lines:?}");
        assert_eq!(account_name(&gone, false), "removed account 3f2a9c1d");
        assert_eq!(
            account_name(&RowAccount::Unattributed, false),
            "unknown account"
        );
    }

    #[test]
    fn an_unattributed_day_wholly_before_the_epoch_reads_before_tracking() {
        let state = showing(days(), |r| r.tracking_since_ms = Some(YESTERDAY));
        let lines = body_of(&state, 80, 24, NOW_MS);
        assert!(
            lines.contains(
                &" 2026-09-19  before tracking    5.0K      900  105.9K       —".to_owned()
            ),
            "{lines:?}"
        );
        // The attributed rows keep their names.
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("▌2026-09-20  1 a1@x.co")),
            "{lines:?}"
        );
    }

    #[test]
    fn a_day_holding_the_epoch_or_no_epoch_reads_unknown_account() {
        for epoch in [Some(YESTERDAY - 1), None] {
            let state = showing(days(), |r| r.tracking_since_ms = epoch);
            let lines = body_of(&state, 80, 24, NOW_MS);
            assert!(
                lines.contains(
                    &" 2026-09-19  unknown account    5.0K      900  105.9K       —".to_owned()
                ),
                "{epoch:?}: {lines:?}"
            );
        }
    }

    #[test]
    fn every_period_kind_follows_its_own_span() {
        // Three unattributed rows per kind: wholly before the epoch E = YESTERDAY, holding it,
        // wholly after it.
        let e = YESTERDAY;
        let unattributed = |period: PeriodKey| {
            token_row(
                period,
                Some(RowAccount::Unattributed),
                None,
                counts(1, 2, 3),
                None,
            )
        };
        let month = |month| PeriodKey::Month { year: 2026, month };
        let session = |first_ms, last_ms| PeriodKey::Session {
            session_id: format!("s{first_ms}"),
            project: None,
            first_ms,
            last_ms,
        };
        let block = |start_ms, end_ms| PeriodKey::Block {
            start_ms,
            end_ms,
            observed: true,
            active: None,
        };
        let kinds: [(&str, [PeriodKey; 3]); 4] = [
            ("pppp", [day(2), day(1), day(0)]),
            ("p", [month(8), month(9), month(10)]),
            (
                "pp",
                [
                    session(e - HOUR_MS, e - 1),
                    session(e - HOUR_MS, e),
                    session(e + 1, e + HOUR_MS),
                ],
            ),
            (
                "ppp",
                [
                    block(e - 5 * HOUR_MS, e),
                    block(e - 5 * HOUR_MS + 1, e + 1),
                    block(e + 1, e + 5 * HOUR_MS + 1),
                ],
            ),
        ];
        for (keys, periods) in kinds {
            let words_of = |epoch| {
                let rows = periods.iter().cloned().map(unattributed).collect();
                let state = showing_after_with(keys, rows, |r| r.tracking_since_ms = epoch);
                body_of(&state, 80, 24, NOW_MS)[4..7]
                    .iter()
                    .map(|l| {
                        [BEFORE_TRACKING, "unknown account"]
                            .into_iter()
                            .find(|w| l.contains(w))
                            .unwrap_or_else(|| panic!("{keys}: {l}"))
                    })
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                words_of(Some(e)),
                [BEFORE_TRACKING, "unknown account", "unknown account"],
                "{keys}"
            );
            assert_eq!(words_of(None), ["unknown account"; 3], "{keys}");
        }
    }

    #[test]
    fn an_attributed_row_before_the_epoch_keeps_its_name() {
        let state = showing(days(), |r| r.tracking_since_ms = Some(YESTERDAY + DAY_MS));
        let lines = body_of(&state, 80, 24, NOW_MS);
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("▌2026-09-20  1 a1@x.co")),
            "{lines:?}"
        );
        // Only the unattributed row changes words.
        assert_eq!(
            lines.iter().filter(|l| l.contains(BEFORE_TRACKING)).count(),
            1,
            "{lines:?}"
        );
    }

    #[test]
    fn an_ungrouped_row_names_before_tracking_in_its_account_list() {
        for (accounts, listed) in [
            (vec![RowAccount::Unattributed], BEFORE_TRACKING),
            (
                vec![row_account(1), RowAccount::Unattributed],
                "1 a1@x.co, before tracking",
            ),
        ] {
            let mut row = token_row(day(2), None, None, counts(1, 2, 3), None);
            row.accounts = accounts;
            let state = showing_after_with("ggg", vec![row], |r| {
                r.tracking_since_ms = Some(YESTERDAY);
            });
            let lines = body_of(&state, 80, 24, NOW_MS);
            assert!(
                lines
                    .iter()
                    .any(|l| l.starts_with("▌2026-09-19  ") && l.contains(listed)),
                "{listed}: {lines:?}"
            );
        }
    }

    #[test]
    fn the_detail_and_a_breadcrumb_use_their_row_s_words() {
        let mut state = showing(days(), |r| r.tracking_since_ms = Some(YESTERDAY));
        // Select the 2026-09-19 row (End).
        update(&mut state, key(KeyCode::End), NOW_MS);
        let lines = body_of(&state, 80, 24, NOW_MS);
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with(" SELECTED  2026-09-19 · before tracking")),
            "{lines:?}"
        );
        // ⏎ into its sessions: the breadcrumb row names `before tracking`.
        let down = update(&mut state, key(KeyCode::Enter), NOW_MS);
        answer_with(&mut state, &down, vec![session("3f2a9c1d", row_account(1))]);
        assert_eq!(
            body_of(&state, 80, 24, NOW_MS)[1],
            " › 2026-09-19 · before tracking"
        );
    }

    #[test]
    fn ids_show_their_first_8_characters_in_the_crumbs_and_the_detail() {
        // T5-19: a session is `session {first 8}` in the breadcrumbs and the detail.
        let mut state = showing_after("pp", vec![realistic()]);
        let lines = draw(&state, 80, 24);
        assert!(reads(&lines, "session 3f2a9c1d"), "{lines:?}");
        assert!(!lines.iter().any(|l| l.contains("7e44")), "{lines:?}");
        let models = ["claude-opus-5-5", "claude-sonnet-5-5"].map(|m| {
            token_row(
                realistic().period,
                Some(row_account(1)),
                Some(m),
                counts(1, 2, 3),
                None,
            )
        });
        let down = update(&mut state, key(KeyCode::Enter), NOW_MS);
        answer_with(&mut state, &down, models.to_vec());
        let lines = body_of(&state, 80, 24, NOW_MS);
        assert_eq!(lines[1], " › session 3f2a9c1d · 1 a1@x.co");
        assert!(!lines.iter().any(|l| l.contains("7e44")), "{lines:?}");
        // Characters, not bytes.
        assert_eq!(short_id("山田太郎のアカウント"), "山田太郎のアカウ");
        assert_eq!(short_id("88aa"), "88aa");
    }

    #[test]
    fn a_session_s_project_is_its_basename_and_keeps_it_before_the_period_shortens() {
        // T5-20: the project is its basename, cut from its end; it keeps `PROJECT_KEEP` columns
        // before the span compacts, and so does the account its name; the project takes what is
        // left.
        let state = showing_after("pp", vec![realistic()]);
        let lines = body_of(&state, 80, 24, NOW_MS);
        assert!(
            lines[4].starts_with("▌09-20 09:12  portal-feature-user-manu…  1 a1@x.co  "),
            "{lines:?}"
        );
        let total = lines.iter().position(|l| l.starts_with(" TOTAL")).unwrap();
        let below = &lines[total + 1..];
        assert!(
            reads(
                below,
                "/Users/alex/Code/ExampleCorp/portal-feature-user-manuals"
            ),
            "the detail keeps the full path: {below:?}"
        );
        // Where no span fits with it, the project still gives way, as before.
        let lines = draw(&widest(), 80, 24);
        let head = lines
            .iter()
            .position(|l| l.starts_with(" SESSION"))
            .unwrap();
        assert!(!lines[head].contains("PROJ"), "{:?}", lines[head]);
        assert!(lines[head + 1].starts_with("▌09-20 09:12  "), "{lines:?}");
        assert_eq!(project_name("/Users/alex/Code/tagteam"), "tagteam");
        assert_eq!(project_name("/w/"), "w");
        assert_eq!(project_name(r"C:\Users\alex\Code\tagteam"), "tagteam");
        assert_eq!(project_name("tagteam"), "tagteam");
        assert_eq!(project_name("/"), "/");
    }

    #[test]
    fn the_span_compacts_before_the_account_s_name_is_cut() {
        // T5-20 and the final review's ruling: the account keeps its whole name while a more
        // compact span makes room for it; the project still keeps `PROJECT_KEEP` first.
        let late = PeriodKey::Session {
            session_id: "3f2a9c1d-7e44-4b1a-9c55-0d1e2f3a4b5c".into(),
            project: Some("/Users/alex/Code/tagteam".into()),
            first_ms: YESTERDAY - 2 * DAY_MS + 21 * HOUR_MS + 7 * 60_000,
            last_ms: YESTERDAY - DAY_MS + 60_000,
        };
        let sam = RowAccount::Account {
            id: id(2),
            position: 2,
            label: "sam@corp.example".into(),
            alias: None,
        };
        let rows = vec![
            token_row(
                late.clone(),
                Some(me()),
                None,
                counts(1_200_000, 310_000, 88_000_000),
                Some(41_200_000_000),
            ),
            token_row(late, Some(sam), None, counts(1, 2, 3), Some(10_000_000)),
        ];
        let state = showing_after("pp", rows);
        let lines = body_of(&state, 120, 40, NOW_MS);
        assert_eq!(
            lines[3..6],
            [
                " SESSION               PROJECT  ACCOUNT             INPUT  OUTPUT  CACHE W  CACHE R  TOTAL  RESPONSES   API $",
                "▌09-18 21:07–00:01+1d  tagteam  1 alex@example.com   1.2M  310.0K        0    88.0M  89.5M          1  $41.20  ━━━━━━━━",
                "                                2 sam@corp.example      1       2        0        3      6          1   $0.01  ╸",
            ]
        );
    }

    #[test]
    fn the_active_block_counts_down_between_queries_and_is_no_longer_active_at_its_end() {
        let blocks = showing_after("ppp", vec![block(NOON, true)]);
        let figures = |at: i64| under_the_block(&body_of(&blocks, 80, 24, at));
        assert!(figures(NOW_MS).starts_with("   active · 2h13m elapsed · 2h46m left"));
        assert!(
            figures(NOW_MS + 61_000).starts_with("   active · 2h14m elapsed · 2h45m left"),
            "from its start and end, with no query between"
        );
        // Its figures are muted (§4.1's secondary figures), whatever the tier.
        let lines = body_of(&blocks, 80, 24, NOW_MS);
        let n = lines
            .iter()
            .position(|l| l.starts_with("   active"))
            .unwrap();
        for theme in TIERS {
            let mut state = showing_after("ppp", vec![block(NOON, true)]);
            state.theme = theme;
            let buf = draw_buffer(&state, 80, 24, NOW_MS);
            let y = body_y(&state, 80, 24, n as u16);
            let want = letters(&[(lines[n].as_str(), 'm')]);
            assert_eq!(
                roles(&buf, theme, y).trim_end(),
                canon(theme, &want),
                "{theme:?}"
            );
        }
        let end = NOON + 5 * HOUR_MS;
        assert!(figures(end - 60_000).starts_with("   active · 4h59m elapsed · 1m left"));
        let lines = body_of(&blocks, 80, 24, end);
        let at = lines.iter().position(|l| l.contains("400.0K")).unwrap();
        assert_eq!(
            lines[at + 1..at + 3],
            [
                format!(" {}", "─".repeat(58)),
                format!(" TOTAL{}400.0K   50.0K   5.5M  $5.00", " ".repeat(25)),
            ],
            "no longer active, until the query its end asks for returns"
        );
    }

    /// The body row under `block()`'s row: its figures while it is active, else the rule above
    /// the total.
    fn under_the_block(lines: &[String]) -> String {
        let at = lines.iter().position(|l| l.contains("400.0K"));
        let at = at.unwrap_or_else(|| panic!("no block row: {lines:?}"));
        lines[at + 1].clone()
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
        let lines = body_of(&models, 80, 24, NOW_MS);
        assert_eq!(
            lines[0],
            bar_row(
                " p Day   [ ] 30 days   g by model   f all",
                "indexed 14:12",
                80
            )
        );
        assert_eq!(
            lines[3],
            " DAY         ACCOUNT             MODEL     INPUT  OUTPUT  TOTAL  API $"
        );
        assert_eq!(
            lines[4],
            "▌2026-09-20  1 a1@x.co, 2 spare  opus-5-5      1       2      6      —"
        );
        opus.model = None;
        let none = showing_after("ggg", vec![opus]);
        let lines = body_of(&none, 80, 24, NOW_MS);
        assert_eq!(
            lines[0],
            bar_row(
                " p Day   [ ] 30 days   g no grouping   f all",
                "indexed 14:12",
                80
            )
        );
        assert_eq!(
            lines[3],
            " DAY         ACCOUNT             INPUT  OUTPUT  TOTAL  API $"
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
            state.theme = Theme::new(color);
            let buf = draw_buffer(&state, 80, 24, NOW_MS);
            let lines = body_of(&state, 80, 24, NOW_MS);
            assert_eq!(
                lines[5],
                "             2 spare          300.0K    80.0K   20.5M   ≥$9.75"
            );
            assert!(lines[6].ends_with("       —"), "{}", lines[6]);
            for needle in ["≥310.0K", "≥$9.75"] {
                let (x, y) = locate(&buf, needle).unwrap();
                let cell = &buf[(x, y)];
                if color {
                    assert_eq!(cell.fg, Color::Yellow, "{needle}");
                } else {
                    assert_eq!(cell.fg, Color::Reset, "{needle}");
                    assert!(cell.modifier.contains(Modifier::BOLD), "{needle}");
                    assert!(
                        !cell.modifier.contains(Modifier::UNDERLINED),
                        "warn, not crit: {needle}"
                    );
                }
            }
            let (x, y) = locate(&buf, "$41.20").unwrap();
            assert_eq!(buf[(x, y)].fg, Color::Reset, "a whole cost is plain");
        }
    }

    #[test]
    fn the_footnotes_say_what_the_report_leaves_out() {
        // Spec §5.3, token report §11.2: with the TUI's `≥` and the unreadable files; after
        // the detail and a blank row, each note `NOTE_INDENT` in (T5-24), J reaching the last.
        let mut state = showing(days(), |r| {
            r.unpriced_models = vec!["claude-test-9".into()];
            r.index.skipped_lines = 12;
            r.index.unreadable_files = 3;
            r.price_overrides = vec!["claude-opus-5-5".into()];
        });
        let seen = scrolled_below(&mut state);
        assert_eq!(
            seen[seen.len() - 7..],
            [
                "",
                "   No price for claude-test-9: counted without cost (≥ marks a partial cost).",
                " ≥ at least: 2 subagent responses are still being counted",
                "   12 transcript lines could not be read, so these totals may be low.",
                "   3 transcript files could not be read; the next refresh tries again.",
                " $ what this would cost on the API at list prices of 2026-10-01, with",
                "   config.toml's for claude-opus-5-5; not your bill",
            ],
            "{seen:?}"
        );
    }

    #[test]
    fn the_bar_s_right_part_says_how_current_the_index_is() {
        // T5-8, spec §5.3 "Indexing": the right end of the bar at 80.
        let bar = |state: &State| body_of(state, 80, 24, NOW_MS)[0].clone();
        let mut state = showing(days(), |_| {});
        assert_eq!(state.tokens.watch().refreshing(), None);
        assert!(crate::update::drawn(&mut state).contains(&Cmd::RefreshIndex { provider: cc() }));
        assert_eq!(bar(&state), bar_row(DAY_BAR, "indexing…", 80));
        let progress = IndexProgress {
            bytes_done: 1_200_000_000,
            bytes_total: 3_300_000_000,
            ..IndexProgress::default()
        };
        update(&mut state, Msg::IndexProgress(progress), NOW_MS);
        assert_eq!(bar(&state), bar_row(DAY_BAR, "indexing 1.2 / 3.3 GB", 80));
        let progress = IndexProgress {
            bytes_done: 900_000_000,
            bytes_total: 3_300_000_000,
            ..IndexProgress::default()
        };
        update(&mut state, Msg::IndexProgress(progress), NOW_MS);
        assert_eq!(
            bar(&state),
            bar_row(DAY_BAR, "indexing 900.0 MB / 3.3 GB", 80)
        );
        update(
            &mut state,
            Msg::Refreshed {
                result: Ok(RefreshOutcome::Busy),
            },
            NOW_MS,
        );
        assert_eq!(bar(&state), bar_row(DAY_BAR, "catching up elsewhere", 80));
        // A report of an index that has never completed a refresh (token report §14).
        let partial = IndexState {
            refreshed_at_ms: None,
            ..IndexState::default()
        };
        let mut state = showing(days(), |r| r.index = partial);
        assert_eq!(bar(&state), bar_row(DAY_BAR, "catching up", 80));
        crate::screens::tokens::requery(&mut state, NOW_MS);
        assert_eq!(bar(&state), bar_row(DAY_BAR, "catching up · updating…", 80));
        // Refreshed on another day: §5.3's time form.
        let state = showing(days(), |r| {
            r.index.refreshed_at_ms = Some(YESTERDAY + HOUR_MS)
        });
        assert_eq!(bar(&state), bar_row(DAY_BAR, "indexed Sun 01:00", 80));
    }

    #[test]
    fn a_cold_catch_up_shows_its_progress_and_never_says_catching_up() {
        // T5-6, T5-7: the zone is not shown; `updating…` gives way before the periods.
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
        assert_eq!(
            body_of(&state, 80, 24, NOW_MS)[0],
            bar_row(DAY_BAR, "catching up", 80)
        );
        crate::update::drawn(&mut state);
        let progress = IndexProgress {
            bytes_done: 12_300_000_000,
            bytes_total: 33_300_000_000,
            ..IndexProgress::default()
        };
        update(&mut state, Msg::IndexProgress(progress), NOW_MS);
        crate::screens::tokens::requery(&mut state, NOW_MS);
        let at = |w: u16| body_of(&state, w, 40, NOW_MS)[0].clone();
        assert_eq!(at(80), bar_row(DAY_BAR, "indexing 12.3 / 33.3 GB", 80));
        assert_eq!(
            at(120),
            bar_row(
                " p Day  Month  Session  5h block   [ ] 30 days   g by account   f all accounts · all models",
                "indexing 12.3 / 33.3 GB",
                120
            )
        );
        assert_eq!(
            at(160),
            bar_row(
                " p Day  Month  Session  5h block     [ ] 30 days     g by account     f all accounts · all models",
                "indexing 12.3 / 33.3 GB · updating…",
                160
            )
        );
        for w in [80, 120, 160] {
            assert!(!at(w).contains("catching up"), "{}", at(w));
            assert!(!at(w).contains("Brussels"), "{}", at(w));
        }
    }

    #[test]
    fn loading_and_an_empty_report_say_so() {
        let mut state = fresh(Screen::Tokens);
        state.start();
        let lines = body_of(&state, 80, 24, NOW_MS);
        assert!(lines[0].starts_with(DAY_BAR), "{lines:?}");
        assert_eq!(lines[1], " loading…");
        let empty = showing(vec![], |_| {});
        let lines = body_of(&empty, 80, 24, NOW_MS);
        assert_eq!(lines[0], bar_row(DAY_BAR, "indexed 14:12", 80));
        assert_eq!(lines[1], " No token usage in this range.");
        assert_eq!(lines[2], "");
    }

    #[test]
    fn an_empty_report_of_an_index_not_built_yet_says_so() {
        let unbuilt = IndexState {
            refreshed_at_ms: None,
            ..IndexState::default()
        };
        let state = showing(vec![], |r| r.index = unbuilt);
        let lines = body_of(&state, 80, 24, NOW_MS);
        assert_eq!(lines[0], bar_row(DAY_BAR, "catching up", 80));
        let title = lines.iter().position(|l| l.trim() == "No token usage yet");
        let sentence = "tagteam reads Claude Code's transcripts; they appear here as you use it.";
        assert_eq!(
            title.map(|n| lines[n + 1].trim()),
            Some(sentence),
            "{lines:?}"
        );
        assert!(
            !lines.iter().any(|l| l.contains("indexed yet")),
            "{lines:?}"
        );
        let built = showing(vec![], |_| {});
        assert_eq!(
            body_of(&built, 80, 24, NOW_MS)[1],
            " No token usage in this range."
        );
    }

    #[test]
    fn an_empty_report_still_shows_the_warnings_that_explain_it() {
        let state = showing(vec![], |r| {
            r.index.skipped_lines = 12;
            r.index.unreadable_files = 3;
        });
        let lines = body_of(&state, 80, 24, NOW_MS);
        assert_eq!(lines[1], " No token usage in this range.");
        assert_eq!(
            lines[2..4],
            [
                " 12 transcript lines could not be read, so these totals may be low.",
                " 3 transcript files could not be read; the next refresh tries again.",
            ]
        );
        assert_eq!(lines[4], "");
    }

    #[test]
    fn an_empty_report_without_warnings_shows_only_the_empty_message() {
        let state = showing(vec![], |_| {});
        let lines = body_of(&state, 80, 24, NOW_MS);
        assert_eq!(lines[1], " No token usage in this range.");
        assert!(lines[2..].iter().all(|l| l.is_empty()), "{lines:?}");
    }

    #[test]
    fn a_drilled_level_names_its_way_down_on_a_row_under_the_bar() {
        // T5-9: the crumbs take a row of their own under the bar, muted, and the bar keeps its
        // place and its words.
        let mut state = showing(days(), |_| {});
        let bar = body_of(&state, 80, 24, NOW_MS)[0].clone();
        let down = update(&mut state, key(KeyCode::Enter), NOW_MS);
        answer_with(&mut state, &down, vec![session("3f2a9c1d", row_account(1))]);
        let lines = body_of(&state, 80, 24, NOW_MS);
        assert_eq!(lines[0], bar);
        assert!(
            lines[0].starts_with(" p Day   "),
            "still `p Day`: {lines:?}"
        );
        assert_eq!(
            lines[1..4],
            [
                " › 2026-09-20 · 1 a1@x.co",
                " $1.23 at API prices    923.0K tokens    1 response",
                "",
            ]
        );
        assert!(lines[4].starts_with(" SESSION"), "{lines:?}");
        let theme = Theme::new(true);
        state.theme = theme;
        let y = body_y(&state, 80, 24, 1);
        let buf = draw_buffer(&state, 80, 24, NOW_MS);
        let want = letters(&[(" ", 't'), ("› 2026-09-20 · 1 a1@x.co", 'm')]);
        assert_eq!(roles(&buf, theme, y).trim_end(), canon(theme, &want));
        let down = update(&mut state, key(KeyCode::Enter), NOW_MS);
        answer_with(&mut state, &down, vec![session("3f2a9c1d", row_account(1))]);
        assert_eq!(
            body_of(&state, 80, 24, NOW_MS)[1],
            " › 2026-09-20 · 1 a1@x.co › session 3f2a9c1d · 1 a1@x.co"
        );
        // A crumb as wide as the row less one is whole, its model short (T5-22); a longer one is
        // cut there; the bar is untouched.
        let drilled = |mut state: State| {
            let bar = body_of(&state, 80, 24, NOW_MS)[0].clone();
            let rows = state.tokens.report().map(|r| r.rows[..1].to_vec());
            let down = update(&mut state, key(KeyCode::Enter), NOW_MS);
            answer_with(&mut state, &down, rows.unwrap_or_default());
            let lines = body_of(&state, 80, 24, NOW_MS);
            assert_eq!(lines[0], bar);
            assert!(
                lines[4].starts_with(" ACCOUNT"),
                "one session's models share the period and the project, which the rows leave out: {lines:?}"
            );
            lines
        };
        let lines = drilled(widest());
        assert_eq!(
            lines[1],
            " › session 3f2a9c1d · 3 山田太郎のアカウント · opus-5-5-with-a-very-long-suffix"
        );
        assert_eq!(width_of(&lines[1]), 79);
        let mut rows = widest().tokens.report().unwrap().rows.clone();
        let longer = "claude-opus-5-5-with-a-very-long-suffix-and-more";
        rows[0].model = Some(longer.into());
        rows[0].models = vec![longer.into()];
        let lines = drilled(showing_after("ppgg]]", rows));
        assert!(
            lines[1].starts_with(" › session 3f2a9c1d · 3 山田太郎のアカウント · opus-5-5"),
            "{lines:?}"
        );
        assert!(lines[1].ends_with('…'), "{lines:?}");
        assert!(width_of(&lines[1]) <= 79, "{lines:?}");
    }

    #[test]
    fn the_bar_names_the_top_level_s_grouping_at_every_depth() {
        // `g` changes the top level's grouping; a drilled level's own is not what it sets.
        let by_model = bar_row(
            " p Day   [ ] 30 days   g by model   f all",
            "indexed 14:12",
            80,
        );
        let mut state = showing_after("g", days());
        assert_eq!(body_of(&state, 80, 24, NOW_MS)[0], by_model);
        for rows in [
            vec![session("3f2a9c1d", row_account(1))],
            vec![session("3f2a9c1d", row_account(1))],
        ] {
            let down = update(&mut state, key(KeyCode::Enter), NOW_MS);
            answer_with(&mut state, &down, rows);
        }
        assert_eq!(state.tokens.depth(), 2);
        assert_eq!(
            state.tokens.report().map(|r| r.query.grouping),
            Some(TokenGrouping::AccountModel)
        );
        assert_eq!(body_of(&state, 80, 24, NOW_MS)[0], by_model);
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

    /// `MODELS` as the table names them (T5-22).
    const SHORT_MODELS: [&str; 3] = ["opus-4-5", "sonnet-4-5", "haiku-4-5"];

    fn me() -> RowAccount {
        RowAccount::Account {
            id: id(1),
            position: 1,
            label: "alex@example.com".into(),
            alias: None,
        }
    }

    fn work_session() -> PeriodKey {
        PeriodKey::Session {
            session_id: "3f2a9c1d-7e44-4b1a-9c55-0d1e2f3a4b5c".into(),
            project: Some("/Users/alex/Code/tagteam".into()),
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
        let lines = body_of(&state, 80, 24, NOW_MS);
        assert!(
            lines[4].starts_with(" ACCOUNT"),
            "the period and the project are the same on every row: {lines:?}"
        );
        for model in SHORT_MODELS {
            assert!(
                lines.iter().any(|l| l.contains(&format!("  {model}  "))),
                "{model} whole: {lines:?}"
            );
        }
        assert!(lines[8].starts_with(" ─"), "{lines:?}");
        assert!(lines[9].starts_with(" TOTAL"), "{lines:?}");
        assert!(lines[9].contains("1.2M"), "{lines:?}");
    }

    #[test]
    fn the_sessions_by_account_and_model_keep_their_models_apart_at_80_columns() {
        // The short model IDs (T5-22) leave the project its room; the span compacts before the
        // account is cut, and the account gives way before the model is cut.
        let mut state = fresh(Screen::Tokens);
        state.start();
        let mut last = vec![];
        for c in "ppgg".chars() {
            last = update(&mut state, ch(c), NOW_MS);
        }
        answer_with(&mut state, &last, work_models());
        let lines = body_of(&state, 80, 24, NOW_MS);
        assert_eq!(
            lines[3..7],
            [
                " SESSION      PROJECT  ACCOUNT         MODEL        INPUT  OUTPUT  TOTAL   API $",
                "▌09-20 09:12  tagteam  1 alex@exampl…  opus-4-5    400.0K  100.0K  30.5M  $13.00",
                "                       1 alex@exampl…  sonnet-4-5  400.0K  100.0K  30.5M  $13.00",
                "                       1 alex@exampl…  haiku-4-5   400.0K  100.0K  30.5M  $13.00",
            ]
        );
    }

    #[test]
    fn model_ids_are_short_in_the_table_and_full_in_the_detail() {
        // T5-22: short in the Model column and the breadcrumbs; full in the detail and the bar.
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
        let lines = body_of(&state, 80, 24, NOW_MS);
        let total = lines.iter().position(|l| l.starts_with(" TOTAL")).unwrap();
        for model in SHORT_MODELS {
            assert!(
                lines[..total]
                    .iter()
                    .any(|l| l.contains(&format!("  {model}  "))),
                "{model} short: {lines:?}"
            );
        }
        assert!(
            !lines[..total].iter().any(|l| l.contains("claude-")),
            "{lines:?}"
        );
        assert!(
            reads(&lines[total + 1..], "claude-opus-4-5-20251101"),
            "the selected row's detail: {lines:?}"
        );
        // At 80 the bar names a model it must cut by its short form (T5-7's form 6); at 120 it
        // has room for it whole.
        state.tokens.models = Some(vec![MODELS[0].into()]);
        assert_eq!(
            body_of(&state, 80, 24, NOW_MS)[0],
            bar_row(
                " p Day   [ ] 30 days   g by account   f all accounts · opus-4-5",
                "indexed 14:12",
                80
            )
        );
        let bar = &body_of(&state, 120, 40, NOW_MS)[0];
        assert!(
            bar.contains("f all accounts · claude-opus-4-5-20251101"),
            "{bar:?}"
        );
        // A by-model day row drilled into names its model short.
        let opus = token_row(day(1), None, Some("claude-opus-5-5"), counts, None);
        let mut state = showing_after("g", vec![opus]);
        let down = update(&mut state, key(KeyCode::Enter), NOW_MS);
        answer_with(&mut state, &down, vec![session("3f2a9c1d", row_account(1))]);
        assert_eq!(
            body_of(&state, 80, 24, NOW_MS)[1],
            " › 2026-09-20 · opus-5-5"
        );
    }

    #[test]
    fn help_says_from_which_width_the_cache_write_splits() {
        // T5-25: help's words for `CACHE W 5M` and `1H` name `CACHE_SPLIT_FROM`, so the two
        // cannot drift apart.
        let mut state = showing(days(), |_| {});
        update(&mut state, ch('?'), NOW_MS);
        let lines = draw(&state, 80, 60);
        let entry = lines
            .iter()
            .find(|l| l.contains("5M 1H  CACHE W 5M and 1H"))
            .unwrap_or_else(|| panic!("{lines:?}"));
        assert!(
            entry.contains(&format!(", from {CACHE_SPLIT_FROM} columns: ")),
            "{entry}"
        );
    }

    #[test]
    fn the_cache_write_is_one_column_where_its_halves_do_not_fit() {
        // At 160 a 70-column alias leaves no room for the halves; it stays whole.
        let alias = format!("alias-{}", "x".repeat(64));
        let mut rows = days();
        if let Some(RowAccount::Account { alias: a, .. }) = &mut rows[1].account {
            *a = Some(alias.clone());
        }
        let state = showing(rows, |_| {});
        let lines = body_of(&state, 160, 40, NOW_MS);
        assert!(lines[3].contains("  CACHE W  CACHE R  "), "{}", lines[3]);
        assert!(!lines[3].contains("5M"), "{}", lines[3]);
        assert!(lines[5].contains(&format!("2 {alias}  ")), "{}", lines[5]);
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
    fn a_failed_report_keeps_the_last_one_dim_under_the_banner() {
        // T2-22, redesign §5.7: the last report stays, DIM, under `! couldn't read the token
        // index (<error>) · r retries`; the next good report clears both.
        // Every cell the row's text takes, from its first column to its last.
        let cells = |buf: &ratatui::buffer::Buffer, needle: &str| {
            let (_, y) = locate(buf, needle).unwrap();
            let used = text_of(buf)[usize::from(y)].trim_end().chars().count();
            (0..used)
                .map(|x| buf[(x as u16, y)].modifier.contains(Modifier::DIM))
                .collect::<Vec<_>>()
        };
        let dim =
            |buf: &ratatui::buffer::Buffer, needle: &str| cells(buf, needle).iter().all(|d| *d);
        let plain =
            |buf: &ratatui::buffer::Buffer, needle: &str| cells(buf, needle).iter().all(|d| !d);
        // The needle's own cells, not DIM: the bar's, whose muted words are DIM by their role.
        let undimmed = |buf: &ratatui::buffer::Buffer, needle: &str| {
            let (x, y) = locate(buf, needle).unwrap();
            (x..x + width_of(needle) as u16).all(|x| !buf[(x, y)].modifier.contains(Modifier::DIM))
        };
        // `needle`'s cells on the row `lead` is on, not DIM.
        let undimmed_on = |buf: &ratatui::buffer::Buffer, lead: &str, needle: &str| {
            let (_, y) = locate(buf, lead).unwrap();
            let row = &text_of(buf)[usize::from(y)];
            let at = row
                .find(needle)
                .unwrap_or_else(|| panic!("{needle} in {row:?}"));
            let x = width_of(&row[..at]) as u16;
            (x..x + width_of(needle) as u16).all(|x| !buf[(x, y)].modifier.contains(Modifier::DIM))
        };
        // A good report: plain on every cell where muted and faint are hues (tier 2); where they
        // are DIM by their role (tiers 0 and 1, T5-15's muted heads and counts, T5-23's muted
        // detail and T5-24's muted notes), on its text: the table's, and below the total the
        // selected row's identity on its own row.
        let good = |buf: &ratatui::buffer::Buffer, theme: Theme, needles: &[&str]| {
            if theme.role(Role::Muted).add_modifier.contains(Modifier::DIM) {
                let rows = text_of(buf);
                let total = rows.iter().position(|l| l.starts_with(" TOTAL")).unwrap() as u16;
                assert!(
                    (1..6).all(|x| !buf[(x, total)].modifier.contains(Modifier::DIM)),
                    "{theme:?} TOTAL: a good report is plain"
                );
                let by_role = [
                    "ACCOUNT",
                    "TOTAL",
                    SELECTED,
                    "what this would cost",
                    "cache write",
                ];
                for needle in needles.iter().filter(|n| !by_role.contains(n)) {
                    assert!(
                        undimmed(buf, needle),
                        "{theme:?} {needle}: a good report is plain"
                    );
                }
                for needle in ["2026-09-21", "1 a1@x.co"] {
                    assert!(
                        undimmed_on(buf, SELECTED, needle),
                        "{theme:?} {SELECTED} {needle}: a good report is plain below the total"
                    );
                }
            } else {
                for needle in needles {
                    assert!(
                        plain(buf, needle),
                        "{theme:?} {needle}: a good report is plain"
                    );
                }
            }
        };
        for theme in TIERS {
            let mut state = showing(many_days(3), |_| {});
            state.theme = theme;
            let buf = draw_buffer(&state, 80, 24, NOW_MS);
            let needles = [
                "ACCOUNT",
                "2026-09-21",
                "2026-09-20",
                "TOTAL",
                SELECTED,
                "what this would cost",
            ];
            good(&buf, theme, &needles);
            assert!(undimmed(&buf, "[ ] 30 days"), "{theme:?}");
            assert!(undimmed(&buf, "$0.03"), "{theme:?}");
            let asked = update(&mut state, ch('p'), NOW_MS);
            let [Cmd::LoadReport { r#gen, .. }] = asked.as_slice() else {
                panic!("{asked:?}");
            };
            let r#gen = *r#gen;
            assert!(state.tokens.pending());
            // Only a failure dims it.
            good(&draw_buffer(&state, 80, 24, NOW_MS), theme, &["2026-09-21"]);
            let failed = Err(SourceError {
                kind: "store".into(),
                message: "the index is locked".into(),
            });
            update(
                &mut state,
                Msg::TokenReport {
                    r#gen,
                    result: failed,
                },
                NOW_MS,
            );
            assert_eq!(state.tokens.report().map(|r| r.rows.len()), Some(3));
            assert_eq!(
                state.tokens.report().map(|r| r.query.period),
                Some(TokenPeriod::Day)
            );
            assert!(!state.tokens.pending());
            let buf = draw_buffer(&state, 80, 24, NOW_MS);
            assert_eq!(
                text_of(&buf)[3],
                " ! couldn't read the token index (the index is locked) · r retries"
            );
            for needle in [
                "ACCOUNT",
                "2026-09-21",
                "2026-09-20",
                "2026-09-19",
                "TOTAL",
                SELECTED,
                "what this would cost",
                "cache write",
            ] {
                assert!(dim(&buf, needle), "{theme:?} {needle}: DIM on every cell");
            }
            assert!(dim(&buf, "at API prices"), "{theme:?}: the headline too");
            assert!(
                undimmed(&buf, "[ ] 12 months"),
                "{theme:?}: the bar, the top level's choices, stays plain"
            );
            let again = update(&mut state, ch('p'), NOW_MS);
            let buf = draw_buffer(&state, 80, 24, NOW_MS);
            assert!(
                dim(&buf, "2026-09-21"),
                "{theme:?}: dim until the new report arrives"
            );
            assert!(undimmed(&buf, "[ ] 30 days"), "{theme:?}");
            let [Cmd::LoadReport { r#gen, query }] = again.as_slice() else {
                panic!("{again:?}");
            };
            let result = Ok(token_report(query.clone(), many_days(3)));
            update(
                &mut state,
                Msg::TokenReport {
                    r#gen: *r#gen,
                    result,
                },
                NOW_MS,
            );
            let buf = draw_buffer(&state, 80, 24, NOW_MS);
            assert!(
                !text_of(&buf).iter().any(|l| l.contains("couldn't read")),
                "{theme:?}"
            );
            good(
                &buf,
                theme,
                &[
                    "ACCOUNT",
                    "2026-09-21",
                    "TOTAL",
                    SELECTED,
                    "what this would cost",
                    "cache write",
                ],
            );
            assert!(undimmed(&buf, "$0.03"), "{theme:?}: the headline cleared");
            assert!(undimmed(&buf, "[ ] 30 days"), "{theme:?}");
        }
    }

    #[test]
    fn the_selected_row_has_the_bar_and_a_bold_first_column_and_no_reverse_video() {
        for theme in TIERS {
            let mut state = showing(many_days(3), |_| {});
            state.theme = theme;
            let buf = draw_buffer(&state, 80, 24, NOW_MS);
            let (x, y) = locate(&buf, "▌2026-09-21").unwrap();
            assert_eq!(x, 0);
            for i in 1..11 {
                assert!(
                    buf[(i, y)].modifier.contains(Modifier::BOLD),
                    "{theme:?} {i}"
                );
            }
            let (_, other) = locate(&buf, " 2026-09-20").unwrap();
            for i in 0..80 {
                assert!(
                    !buf[(i, y)].modifier.contains(Modifier::REVERSED),
                    "{theme:?} {i}"
                );
                assert!(
                    !buf[(i, other)].modifier.contains(Modifier::BOLD),
                    "{theme:?} {i}"
                );
                assert_eq!(
                    buf[(i, y)].bg,
                    theme.sel_bg().bg.unwrap_or(Color::Reset),
                    "{theme:?} {i}"
                );
                assert_eq!(buf[(i, other)].bg, Color::Reset, "{theme:?} {i}");
            }
        }
    }

    #[test]
    fn a_selected_row_with_a_repeated_period_bolds_its_first_non_empty_cell_alone() {
        for theme in TIERS {
            let mut state = showing(days(), |_| {});
            state.theme = theme;
            update(&mut state, key(KeyCode::Down), NOW_MS);
            let buf = draw_buffer(&state, 80, 24, NOW_MS);
            let (account, y) = locate(&buf, "2 spare").unwrap();
            assert_eq!(buf[(0, y)].symbol(), "▌", "{theme:?}");
            // Past column 0 (the bar is accent, and accent is bold without colour), the bold
            // cells are one run, and it starts at the account: the period is blank.
            let bold: Vec<u16> = (1..80)
                .filter(|x| buf[(*x, y)].modifier.contains(Modifier::BOLD))
                .collect();
            assert_eq!(bold.first(), Some(&account), "{theme:?} {bold:?}");
            assert!(
                bold.windows(2).all(|pair| pair[1] == pair[0] + 1),
                "{theme:?} {bold:?}"
            );
            assert!(bold.len() >= "2 spare".len(), "{theme:?} {bold:?}");
        }
    }

    #[test]
    fn a_long_report_scrolls_to_keep_the_selection_in_view_and_a_resize_keeps_it_there() {
        // R22: the stored scroll is clamped where it is used.
        let mut state = showing(many_days(30), |_| {});
        state.size = (120, 40);
        update(&mut state, key(KeyCode::End), NOW_MS);
        let lines = draw(&state, 120, 40);
        // Column 0 is the selection column, so a row starts at column 1.
        let from_1 = |l: &String| l.chars().skip(1).collect::<String>();
        assert!(
            lines.iter().any(|l| from_1(l).starts_with("2026-08-23")),
            "the last row shows"
        );
        for (w, h) in [(80, 24), (100, 30)] {
            state.size = (w, h);
            let lines = draw(&state, w, h);
            let last = lines
                .iter()
                .position(|l| from_1(l).starts_with("2026-08-23"));
            assert!(last.is_some(), "{w}x{h}: {lines:?}");
            assert!(lines[last.unwrap() + 1].starts_with(" ─"), "{w}x{h}");
            assert!(lines[last.unwrap() + 2].starts_with(" TOTAL"), "{w}x{h}");
        }
        update(&mut state, key(KeyCode::Home), NOW_MS);
        assert_eq!(
            body_of(&state, 100, 30, NOW_MS)[4],
            "▌2026-09-21  1 a1@x.co      1       2      6  $0.01"
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
            project: Some("/Users/alex/Code/ExampleCorp/portal-feature-user-manuals".into()),
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
            " SESSION      AC…  MODEL                      INPUT  OUTPUT  TOTAL         API $"
        );
        let big = &lines[head + 1];
        for value in ["09-20 09:12", "9.9B", "≥1.2B", "16.7B", "≥$123,456.79"] {
            assert!(big.contains(value), "{value} in {big:?}");
        }
        assert!(
            big.contains(" 3 …"),
            "the account by position, cut: {big:?}"
        );
        assert!(
            big.contains("  opus-5-5-with-a-very-lon…  "),
            "the model, short and cut last: {big:?}"
        );
        assert!(!lines[head].contains("PROJ"), "the project gave way first");
        let small = &lines[head + 2];
        assert!(small.contains("09-20 09:00"), "{small:?}");
        assert!(lines[head + 3].starts_with(" ─"), "{:?}", lines[head + 3]);
        let total = &lines[head + 4];
        for value in ["9.9B", "≥1.2B", "16.7B", "≥$123,456.80"] {
            assert!(total.contains(value), "{value} in {total:?}");
        }
        // At 120 the cache, the responses and the share line take room, and the span is cut to its
        // start, the account to its position and the model to what is left. At 160 the project
        // keeps `PROJECT_KEEP` columns (T5-20) and the account, still cut, leaves the span its
        // start; wider still, it shows the span and the years.
        let at_120 = draw(&state, 120, 40);
        assert!(
            at_120
                .iter()
                .any(|l| l.contains("▌09-20 09:12  3 …  opus-5-5-with-a-very-long…  ")),
            "{at_120:?}"
        );
        let at_160 = draw(&state, 160, 40);
        assert!(
            at_160
                .iter()
                .any(|l| l.contains("▌09-20 09:12  portal-feature-user…  3 山田太郎のアカ…  ")),
            "{at_160:?}"
        );
        let at_190 = draw(&state, 190, 40);
        assert!(
            at_190
                .iter()
                .any(|l| l.contains("2025-09-20 09:12–2025-09-21 11:40")),
            "{at_190:?}"
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
        let lines = body_of(&day, 80, 24, NOW_MS);
        assert!(
            lines[4].starts_with("▌2026-09-20  3 山…   opus-5-5-with-a-very-long-suf…  "),
            "{}",
            lines[4]
        );
    }

    /// A session as the engine gives one: a UUID, a long project, two models and the cache.
    fn realistic() -> TokenRow {
        let period = PeriodKey::Session {
            session_id: "3f2a9c1d-7e44-4b1a-9c55-0d1e2f3a4b5c".into(),
            project: Some("/Users/alex/Code/ExampleCorp/portal-feature-user-manuals".into()),
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

    /// What the selected row's detail must let a person read (B25), as T5-23 lays it out: its
    /// identity, its numbers, the rest; then the price table's note.
    const DETAIL: [&str; 7] = [
        "SELECTED  session 3f2a9c1d · 1 a1@x.co   ⏎ its models",
        "412 responses · cache write 4.1M (5m 3.0M, 1h 1.1M)",
        "cache read 88.0M",
        "/Users/alex/Code/ExampleCorp/portal-feature-user-manuals",
        "2026-09-20 09:12 – 2026-09-20 11:40",
        "claude-opus-5-5, claude-sonnet-5-5",
        "$ what this would cost on the API at list prices of 2026-10-01; not your bill",
    ];

    #[test]
    fn a_sessions_detail_shows_its_numbers_first_and_every_value_whole() {
        // B25, T5-23: after a blank row the identity, then the numbers, never cut; then the
        // session's text, wrapped, all `DETAIL_INDENT` in.
        let state = showing_after("pp", vec![realistic()]);
        let lines = draw(&state, 80, 24);
        let total = lines.iter().position(|l| l.starts_with(" TOTAL")).unwrap();
        assert_eq!(
            lines[total + 1..total + 8],
            [
                "",
                " SELECTED  session 3f2a9c1d · 1 a1@x.co   ⏎ its models",
                "           412 responses · cache write 4.1M (5m 3.0M, 1h 1.1M)",
                "           cache read 88.0M",
                "           /Users/alex/Code/ExampleCorp/portal-feature-user-manuals",
                "           2026-09-20 09:12 – 2026-09-20 11:40",
                "           claude-opus-5-5, claude-sonnet-5-5",
            ]
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

    /// R21 at 80×24: the auto line, a three-row banner and a three-row toast leave the body 16
    /// rows, from row 5, over ten sessions with every note.
    fn crowded() -> State {
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
        state.raise_banner(BannerKind::Refusal, [words; 3].join("; "));
        state.toast(crate::state::ToastKind::Info, [words; 3].join("; "), NOW_MS);
        state
    }

    #[test]
    fn under_a_full_banner_and_toast_j_and_k_reach_every_value_of_the_detail_and_the_notes() {
        // R21 at 80×24 (`crowded`): the table keeps three rows; the detail and the notes below
        // it scroll (B25).
        let mut state = crowded();
        let notes = [
            "No price for claude-test-1, claude-test-2, claude-test-3, claude-test-4, claude-test-5, claude-test-6, claude-test-7, claude-test-8, claude-test-9, claude-test-10, claude-test-11, claude-test-12: counted without cost (≥ marks a partial cost).",
            "≥ at least: 1 subagent response is still being counted",
            "12 transcript lines could not be read, so these totals may be low.",
            "3 transcript files could not be read; the next refresh tries again.",
            "$ what this would cost on the API at list prices of 2026-10-01, with config.toml's for claude-opus-5-5, claude-sonnet-5-5; not your bill",
        ];
        let lines = draw(&state, 80, 24);
        let total = lines.iter().position(|l| l.starts_with(" TOTAL")).unwrap();
        assert!(total >= 5 + 1 + 3, "the table keeps three rows: {lines:?}");
        assert_eq!(lines[total + 1], "", "{lines:?}");
        assert_eq!(
            lines[total + 2],
            format!(" {}", DETAIL[0]),
            "the identity first"
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
        assert_eq!(lines[total + 1], "", "K goes back to the top: {lines:?}");
        assert_eq!(
            lines[total + 2],
            format!(" {}", DETAIL[0]),
            "K goes back to the top"
        );
        assert!(
            lines[21].starts_with("the store could not"),
            "the toast: {lines:?}"
        );
    }

    #[test]
    fn the_j_k_hint_is_muted() {
        // T5-24: the hint is muted, not bold, as History's and the cards' are.
        let mut state = crowded();
        let theme = TIERS[1];
        state.theme = theme;
        let lines = draw(&state, 80, 24);
        assert!(lines[20].starts_with(" J K: lines 1–"), "{lines:?}");
        let buf = draw_buffer(&state, 80, 24, NOW_MS);
        let want = letters(&[(lines[20].as_str(), 'm')]);
        assert_eq!(roles(&buf, theme, 20).trim_end(), canon(theme, &want));
        assert!(
            (0..80).all(|x| !buf[(x, 20)].modifier.contains(Modifier::BOLD)),
            "not bold"
        );
    }

    #[test]
    fn the_bar_cuts_a_long_filter_and_keeps_its_keys() {
        // T5-7 form 6: the filter, which its dialog shows whole, gives way to the index's words
        // down to `FILTER_MIN`; every key keeps its place.
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
        let lines = body_of(&state, 80, 24, NOW_MS);
        assert_eq!(
            lines[0],
            " p Day   [ ] 30 days   g by account   f 2 a-rather-lon…   indexing 1.2 / 3.3 GB"
        );
        assert!(lines[1].starts_with(" $50.95 at API prices"), "{lines:?}");
        assert_eq!(lines[2], "");
        assert!(lines[3].starts_with(" DAY"), "{lines:?}");
    }

    #[test]
    fn the_filter_gives_way_before_the_index_words() {
        // T5-7 forms 6 and 7: the filter is cut to the room the index's words leave, down to
        // `FILTER_MIN`; only where that leaves them no room are they cut.
        let model = "claude-opus-5-5-with-a-very-long-suffix";
        let bar = |keys: &str| {
            let mut state = showing_after(keys, vec![session("3f2a9c1d", row_account(1))]);
            state.tokens.models = Some(vec![model.into()]);
            body_of(&state, 80, 24, NOW_MS)[0].clone()
        };
        assert_eq!(
            bar("pp"),
            " p Session   [ ] 30 days   g by account   f all accounts · cla…   indexed 14:12"
        );
        assert_eq!(
            bar("ppgg"),
            " p Session   [ ] 30 days   g by account and model   f all accou…   indexed 14:…"
        );
    }

    #[test]
    fn at_80x24_a_2000_column_model_leaves_the_table_three_rows_and_the_detail_reachable() {
        // B29, by construction: under the Now strip, a 2-row banner and a full toast the body is
        // 16 rows, from row 5. The 80×24 worst case, a 15-row body under a full banner too, is
        // the worst live stack's below.
        // The rows above the table take three; the table's heading and total two; what lies
        // below the total half the body at most and never the table's last three. Below the
        // total nothing is cut: it scrolls, and J reaches every line of it.
        let long = format!("claude-{}", "x".repeat(1993));
        let project = format!("/Users/alex/{}", "p".repeat(600));
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
        state.raise_banner(BannerKind::Refusal, [words; 2].join("; "));
        state.toast(crate::state::ToastKind::Info, [words; 3].join("; "), NOW_MS);
        let lines = draw(&state, 80, 24);
        assert!(lines.iter().all(|l| width_of(l) <= 80), "{lines:?}");
        assert_eq!(
            lines[5],
            " p Session   [ ] 30 days   g by account   f all accounts · xxx…   indexed 14:12",
            "the filter names the model by its short form, and gives way to the index's words"
        );
        assert!(lines[6].starts_with(" $"), "{lines:?}");
        assert_eq!(lines[7], "", "{lines:?}");
        assert!(lines[8].starts_with(" SESSION"), "{lines:?}");
        let total = lines.iter().position(|l| l.starts_with(" TOTAL")).unwrap();
        assert!(total >= 9 + 3, "the table keeps three rows: {lines:?}");
        assert_eq!(lines[total + 1], "", "{lines:?}");
        assert_eq!(lines[total + 2], format!(" {}", DETAIL[0]), "{lines:?}");
        assert!(lines[20].starts_with(" J K: lines 1–"), "{lines:?}");
        assert!(lines[21].starts_with("the store could not"), "{lines:?}");
        // Every line below the total, read by scrolling: each long piece whole.
        let seen = scrolled_below(&mut state);
        for value in [
            project.clone(),
            format!("{long}, claude-sonnet-5-5"),
            format!("No price for {long}: counted without cost (≥ marks a partial cost)."),
            "$ what this would cost on the API at list prices of 2026-10-01; not your bill"
                .to_owned(),
        ] {
            assert!(reads(&seen, &value), "{seen:?}");
        }
        assert!(!seen.iter().any(|l| l.contains('…')), "{seen:?}");
        assert!(seen.iter().all(|l| width_of(l) <= 80), "{seen:?}");
        let lines = draw(&state, 80, 24);
        let total = lines.iter().position(|l| l.starts_with(" TOTAL")).unwrap();
        assert!(
            total >= 9 + 3,
            "scrolled, the table keeps three rows: {lines:?}"
        );
    }

    #[test]
    fn the_worst_live_stack_at_80_by_24_keeps_three_table_rows_and_scrolls_to_a_2000_column_model_line()
     {
        // The header, the auto line, the Now strip, a 3-row banner and a 3-row toast leave a
        // 15-row body; the table keeps three rows, and J/K reach the model line, the footnote's
        // last.
        let long = format!("claude-{}", "x".repeat(1993));
        let mut big = realistic();
        big.models = vec![long.clone(), "claude-sonnet-5-5".into()];
        let mut rows = vec![big];
        rows.extend((1..=9).map(|n| session(&format!("s-{n}"), row_account(1))));
        let mut state = showing_after("pp", rows);
        state.tokens.models = Some(vec![long.clone()]);
        if let Some(report) = state.tokens.levels[0].report.as_mut() {
            report.unpriced_models = vec![long.clone()];
        }
        let mut locked = daemon_job("claude-code", "auto", true);
        locked.backoff = Some(tagteam_daemon::protocol::StatusBackoff {
            reason: "keychain".into(),
            until: "2026-09-21T15:05:00Z".into(),
        });
        // The auto line says nothing before the first accounts read (T4-30).
        state.accounts.apply(snapshot(vec![]));
        go_live(&mut state, daemon_status(vec![locked], vec![]));
        let ev = tagteam_daemon::protocol::Event::Auto {
            kind: "error".into(),
            provider: "claude-code".into(),
            raw: serde_json::json!({"event": "error", "ts": "2026-09-21T14:04:00Z", "message": "x".repeat(300)}),
        };
        update(&mut state, Msg::Live(crate::live::Live::Event(ev)), NOW_MS);
        let words = "the store could not be opened because another tagteam holds its lock";
        state.raise_banner(BannerKind::Refusal, [words; 3].join("; "));
        state.toast(crate::state::ToastKind::Info, [words; 3].join("; "), NOW_MS);
        let lines = draw(&state, 80, 24);
        assert_eq!(lines.len(), 24, "{lines:?}");
        assert!(lines.iter().all(|l| width_of(l) <= 80), "{lines:?}");
        // The stack: the header, the auto line, the Now strip, the banner (3), a 15-row body, the
        // toast (3).
        assert!(
            lines[0].ends_with("! keychain locked 15:05   ? keys"),
            "{lines:?}"
        );
        assert!(lines[1].starts_with(" ! 14:04  xxx"), "{lines:?}");
        assert!(lines[1].ends_with("…   w why"), "{lines:?}");
        assert!(
            lines[3..6]
                .iter()
                .all(|l| l.contains("the store could not be opened")
                    || l.contains("be opened because another tagteam holds its lock")),
            "{lines:?}"
        );
        assert!(
            lines[6].starts_with(" p Session"),
            "the body starts at row 6: {lines:?}"
        );
        assert!(
            lines[21..]
                .iter()
                .all(|l| l.contains("lock") || l.contains("tagteam")),
            "{lines:?}"
        );
        let total = lines.iter().position(|l| l.starts_with(" TOTAL")).unwrap();
        let head = lines
            .iter()
            .position(|l| l.starts_with(" SESSION"))
            .unwrap();
        assert_eq!(
            total - head - 2,
            3,
            "the table keeps three rows, then the rule: {lines:?}"
        );
        assert_eq!(lines[total + 1], "", "{lines:?}");
        assert_eq!(lines[total + 2], format!(" {}", DETAIL[0]), "{lines:?}");
        assert!(lines[20].starts_with(" J K: lines 1–"), "{lines:?}");
        let seen = scrolled_below(&mut state);
        for value in [
            format!("{long}, claude-sonnet-5-5"),
            format!("No price for {long}: counted without cost (≥ marks a partial cost)."),
        ] {
            assert!(reads(&seen, &value), "{seen:?}");
        }
        assert!(!seen.iter().any(|l| l.contains('…')), "{seen:?}");
        let lines = draw(&state, 80, 24);
        let total = lines.iter().position(|l| l.starts_with(" TOTAL")).unwrap();
        let head = lines
            .iter()
            .position(|l| l.starts_with(" SESSION"))
            .unwrap();
        assert_eq!(
            total - head - 2,
            3,
            "scrolled, the table keeps three rows, then the rule: {lines:?}"
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
        // A session of no account of its own lists every account it holds (T5-23).
        let mut row = realistic();
        row.account = None;
        row.key = RowKey::new(&row.period, None, None);
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
        let accounts: Vec<String> = row
            .accounts
            .iter()
            .map(|a| account_name(a, false))
            .collect();
        let accounts = accounts.join(", ");
        let models = row.models.join(", ");
        assert!(width_of(&accounts) > 3 * 79, "longer than three rows");
        assert!(width_of(&models) > 3 * 79, "longer than three rows");
        let mut state = showing_after("pp", vec![row]);
        let seen = scrolled_below(&mut state);
        assert!(reads(&seen, &accounts), "{seen:?}");
        assert!(reads(&seen, &models), "{seen:?}");
    }

    /// Every line below the total at 80×24, read as a person reads it from the row after the
    /// total, the blank one included: the first window, then the line each `J` brings in, until
    /// the hint says the last shows.
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
    fn the_short_forms_are_found_once_per_report() {
        // T5-22: a report's short model IDs are found when it is first drawn, and kept.
        let mut state = showing_after("g", work_models());
        let found = |state: &State| state.tokens.level().models.get().cloned();
        let at = |state: &State| {
            state
                .tokens
                .level()
                .models
                .get()
                .map(|m| m as *const ShortModels)
        };
        assert_eq!(found(&state), None, "not drawn yet");
        draw(&state, 80, 24);
        let first = found(&state).expect("found by the first frame");
        assert_eq!(first.get(MODELS[0]), "opus-4-5");
        let first_at = at(&state);
        draw(&state, 120, 40);
        assert_eq!(found(&state).as_ref(), Some(&first), "unchanged");
        assert_eq!(at(&state), first_at, "the same, not found again");
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
        assert_eq!(found(&state), None, "a new report has none until drawn");
        draw(&state, 80, 24);
        assert!(found(&state).is_some_and(|m| m != first));
    }

    /// The shown level's layout builds so far.
    fn builds(state: &State) -> usize {
        state.tokens.level().layout.builds()
    }

    #[test]
    fn the_table_is_laid_out_once_per_report_width_and_clock() {
        // PERF-R-10b: a frame reads the lines it shows from the layout, and lays them out
        // anew only for a new width or a new clock of an active block.
        let base = NOW_MS - NOW_MS.rem_euclid(1000);
        let mut state = showing(many_days(60), |_| {});
        state.size = (120, 40);
        assert_eq!(builds(&state), 0, "not drawn yet");
        draw_at(&state, 120, 40, base + 100);
        assert_eq!(builds(&state), 1);
        update(&mut state, ch('j'), base + 100);
        assert_eq!(builds(&state), 1, "following the selection reads it");
        draw_at(&state, 120, 40, base + 5_100);
        assert_eq!(builds(&state), 1, "no block, so no clock");
        state.size = (80, 24);
        draw_at(&state, 80, 24, base + 5_100);
        assert_eq!(builds(&state), 2, "a new width");

        let again = crate::screens::tokens::requery(&mut state, base);
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
            base,
        );
        assert_eq!(builds(&state), 0, "a new report has a new layout");
        draw_at(&state, 80, 24, base);
        assert_eq!(builds(&state), 1);
    }

    #[test]
    fn an_active_block_s_countdown_changes_when_it_did() {
        // R-5: the text is the one a frame at that millisecond has drawn, and the layout is
        // built again only where the countdown's seconds change.
        let base = NOW_MS - NOW_MS.rem_euclid(1000);
        let fresh = || showing_after("ppp", vec![block(base - 17_940_000, true)]);
        let state = fresh();
        let figures = |at: i64| under_the_block(&body_of(&state, 80, 24, at));
        for (at, builds_so_far) in [
            (base, 1),
            (base + 100, 2),
            (base + 150, 2),
            (base + 60_000, 3),
        ] {
            let lines = draw_at(&state, 80, 24, at);
            assert_eq!(lines, draw_at(&fresh(), 80, 24, at), "at {}", at - base);
            assert_eq!(builds(&state), builds_so_far, "at {}", at - base);
        }
        assert!(figures(base).contains("1m left"), "{}", figures(base));
        assert!(figures(base + 100).contains("<1m left"));
        assert!(figures(base + 60_000).starts_with(" ─"));
    }

    #[test]
    fn a_layout_asked_for_a_shorter_report_is_built_anew() {
        let base = NOW_MS - NOW_MS.rem_euclid(1000);
        let mut long_rows = many_days(3);
        long_rows.push(block(base - 17_940_000, true));
        let (long, short) = (showing(long_rows, |_| {}), showing(many_days(2), |_| {}));
        let (long, short) = (
            long.tokens.level().report.as_ref().unwrap(),
            short.tokens.level().report.as_ref().unwrap(),
        );
        let layout = Layout::default();
        let first = layout.rows(long, 80, base);
        assert_eq!((first.starts.len(), layout.builds()), (4, 1));
        let second = layout.rows(short, 80, base);
        assert_eq!((second.starts.len(), layout.builds()), (2, 2));
    }

    #[test]
    fn a_new_width_in_the_same_second_draws_like_a_fresh_layout() {
        let base = NOW_MS - NOW_MS.rem_euclid(1000);
        let build = || showing_after("ppp", vec![block(base - 17_940_000, true)]);
        let state = build();
        draw_at(&state, 120, 40, base + 100);
        let buffer = draw_buffer(&state, 80, 24, base + 100);
        assert_eq!(buffer, draw_buffer(&build(), 80, 24, base + 100));
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
