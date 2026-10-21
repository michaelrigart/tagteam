//! The Accounts screen (spec §5.2): `list`'s table, in `list`'s words, with a gauge per window
//! when the terminal is wide enough, and the selected account's detail below it.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use tagteam_core::{AccountId, Pace, ProviderId, Window, WindowKind};
use tagteam_engine::views::{AccountView, HistoryWindow, UsageStatus};
use tagteam_engine::words::{self, WindowColumn};

use crate::host::AccountRef;
use crate::msg::Cmd;
use crate::source::AccountsSnapshot;
use crate::state::{Screen, State};
use crate::theme::{Role, Theme};
use crate::update;
use crate::view::{
    fit, flow, furthest_scroll, keep_visible, pad, scrolled_windows, select, width_of, wrap_rows,
};

/// Spec §10.4's empty state: no store, or no accounts.
pub const NO_ACCOUNTS: &str = "No accounts yet. Log in with claude, then press A to add the login.";
/// Spec §6.1: drawn until the first `accounts` result arrives.
pub const LOADING: &str = "loading…";
/// The banners the accounts view raises (spec §5.1).
pub const RUN_SHELL: &str = "inside a `tagteam run` session: account changes are refused here";
/// A gauge's width, in cells, and the narrowest the ACCOUNT column is cut to.
/// The account cell's place among a row's spans (`Table::row`): the selection bolds it.
const NAME_SPAN: usize = 3;

const GAUGE: usize = 6;
const ACCOUNT_MIN: usize = 12;
/// The room the notes after a row's cells are given at least, when columns or the ACCOUNT
/// column give way for them: the longest note, up to this many columns.
const NOTES_ROOM: usize = 36;
/// The detail pane's least height: a rule, four windows, the reading's age, and its trouble.
const DETAIL_MIN: u16 = 7;
/// The detail pane's lines besides its windows' own: the rule, the reading's age, and its
/// trouble.
const DETAIL_FRAME: usize = 3;
/// The table keeps this many rows at least, its header and the selected row among them, when
/// the pane grows past half the body to hold its tallest window.
const TABLE_MIN: u16 = 3;
/// The rows a long label takes above its value, at most (R20).
const LABEL_ROWS: usize = 2;

#[derive(Default)]
pub struct AccountsState {
    snapshot: Option<AccountsSnapshot>,
    /// The selected account, by ID, so a refresh that reorders rows keeps it (Review Focus 5).
    selected: Option<AccountId>,
    /// The first table line shown.
    offset: usize,
    /// The first of the selected account's windows the detail pane shows (`J`/`K`), when they
    /// do not all fit.
    detail_from: usize,
}

impl AccountsState {
    pub fn snapshot(&self) -> Option<&AccountsSnapshot> {
        self.snapshot.as_ref()
    }

    /// Takes a new read. The selection stays on the same account; when that account is gone it
    /// moves to the row now at its old place, or the last row (Review Focus 5).
    pub fn apply(&mut self, snap: AccountsSnapshot) {
        let old = self.index();
        let ids: Vec<AccountId> = rows_of(&snap).iter().map(|v| v.row.id.clone()).collect();
        let selected = match &self.selected {
            Some(id) if ids.contains(id) => Some(id.clone()),
            _ if ids.is_empty() => None,
            _ => Some(ids[old.unwrap_or(0).min(ids.len() - 1)].clone()),
        };
        if selected != self.selected {
            self.detail_from = 0;
        }
        self.selected = selected;
        self.snapshot = Some(snap);
    }

    /// The selected account, as the TUI last read it: what every account action names.
    pub fn selected(&self) -> Option<AccountRef> {
        self.selected_view().map(|v| AccountRef {
            id: v.row.id.clone(),
            provider: v.row.provider.clone(),
            position: v.row.position,
        })
    }

    /// Every account shown, for `collect_due`.
    pub fn shown_ids(&self) -> Vec<AccountId> {
        self.rows().iter().map(|v| v.row.id.clone()).collect()
    }

    pub(crate) fn selected_view(&self) -> Option<&AccountView> {
        let id = self.selected.as_ref()?;
        self.rows().into_iter().find(|v| &v.row.id == id)
    }

    /// The accounts shown, in table order.
    fn rows(&self) -> Vec<&AccountView> {
        self.snapshot.as_ref().map(rows_of).unwrap_or_default()
    }

    fn index(&self) -> Option<usize> {
        let id = self.selected.as_ref()?;
        self.rows().iter().position(|v| &v.row.id == id)
    }

    /// Moves the selection `by` rows, stopping at either end (Review Focus 4).
    fn step(&mut self, by: isize) {
        let rows = self.rows();
        if rows.is_empty() {
            return;
        }
        let at = self.index().unwrap_or(0) as isize;
        let to = (at + by).clamp(0, rows.len() as isize - 1) as usize;
        let id = rows[to].row.id.clone();
        if self.selected.as_ref() != Some(&id) {
            self.detail_from = 0;
        }
        self.selected = Some(id);
    }
}

/// Every provider's accounts, in the order `list` prints them.
fn rows_of(snap: &AccountsSnapshot) -> Vec<&AccountView> {
    snap.providers.iter().flat_map(|p| &p.accounts).collect()
}

/// An account's windows, as its reading lists them; none without a reading.
fn windows_of(v: &AccountView) -> &[(Window, Pace)] {
    v.usage.windows.as_deref().unwrap_or_default()
}

/// What the accounts view has to say above the body: a run shell (spec §3), else a live login
/// tagteam does not manage (`status`'s own words).
pub fn banner(state: &AccountsState) -> Option<String> {
    let snap = state.snapshot.as_ref()?;
    if snap.in_run_shell {
        return Some(RUN_SHELL.to_owned());
    }
    snap.unmanaged_live
        .as_ref()
        .map(|email| format!("Live: {email} (not managed by tagteam); press A to add it"))
}

pub fn handle_key(state: &mut State, key: KeyEvent, now_ms: i64) -> Vec<Cmd> {
    let body = crate::view::body_at(state);
    let now_s = now_ms.div_euclid(1000);
    let page = visible_rows(&state.accounts, body, now_s).max(1) as isize;
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => state.accounts.step(1),
        KeyCode::Char('k') | KeyCode::Up => state.accounts.step(-1),
        KeyCode::PageDown => state.accounts.step(page),
        KeyCode::PageUp => state.accounts.step(-page),
        KeyCode::Home | KeyCode::Char('g') => state.accounts.step(isize::MIN / 2),
        KeyCode::End | KeyCode::Char('G') => state.accounts.step(isize::MAX / 2),
        KeyCode::Char('J') => {
            scroll_detail(state, body, now_s, 1);
            return vec![];
        }
        KeyCode::Char('K') => {
            scroll_detail(state, body, now_s, -1);
            return vec![];
        }
        KeyCode::Enter => {
            let Some(id) = state.accounts.selected.clone() else {
                return vec![];
            };
            state.history.select(id);
            return update::show(state, Screen::History, now_ms);
        }
        _ => return vec![],
    }
    let lines = layout(&state.accounts).len();
    let at = line_of(&state.accounts);
    let rows = visible_rows(&state.accounts, body, now_s);
    state.accounts.offset = keep_visible(state.accounts.offset, at, at, rows, lines);
    vec![]
}

/// `J`/`K`: the detail pane's windows move `by` one, stopping once the last one shows, or at
/// the first.
fn scroll_detail(state: &mut State, body: Rect, now_s: i64, by: isize) {
    let st = &mut state.accounts;
    let Some(v) = st.selected_view() else {
        return;
    };
    let heights = window_heights(v, usize::from(body.width), now_s);
    let furthest = furthest_scroll(&heights, window_rows(st, v, body, now_s));
    // From what shows: a resize or a new read may have left the stored scroll past the end.
    let from = st.detail_from.min(furthest) as isize;
    st.detail_from = (from + by).clamp(0, furthest as isize) as usize;
}

/// The table's area and the detail pane's. The pane has room for the lines of the most windows
/// any account shown has, at the body's width (spec §5.2: each relevant window), and at least
/// `DETAIL_MIN` lines, in at most half the body: the table keeps the rest. Windows that still
/// do not fit scroll (`J`/`K`), a whole window at a time, so the pane is never shorter than its
/// tallest window, taking rows from the table down to `TABLE_MIN` for it (R20).
fn split(state: &AccountsState, body: Rect, now_s: i64) -> (Rect, Rect) {
    let width = usize::from(body.width);
    let heights: Vec<Vec<usize>> = state
        .rows()
        .iter()
        .map(|v| window_heights(v, width, now_s))
        .collect();
    let rows = |n: usize| u16::try_from(DETAIL_FRAME + n).unwrap_or(u16::MAX);
    let most = rows(heights.iter().map(|h| h.iter().sum()).max().unwrap_or(0));
    let tallest = rows(heights.iter().flatten().copied().max().unwrap_or(0));
    let detail = most
        .max(DETAIL_MIN)
        .min(body.height / 2)
        .max(tallest)
        .min(body.height.saturating_sub(TABLE_MIN));
    let table = Rect::new(body.x, body.y, body.width, body.height - detail);
    let pane = Rect::new(body.x, body.y + table.height, body.width, detail);
    (table, pane)
}

/// How many lines the detail pane has for `v`'s windows: its height less the rule, the
/// reading's age, and the trouble line when the usage is not current.
fn window_rows(state: &AccountsState, v: &AccountView, body: Rect, now_s: i64) -> usize {
    let trouble = usize::from(v.usage.status != UsageStatus::Ok);
    usize::from(split(state, body, now_s).1.height).saturating_sub(2 + trouble)
}

/// How many scrolling table lines fit in `body`: one provider's header is pinned above them.
fn visible_rows(state: &AccountsState, body: Rect, now_s: i64) -> usize {
    let pinned = sections(state).len() == 1;
    usize::from(split(state, body, now_s).0.height).saturating_sub(usize::from(pinned))
}

/// One line of the scrolling part of the table.
enum TableLine<'a> {
    /// A provider's name, when there is more than one (spec §5.1, "Providers").
    Provider(&'a ProviderId),
    /// A provider's column header, below its name; one provider's is pinned instead.
    Header(usize),
    Account(usize, &'a AccountView),
}

/// The sections shown: one per provider with accounts.
fn sections(state: &AccountsState) -> Vec<(&ProviderId, &[AccountView])> {
    state
        .snapshot
        .as_ref()
        .map(|s| {
            s.providers
                .iter()
                .filter(|p| !p.accounts.is_empty())
                .map(|p| (&p.provider, p.accounts.as_slice()))
                .collect()
        })
        .unwrap_or_default()
}

/// The table's scrolling lines. With one provider its header is pinned above them.
fn layout(state: &AccountsState) -> Vec<TableLine<'_>> {
    let sections = sections(state);
    let several = sections.len() > 1;
    let mut lines = Vec::new();
    for (i, (provider, accounts)) in sections.into_iter().enumerate() {
        if several {
            lines.push(TableLine::Provider(provider));
            lines.push(TableLine::Header(i));
        }
        lines.extend(accounts.iter().map(|v| TableLine::Account(i, v)));
    }
    lines
}

/// The selected account's line.
fn line_of(state: &AccountsState) -> usize {
    let Some(id) = state.selected.as_ref() else {
        return 0;
    };
    layout(state)
        .iter()
        .position(|l| matches!(l, TableLine::Account(_, v) if &v.row.id == id))
        .unwrap_or(0)
}

/// A cell: its text, and its parts with their styles.
struct Cell {
    spans: Vec<Span<'static>>,
}

impl Cell {
    fn plain(text: impl Into<String>) -> Self {
        Cell {
            spans: vec![Span::raw(text.into())],
        }
    }

    fn push(&mut self, text: impl Into<String>, style: Style) {
        self.spans.push(Span::styled(text.into(), style));
    }

    fn width(&self) -> usize {
        self.spans.iter().map(|s| width_of(&s.content)).sum()
    }
}

/// One provider's table, laid out for the width it has.
struct Table<'a> {
    cols: Vec<WindowColumn>,
    widths: Vec<usize>,
    account_w: usize,
    gauges: bool,
    sessions: bool,
    this: Option<&'a AccountId>,
    width: usize,
    /// When the accounts were read: a reading's age grows from it (`age_now`).
    read_at_s: i64,
}

/// A reading's age at `now_s`: its age when the accounts were read at `read_at_s`, and the
/// time since. Never less than the age it had.
fn age_now(age_s: i64, read_at_s: i64, now_s: i64) -> i64 {
    age_s + (now_s - read_at_s).max(0)
}

impl<'a> Table<'a> {
    /// `list`'s columns for `accounts`, with gauges when the whole of every row still fits
    /// beside them, and the ACCOUNT column cut by display width when even `list`'s line does
    /// not (Review Focus 2).
    fn new(
        accounts: &[AccountView],
        this: Option<&'a AccountId>,
        width: usize,
        read_at_s: i64,
        now_s: i64,
        theme: Theme,
    ) -> Self {
        let cols = words::window_columns(accounts);
        let sessions = accounts.iter().any(|v| v.in_session);
        let mut table = Table {
            widths: vec![],
            cols,
            account_w: 0,
            gauges: false,
            sessions,
            this,
            width,
            read_at_s,
        };
        let heads: Vec<usize> = table.heads().iter().map(|h| width_of(h)).collect();
        let widths: Vec<usize> = heads
            .iter()
            .enumerate()
            .map(|(i, head)| {
                accounts
                    .iter()
                    .filter_map(|v| table.cells(v, now_s, theme))
                    .map(|cells| cells[i].width())
                    .fold(*head, usize::max)
            })
            .collect();
        let account_w = accounts
            .iter()
            .map(|v| width_of(&words::account_cell(&v.row)))
            .fold(width_of("ACCOUNT"), usize::max);
        let mut widths = widths;
        let base = 5 + if sessions { 2 } else { 0 } + 2;
        // The notes after a row's cells (its status in words, `disabled`, `this`) must still
        // fit beside the gauges: they say more than a gauge does.
        let notes_w = accounts
            .iter()
            .filter(|v| table.cells(v, now_s, theme).is_some())
            .map(|v| {
                words::row_notes(v, this == Some(&v.row.id), now_s)
                    .iter()
                    .map(|n| 2 + width_of(n))
                    .sum::<usize>()
            })
            .max()
            .unwrap_or(0);
        // Notes get room of their own: when even a minimal ACCOUNT column leaves them too
        // little, window columns give way, scoped ones first, then spend; 5H and 7D stay. The
        // dropped windows stay in the detail pane.
        let room = notes_w.min(NOTES_ROOM);
        loop {
            let fixed = base + widths.iter().map(|w| w + 2).sum::<usize>();
            if fixed + account_w.min(ACCOUNT_MIN) + room <= width {
                break;
            }
            let kind = |k| table.cols.iter().rposition(|c| c.kind == k);
            let Some(i) = kind(WindowKind::Scoped).or_else(|| kind(WindowKind::Spend)) else {
                break;
            };
            table.cols.remove(i);
            widths.remove(i);
        }
        let fixed = base + widths.iter().map(|w| w + 2).sum::<usize>();
        let gauge_w = table.gauge_columns() * (GAUGE + 1);
        table.widths = widths;
        if fixed + account_w + gauge_w + notes_w <= width {
            table.gauges = true;
            table.account_w = account_w;
            for (i, col) in table.cols.iter().enumerate() {
                if col.kind != WindowKind::Spend {
                    table.widths[i] += GAUGE + 1;
                }
            }
        } else {
            table.account_w = account_w.min(width.saturating_sub(fixed + room).max(ACCOUNT_MIN));
        }
        table
    }

    /// The window columns that take a gauge: every one but spend.
    fn gauge_columns(&self) -> usize {
        self.cols
            .iter()
            .filter(|c| c.kind != WindowKind::Spend)
            .count()
    }

    fn heads(&self) -> Vec<String> {
        self.cols
            .iter()
            .map(|c| c.head.clone())
            .chain((!self.cols.is_empty()).then(|| "AGE".to_owned()))
            .collect()
    }

    fn header(&self, theme: Theme) -> Line<'static> {
        let gap = if self.sessions { "    " } else { "  " };
        let mut text = format!("    #{gap}{}", pad("ACCOUNT", self.account_w));
        for (head, w) in self.heads().iter().zip(&self.widths) {
            text.push_str("  ");
            text.push_str(&pad(head, *w));
        }
        Line::from(Span::styled(text.trim_end().to_owned(), theme.strong()))
    }

    /// A row's window cells and AGE, when it has a reading with windows.
    fn cells(&self, v: &AccountView, now_s: i64, theme: Theme) -> Option<Vec<Cell>> {
        let ws = v.usage.windows.as_deref().filter(|ws| !ws.is_empty())?;
        let mut cells: Vec<Cell> = self
            .cols
            .iter()
            .map(|c| {
                match ws
                    .iter()
                    .find(|(w, _)| c.key.as_deref() == Some(w.key.as_str()))
                {
                    Some((w, p)) => self.window_cell(w, p, now_s, theme),
                    None => self.missing(c),
                }
            })
            .collect();
        cells.push(Cell::plain(v.usage.age_s.map_or_else(
            || words::MISSING.to_owned(),
            |age| words::duration(age_now(age, self.read_at_s, now_s)),
        )));
        Some(cells)
    }

    /// `list`'s cell (`  9%  2h40m  ▲ pace`, or the spend amounts), after a gauge.
    fn window_cell(&self, w: &Window, p: &Pace, now_s: i64, theme: Theme) -> Cell {
        let style = theme.severity(words::severity(w.pct.round() as i64));
        let mut cell = Cell { spans: vec![] };
        match words::spend_text(w) {
            Some(spend) => cell.push(spend, style),
            None => {
                if self.gauges {
                    cell.push(gauge(w.pct), style);
                    cell.push(" ", Style::new());
                }
                cell.push(
                    format!("{:>4}", format!("{}%", w.pct.round() as i64)),
                    style,
                );
            }
        }
        if let Some(at) = w.resets_at {
            cell.push(format!("  {}", words::countdown(at, now_s)), Style::new());
        }
        if p.ahead == Some(true) {
            cell.push(format!("  {}", words::AHEAD), Style::new());
        }
        cell
    }

    /// A window this row's reading lacks: `—` under the `%`, or at the start of spend.
    fn missing(&self, c: &WindowColumn) -> Cell {
        match c.kind {
            WindowKind::Spend => Cell::plain(words::MISSING),
            _ if self.gauges => {
                Cell::plain(format!("{}{:>4}", " ".repeat(GAUGE + 1), words::MISSING))
            }
            _ => Cell::plain(format!("{:>4}", words::MISSING)),
        }
    }

    fn row(&self, v: &AccountView, now_s: i64, theme: Theme) -> Line<'static> {
        let session = match (self.sessions, v.in_session) {
            (false, _) => String::new(),
            (true, true) => format!(" {}", words::IN_SESSION),
            (true, false) => "  ".to_owned(),
        };
        let marker = if v.active { "*" } else { " " };
        let mut spans = vec![
            Span::raw(" "),
            Span::styled(marker, theme.role(Role::Accent)),
            Span::raw(format!(" {:>2}{session}  ", v.row.position)),
            Span::raw(pad(&words::account_cell(&v.row), self.account_w)),
        ];
        if let Some(cells) = self.cells(v, now_s, theme) {
            for (cell, w) in cells.into_iter().zip(&self.widths) {
                let fill = w.saturating_sub(cell.width());
                spans.push(Span::raw("  "));
                spans.extend(cell.spans);
                spans.push(Span::raw(" ".repeat(fill)));
            }
        }
        let this = self.this == Some(&v.row.id);
        let notes: String = words::row_notes(v, this, now_s)
            .iter()
            .map(|note| format!("  {note}"))
            .collect();
        if !notes.is_empty() {
            let used: usize = spans.iter().map(|s| width_of(&s.content)).sum();
            spans.push(Span::raw(fit(&notes, self.width.saturating_sub(used))));
        }
        Line::from(spans)
    }
}

/// A gauge of `GAUGE` cells, filled in proportion to `pct`.
fn gauge(pct: f64) -> String {
    let filled = ((pct.clamp(0.0, 100.0) / 100.0) * GAUGE as f64).round() as usize;
    format!("{}{}", "█".repeat(filled), "░".repeat(GAUGE - filled))
}

pub fn render(state: &State, frame: &mut Frame, area: Rect, now_ms: i64) {
    let st = &state.accounts;
    let Some(snap) = st.snapshot.as_ref() else {
        frame.render_widget(Paragraph::new(format!(" {LOADING}")), area);
        return;
    };
    if st.rows().is_empty() {
        frame.render_widget(Paragraph::new(format!(" {NO_ACCOUNTS}")), area);
        return;
    }
    let now_s = now_ms.div_euclid(1000);
    let width = usize::from(area.width);
    let (table_area, pane) = split(st, area, now_s);
    let sections = sections(st);
    let tables: Vec<Table<'_>> = sections
        .iter()
        .map(|(_, accounts)| {
            Table::new(
                accounts,
                snap.this.as_ref(),
                width,
                snap.read_at_s,
                now_s,
                state.theme,
            )
        })
        .collect();
    let lines = layout(st);
    let mut shown: Vec<Line<'static>> = Vec::new();
    if tables.len() == 1 {
        shown.push(tables[0].header(state.theme));
    }
    let rows = visible_rows(st, area, now_s);
    let at = line_of(st);
    let offset = keep_visible(st.offset, at, at, rows, lines.len());
    for line in lines.iter().skip(offset).take(rows) {
        shown.push(match line {
            TableLine::Provider(p) => {
                Line::from(Span::styled(format!(" {p}"), state.theme.strong()))
            }
            TableLine::Header(i) => tables[*i].header(state.theme),
            TableLine::Account(i, v) => {
                let row = tables[*i].row(v, now_s, state.theme);
                if st.selected.as_ref() == Some(&v.row.id) {
                    select(row, Some(NAME_SPAN), width, state.theme)
                } else {
                    row
                }
            }
        });
    }
    frame.render_widget(Paragraph::new(shown), table_area);
    if let Some(v) = st.selected_view() {
        let shown = Shown {
            from: st.detail_from,
            rows: window_rows(st, v, area, now_s),
        };
        frame.render_widget(
            Paragraph::new(detail(v, snap, shown, width, now_s, state.theme)),
            pane,
        );
    }
}

/// Where the detail pane's windows are scrolled to (`from`, a window), and how many lines they
/// have (`rows`).
struct Shown {
    from: usize,
    rows: usize,
}

/// Each of `v`'s windows' lines in the detail pane at `width`: its label and value, then its
/// reset, its pace and its projection, which go on under the value when they do not fit (R20).
/// The label column is a third of the width at most, so the value and what follows it always
/// have room.
fn window_blocks(
    v: &AccountView,
    width: usize,
    now_s: i64,
    theme: Theme,
) -> Vec<Vec<Line<'static>>> {
    let windows = windows_of(v);
    let label_w = windows
        .iter()
        .map(|(w, _)| width_of(&w.label))
        .max()
        .unwrap_or(0)
        .min(width / 3);
    windows
        .iter()
        .map(|(w, p)| window_lines(w, p, label_w, width, now_s, theme))
        .collect()
}

/// How many lines each of `v`'s windows takes in the detail pane at `width`.
fn window_heights(v: &AccountView, width: usize, now_s: i64) -> Vec<usize> {
    window_blocks(v, width, now_s, Theme::new(false))
        .iter()
        .map(Vec::len)
        .collect()
}

fn window_lines(
    w: &Window,
    p: &Pace,
    label_w: usize,
    width: usize,
    now_s: i64,
    theme: Theme,
) -> Vec<Line<'static>> {
    let style = theme.severity(words::severity(w.pct.round() as i64));
    let value = words::spend_text(w)
        .unwrap_or_else(|| format!("{:>4}", format!("{}%", w.pct.round() as i64)));
    // A label wider than the column takes rows of its own, above its value: `LABEL_ROWS` at
    // most, the rest cut.
    let mut lines: Vec<Line<'static>> = Vec::new();
    let label = if width_of(&w.label) > label_w {
        let rows = wrap_rows(&w.label, width.saturating_sub(1), LABEL_ROWS);
        lines.extend(rows.into_iter().map(|row| Line::from(format!(" {row}"))));
        ""
    } else {
        w.label.as_str()
    };
    let lead = format!(" {}  ", pad(label, label_w));
    let indent = width_of(&lead);
    // One row for the value whatever it is: a spend can be any number.
    let value = fit(&value, width.saturating_sub(indent));
    let mut parts = Vec::new();
    if let Some(at) = w.resets_at {
        let when = words::countdown(at, now_s);
        parts.push(if at > now_s {
            format!("resets in {when}")
        } else {
            when
        });
    }
    if p.ahead == Some(true) {
        let expected = p.expected_pct.map_or_else(String::new, |e| {
            format!(" (expected {}%)", e.round() as i64)
        });
        parts.push(format!("▲ ahead of pace{expected}"));
    }
    if p.rate_per_hour.is_some() {
        let hw = HistoryWindow {
            window: w.clone(),
            samples: vec![],
            pace: *p,
        };
        parts.push(words::projection_text(&hw, now_s));
    }
    let head = vec![Span::raw(lead), Span::styled(value, style)];
    lines.extend(flow(head, parts, indent, width));
    lines
}

/// The detail pane (spec §5.2): the account, each window with its reset, pace and projection,
/// the reading's age and the login's expiry, and why the usage is not current. Windows that do
/// not fit are scrolled to (`J`/`K`), which the rule says. On the rule the name gives way
/// first: the facts after it are the pane's to say.
fn detail(
    v: &AccountView,
    snap: &AccountsSnapshot,
    shown: Shown,
    width: usize,
    now_s: i64,
    theme: Theme,
) -> Vec<Line<'static>> {
    let row = &v.row;
    let mut facts = vec![
        if v.active { "active" } else { "not active" }.to_owned(),
        v.kind
            .display
            .map_or_else(|| row.kind.clone(), str::to_owned),
    ];
    if v.in_session {
        facts.push("in a session".into());
    }
    if snap.this.as_ref() == Some(&row.id) {
        facts.push(format!("{} {}", words::IN_SESSION, words::THIS));
    }
    if row.disabled {
        facts.push("disabled".into());
    }
    let blocks = window_blocks(v, width, now_s, theme);
    let heights: Vec<usize> = blocks.iter().map(Vec::len).collect();
    let range = scrolled_windows(&heights, shown.from, shown.rows);
    let more = if range != (0..blocks.len()) {
        let (from, to) = (range.start + 1, range.end);
        format!(" J K: windows {from}–{to} of {} ─", blocks.len())
    } else {
        String::new()
    };
    let head = format!("─ {} ", row.position);
    let facts = format!(" · {} ", facts.join(" · "));
    // One column of rule at least.
    let room = width.saturating_sub(width_of(&head) + width_of(&facts) + width_of(&more) + 1);
    let name = fit(&words::account_cell(row), room.max(1));
    let title = fit(
        &format!("{head}{name}{facts}"),
        width.saturating_sub(width_of(&more)),
    );
    let rule = "─".repeat(width.saturating_sub(width_of(&title) + width_of(&more)));
    let mut lines = vec![Line::from(Span::styled(
        format!("{title}{rule}{more}"),
        theme.strong(),
    ))];
    for block in blocks.into_iter().take(range.end).skip(range.start) {
        lines.extend(block);
    }
    let mut status = Vec::new();
    match v.usage.age_s {
        Some(age) => {
            let age = age_now(age, snap.read_at_s, now_s);
            status.push(format!("fetched {} ago", words::duration(age)));
        }
        None => status.push("never fetched".to_owned()),
    }
    if let Some(at) = v
        .usage
        .next_poll_at
        .filter(|&at| v.usage.status == UsageStatus::Ok && at > now_s)
    {
        status.push(format!("next poll in {}", words::countdown(at, now_s)));
    }
    if let Some(at_ms) = row.login_expires_at {
        let at = at_ms.div_euclid(1000);
        status.push(if at > now_s {
            format!("login expires in {}", words::duration(at - now_s))
        } else {
            "login expired".to_owned()
        });
    }
    lines.push(Line::from(format!(" {}", status.join(" · "))));
    if v.usage.status != UsageStatus::Ok {
        lines.push(Line::from(format!(
            " usage: {}",
            words::usage_words(&v.usage, now_s)
        )));
    }
    lines
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyCode;
    use ratatui::style::{Color, Modifier};
    use tagteam_core::Pace;
    use tagteam_engine::views::{ProviderAccounts, UsageStatus, UsageView};

    use super::*;
    use crate::msg::Msg;
    use crate::state::BannerKind;
    use crate::testutil::*;
    use crate::theme::Theme;
    use crate::update::update;

    /// §13.1's fixture: an active account ahead of pace, a stale spare, a quarantined alias.
    fn three() -> Vec<AccountView> {
        let mut live = account(1, "michael@example.com", read(120, 9.0, 77.0, true));
        live.active = true;
        let spare = account(2, "spare@example.com", read(840, 31.0, 12.0, false));
        let mut work = account(
            3,
            "w@corp.com",
            unread(UsageStatus::ReloginRequired, None, None),
        );
        work.row.alias = Some("work".into());
        work.row.quarantine_reason = Some("invalid_grant".into());
        vec![live, spare, work]
    }

    fn loaded(accounts: Vec<AccountView>) -> State {
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snapshot(accounts));
        state
    }

    #[test]
    fn a_readings_age_advances_with_the_clock_between_reads() {
        let state = loaded(three());
        let at = |seconds: i64| draw_at(&state, 120, 24, NOW_MS + seconds * 1000);
        let row_age = |lines: &[String]| {
            let row = lines
                .iter()
                .find(|l| l.contains("michael@example.com") && l.contains('%'));
            row.map(|l| l.split_whitespace().last().unwrap().to_owned())
        };
        let detail = |lines: &[String]| {
            let line = lines.iter().find(|l| l.contains("fetched "));
            line.map(|l| l.split(" ago").next().unwrap().to_owned())
        };
        let now = at(0);
        assert_eq!(row_age(&now).as_deref(), Some("2m"), "{now:?}");
        assert_eq!(detail(&now).as_deref(), Some(" fetched 2m"), "{now:?}");
        let later = at(185);
        assert_eq!(row_age(&later).as_deref(), Some("5m"), "{later:?}");
        assert_eq!(detail(&later).as_deref(), Some(" fetched 5m"), "{later:?}");
        // A clock behind the read never makes an age shorter than the reading's own.
        let earlier = at(-500);
        assert_eq!(row_age(&earlier).as_deref(), Some("2m"), "{earlier:?}");
        assert_eq!(
            detail(&earlier).as_deref(),
            Some(" fetched 2m"),
            "{earlier:?}"
        );
    }

    #[test]
    fn the_table_says_what_list_says_at_80_columns() {
        let body = body_of(&loaded(three()), 80, 24, NOW_MS);
        assert_eq!(
            &body[0..4],
            [
                "    #  ACCOUNT              5H           7D                   SPEND  AGE",
                "▌*  1  michael@example.com    9%  2h40m   77%  3d09h  ▲ pace  —      2m",
                "    2  spare@example.com     31%  2h40m   12%  3d09h          —      14m",
                "    3  work (w@corp.com)    relogin required",
            ]
        );
    }

    #[test]
    fn a_wider_terminal_adds_a_gauge_per_window() {
        let body = body_of(&loaded(three()), 120, 40, NOW_MS);
        assert_eq!(
            &body[0..4],
            [
                "    #  ACCOUNT              5H                  7D                          SPEND  AGE",
                "▌*  1  michael@example.com  █░░░░░   9%  2h40m  █████░  77%  3d09h  ▲ pace  —      2m",
                "    2  spare@example.com    ██░░░░  31%  2h40m  █░░░░░  12%  3d09h          —      14m",
                "    3  work (w@corp.com)    relogin required",
            ]
        );
    }

    #[test]
    fn the_detail_pane_shows_the_selected_account() {
        let mut accounts = three();
        accounts[0].usage.windows.as_mut().unwrap()[1].1 = Pace {
            expected_pct: Some(62.0),
            ahead: Some(true),
            rate_per_hour: Some(1.2),
            method: Some(tagteam_core::ProjectionMethod::Regression),
            exhaustion_at: Some(NOW_S + 187_200),
            will_last_to_reset: Some(false),
        };
        accounts[0].row.login_expires_at = Some((NOW_S + 6 * 86_400 + 3 * HOUR) * 1000);
        let body = body_of(&loaded(accounts), 120, 40, NOW_MS);
        let top = body.len() - 7;
        assert_eq!(
            &body[top..top + 4],
            [
                format!("─ 1 michael@example.com · active · oauth {}", "─".repeat(79)),
                " 5h    9%  resets in 2h40m".to_owned(),
                " 7d   77%  resets in 3d09h  ▲ ahead of pace (expected 62%)  +1.2 pts/h · runs out in 2d04h (regression)".to_owned(),
                " fetched 2m ago · login expires in 6d03h".to_owned(),
            ]
        );
    }

    #[test]
    fn a_row_without_a_reading_says_why_in_the_detail_pane() {
        let mut state = loaded(three());
        update(&mut state, ch('G'), NOW_MS);
        let body = body_of(&state, 80, 24, NOW_MS);
        let top = body.len() - 7;
        assert!(
            body[top].starts_with("─ 3 work (w@corp.com) · not active · oauth ──"),
            "{}",
            body[top]
        );
        assert_eq!(width_of(&body[top]), 80);
        assert_eq!(body[top + 1], " never fetched");
        assert_eq!(body[top + 2], " usage: relogin required");
    }

    #[test]
    fn the_detail_pane_counts_down_to_the_next_poll_while_it_is_ahead() {
        // Spec §5.2: `fetched 2m ago · next poll in 1m · login expires in 6d`.
        let mut accounts = three();
        accounts[0].usage.next_poll_at = Some(NOW_S + 60);
        accounts[0].row.login_expires_at = Some((NOW_S + 6 * 86_400) * 1000);
        let state = loaded(accounts);
        let top = body_of(&state, 80, 24, NOW_MS).len() - 7;
        assert_eq!(
            body_of(&state, 80, 24, NOW_MS)[top + 3],
            " fetched 2m ago · next poll in 1m · login expires in 6d00h"
        );
        // Once it has passed, a frame drawn before the next read says nothing of it.
        assert_eq!(
            body_of(&state, 80, 24, NOW_MS + 60_000)[top + 3],
            " fetched 3m ago · login expires in 5d23h"
        );
    }

    /// `read`'s 5h and 7d, then a scoped window for each of `models`, each of those ahead of
    /// pace and running out.
    fn with_windows(models: &[&str]) -> UsageView {
        let mut usage = read(120, 9.0, 77.0, false);
        let windows = usage.windows.as_mut().unwrap();
        for (i, model) in models.iter().enumerate() {
            let key = format!("scoped:{model}");
            let pct = 40.0 + i as f64;
            windows.push((
                window(&key, model, WindowKind::Scoped, pct, Some(291_630)),
                Pace {
                    expected_pct: Some(20.0),
                    ahead: Some(true),
                    rate_per_hour: Some(1.5),
                    method: Some(tagteam_core::ProjectionMethod::Average),
                    exhaustion_at: Some(NOW_S + 86_400),
                    will_last_to_reset: Some(false),
                },
            ));
        }
        usage
    }

    /// The window labels the detail pane shows, between its rule and the reading's age; a
    /// window's continuation lines start further in.
    fn pane_windows(state: &State, w: u16, h: u16) -> Vec<String> {
        let lines = draw(state, w, h);
        let rule = lines.iter().position(|l| l.starts_with("─ ")).unwrap();
        lines[rule + 1..]
            .iter()
            .take_while(|l| !l.starts_with(" fetched"))
            .filter(|l| !l.starts_with("  "))
            .map(|l| l.split_whitespace().next().unwrap_or("").to_owned())
            .collect()
    }

    /// The widest lines a detail pane draws: a long scoped-model label, a spend value, and a
    /// reset, pace and projection on one window.
    fn widest() -> UsageView {
        let mut usage = read(120, 9.0, 77.0, false);
        let windows = usage.windows.as_mut().unwrap();
        windows[1].1 = Pace {
            expected_pct: Some(62.0),
            ahead: Some(true),
            rate_per_hour: Some(1.2),
            method: Some(tagteam_core::ProjectionMethod::Regression),
            exhaustion_at: Some(NOW_S + 187_200),
            will_last_to_reset: Some(false),
        };
        let mut spend = window("spend", "spend", WindowKind::Spend, 61.7, Some(1_209_630));
        spend.detail = Some(
            r#"{"used": 123.45, "limit": 200, "currency": "EUR"}"#
                .parse()
                .unwrap(),
        );
        windows.push((spend, Pace::default()));
        let sonnet = "Claude Sonnet 4.5";
        windows.push((
            window(
                &format!("scoped:{sonnet}"),
                sonnet,
                WindowKind::Scoped,
                41.0,
                Some(291_630),
            ),
            Pace {
                expected_pct: Some(20.0),
                ahead: Some(true),
                rate_per_hour: Some(1.5),
                method: Some(tagteam_core::ProjectionMethod::Average),
                exhaustion_at: Some(NOW_S + 86_400),
                will_last_to_reset: Some(false),
            },
        ));
        usage
    }

    #[test]
    fn a_window_too_wide_for_80_columns_wraps_its_parts_under_its_value() {
        // Spec §5.1: 80×24 is enough; spec §5.2: each window's reset, pace and projection.
        let state = loaded(vec![account(1, "a@x.co", widest())]);
        let lines = draw(&state, 80, 24);
        let rule = lines.iter().position(|l| l.starts_with("─ ")).unwrap();
        let under = " ".repeat(20);
        assert_eq!(
            lines[rule + 1..rule + 8],
            [
                format!(" {:<17}  {:>4}  resets in 2h40m", "5h", "9%"),
                format!(
                    " {:<17}  {:>4}  resets in 3d09h  ▲ ahead of pace (expected 62%)",
                    "7d", "77%"
                ),
                format!("{under}+1.2 pts/h · runs out in 2d04h (regression)"),
                format!(" {:<17}  €123.45 of €200  resets in 14d00h", "spend"),
                format!(
                    " {:<17}  {:>4}  resets in 3d09h  ▲ ahead of pace (expected 20%)",
                    "Claude Sonnet 4.5", "41%"
                ),
                format!("{under}+1.5 pts/h · runs out in 1d00h (average)"),
                " fetched 2m ago".to_owned(),
            ]
        );
        assert!(lines.iter().all(|l| width_of(l) <= 80 && !l.contains('…')));
        // A wide terminal has room for each window on one line.
        let wide = draw(&state, 120, 40);
        let rule = wide.iter().position(|l| l.starts_with("─ ")).unwrap();
        assert_eq!(
            wide[rule + 2],
            format!(
                " {:<17}  {:>4}  resets in 3d09h  ▲ ahead of pace (expected 62%)  +1.2 pts/h · runs out in 2d04h (regression)",
                "7d", "77%"
            )
        );
        assert!(
            wide[rule + 1..rule + 5]
                .iter()
                .all(|l| !l.starts_with("  ")),
            "no continuation lines: {wide:?}"
        );
    }

    /// An 80-column model name: any non-empty `display_name` is a label (the provider's §8.2).
    const OVERSIZED: &str =
        "Claude Opus 4.5 with extended thinking, for long-running agentic coding sessions";

    #[test]
    fn an_oversized_label_goes_above_its_value_and_the_column_stays_capped() {
        // The label column is a third of the pane at most, so every value and what follows it
        // keep room; the oversized label is not cut, but takes rows of its own.
        let mut usage = with_windows(&["Opus"]);
        usage.windows.as_mut().unwrap()[2].0.label = OVERSIZED.to_owned();
        let state = loaded(vec![account(1, "a@x.co", usage)]);
        let lines = draw(&state, 80, 40);
        let rule = lines.iter().position(|l| l.starts_with("─ ")).unwrap();
        let age = lines
            .iter()
            .position(|l| l.starts_with(" fetched"))
            .unwrap();
        let pane = &lines[rule + 1..age];
        assert!(
            pane.iter().all(|l| width_of(l) <= 80 && !l.contains('…')),
            "{pane:?}"
        );
        assert!(
            pane[0].starts_with(&format!(" {:<26}    9%", "5h")),
            "{pane:?}"
        );
        let label = pane
            .iter()
            .position(|l| l.starts_with(" Claude Opus"))
            .unwrap();
        assert_eq!(
            [pane[label].trim(), pane[label + 1].trim()].join(" "),
            OVERSIZED,
            "the whole label, on its own rows"
        );
        assert!(
            pane[label + 2].starts_with(&format!(" {:26}   40%  resets in 3d09h", "")),
            "{pane:?}"
        );
        let rest = pane[label + 2..].join(" ");
        assert!(rest.contains("▲ ahead of pace (expected 20%)"), "{rest}");
        assert!(
            rest.contains("+1.5 pts/h · runs out in 1d00h (average)"),
            "{rest}"
        );
    }

    /// A 1,000-character model name, as the provider accepts any non-empty `display_name`.
    fn thousand() -> String {
        "Claude ".repeat(143).trim_end().to_owned()
    }

    #[test]
    fn a_1000_character_label_leaves_its_window_reachable_at_80x24_under_a_full_banner() {
        // The smallest pane 80×24 gives: a 3-row banner, a 3-row toast, and a trouble line.
        let mut usage = with_windows(&["Opus"]);
        usage.status = UsageStatus::Unavailable;
        usage.error = Some("http-429".into());
        usage.windows.as_mut().unwrap()[2].0.label = thousand();
        let mut state = loaded(vec![account(1, "a@x.co", usage)]);
        state.raise_banner(
            BannerKind::Refusal,
            ["the store could not be read"; 12].join("; "),
        );
        state.toast(["Switched to a@x.co (position 1)"; 8].join(" "), NOW_MS);
        for _ in 0..3 {
            update(&mut state, ch('J'), NOW_MS);
        }
        let lines = draw(&state, 80, 24);
        assert!(lines.iter().all(|l| width_of(l) <= 80), "{lines:?}");
        let label = lines
            .iter()
            .position(|l| l.starts_with(" Claude Claude"))
            .unwrap_or_else(|| panic!("{lines:?}"));
        assert!(
            lines[label + 1].ends_with('…'),
            "two rows at most: {lines:?}"
        );
        assert!(
            lines[label + 2].starts_with(&format!(" {:26}   40%  resets in 3d09h", "")),
            "{lines:?}"
        );
        let rest = lines[label + 2..].join(" ");
        assert!(rest.contains("▲ ahead of pace (expected 20%)"), "{lines:?}");
        assert!(
            rest.contains("+1.5 pts/h · runs out in 1d00h (average)"),
            "{lines:?}"
        );
        assert!(rest.contains(" usage: unavailable (http-429)"), "{lines:?}");
    }

    #[test]
    fn the_worst_live_stack_at_80_by_24_still_shows_a_long_labelled_window_whole() {
        // The header, the auto line, a 3-row banner and a 3-row toast leave a 16-row body; the
        // window's label, value, reset, pace, rate and trouble all fit in what is left.
        let mut usage = with_windows(&["Opus"]);
        usage.status = UsageStatus::Unavailable;
        usage.error = Some("http-429".into());
        usage.windows.as_mut().unwrap()[2].0.label = thousand();
        let mut state = loaded(vec![account(1, "a@x.co", usage)]);
        let mut locked = daemon_job("claude-code", "auto", true);
        locked.backoff = Some(tagteam_daemon::protocol::StatusBackoff {
            reason: "keychain".into(),
            until: "2026-09-21T15:05:00Z".into(),
        });
        go_live(&mut state, daemon_status(vec![locked], vec![]));
        let ev = tagteam_daemon::protocol::Event::Auto {
            kind: "error".into(),
            provider: "claude-code".into(),
            raw: serde_json::json!({"event": "error", "ts": "2026-09-21T14:04:00Z", "message": "x".repeat(300)}),
        };
        update(&mut state, Msg::Live(crate::live::Live::Event(ev)), NOW_MS);
        state.raise_banner(
            BannerKind::Refusal,
            ["the store could not be read"; 12].join("; "),
        );
        state.toast(["Switched to a@x.co (position 1)"; 8].join(" "), NOW_MS);
        for _ in 0..3 {
            update(&mut state, ch('J'), NOW_MS);
        }
        let lines = draw(&state, 80, 24);
        assert_eq!(lines.len(), 24, "{lines:?}");
        assert!(lines.iter().all(|l| width_of(l) <= 80), "{lines:?}");
        // The stack: the header, the auto line, the banner (3), a 16-row body, the toast (3).
        assert!(
            lines[0].ends_with("! keychain locked 15:05   ? keys"),
            "{lines:?}"
        );
        assert!(lines[1].starts_with(" ! 14:04  xxx"), "{lines:?}");
        assert!(lines[1].ends_with('…'), "{lines:?}");
        assert!(
            lines[2..5]
                .iter()
                .all(|l| l.contains("the store could not be")),
            "{lines:?}"
        );
        assert!(
            lines[5].contains("ACCOUNT"),
            "the body starts at row 5: {lines:?}"
        );
        assert!(
            lines[21..]
                .iter()
                .all(|l| l.contains("a@x.co (position 1)") || l.starts_with("1) Switched")),
            "{lines:?}"
        );
        let label = lines
            .iter()
            .position(|l| l.starts_with(" Claude Claude"))
            .unwrap_or_else(|| panic!("{lines:?}"));
        assert!(lines[label + 1].starts_with(" Claude Claude"), "{lines:?}");
        assert!(lines[label + 2].contains("40%"), "{lines:?}");
        assert!(lines[label + 2].contains("resets in 3d09h"), "{lines:?}");
        let rest = lines[label + 2..].join(" ");
        assert!(rest.contains("▲ ahead of pace (expected 20%)"), "{lines:?}");
        assert!(
            rest.contains("+1.5 pts/h · runs out in 1d00h (average)"),
            "{lines:?}"
        );
        assert!(rest.contains(" usage: unavailable (http-429)"), "{lines:?}");
    }

    #[test]
    fn a_window_takes_seven_rows_at_most_whatever_its_words() {
        // Two label rows, the value's, and a row for each of reset, pace, rate and where it
        // leads: the bound the pane's height is held to (R20).
        let mut w = window(
            "spend",
            &thousand(),
            WindowKind::Spend,
            99.0,
            Some(400 * 86_400),
        );
        w.detail = Some(
            r#"{"used": 1e40, "limit": 1e41, "currency": "EUR"}"#
                .parse()
                .unwrap(),
        );
        let p = Pace {
            expected_pct: Some(100.0),
            ahead: Some(true),
            rate_per_hour: Some(12_345.6),
            method: Some(tagteam_core::ProjectionMethod::Regression),
            exhaustion_at: Some(NOW_S - 86_400 * 400),
            will_last_to_reset: Some(false),
        };
        for width in [80, 120] {
            let lines = window_lines(&w, &p, width / 3, width, NOW_S, Theme::new(false));
            assert!(lines.len() <= 7, "{width}: {} rows", lines.len());
            for line in &lines {
                let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
                assert!(width_of(&text) <= width, "{width}: {text}");
            }
        }
        // The pane holds it whole at 80×24 under a 3-row banner, a 3-row toast and a trouble
        // line: it takes rows from the table for its tallest window.
        let mut usage = read(120, 9.0, 77.0, false);
        usage.status = UsageStatus::Unavailable;
        usage.error = Some("http-429".into());
        usage.windows.as_mut().unwrap().push((w, p));
        let mut state = loaded(vec![account(1, "a@x.co", usage)]);
        state.raise_banner(
            BannerKind::Refusal,
            ["the store could not be read"; 12].join("; "),
        );
        state.toast(["Switched to a@x.co (position 1)"; 8].join(" "), NOW_MS);
        for _ in 0..2 {
            update(&mut state, ch('J'), NOW_MS);
        }
        let lines = draw(&state, 80, 24);
        let label = lines.iter().position(|l| l.starts_with(" Claude")).unwrap();
        let age = lines
            .iter()
            .position(|l| l.starts_with(" fetched"))
            .unwrap();
        let block = lines[label..age].join("\n");
        assert!(block.contains("of €100…"), "{lines:?}");
        assert!(
            block.contains("should have run out 400d00h ago (regression)"),
            "{lines:?}"
        );
        assert_eq!(lines[age + 1], " usage: unavailable (http-429)");
    }

    #[test]
    fn a_resize_clamps_the_detail_scroll_so_the_next_key_moves_at_once() {
        let models = [
            "M1", "M2", "M3", "M4", "M5", "M6", "M7", "M8", "M9", "M10", "M11",
        ];
        let mut state = loaded(vec![account(1, "a@x.co", with_windows(&models))]);
        let rule = |state: &State, h: u16| {
            draw(state, 80, h)
                .into_iter()
                .find(|l| l.starts_with("─ "))
                .unwrap()
        };
        for _ in 0..10 {
            update(&mut state, ch('J'), NOW_MS);
        }
        assert!(rule(&state, 24).ends_with(" J K: windows 10–13 of 13 ─"));
        // Taller: the last window shows from further up, and `K` moves from what shows. At 32
        // rows the body has the 28 rows 30 gave it before the auto line and the rule.
        update(
            &mut state,
            Msg::Resize {
                width: 80,
                height: 32,
            },
            NOW_MS,
        );
        assert!(rule(&state, 32).ends_with(" J K: windows 8–13 of 13 ─"));
        update(&mut state, ch('K'), NOW_MS);
        assert!(
            rule(&state, 32).ends_with(" J K: windows 7–12 of 13 ─"),
            "{}",
            rule(&state, 32)
        );
    }

    #[test]
    fn the_rule_cuts_the_name_before_the_facts() {
        // Spec §5.2: whether the account is session-owned is the detail pane's to say.
        let long = "a".repeat(52) + "@example.com";
        let mut v = account(1, &long, read(60, 9.0, 12.0, false));
        v.in_session = true;
        let lines = draw(&loaded(vec![v]), 80, 24);
        let rule = lines.iter().find(|l| l.starts_with("─ ")).unwrap();
        assert!(rule.starts_with("─ 1 aaaa"), "{rule}");
        assert!(
            rule.contains("… · not active · oauth · in a session ─"),
            "{rule}"
        );
        assert_eq!(width_of(rule), 80);
    }

    #[test]
    fn the_detail_pane_grows_to_show_every_window() {
        // Spec §5.2: each relevant window, with its reset, pace and projection. 5h, 7d and
        // three scoped windows are five: one more than the pane's least height holds.
        let state = loaded(vec![account(
            1,
            "a@x.co",
            with_windows(&["Opus", "Sonnet", "Haiku"]),
        )]);
        for (w, h) in [(80, 24), (120, 40)] {
            assert_eq!(
                pane_windows(&state, w, h),
                ["5h", "7d", "Opus", "Sonnet", "Haiku"],
                "{w}×{h}"
            );
        }
        let lines = draw(&state, 120, 40);
        let haiku = lines.iter().find(|l| l.starts_with(" Haiku")).unwrap();
        assert_eq!(
            haiku.trim_end(),
            " Haiku    42%  resets in 3d09h  ▲ ahead of pace (expected 20%)  +1.5 pts/h · runs out in 1d00h (average)"
        );
        assert!(
            !lines.iter().any(|l| l.contains("J K")),
            "every window fits: nothing to scroll"
        );
    }

    #[test]
    fn windows_beyond_the_pane_scroll_into_view_with_shift_j_and_k() {
        // 13 windows, each scoped one two lines at 80 columns: more than half of 80×24's body
        // holds, so the pane scrolls them, by whole windows.
        let models = [
            "M1", "M2", "M3", "M4", "M5", "M6", "M7", "M8", "M9", "M10", "M11",
        ];
        let mut state = loaded(vec![
            account(1, "a@x.co", with_windows(&models)),
            account(2, "b@x.co", with_windows(&models)),
        ]);
        let first: Vec<&str> = vec!["5h", "7d", "M1", "M2", "M3"];
        assert_eq!(pane_windows(&state, 80, 24), first);
        // Whole windows only: M3's projection is the last line before the reading's age.
        let lines = draw(&state, 80, 24);
        let age = lines
            .iter()
            .position(|l| l.starts_with(" fetched"))
            .unwrap();
        assert!(lines[age - 2].starts_with(" M3 "), "{lines:?}");
        assert!(lines[age - 1].trim_start().starts_with("+1.5 pts/h"));
        let rule = |state: &State| {
            draw(state, 80, 24)
                .into_iter()
                .find(|l| l.starts_with("─ "))
                .unwrap()
        };
        assert!(
            rule(&state).ends_with(" J K: windows 1–5 of 13 ─"),
            "{}",
            rule(&state)
        );
        assert_eq!(width_of(&rule(&state)), 80);
        for _ in 0..10 {
            update(&mut state, ch('J'), NOW_MS);
        }
        assert_eq!(
            pane_windows(&state, 80, 24),
            ["M8", "M9", "M10", "M11"],
            "J stops at the last window"
        );
        assert!(rule(&state).ends_with(" J K: windows 10–13 of 13 ─"));
        update(&mut state, ch('K'), NOW_MS);
        assert_eq!(pane_windows(&state, 80, 24), ["M7", "M8", "M9", "M10"]);
        // Another account's detail starts at its first window.
        update(&mut state, ch('j'), NOW_MS);
        assert_eq!(pane_windows(&state, 80, 24), first);
        // A wider and taller terminal shows them all, a line each.
        assert_eq!(pane_windows(&state, 120, 40).len(), 13);
        // Help lists the keys.
        update(&mut state, ch('?'), NOW_MS);
        let help = draw(&state, 80, 24);
        assert!(
            help.iter()
                .any(|l| l.contains("J K") && l.contains("scroll the detail's windows")),
            "{help:?}"
        );
    }

    #[test]
    fn the_active_marker_session_column_and_this_follow_list() {
        let mut accounts = three();
        accounts[1].in_session = true;
        let mut snap = snapshot(accounts);
        snap.this = Some(id(2));
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snap);
        let body = body_of(&state, 80, 24, NOW_MS);
        assert_eq!(
            &body[0..3],
            [
                "    #    ACCOUNT              5H           7D                   SPEND  AGE",
                "▌*  1    michael@example.com    9%  2h40m   77%  3d09h  ▲ pace  —      2m",
                "    2 ▶  spare@example.com     31%  2h40m   12%  3d09h          —      14m  this",
            ]
        );
    }

    #[test]
    fn percentages_and_gauges_take_their_severity_colour_or_bold_without() {
        let hot = account(1, "a@x.co", read(60, 95.0, 77.0, false));
        for color in [true, false] {
            let mut state = loaded(vec![hot.clone()]);
            state.theme = Theme::new(color);
            let buf = draw_buffer(&state, 120, 40, NOW_MS);
            let (x, y) = locate(&buf, "95%").unwrap();
            let (gx, gy) = locate(&buf, "██████").unwrap();
            let (wx, wy) = locate(&buf, "77%").unwrap();
            if color {
                assert_eq!(buf[(x, y)].fg, Color::Red);
                assert_eq!(buf[(gx, gy)].fg, Color::Red);
                assert_eq!(buf[(wx, wy)].fg, Color::Yellow);
            } else {
                for (x, y) in [(x, y), (gx, gy), (wx, wy)] {
                    assert_eq!(buf[(x, y)].fg, Color::Reset);
                    assert!(buf[(x, y)].modifier.contains(Modifier::BOLD));
                }
                // §4.1: crit is underlined as well, so it reads apart from warn without colour.
                for (x, y) in [(x, y), (gx, gy)] {
                    assert!(buf[(x, y)].modifier.contains(Modifier::UNDERLINED));
                }
                assert!(!buf[(wx, wy)].modifier.contains(Modifier::UNDERLINED));
            }
        }
    }

    #[test]
    fn the_selected_row_has_the_bar_and_a_bold_name_and_no_reverse_video() {
        for theme in TIERS {
            let mut state = loaded(three());
            state.theme = theme;
            let buf = draw_buffer(&state, 80, 24, NOW_MS);
            // The body's first row is the column heads, its second the selected account.
            let y = crate::view::areas(&state, buf.area).body.y + 1;
            assert!(
                roles(&buf, theme, y).starts_with(&canon(theme, "a")),
                "{theme:?}"
            );
            assert_eq!(buf[(0, y)].symbol(), "▌", "{theme:?}");
            assert_eq!(buf[(0, y + 1)].symbol(), " ", "{theme:?}");
            let (x, ny) = locate(&buf, "michael@example.com").unwrap();
            assert_eq!(ny, y);
            let (ox, oy) = locate(&buf, "spare@example.com").unwrap();
            for i in 0..19 {
                assert!(
                    buf[(x + i, y)].modifier.contains(Modifier::BOLD),
                    "{theme:?}"
                );
            }
            assert!(
                !buf[(ox, oy)].modifier.contains(Modifier::BOLD),
                "{theme:?}"
            );
            for x in 0..80 {
                for row in [y, y + 1] {
                    assert!(
                        !buf[(x, row)].modifier.contains(Modifier::REVERSED),
                        "{theme:?} {x},{row}"
                    );
                }
                let tint = theme.sel_bg().bg.unwrap_or(Color::Reset);
                assert_eq!(buf[(x, y)].bg, tint, "{theme:?} {x}");
                assert_eq!(buf[(x, y + 1)].bg, Color::Reset, "{theme:?} {x}");
            }
        }
    }

    #[test]
    fn a_selected_active_row_bolds_its_name_alone_and_draws_the_marker_in_accent() {
        for theme in TIERS {
            let mut state = loaded(three());
            state.theme = theme;
            let buf = draw_buffer(&state, 80, 24, NOW_MS);
            let y = crate::view::areas(&state, buf.area).body.y + 1;
            assert_eq!(buf[(1, y)].symbol(), "*", "{theme:?}");
            let accent = theme.role(Role::Accent);
            assert_eq!(
                buf[(1, y)].fg,
                accent.fg.unwrap_or(Color::Reset),
                "{theme:?}"
            );
            let (name, _) = locate(&buf, "michael@example.com").unwrap();
            // Without colour, accent is bold, so the marker is the one other bold cell there.
            let marker_bold = theme == TIERS[0];
            assert_eq!(
                buf[(1, y)].modifier.contains(Modifier::BOLD),
                marker_bold,
                "{theme:?}"
            );
            // Past the marker, the name's cells are the one bold run on the row. Without colour a
            // warn percentage is bold as well, so the row is read up to the window columns.
            let end = if theme == TIERS[0] { 30 } else { 80 };
            let runs: Vec<u16> = (2..end)
                .filter(|x| buf[(*x, y)].modifier.contains(Modifier::BOLD))
                .collect();
            assert_eq!(runs.first(), Some(&name), "{theme:?} {runs:?}");
            assert!(
                runs.windows(2).all(|p| p[1] == p[0] + 1),
                "{theme:?} {runs:?}"
            );
        }
    }

    /// Review Focus 2: names wider than their column are cut by display width, never by bytes,
    /// and the table never overflows or panics at 80 columns.
    #[test]
    fn wide_names_are_cut_by_display_width_and_never_overflow() {
        let long = "a".repeat(52) + "@example.com";
        let mut cjk = account(2, "山田太郎@x.co", read(60, 9.0, 12.0, false));
        cjk.row.alias = Some("山田さんの仕事用アカウント😀😀😀😀".into());
        let accounts = vec![
            account(1, &long, read(60, 9.0, 12.0, false)),
            cjk,
            account(
                3,
                "😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀@x.co",
                read(60, 9.0, 12.0, false),
            ),
        ];
        for w in [80, 100, 120] {
            let buf = draw_buffer(&loaded(accounts.clone()), w, 24, NOW_MS);
            let lines = text_of(&buf);
            for line in &lines {
                assert!(width_of(line) <= usize::from(w), "{w}: {line}");
            }
            // Every row's window columns still line up with the header's.
            let body = body_of(&loaded(accounts.clone()), w, 24, NOW_MS);
            let col = body[0].find("5H").unwrap();
            let head = width_of(&body[0][..col]);
            for row in &body[1..4] {
                let pct = row.find("9%").unwrap();
                assert_eq!(width_of(&row[..pct]), head + 2, "{w}: {row}");
            }
        }
        let narrow = body_of(&loaded(accounts), 80, 24, NOW_MS);
        assert!(narrow[1].contains("aaaa…"), "{}", narrow[1]);
        assert!(
            narrow[2].starts_with("    2  山田さんの仕"),
            "{}",
            narrow[2]
        );
    }

    /// Review Focus 3: with no store and no accounts, the empty state.
    #[test]
    fn no_accounts_shows_the_empty_state() {
        let state = loaded(vec![]);
        let body = body_of(&state, 80, 24, NOW_MS);
        assert_eq!(
            body[0],
            " No accounts yet. Log in with claude, then press A to add the login."
        );
        assert_eq!(state.accounts.selected(), None);
        assert!(state.accounts.shown_ids().is_empty());
        let unloaded = body_of(&fresh(Screen::Accounts), 80, 24, NOW_MS);
        assert_eq!(unloaded[0], " loading…");
    }

    /// Review Focus 4: the table scrolls so the selected row is always visible, and `j`/`k`
    /// past either end stops there.
    #[test]
    fn more_accounts_than_fit_scroll_with_the_selection() {
        let mut state = loaded(many(30));
        // 80×25: 22 body rows (80×24's before the auto line), 7 for the detail, 1 for the
        // header: 14 account rows.
        state.size = (80, 25);
        let shown = |state: &State| {
            body_of(state, 80, 25, NOW_MS)[1..15]
                .iter()
                .map(|l| {
                    l.split_whitespace()
                        .find(|w| w.contains('@'))
                        .unwrap()
                        .to_owned()
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(shown(&state)[0], "a1@x.co");
        update(&mut state, ch('k'), NOW_MS);
        assert_eq!(
            state.accounts.selected().unwrap().position,
            1,
            "k stops at the top"
        );
        for _ in 0..14 {
            update(&mut state, ch('j'), NOW_MS);
        }
        assert_eq!(state.accounts.selected().unwrap().position, 15);
        assert_eq!(shown(&state).first().unwrap(), "a2@x.co");
        assert_eq!(shown(&state).last().unwrap(), "a15@x.co");
        for _ in 0..40 {
            update(&mut state, key(KeyCode::Down), NOW_MS);
        }
        assert_eq!(
            state.accounts.selected().unwrap().position,
            30,
            "j stops at the end"
        );
        assert_eq!(shown(&state).last().unwrap(), "a30@x.co");
        // Going back up scrolls only once the selection reaches the top line.
        for _ in 0..13 {
            update(&mut state, ch('k'), NOW_MS);
        }
        assert_eq!(shown(&state).first().unwrap(), "a17@x.co");
        update(&mut state, ch('k'), NOW_MS);
        assert_eq!(shown(&state).first().unwrap(), "a16@x.co");
        update(&mut state, key(KeyCode::Home), NOW_MS);
        assert_eq!(shown(&state).first().unwrap(), "a1@x.co");
        // A smaller terminal still shows the selected row.
        update(&mut state, key(KeyCode::End), NOW_MS);
        update(
            &mut state,
            Msg::Resize {
                width: 80,
                height: 24,
            },
            NOW_MS,
        );
        let small = draw(&state, 80, 24);
        assert!(small.iter().any(|l| l.contains("a30@x.co")));
    }

    #[test]
    fn a_resize_keeps_the_tables_scroll_in_range_and_the_next_key_moves_at_once() {
        // At 80×25 the body has the 22 rows 80×24 gave it before the auto line.
        let mut state = loaded(many(30));
        state.size = (80, 25);
        update(&mut state, key(KeyCode::End), NOW_MS);
        let first = |state: &State, h: u16| {
            body_of(state, 80, h, NOW_MS)[1]
                .split_whitespace()
                .find(|w| w.contains('@'))
                .unwrap()
                .to_owned()
        };
        assert_eq!(first(&state, 25), "a17@x.co");
        // Tall enough for every row: the table shows from the top, never blank rows.
        update(
            &mut state,
            Msg::Resize {
                width: 80,
                height: 60,
            },
            NOW_MS,
        );
        assert_eq!(first(&state, 60), "a1@x.co");
        update(&mut state, ch('k'), NOW_MS);
        assert_eq!(state.accounts.selected().unwrap().position, 29);
        assert_eq!(first(&state, 60), "a1@x.co");
        // And back: the selected row is in view at once.
        update(
            &mut state,
            Msg::Resize {
                width: 80,
                height: 24,
            },
            NOW_MS,
        );
        assert!(draw(&state, 80, 24).iter().any(|l| l.contains("a29@x.co")));
    }

    /// Review Focus 5: the selection follows the account, not the row.
    #[test]
    fn the_selection_follows_the_account_across_a_refresh() {
        let mut st = AccountsState::default();
        let mut accounts = many(4);
        st.apply(snapshot(accounts.clone()));
        st.step(2);
        assert_eq!(st.selected().unwrap().id, id(3));
        // Reordered: 3 moved to the top.
        let moved = accounts.remove(2);
        accounts.insert(0, moved);
        st.apply(snapshot(accounts.clone()));
        assert_eq!(st.selected().unwrap().id, id(3));
        assert_eq!(st.index(), Some(0));
        // Gone: the row now at its old place.
        st.step(1);
        assert_eq!(st.selected().unwrap().id, id(1));
        accounts.remove(1);
        st.apply(snapshot(accounts.clone()));
        assert_eq!(st.selected().unwrap().id, id(2), "{:?}", st.selected());
        // The last row gone: the new last row.
        st.step(10);
        let last = st.selected().unwrap().id;
        accounts.retain(|v| v.row.id != last);
        st.apply(snapshot(accounts.clone()));
        assert_eq!(st.selected().unwrap().id, accounts.last().unwrap().row.id);
        // Everything gone, then back: the first row.
        st.apply(snapshot(vec![]));
        assert_eq!(st.selected(), None);
        st.apply(snapshot(many(2)));
        assert_eq!(st.selected().unwrap().id, id(1));
    }

    #[test]
    fn the_selection_names_the_account_as_last_read() {
        let mut accounts = many(2);
        accounts[1].row.position = 7;
        let mut state = loaded(accounts);
        update(&mut state, ch('j'), NOW_MS);
        assert_eq!(
            state.accounts.selected(),
            Some(AccountRef {
                id: id(2),
                provider: cc(),
                position: 7,
            })
        );
        assert_eq!(state.accounts.shown_ids(), [id(1), id(2)]);
    }

    #[test]
    fn enter_opens_history_for_the_selected_account() {
        let mut state = loaded(many(3));
        update(&mut state, ch('j'), NOW_MS);
        update(&mut state, key(KeyCode::Enter), NOW_MS);
        assert_eq!(state.screen, Screen::History);
        assert_eq!(state.history.account(), Some(&id(2)));
        let mut empty = loaded(vec![]);
        let cmds = update(&mut empty, key(KeyCode::Enter), NOW_MS);
        assert_eq!((empty.screen, cmds), (Screen::Accounts, vec![]));
    }

    #[test]
    fn several_providers_are_grouped_under_their_names() {
        let mut other = account(1, "o@x.co", read(60, 50.0, 60.0, false));
        other.row.provider = ProviderId::new("fake-agent");
        other.row.id = AccountId::from_string("other-1");
        let mut snap = snapshot(many(1));
        snap.providers.push(ProviderAccounts {
            provider: ProviderId::new("fake-agent"),
            active_position: None,
            accounts: vec![other],
            threshold: 90.0,
            strategy: tagteam_core::autoswitch::Strategy::Best,
            names: tagteam_engine::views::ProviderNames {
                display_name: "FakeAgent",
                launch_command: "fakeagent",
            },
        });
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snap);
        let body = body_of(&state, 80, 24, NOW_MS);
        assert_eq!(body[0], " claude-code");
        assert!(body[1].starts_with("    #  ACCOUNT"), "{}", body[1]);
        assert!(body[2].contains("a1@x.co"));
        assert_eq!(body[3], " fake-agent");
        assert!(body[4].starts_with("    #  ACCOUNT"));
        assert!(body[5].contains("o@x.co"));
        update(&mut state, ch('j'), NOW_MS);
        assert_eq!(
            state.accounts.selected().unwrap().provider,
            ProviderId::new("fake-agent")
        );
    }

    #[test]
    fn a_run_shell_or_an_unmanaged_live_login_raises_a_banner() {
        let mut snap = snapshot(many(1));
        snap.unmanaged_live = Some("s@x.co".into());
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snap.clone());
        assert_eq!(
            draw(&state, 80, 24)[2],
            " ! Live: s@x.co (not managed by tagteam); press A to add it"
        );
        snap.in_run_shell = true;
        state.accounts.apply(snap);
        assert_eq!(
            draw(&state, 80, 24)[2],
            " ! inside a `tagteam run` session: account changes are refused here"
        );
        state.raise_banner(BannerKind::Refusal, "the store is locked");
        assert_eq!(draw(&state, 80, 24)[2], " ! the store is locked");
    }

    #[test]
    fn a_long_unmanaged_live_login_banner_wraps_and_still_says_press_a() {
        // Spec §5.1: at 80 columns the whole banner shows, its instruction included.
        let long = "a".repeat(48) + "@example.com";
        let mut snap = snapshot(many(1));
        snap.unmanaged_live = Some(long.clone());
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snap);
        let lines = draw(&state, 80, 24);
        assert_eq!(
            lines[2..4],
            [
                format!(" ! Live: {long} (not"),
                "   managed by tagteam); press A to add it".to_owned(),
            ]
        );
        let body = body_of(&state, 80, 24, NOW_MS);
        assert!(body[0].starts_with("    #  ACCOUNT"), "{}", body[0]);
    }

    #[test]
    fn the_api_key_and_disabled_notes_follow_list() {
        let mut key = account(
            1,
            "api-key-1@token.local",
            unread(UsageStatus::ApiKey, None, None),
        );
        key.kind = API_KEY;
        let mut off = account(2, "b@x.co", read(60, 9.0, 12.0, false));
        off.row.disabled = true;
        let body = body_of(&loaded(vec![key, off]), 80, 24, NOW_MS);
        assert!(body[1].ends_with("api key"), "{}", body[1]);
        assert!(body[2].ends_with("disabled"), "{}", body[2]);
    }

    #[test]
    fn the_detail_pane_announces_no_poll_the_status_rules_out() {
        // A quarantined account keeps its stored plan, but no poll comes (core §8.3).
        let mut accounts = three();
        accounts[2].usage.next_poll_at = Some(NOW_S + 180);
        let mut state = loaded(accounts);
        update(&mut state, ch('G'), NOW_MS);
        let lines = draw(&state, 80, 24);
        let body = body_of(&state, 80, 24, NOW_MS);
        assert_eq!(body[body.len() - 7 + 1], " never fetched");
        assert!(!lines.iter().any(|l| l.contains("next poll")), "{lines:?}");
        // A backoff later than the plan: the usage line's retry is the only time given.
        let mut backed_off = read(120, 9.0, 77.0, false);
        backed_off.status = UsageStatus::Unavailable;
        backed_off.error = Some("http-429".into());
        backed_off.retry_at = Some(NOW_S + 900);
        backed_off.next_poll_at = Some(NOW_S + 120);
        let lines = draw(&loaded(vec![account(1, "a@x.co", backed_off)]), 80, 24);
        assert!(!lines.iter().any(|l| l.contains("next poll")), "{lines:?}");
        assert!(
            lines
                .iter()
                .any(|l| l == " usage: unavailable (http-429, retry 15m)"),
            "{lines:?}"
        );
    }

    /// A spend, a scoped model window, a disabled row and a failing one: more than 80 columns
    /// hold with their notes.
    fn crowded(error: &str) -> Vec<AccountView> {
        let mut usage = with_windows(&["Claude Sonnet 4.5"]);
        let spend = window("spend", "spend", WindowKind::Spend, 61.7, Some(1_209_630));
        usage
            .windows
            .as_mut()
            .unwrap()
            .push((spend, Pace::default()));
        let mut off = account(2, "b@x.co", usage.clone());
        off.row.disabled = true;
        let mut failing = usage.clone();
        failing.status = UsageStatus::Unavailable;
        failing.error = Some(error.into());
        failing.retry_at = Some(NOW_S + 900);
        vec![
            account(1, "a@x.co", usage),
            off,
            account(3, "c@x.co", failing),
        ]
    }

    #[test]
    fn columns_give_way_to_the_notes_at_80_columns() {
        let lines = draw(&loaded(crowded("http-429")), 80, 24);
        assert!(lines.iter().all(|l| width_of(l) <= 80), "{lines:?}");
        let body = body_of(&loaded(crowded("http-429")), 80, 24, NOW_MS);
        let head = &body[0];
        assert!(head.contains("5H") && head.contains("7D"), "{head}");
        assert!(head.ends_with("AGE"), "{head}");
        assert!(!head.contains("SONNET"), "{head}");
        assert!(body[2].ends_with("disabled"), "{}", body[2]);
        assert!(
            body[3].ends_with("unavailable (http-429, retry 15m)"),
            "{}",
            body[3]
        );
        // At 120 columns every column and every note fit.
        let wide = body_of(&loaded(crowded("http-429")), 120, 40, NOW_MS);
        assert!(wide[0].contains("CLAUDE SONNET 4.5"), "{}", wide[0]);
        assert!(
            wide[0].contains("SPEND") && wide[0].ends_with("AGE"),
            "{}",
            wide[0]
        );
        assert!(wide[2].ends_with("disabled"), "{}", wide[2]);
        assert!(
            wide[3].ends_with("unavailable (http-429, retry 15m)"),
            "{}",
            wide[3]
        );
    }

    #[test]
    fn a_note_too_long_for_its_room_ends_in_an_ellipsis() {
        let long = "x".repeat(70);
        let lines = draw(&loaded(crowded(&long)), 80, 24);
        assert!(lines.iter().all(|l| width_of(l) <= 80), "{lines:?}");
        let body = body_of(&loaded(crowded(&long)), 80, 24, NOW_MS);
        assert!(body[3].ends_with('…'), "{}", body[3]);
        assert!(body[3].contains("unavailable (xxx"), "{}", body[3]);
    }
}
