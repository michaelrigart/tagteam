//! The Accounts table (redesign §6.3, T3-15, T3-16): a marker, the name, a line gauge per short
//! and long window, UPDATED and STATUS, laid out for the width it has.

use ratatui::text::{Line, Span};
use tagteam_core::{AccountId, Pace, Window, WindowKind};
use tagteam_engine::switch::BestForecast;
use tagteam_engine::views::AccountView;
use tagteam_engine::words;

use super::status;
use crate::chrome::fit_spans;
use crate::gauge::line_gauge;
use crate::theme::{Role, Theme};
use crate::view::{fit, pad, width_of};

/// The width from which STATUS is a column of phrases; below it a glyph follows UPDATED.
pub(crate) const STATUS_FROM: u16 = 100;
/// The width from which scoped windows and spend get columns of their own (§4.5).
pub(crate) const EXTRA_FROM: u16 = 160;
/// The span `select` bolds from: the name's first span.
pub(crate) const NAME_SPAN: usize = 4;

/// The width from which gauges take `GAUGE_WIDE` cells (§4.5).
const WIDE_FROM: usize = 120;
const GAUGE_WIDE: usize = 14;
const GAUGE: usize = 9;
/// What a gauge shrinks to before it goes (T3-15).
const GAUGE_LEAST: usize = 6;
/// The name keeps this many columns before anything else shrinks (§6.3).
const NAME_LEAST: usize = 16;
/// The columns before the name: the selection marker, the position, the marker and their gaps.
const LEAD: usize = 8;
/// Between columns, and between groups (§4.3).
const GAP: usize = 2;
const GROUP_GAP: usize = 3;
const UPDATED_W: usize = 7;
const PCT_W: usize = 4;
/// The `▲` slot: ` ▲`, or two spaces, so the columns stay put when pace changes.
const AHEAD_W: usize = 2;
const AHEAD_MARK: &str = "▲";
const IN_USE: &str = "●";
const ALIAS_GAP: usize = 2;
const ORG_SEP: &str = " · ";
const STATUS_HEAD: &str = "STATUS";

/// What follows UPDATED.
#[derive(Clone, Copy)]
enum Status {
    None,
    /// A glyph, under `STATUS_FROM` columns.
    Glyph,
    /// A column of phrases this wide.
    Column(usize),
}

/// One window column, and how many cells its gauge has.
struct Column {
    key: String,
    kind: WindowKind,
    head: String,
    gauge: usize,
    /// The widest spend a row has, for the spend column.
    spend_w: usize,
}

impl Column {
    fn width(&self) -> usize {
        let cell = match self.kind {
            WindowKind::Spend => self.spend_w,
            _ => self.gauge + usize::from(self.gauge > 0) + PCT_W + AHEAD_W,
        };
        cell.max(width_of(&self.head))
    }
}

/// A row's windows, when it has a reading.
fn reading(v: &AccountView) -> Option<&[(Window, Pace)]> {
    v.usage.windows.as_deref().filter(|ws| !ws.is_empty())
}

/// A reading's age at `now_s`: its age when the accounts were read at `read_at_s`, and the
/// time since. Never less than the age it had.
pub(crate) fn age_now(age_s: i64, read_at_s: i64, now_s: i64) -> i64 {
    age_s + (now_s - read_at_s).max(0)
}

/// Columns and widths for one provider's section.
pub(crate) struct Table<'a> {
    cols: Vec<Column>,
    name_w: usize,
    /// The aliases' column inside the name's.
    alias_w: usize,
    orgs: bool,
    /// The window group's width: its columns', or a row's status words when no row has any.
    group_w: usize,
    status: Status,
    this: Option<&'a AccountId>,
    best: Option<u32>,
    width: usize,
    read_at_s: i64,
}

impl<'a> Table<'a> {
    /// T3-15's layout of `accounts` in `width` columns: the name at its widest, the gauges at
    /// §4.5's cells, scoped and spend from 160 when they fit with the name at 16; then, while a
    /// row is too wide, the name down to 16, the 7D gauge to 6, the 5H gauge to 6, the 7D
    /// gauge to none, the 5H gauge to none.
    pub(crate) fn new(
        accounts: &'a [AccountView],
        forecast: &BestForecast,
        this: Option<&'a AccountId>,
        width: usize,
        read_at_s: i64,
        now_s: i64,
    ) -> Table<'a> {
        let best = match forecast {
            BestForecast::Pick { order } => order.first().copied(),
            BestForecast::AlreadyBest | BestForecast::None(_) => None,
        };
        let marks = |v: &AccountView| (this == Some(&v.row.id), best == Some(v.row.position));
        let status = if width >= usize::from(STATUS_FROM) {
            accounts
                .iter()
                .filter_map(|v| {
                    let (this, best) = marks(v);
                    status::phrase(v, this, best)
                })
                .map(|(p, _)| width_of(&p))
                .max()
                .map_or(Status::None, |w| Status::Column(w.max(STATUS_HEAD.len())))
        } else if accounts.iter().any(|v| {
            let (this, best) = marks(v);
            status::glyph(v, this, best).is_some()
        }) {
            Status::Glyph
        } else {
            Status::None
        };
        let gauge = if width >= WIDE_FROM {
            GAUGE_WIDE
        } else {
            GAUGE
        };
        let all = words::window_columns(accounts);
        let column = |kind: WindowKind, gauge: usize| {
            all.iter()
                .filter(move |c| c.kind == kind)
                .filter_map(move |c| {
                    let key = c.key.clone()?;
                    let spend_w = accounts
                        .iter()
                        .filter_map(reading)
                        .flatten()
                        .filter(|(w, _)| w.key == key)
                        .map(|(w, _)| width_of(&spend_value(w)))
                        .fold(width_of(words::MISSING), usize::max);
                    Some(Column {
                        key,
                        kind,
                        head: c.head.clone(),
                        gauge,
                        spend_w,
                    })
                })
        };
        let cols: Vec<Column> = [WindowKind::Short, WindowKind::Long]
            .into_iter()
            .filter_map(|kind| column(kind, gauge).next())
            .collect();
        let alias_w = accounts
            .iter()
            .filter_map(|v| v.row.alias.as_deref())
            .map(width_of)
            .max()
            .unwrap_or(0);
        let unread_w = accounts
            .iter()
            .filter(|v| reading(v).is_none())
            .map(|v| width_of(&status::unread_words(v, now_s).0))
            .max()
            .unwrap_or(0);
        let mut table = Table {
            cols,
            name_w: 0,
            alias_w,
            orgs: false,
            group_w: unread_w,
            status,
            this,
            best,
            width,
            read_at_s,
        };
        let natural = accounts
            .iter()
            .map(|v| table.name_width(v))
            .fold(NAME_LEAST, usize::max);
        if width >= usize::from(EXTRA_FROM) {
            let extras = column(WindowKind::Scoped, 0).chain(column(WindowKind::Spend, 0));
            for col in extras {
                table.cols.push(col);
                if table.others(unread_w) + NAME_LEAST > width {
                    table.cols.pop();
                    break;
                }
            }
        }
        table.name_w = natural
            .min(width.saturating_sub(table.others(unread_w)))
            .max(NAME_LEAST);
        let short = table.cols.iter().position(|c| c.kind == WindowKind::Short);
        let long = table.cols.iter().position(|c| c.kind == WindowKind::Long);
        for (col, cells) in [
            (long, GAUGE_LEAST),
            (short, GAUGE_LEAST),
            (long, 0),
            (short, 0),
        ] {
            if table.others(unread_w) + table.name_w <= width {
                break;
            }
            if let Some(c) = col.and_then(|i| table.cols.get_mut(i)) {
                c.gauge = c.gauge.min(cells);
            }
        }
        table.group_w = table.group(unread_w);
        if table.cols.is_empty() {
            let room = width.saturating_sub(table.others(0) + table.name_w);
            table.group_w = unread_w.min(room);
        }
        if table.name_w < natural {
            // A cut column keeps room for the emails: an alias takes half of it at most.
            table.alias_w = table.alias_w.min(table.name_w / 2);
        }
        table.orgs = accounts.iter().all(|v| {
            v.row.org_name.as_deref().is_none_or(|org| {
                table.name_width(v) + width_of(ORG_SEP) + width_of(org) <= table.name_w
            })
        });
        table
    }

    /// The name cell's width for `v` without its org: the alias padded to `alias_w`, the gap
    /// and the email, or the email alone.
    fn name_width(&self, v: &AccountView) -> usize {
        let email = width_of(&words::email(&v.row));
        match v.row.alias {
            Some(_) => self.alias_w + ALIAS_GAP + email,
            None => email,
        }
    }

    /// The window group's width: its columns and the gaps between them, or `unread_w` without
    /// columns.
    fn group(&self, unread_w: usize) -> usize {
        if self.cols.is_empty() {
            return unread_w;
        }
        let cells: usize = self.cols.iter().map(Column::width).sum();
        cells + GAP * (self.cols.len() - 1)
    }

    /// A row's width less the name's.
    fn others(&self, unread_w: usize) -> usize {
        let status = match self.status {
            Status::None => 0,
            Status::Glyph => 2,
            Status::Column(w) => GAP + w,
        };
        LEAD + GROUP_GAP + self.group(unread_w) + GROUP_GAP + UPDATED_W + status
    }

    /// The column heads, muted (§4.3).
    pub(crate) fn header(&self, theme: Theme) -> Line<'static> {
        let heads: Vec<String> = self.cols.iter().map(|c| pad(&c.head, c.width())).collect();
        let mut text = format!(
            " {:>4}   {}{}{}",
            "#",
            pad("ACCOUNT", self.name_w),
            " ".repeat(GROUP_GAP),
            pad(&heads.join(&" ".repeat(GAP)), self.group_w)
        );
        text.push_str(&" ".repeat(GROUP_GAP));
        text.push_str(&format!("{:>UPDATED_W$}", words::UPDATED_HEAD));
        if let Status::Column(_) = self.status {
            text.push_str(&" ".repeat(GAP));
            text.push_str(STATUS_HEAD);
        }
        let text = fit(text.trim_end(), self.width);
        Line::from(Span::styled(text, theme.role(Role::Muted)))
    }

    /// `v`'s row (T3-15). The selection's bar takes its first cell, and its name starts at
    /// `NAME_SPAN`.
    pub(crate) fn row(&self, v: &AccountView, now_s: i64, theme: Theme) -> Line<'static> {
        let mut spans = vec![
            Span::raw(" "),
            Span::raw(format!("{:>4} ", v.row.position)),
            mark(v, theme),
            Span::raw(" "),
        ];
        spans.extend(self.name(v, theme));
        spans.push(Span::raw(" ".repeat(GROUP_GAP)));
        let mut group = Vec::new();
        match reading(v) {
            Some(ws) => {
                for (i, col) in self.cols.iter().enumerate() {
                    if i > 0 {
                        group.push(Span::raw(" ".repeat(GAP)));
                    }
                    group.extend(cell(col, ws, theme));
                }
            }
            None => {
                let (words, role) = status::unread_words(v, now_s);
                group.push(Span::styled(fit(&words, self.group_w), theme.role(role)));
            }
        }
        padded(&mut group, self.group_w);
        spans.extend(group);
        spans.push(Span::raw(" ".repeat(GROUP_GAP)));
        spans.extend(self.updated(v, now_s, theme));
        let this = self.this == Some(&v.row.id);
        let best = self.best == Some(v.row.position);
        match self.status {
            Status::Column(_) => {
                if let Some((phrase, role)) = status::phrase(v, this, best) {
                    spans.push(Span::raw(" ".repeat(GAP)));
                    spans.push(Span::styled(phrase, theme.role(role)));
                }
            }
            Status::Glyph => {
                if let Some((glyph, role)) = status::glyph(v, this, best) {
                    spans.push(Span::raw(" "));
                    spans.push(Span::styled(glyph, theme.role(role)));
                }
            }
            Status::None => {}
        }
        Line::from(fit_spans(spans, self.width))
    }

    /// The name cell, exactly `name_w` wide: the alias, then the email muted, and the org muted
    /// when every row's fits (T3-15).
    fn name(&self, v: &AccountView, theme: Theme) -> Vec<Span<'static>> {
        let muted = theme.role(Role::Muted);
        let email = words::email(&v.row);
        let mut spans = match &v.row.alias {
            Some(alias) => {
                let alias = fit(alias, self.alias_w);
                let gap = self.alias_w.saturating_sub(width_of(&alias)) + ALIAS_GAP;
                vec![
                    Span::raw(alias),
                    Span::raw(" ".repeat(gap)),
                    Span::styled(email, muted),
                ]
            }
            None => vec![Span::raw(email)],
        };
        if let Some(org) = v.row.org_name.as_deref().filter(|_| self.orgs) {
            spans.push(Span::styled(format!("{ORG_SEP}{org}"), muted));
        }
        let mut spans = fit_spans(spans, self.name_w);
        padded(&mut spans, self.name_w);
        spans
    }

    /// UPDATED: the reading's age, muted, warn when the reading is not decision-grade
    /// (§6.3), whatever the status; `—` faint without one.
    fn updated(&self, v: &AccountView, now_s: i64, theme: Theme) -> Vec<Span<'static>> {
        let (text, role) = match (reading(v), v.usage.age_s) {
            (Some(_), Some(age)) => {
                let stale = !v.usage.decision_grade;
                let age = words::duration(age_now(age, self.read_at_s, now_s));
                (age, if stale { Role::Warn } else { Role::Muted })
            }
            _ => (words::MISSING.to_owned(), Role::Faint),
        };
        vec![
            Span::raw(" ".repeat(UPDATED_W.saturating_sub(width_of(&text)))),
            Span::styled(text, theme.role(role)),
        ]
    }
}

/// `spans` padded with spaces to `w` columns.
fn padded(spans: &mut Vec<Span<'static>>, w: usize) {
    let used: usize = spans.iter().map(|s| width_of(&s.content)).sum();
    if used < w {
        spans.push(Span::raw(" ".repeat(w - used)));
    }
}

/// The marker column (§6.3): `●` in use, else `▶` in a session, else `○` disabled.
fn mark(v: &AccountView, theme: Theme) -> Span<'static> {
    if v.active {
        Span::styled(IN_USE, theme.role(Role::Accent))
    } else if v.in_session {
        Span::styled(words::IN_SESSION, theme.role(Role::Accent))
    } else if v.row.disabled {
        Span::styled(status::DISABLED, theme.role(Role::Muted))
    } else {
        Span::raw(" ")
    }
}

/// A spend window's value: its amounts, else its percentage.
fn spend_value(w: &Window) -> String {
    words::spend_text(w).unwrap_or_else(|| format!("{}%", w.pct.round() as i64))
}

/// A window cell, exactly `col.width()` wide: `{gauge} {pct:>4}{▲}` (T3-15), or spend's value
/// right-aligned (§4.3); `—` faint where the reading lacks the window, where its value would
/// end.
fn cell(col: &Column, ws: &[(Window, Pace)], theme: Theme) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let gauge_w = col.gauge + usize::from(col.gauge > 0);
    let spend = col.kind == WindowKind::Spend;
    let right = |text: &str| " ".repeat(col.width().saturating_sub(width_of(text)));
    match ws.iter().find(|(w, _)| w.key == col.key) {
        None => {
            if spend {
                spans.push(Span::raw(right(words::MISSING)));
            } else {
                spans.push(Span::raw(" ".repeat(gauge_w + PCT_W - 1)));
            }
            spans.push(Span::styled(words::MISSING, theme.role(Role::Faint)));
        }
        Some((w, p)) => {
            let pct = w.pct.round() as i64;
            let style = theme.severity(words::severity(pct));
            match spend.then(|| spend_value(w)) {
                Some(value) => {
                    spans.push(Span::raw(right(&value)));
                    spans.push(Span::styled(value, style));
                }
                None => {
                    if col.gauge > 0 {
                        let expected = match col.kind {
                            WindowKind::Long | WindowKind::Scoped => p.expected_pct,
                            WindowKind::Short | WindowKind::Spend => None,
                        };
                        spans.extend(line_gauge(w.pct, expected, col.gauge, theme));
                        spans.push(Span::raw(" "));
                    }
                    let text = format!("{pct}%");
                    spans.push(Span::raw(" ".repeat(PCT_W.saturating_sub(width_of(&text)))));
                    spans.push(Span::styled(text, style));
                    if p.ahead == Some(true) {
                        spans.push(Span::raw(" "));
                        spans.push(Span::styled(AHEAD_MARK, theme.role(Role::Warn)));
                    }
                }
            }
        }
    }
    padded(&mut spans, col.width());
    spans
}
