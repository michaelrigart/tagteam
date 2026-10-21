//! The Accounts screen (redesign §6): the table of accounts (§6.3), and the selected account's
//! detail below it.

mod status;
mod table;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use tagteam_core::{AccountId, Pace, Window};
use tagteam_engine::switch::LiveLogin;
use tagteam_engine::views::{AccountView, HistoryWindow, ProviderAccounts, UsageStatus};
use tagteam_engine::words;

use self::table::{NAME_SPAN, Table, age_now};

use crate::daemon;
use crate::host::AccountRef;
use crate::msg::Cmd;
use crate::source::AccountsSnapshot;
use crate::state::{Screen, State};
use crate::theme::{Role, Theme};
use crate::update;
use crate::view::{
    centred_block, fit, flow, furthest_scroll, keep_visible, pad, scrolled_windows, select,
    width_of, wrap_rows,
};

/// Redesign §5.7: drawn until the first `accounts` result arrives.
const LOADING: &str = "Loading accounts…";
/// The title of the empty state: no store, or no accounts.
const NO_ACCOUNTS: &str = "No accounts yet";
/// The banners the accounts view raises (spec §5.1).
pub const RUN_SHELL: &str = "inside a `tagteam run` session: account changes are refused here";
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
/// of focus's provider that tagteam does not manage (redesign §5.6's sentence, T3-7).
pub fn banner(state: &State) -> Option<String> {
    let snap = state.accounts.snapshot.as_ref()?;
    if snap.in_run_shell {
        return Some(RUN_SHELL.to_owned());
    }
    let view = state.view_of(&daemon::focus(state))?;
    match &view.live {
        LiveLogin::Unmanaged { email } => {
            let display = view.names.display_name;
            Some(format!(
                "{display} is logged in as {email}, which tagteam doesn't manage · press A to add it"
            ))
        }
        LiveLogin::None | LiveLogin::Managed { .. } => None,
    }
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
        KeyCode::Home => state.accounts.step(isize::MIN / 2),
        KeyCode::End => state.accounts.step(isize::MAX / 2),
        KeyCode::Esc => {
            // The start-up warnings stay until a person has seen them (redesign §5.6, T2-21).
            state.startup.warnings.clear();
            return vec![];
        }
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
    /// A provider's display name, when there is more than one (§6.3).
    Provider(&'a str),
    /// A provider's column header, below its name; one provider's is pinned instead.
    Header(usize),
    Account(usize, &'a AccountView),
}

/// The sections shown: one per provider with accounts.
fn sections(state: &AccountsState) -> Vec<&ProviderAccounts> {
    state
        .snapshot
        .as_ref()
        .map(|s| {
            s.providers
                .iter()
                .filter(|p| !p.accounts.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// The table's scrolling lines. With one provider its header is pinned above them.
fn layout(state: &AccountsState) -> Vec<TableLine<'_>> {
    let sections = sections(state);
    let several = sections.len() > 1;
    let mut lines = Vec::new();
    for (i, provider) in sections.into_iter().enumerate() {
        if several {
            lines.push(TableLine::Provider(provider.names.display_name));
            lines.push(TableLine::Header(i));
        }
        lines.extend(provider.accounts.iter().map(|v| TableLine::Account(i, v)));
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

pub fn render(state: &State, frame: &mut Frame, area: Rect, now_ms: i64) {
    let st = &state.accounts;
    let Some(snap) = st.snapshot.as_ref() else {
        centred_block(frame, area, LOADING, None, state.theme);
        return;
    };
    if st.rows().is_empty() {
        let launch = state.names_of(&daemon::focus(state)).launch_command;
        let sentence = format!("Log in with {launch}, then press A to add that login.");
        centred_block(frame, area, NO_ACCOUNTS, Some(&sentence), state.theme);
        return;
    }
    let now_s = now_ms.div_euclid(1000);
    let width = usize::from(area.width);
    let (table_area, pane) = split(st, area, now_s);
    let sections = sections(st);
    let tables: Vec<Table<'_>> = sections
        .iter()
        .map(|p| {
            Table::new(
                &p.accounts,
                &p.forecast,
                snap.this.as_ref(),
                width,
                snap.read_at_s,
                now_s,
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
            TableLine::Provider(name) => Line::from(Span::styled(
                fit(&format!(" {name}"), width),
                state.theme.role(Role::Muted),
            )),
            TableLine::Header(i) => tables[*i].header(state.theme),
            TableLine::Account(i, v) => {
                let mut row = tables[*i].row(v, now_s, state.theme);
                // §4.3: a disabled row is DIM throughout, selected or not.
                if v.row.disabled {
                    row = row.style(state.theme.dim());
                }
                if st.selected.as_ref() == Some(&v.row.id) {
                    select(row, Some(NAME_SPAN), width, state.theme)
                } else {
                    row
                }
            }
        });
    }
    if let Some(focus) = state
        .view_of(&daemon::focus(state))
        .filter(|p| p.accounts.len() == 1)
    {
        let room = usize::from(table_area.height).saturating_sub(shown.len());
        if room > 0 {
            let display = focus.names.display_name;
            let hint = format!(
                " Add a second account to switch: log in to {display} as it, then press A."
            );
            let hint = fit(&hint, width);
            shown.push(Line::from(Span::styled(
                hint,
                state.theme.role(Role::Muted),
            )));
        }
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
    use tagteam_core::{Pace, ProviderId, WindowKind};
    use tagteam_engine::switch::LiveLogin;
    use tagteam_engine::views::{ProviderAccounts, UsageStatus, UsageView};

    use super::*;
    use crate::msg::Msg;
    use crate::state::BannerKind;
    use crate::testutil::*;
    use crate::theme::Theme;
    use crate::update::update;

    /// The detail pane's fixture, until Task 12 replaces the pane: an active account ahead of
    /// pace, a stale spare, a quarantined alias.
    fn detail_three() -> Vec<AccountView> {
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

    /// T3-27's fixture: `alex@example.com` read 2m ago, `work` of Example Corp in use, and
    /// `spare` with an old reading. The forecast picks 1.
    fn three() -> Vec<AccountView> {
        let one = account(1, "alex@example.com", read(120, 0.0, 30.0, false));
        let mut work = account(2, "sam@corp.example", read(60, 53.0, 6.0, false));
        work.row.alias = Some("work".into());
        work.row.org_name = Some("Example Corp".into());
        work.active = true;
        let mut spare = account(3, "alex@personal.example", read(840, 0.0, 31.0, false));
        spare.row.alias = Some("spare".into());
        vec![one, work, spare]
    }

    fn loaded(accounts: Vec<AccountView>) -> State {
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snapshot(accounts));
        state
    }

    /// `three()` in `theme`, the selection on row 2.
    fn on_work(theme: Theme) -> State {
        let mut state = loaded(three());
        state.theme = theme;
        update(&mut state, ch('j'), NOW_MS);
        state
    }

    /// The display column `needle` starts at in `line`.
    fn col(line: &str, needle: &str) -> usize {
        let at = line
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} in {line:?}"));
        width_of(&line[..at])
    }

    /// Display columns `from..to` of `line`.
    fn columns(line: &str, from: usize, to: usize) -> String {
        let mut at = 0;
        let mut out = String::new();
        for c in line.chars() {
            let w = width_of(c.encode_utf8(&mut [0; 4]));
            if at >= from && at + w <= to {
                out.push(c);
            }
            at += w;
        }
        out
    }

    /// The body's first row on a `w`×`h` terminal.
    fn body_y(state: &State, w: u16, h: u16) -> u16 {
        crate::view::areas(state, Rect::new(0, 0, w, h)).body.y
    }

    /// The columns of row `y` whose cell draws a glyph in bold.
    fn bold_cells(buf: &ratatui::buffer::Buffer, y: u16) -> Vec<u16> {
        (0..buf.area.width)
            .filter(|x| {
                let cell = &buf[(*x, y)];
                cell.symbol() != " " && cell.modifier.contains(Modifier::BOLD)
            })
            .collect()
    }

    /// The text of a line `Table` draws.
    fn text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn g_does_nothing_on_accounts() {
        // §10 lists Home and End for the first and last row, and no `g` or `G` (UX-7).
        let mut state = loaded(three());
        update(&mut state, ch('j'), NOW_MS);
        let before = (draw(&state, 80, 24), state.accounts.selected());
        for c in ['g', 'G'] {
            assert!(update(&mut state, ch(c), NOW_MS).is_empty(), "{c}");
            assert_eq!(
                (draw(&state, 80, 24), state.accounts.selected()),
                before,
                "{c}"
            );
        }
        update(&mut state, key(KeyCode::End), NOW_MS);
        assert_eq!(state.accounts.selected().map(|a| a.position), Some(3));
        update(&mut state, key(KeyCode::Home), NOW_MS);
        assert_eq!(state.accounts.selected().map(|a| a.position), Some(1));
    }

    #[test]
    fn a_readings_age_advances_with_the_clock_between_reads() {
        let state = loaded(three());
        let at = |seconds: i64| draw_at(&state, 120, 24, NOW_MS + seconds * 1000);
        // UPDATED, right-aligned under its head.
        let row_age = |lines: &[String]| {
            let head = lines.iter().find(|l| l.contains("UPDATED"))?;
            let from = col(head, "UPDATED");
            let row = lines
                .iter()
                .find(|l| l.contains("alex@example.com") && l.contains('%'));
            row.map(|l| columns(l, from, from + 7).trim().to_owned())
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
    fn the_table_at_120_columns_has_14_cell_gauges_and_a_status_column() {
        assert_eq!(
            snapshot(three()).providers[0].forecast,
            tagteam_engine::switch::BestForecast::Pick { order: vec![1] }
        );
        for theme in TIERS {
            let state = on_work(theme);
            let body = body_of(&state, 120, 40, NOW_MS);
            let head = &body[0];
            for (name, at) in [
                ("ACCOUNT", 8),
                ("5H", 39),
                ("7D", 62),
                ("UPDATED", 86),
                ("STATUS", 95),
            ] {
                assert_eq!(col(head, name), at, "{theme:?} {name}: {head}");
            }
            let buf = draw_buffer(&state, 120, 40, NOW_MS);
            let y = body_y(&state, 120, 40);
            let heads: String = roles(&buf, theme, y)
                .chars()
                .filter(|c| *c != ' ')
                .collect();
            let muted = canon(theme, "m").repeat(heads.chars().count());
            assert_eq!(heads, muted, "{theme:?}: the heads are muted");
            if theme.colourless() {
                continue;
            }
            assert_eq!(
                body[2],
                format!(
                    "▌   2 ● work   sam@corp.example{}━━━━━━━───────  53%{}╸─────────────   6%{}1m  in use",
                    " ".repeat(8),
                    " ".repeat(4),
                    " ".repeat(10)
                ),
                "{theme:?}"
            );
            assert!(body[1].ends_with("2m  best"), "{theme:?}: {}", body[1]);
            assert!(
                body[3].ends_with("14m  ! old reading"),
                "{theme:?}: {}",
                body[3]
            );
            // The old reading's age and its phrase are in warn.
            let row = roles(&buf, theme, y + 3);
            let warn = canon(theme, "w");
            for x in col(&body[3], "14m")..width_of(&body[3]) {
                let letter = row.chars().nth(x).unwrap();
                assert!(
                    letter == ' ' || warn.starts_with(letter),
                    "{theme:?} {x}: {row}"
                );
            }
            // T3-15: Example Corp does not fit beside every name, so no org shows.
            assert!(
                body[0..4].iter().all(|l| !l.contains("Example Corp")),
                "{body:?}"
            );
        }
    }

    #[test]
    fn the_table_at_80_columns_has_9_cell_gauges_and_a_status_glyph() {
        let state = on_work(Theme::new(true));
        let body = body_of(&state, 80, 24, NOW_MS);
        // The name column is cut to 23.
        assert_eq!(col(&body[0], "5H"), 8 + 23 + 3, "{}", body[0]);
        assert!(!body[0].contains("STATUS"), "{}", body[0]);
        assert_eq!(
            body[3],
            format!(
                "    3   spare  alex@personal.e…   {}   0%{}━━╸──────  31%{}14m !",
                "─".repeat(9),
                " ".repeat(4),
                " ".repeat(9)
            )
        );
        assert!(body[1].contains("━━╸──────  30%"), "{}", body[1]);
        assert!(body[1].ends_with("  2m"), "{}", body[1]);
        assert!(body[2].ends_with("  1m"), "{}", body[2]);
        assert!(body[0..4].iter().all(|l| width_of(l) <= 80), "{body:?}");
    }

    #[test]
    fn a_stale_reading_s_age_is_warn_whatever_its_status() {
        // §6.3: UPDATED is warn when the reading is not decision-grade, a retry's included.
        let mut retrying = read(840, 9.0, 12.0, false);
        retrying.status = UsageStatus::Unavailable;
        retrying.error = Some("refresh-failed".into());
        let mut limited = read(60, 9.0, 12.0, false);
        limited.status = UsageStatus::Unavailable;
        limited.error = Some("http-429".into());
        let accounts = vec![
            account(1, "alex@example.com", read(60, 9.0, 12.0, false)),
            account(2, "sam@corp.example", read(840, 9.0, 12.0, false)),
            account(3, "alex@personal.example", retrying),
            account(4, "a4@x.co", limited),
        ];
        for theme in TIERS {
            let mut state = loaded(accounts.clone());
            state.theme = theme;
            let body = body_of(&state, 120, 40, NOW_MS);
            let updated = col(&body[0], "UPDATED");
            let buf = draw_buffer(&state, 120, 40, NOW_MS);
            let y = body_y(&state, 120, 40);
            let rows = [
                ("1m", "m", "best"),
                ("14m", "w", "! old reading"),
                ("14m", "w", "! retrying"),
                ("1m", "m", "! rate-limited"),
            ];
            for (i, (age, role, phrase)) in rows.into_iter().enumerate() {
                let row = &body[i + 1];
                assert_eq!(columns(row, updated, updated + 7).trim(), age, "{row}");
                assert!(row.ends_with(phrase), "{row}");
                let letters = roles(&buf, theme, y + 1 + i as u16);
                let at = updated + 7 - age.len();
                let want = canon(theme, &role.repeat(age.len()));
                let got: String = letters.chars().skip(at).take(age.len()).collect();
                assert_eq!(got, want, "{theme:?}: {row}");
            }
        }
    }

    #[test]
    fn the_table_draws_the_pace_mark_on_long_windows_only() {
        // C-8: a long window's pace mark shows in the table; a short window has none, even with
        // an expected percentage.
        let mut usage = read(60, 30.0, 30.0, false);
        let windows = usage.windows.as_mut().unwrap();
        windows[0].1.expected_pct = Some(10.0);
        windows[1].1.expected_pct = Some(50.0);
        let mut inside = read(60, 30.0, 60.0, false);
        inside.windows.as_mut().unwrap()[1].1.expected_pct = Some(20.0);
        for theme in TIERS {
            let mut state = loaded(vec![
                account(1, "alex@example.com", usage.clone()),
                account(2, "sam@corp.example", inside.clone()),
            ]);
            state.theme = theme;
            let body = body_of(&state, 120, 40, NOW_MS);
            let (five, seven) = (col(&body[0], "5H"), col(&body[0], "7D"));
            let track = if theme.colourless() { "·" } else { "─" };
            let gauge = |row: &str, at: usize| columns(row, at, at + 14);
            assert_eq!(gauge(&body[1], five), format!("━━━━{}", track.repeat(10)));
            assert_eq!(
                gauge(&body[1], seven),
                format!("━━━━{}┼{}", track.repeat(3), track.repeat(6))
            );
            assert_eq!(
                gauge(&body[2], seven),
                format!("━━┿━━━━━{}", track.repeat(6))
            );
            // `┿` in the fill's role inside it, `┼` muted outside.
            let buf = draw_buffer(&state, 120, 40, NOW_MS);
            let y = body_y(&state, 120, 40);
            let at = |row: u16, x: usize| roles(&buf, theme, y + row).chars().nth(x);
            assert_eq!(
                at(1, seven + 7),
                canon(theme, "m").chars().next(),
                "{theme:?}"
            );
            assert_eq!(
                at(2, seven + 2),
                canon(theme, "a").chars().next(),
                "{theme:?}"
            );
        }
    }

    #[test]
    fn status_is_the_first_phrase_that_applies_with_a_quarantined_account_in_use() {
        // §16: the STATUS order, with a quarantined account in use.
        let quarantined = |position: u32, email: &str| {
            let mut v = account(
                position,
                email,
                unread(UsageStatus::ReloginRequired, None, None),
            );
            v.row.quarantine_reason = Some("invalid_grant".into());
            v
        };
        let mut in_use = quarantined(1, "alex@example.com");
        in_use.active = true;
        let mut off_quarantined = quarantined(2, "sam@corp.example");
        off_quarantined.row.disabled = true;
        let mut off = account(3, "alex@personal.example", read(60, 9.0, 12.0, false));
        off.row.disabled = true;
        for theme in TIERS {
            let mut state = loaded(vec![in_use.clone(), off_quarantined.clone(), off.clone()]);
            state.theme = theme;
            let body = body_of(&state, 120, 40, NOW_MS);
            assert!(body[1].ends_with("  ✕ new login"), "{}", body[1]);
            assert_eq!(columns(&body[1], 6, 7), "●", "{}", body[1]);
            assert!(body[2].ends_with("  ✕ new login"), "{}", body[2]);
            assert!(body[3].ends_with("  ○ disabled"), "{}", body[3]);
            let buf = draw_buffer(&state, 120, 40, NOW_MS);
            let y = body_y(&state, 120, 40) + 1;
            let row = roles(&buf, theme, y);
            let at = col(&body[1], "✕ new login");
            let crit = canon(theme, "c").chars().next().unwrap();
            assert_eq!(row.chars().nth(at), Some(crit), "{theme:?}: {row}");
            assert_eq!(
                row.chars().nth(6),
                canon(theme, "a").chars().next(),
                "{theme:?}: {row}"
            );
        }
        // Each problem comes before disabled, this session, in use and best.
        let problems = [
            (
                unread(UsageStatus::ReloginRequired, None, None),
                Some(("✕ new login", Role::Crit)),
            ),
            (
                unread(UsageStatus::TokenExpired, None, None),
                Some(("✕ expired", Role::Crit)),
            ),
            (
                unread(UsageStatus::NoCredentials, None, None),
                Some(("✕ no login", Role::Crit)),
            ),
            (
                unread(UsageStatus::Unavailable, Some("http-429"), None),
                Some(("! rate-limited", Role::Warn)),
            ),
            (
                unread(UsageStatus::KeychainUnavailable, None, None),
                Some(("! keychain", Role::Warn)),
            ),
            (
                unread(UsageStatus::Unavailable, Some("over-budget"), None),
                Some(("! budget", Role::Warn)),
            ),
            (
                unread(UsageStatus::ForeignCredential, None, None),
                Some(("! retrying", Role::Warn)),
            ),
            (
                unread(UsageStatus::Unavailable, Some("refresh-failed"), None),
                Some(("! retrying", Role::Warn)),
            ),
            (
                read(840, 9.0, 12.0, false),
                Some(("! old reading", Role::Warn)),
            ),
            // Not checked yet has no phrase of its own: the window columns say it.
            (unread(UsageStatus::Unavailable, None, None), None),
        ];
        let kinds: Vec<_> = problems
            .iter()
            .map(|(usage, _)| {
                tagteam_engine::words::problem(&account(1, "a1@x.co", usage.clone())).unwrap()
            })
            .collect();
        assert_eq!(kinds, tagteam_engine::words::Problem::ALL);
        for (usage, want) in problems {
            let mut v = account(1, "a1@x.co", usage);
            v.active = true;
            v.row.disabled = true;
            let got = status::phrase(&v, true, true);
            let want = want.map_or(("○ disabled".to_owned(), Role::Muted), |(p, r)| {
                (p.to_owned(), r)
            });
            assert_eq!(got, Some(want.clone()));
            let glyph = want.0.split(' ').next().unwrap().to_owned();
            assert_eq!(
                status::glyph(&v, true, true).map(|(g, r)| (g.to_owned(), r)),
                Some((glyph, want.1))
            );
        }
        let mut v = account(1, "a1@x.co", read(60, 9.0, 12.0, false));
        v.active = true;
        v.row.disabled = true;
        let order = [
            ("○ disabled", Role::Muted, Some("○")),
            ("▶ this session", Role::Muted, None),
            ("in use", Role::Accent, None),
            ("best", Role::Text, None),
        ];
        for (i, (phrase, role, glyph)) in order.into_iter().enumerate() {
            v.row.disabled = i < 1;
            let this = i < 2;
            v.active = i < 3;
            assert_eq!(
                status::phrase(&v, this, true),
                Some((phrase.to_owned(), role)),
                "{i}"
            );
            assert_eq!(
                status::glyph(&v, this, true),
                glyph.map(|g| (g, role)),
                "{i}"
            );
        }
        assert_eq!(status::phrase(&v, false, false), None);
        let mut key = account(1, "a1@x.co", unread(UsageStatus::ApiKey, None, None));
        key.kind = API_KEY;
        assert_eq!(
            status::phrase(&key, false, false),
            Some(("api key".to_owned(), Role::Muted))
        );
        assert_eq!(status::glyph(&key, false, false), None);
    }

    #[test]
    fn a_token_from_a_newer_build_reads_as_retrying() {
        // Review Focus 1: a failure kind this build cannot name still says its token.
        let v = account(
            1,
            "alex@example.com",
            unread(
                UsageStatus::Unavailable,
                Some("a-token-from-a-newer-build"),
                None,
            ),
        );
        for theme in TIERS {
            let mut state = loaded(vec![v.clone()]);
            state.theme = theme;
            let body = body_of(&state, 120, 40, NOW_MS);
            assert_eq!(
                body[1],
                format!(
                    "▌   1   alex@example.com   ! usage check failed (a-token-from-a-newer-build)   {:>7}  ! retrying",
                    "—"
                ),
                "{theme:?}"
            );
            let buf = draw_buffer(&state, 120, 40, NOW_MS);
            let row = roles(&buf, theme, body_y(&state, 120, 40) + 1);
            let warn = canon(theme, "w").chars().next();
            for x in [27, col(&body[1], "! retrying")] {
                assert_eq!(row.chars().nth(x), warn, "{theme:?} {x}: {row}");
            }
        }
    }

    #[test]
    fn a_row_without_a_reading_says_its_status_across_the_windows() {
        let mut key = account(
            3,
            "alex@personal.example",
            unread(UsageStatus::ApiKey, None, None),
        );
        key.kind = API_KEY;
        let accounts = vec![
            account(1, "alex@example.com", read(60, 9.0, 12.0, false)),
            account(
                2,
                "sam@corp.example",
                unread(UsageStatus::ReloginRequired, None, None),
            ),
            key,
            account(
                4,
                "a4@x.co",
                unread(UsageStatus::KeychainUnavailable, None, Some(120)),
            ),
            account(5, "a5@x.co", unread(UsageStatus::Unavailable, None, None)),
        ];
        let said = [
            ("✕ needs a new login", "c"),
            ("api key · has no usage limits to show", "m"),
            ("! can't read the Keychain · retry in 2m", "w"),
            ("not checked yet", "m"),
        ];
        for theme in TIERS {
            let mut state = loaded(accounts.clone());
            state.theme = theme;
            for w in [80u16, 120] {
                let body = body_of(&state, w, 40, NOW_MS);
                let head = &body[0];
                let from = col(head, "5H");
                let to = col(head, "UPDATED") - 3;
                assert_eq!(to - from, if w == 80 { 34 } else { 44 }, "{head}");
                let updated = col(head, "UPDATED");
                let buf = draw_buffer(&state, w, 40, NOW_MS);
                let y = body_y(&state, w, 40);
                for (i, (words, role)) in said.iter().enumerate() {
                    let row = &body[i + 2];
                    assert_eq!(
                        columns(row, from, to).trim_end(),
                        fit(words, to - from),
                        "{w}: {row}"
                    );
                    let roles = roles(&buf, theme, y + 2 + i as u16);
                    assert_eq!(
                        roles.chars().nth(from),
                        canon(theme, role).chars().next(),
                        "{theme:?} {w}: {row}"
                    );
                    assert_eq!(columns(row, updated, updated + 7), "      —", "{row}");
                    assert_eq!(
                        roles.chars().nth(updated + 6),
                        canon(theme, "f").chars().next(),
                        "{theme:?} {w}: {row}"
                    );
                }
            }
        }
        // At 80 columns the longer two are cut.
        let body = body_of(&loaded(accounts), 80, 24, NOW_MS);
        assert!(
            body[3].contains("api key · has no usage limits to …"),
            "{}",
            body[3]
        );
        assert!(
            body[4].contains("! can't read the Keychain · retry…"),
            "{}",
            body[4]
        );
    }

    #[test]
    fn the_marker_column_shows_in_use_in_a_session_and_disabled() {
        let mut session = account(1, "alex@example.com", read(60, 9.0, 12.0, false));
        session.in_session = true;
        let mut in_use = account(2, "sam@corp.example", read(60, 53.0, 6.0, false));
        in_use.active = true;
        let mut off = account(3, "alex@personal.example", read(60, 9.0, 12.0, false));
        off.row.disabled = true;
        let plain = account(4, "a4@x.co", read(60, 9.0, 12.0, false));
        let mut snap = snapshot(vec![session, in_use, off, plain]);
        // A run shell: its own account is the one in the session.
        snap.this = Some(id(1));
        for theme in TIERS {
            let mut state = fresh(Screen::Accounts);
            state.theme = theme;
            state.accounts.apply(snap.clone());
            let body = body_of(&state, 120, 40, NOW_MS);
            let marks: Vec<String> = body[1..5].iter().map(|l| columns(l, 6, 7)).collect();
            assert_eq!(marks, ["▶", "●", "○", " "], "{body:?}");
            let buf = draw_buffer(&state, 120, 40, NOW_MS);
            let y = body_y(&state, 120, 40);
            let at = |row: u16| roles(&buf, theme, y + row).chars().nth(6);
            assert_eq!(at(1), canon(theme, "a").chars().next(), "{theme:?}");
            assert_eq!(at(2), canon(theme, "a").chars().next(), "{theme:?}");
            assert_eq!(at(3), canon(theme, "m").chars().next(), "{theme:?}");
            assert!(body[1].ends_with("  ▶ this session"), "{}", body[1]);
            let row = roles(&buf, theme, y + 1);
            let phrase = col(&body[1], "▶ this session");
            assert_eq!(
                row.chars().nth(phrase),
                canon(theme, "m").chars().next(),
                "{theme:?}: {row}"
            );
            assert!(body[2].ends_with("  in use"), "{}", body[2]);
            assert!(body[3].ends_with("  ○ disabled"), "{}", body[3]);
        }
    }

    #[test]
    fn the_detail_pane_shows_the_selected_account() {
        let mut accounts = detail_three();
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
                " 7d   77%  resets in 3d09h  ▲ ahead of pace (expected 62%)  +1.2%/h · full in 2d04h".to_owned(),
                " fetched 2m ago · login expires in 6d03h".to_owned(),
            ]
        );
    }

    #[test]
    fn a_row_without_a_reading_says_why_in_the_detail_pane() {
        let mut state = loaded(detail_three());
        update(&mut state, key(KeyCode::End), NOW_MS);
        let body = body_of(&state, 80, 24, NOW_MS);
        let top = body.len() - 7;
        assert!(
            body[top].starts_with("─ 3 work (w@corp.com) · not active · oauth ──"),
            "{}",
            body[top]
        );
        assert_eq!(width_of(&body[top]), 80);
        assert_eq!(body[top + 1], " never fetched");
        assert_eq!(body[top + 2], " usage: needs a new login");
    }

    #[test]
    fn the_detail_pane_counts_down_to_the_next_poll_while_it_is_ahead() {
        // Spec §5.2: `fetched 2m ago · next poll in 1m · login expires in 6d`.
        let mut accounts = detail_three();
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
                format!("{under}+1.2%/h · full in 2d04h"),
                format!(" {:<17}  €123.45 of €200  resets in 14d00h", "spend"),
                format!(
                    " {:<17}  {:>4}  resets in 3d09h  ▲ ahead of pace (expected 20%)",
                    "Claude Sonnet 4.5", "41%"
                ),
                format!("{under}+1.5%/h · full in 1d00h"),
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
                " {:<17}  {:>4}  resets in 3d09h  ▲ ahead of pace (expected 62%)  +1.2%/h · full in 2d04h",
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
        assert!(rest.contains("+1.5%/h · full in 1d00h"), "{rest}");
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
        state.toast(
            crate::state::ToastKind::Info,
            ["Switched to a@x.co (position 1)"; 8].join(" "),
            NOW_MS,
        );
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
        assert!(rest.contains("+1.5%/h · full in 1d00h"), "{lines:?}");
        assert!(rest.contains(" usage: rate-limited"), "{lines:?}");
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
        state.toast(
            crate::state::ToastKind::Info,
            ["Switched to a@x.co (position 1)"; 8].join(" "),
            NOW_MS,
        );
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
        assert!(rest.contains("+1.5%/h · full in 1d00h"), "{lines:?}");
        assert!(rest.contains(" usage: rate-limited"), "{lines:?}");
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
        state.toast(
            crate::state::ToastKind::Info,
            ["Switched to a@x.co (position 1)"; 8].join(" "),
            NOW_MS,
        );
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
            block.contains("should have been full 400d00h ago"),
            "{lines:?}"
        );
        assert_eq!(lines[age + 1], " usage: rate-limited");
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
            " Haiku    42%  resets in 3d09h  ▲ ahead of pace (expected 20%)  +1.5%/h · full in 1d00h"
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
        assert!(lines[age - 1].trim_start().starts_with("+1.5%/h"));
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
                .any(|l| l.contains("J K") && l.contains("scroll the selected card")),
            "{help:?}"
        );
    }

    #[test]
    fn percentages_and_fills_take_their_severity_role_in_every_tier() {
        let accounts = vec![
            account(1, "alex@example.com", read(60, 53.0, 75.0, true)),
            account(2, "sam@corp.example", read(60, 95.0, 0.0, false)),
        ];
        for theme in TIERS {
            let mut state = loaded(accounts.clone());
            state.theme = theme;
            let body = body_of(&state, 120, 40, NOW_MS);
            let five = col(&body[0], "5H");
            let seven = col(&body[0], "7D");
            let buf = draw_buffer(&state, 120, 40, NOW_MS);
            let y = body_y(&state, 120, 40);
            let cell = |row: u16, from: usize| -> String {
                roles(&buf, theme, y + row)
                    .chars()
                    .skip(from)
                    .take(21)
                    .collect()
            };
            // 53%: an accent fill on a faint track, the percentage in text.
            assert_eq!(
                cell(1, five),
                canon(theme, "aaaaaaafffffff  ttt  "),
                "{theme:?}"
            );
            // 75% ahead of pace: a warn fill and a warn percentage, then a warn `▲`.
            assert_eq!(
                cell(1, seven),
                canon(theme, "wwwwwwwwwwwfff  www w"),
                "{theme:?}"
            );
            assert!(body[1][..].contains("75% ▲"), "{}", body[1]);
            // 95%: crit, fill and percentage.
            assert_eq!(
                cell(2, five),
                canon(theme, "cccccccccccccf  ccc  "),
                "{theme:?}"
            );
            // 0%: the track alone.
            assert_eq!(
                cell(2, seven),
                canon(theme, "ffffffffffffff   tt  "),
                "{theme:?}"
            );
        }
    }

    #[test]
    fn the_selected_row_has_the_bar_and_a_bold_name_and_no_reverse_video() {
        for theme in TIERS {
            let state = on_work(theme);
            let buf = draw_buffer(&state, 80, 24, NOW_MS);
            // The body's first row is the column heads; the selection is on the third.
            let y = body_y(&state, 80, 24) + 2;
            assert!(
                roles(&buf, theme, y).starts_with(&canon(theme, "a")),
                "{theme:?}"
            );
            assert_eq!(buf[(0, y)].symbol(), "▌", "{theme:?}");
            for other in [y - 1, y + 1] {
                assert_eq!(buf[(0, other)].symbol(), " ", "{theme:?}");
            }
            let rows = text_of(&buf);
            assert_eq!(columns(&rows[usize::from(y)], 8, 12), "work");
            for x in 8..12 {
                assert!(buf[(x, y)].modifier.contains(Modifier::BOLD), "{theme:?}");
            }
            assert_eq!(
                columns(&rows[usize::from(y - 1)], 8, 24),
                "alex@example.com"
            );
            assert!(
                !buf[(8, y - 1)].modifier.contains(Modifier::BOLD),
                "{theme:?}"
            );
            let tint = theme.sel_bg().bg.unwrap_or(Color::Reset);
            for x in 0..80 {
                for row in [y - 1, y, y + 1] {
                    assert!(
                        !buf[(x, row)].modifier.contains(Modifier::REVERSED),
                        "{theme:?} {x},{row}"
                    );
                }
                assert_eq!(buf[(x, y)].bg, tint, "{theme:?} {x}");
                for row in [y - 1, y + 1] {
                    assert_eq!(buf[(x, row)].bg, Color::Reset, "{theme:?} {x}");
                }
            }
        }
    }

    #[test]
    fn only_the_selected_row_s_name_is_bold() {
        // T3-16: in tiers 1 and 2 the selected row's name is its one bold, and the account in
        // use has `●`, not bold.
        for theme in &TIERS[1..] {
            let theme = *theme;
            let mut state = on_work(theme);
            let y = body_y(&state, 120, 40);
            let buf = draw_buffer(&state, 120, 40, NOW_MS);
            assert_eq!(
                bold_cells(&buf, y + 2),
                (8..12).collect::<Vec<_>>(),
                "{theme:?}"
            );
            for row in [y, y + 1, y + 3] {
                assert_eq!(bold_cells(&buf, row), Vec::<u16>::new(), "{theme:?} {row}");
            }
            // Without an alias the email is the name.
            update(&mut state, ch('k'), NOW_MS);
            let buf = draw_buffer(&state, 120, 40, NOW_MS);
            assert_eq!(
                bold_cells(&buf, y + 1),
                (8..24).collect::<Vec<_>>(),
                "{theme:?}"
            );
            assert_eq!(
                bold_cells(&buf, y + 2),
                Vec::<u16>::new(),
                "{theme:?}: the row in use"
            );
        }
        // Tier 0 draws accent, warn and crit bold (§4.1): the selected row in use bolds `▌`,
        // `●` and its name, and past them only what its roles draw bold, its fills and its
        // `in use`.
        let theme = TIERS[0];
        let state = on_work(theme);
        let y = body_y(&state, 120, 40) + 2;
        let buf = draw_buffer(&state, 120, 40, NOW_MS);
        let bold = bold_cells(&buf, y);
        let (identity, rest): (Vec<u16>, Vec<u16>) = bold.into_iter().partition(|x| *x < 39);
        assert_eq!(identity, [0, 6, 8, 9, 10, 11]);
        let mut roles_bold: Vec<u16> = (39..46).collect();
        roles_bold.push(62);
        roles_bold.extend([95, 96, 98, 99, 100]);
        assert_eq!(rest, roles_bold);
    }

    #[test]
    fn a_disabled_row_is_dim_even_when_selected() {
        let mut accounts = three();
        accounts[0].row.disabled = true;
        for theme in TIERS {
            let mut state = loaded(accounts.clone());
            state.theme = theme;
            let y = body_y(&state, 120, 40) + 1;
            update(&mut state, ch('j'), NOW_MS);
            let buf = draw_buffer(&state, 120, 40, NOW_MS);
            let drawn = width_of(&text_of(&buf)[usize::from(y)]) as u16;
            assert!(drawn > 60, "{drawn}");
            for x in 0..drawn {
                assert!(
                    buf[(x, y)].modifier.contains(Modifier::DIM),
                    "{theme:?} {x}"
                );
            }
            update(&mut state, ch('k'), NOW_MS);
            let buf = draw_buffer(&state, 120, 40, NOW_MS);
            assert_eq!(buf[(0, y)].symbol(), "▌");
            let tint = theme.sel_bg().bg.unwrap_or(Color::Reset);
            for x in 0..120 {
                let cell = &buf[(x, y)];
                assert!(cell.modifier.contains(Modifier::DIM), "{theme:?} {x}");
                assert_eq!(cell.bg, tint, "{theme:?} {x}");
            }
        }
    }

    #[test]
    fn from_160_columns_scoped_windows_and_spend_get_columns() {
        let mut usage = read(60, 9.0, 12.0, false);
        let windows = usage.windows.as_mut().unwrap();
        windows.push((
            window(
                "scoped:fable",
                "Fable",
                WindowKind::Scoped,
                40.0,
                Some(291_630),
            ),
            Pace::default(),
        ));
        let mut spend = window("spend", "spend", WindowKind::Spend, 0.0, Some(1_209_630));
        spend.detail = Some(r#"{"used": 0, "limit": 20, "currency": "EUR"}"#.parse().unwrap());
        windows.push((spend, Pace::default()));
        let mut more = read(60, 9.0, 12.0, false);
        let mut spent = window("spend", "spend", WindowKind::Spend, 24.8, Some(1_209_630));
        spent.detail = Some(r#"{"used": 12.4, "limit": 50, "currency": "EUR"}"#.parse().unwrap());
        more.windows
            .as_mut()
            .unwrap()
            .push((spent, Pace::default()));
        let mut state = loaded(vec![
            account(1, "alex@example.com", usage),
            account(2, "sam@corp.example", read(60, 9.0, 12.0, false)),
            account(3, "alex@personal.example", more),
        ]);
        state.theme = Theme::new(true);
        let wide = body_of(&state, 160, 40, NOW_MS);
        let head = &wide[0];
        let (fable, spend) = (col(head, "FABLE"), col(head, "SPEND"));
        assert!(col(head, "7D") < fable && fable < spend, "{head}");
        // The spend column is its widest amount, `€12.40 of €50`, and the group ends there.
        assert_eq!(spend + 13 + 3, col(head, "UPDATED"), "{head}");
        assert_eq!(columns(&wide[1], fable, fable + 6), " 40%  ", "{}", wide[1]);
        // §4.3: amounts are right-aligned.
        assert_eq!(
            columns(&wide[1], spend, spend + 13),
            "    €0 of €20",
            "{}",
            wide[1]
        );
        assert_eq!(
            columns(&wide[3], spend, spend + 13),
            "€12.40 of €50",
            "{}",
            wide[3]
        );
        // A row without them has `—` where their values end.
        assert_eq!(columns(&wide[2], fable, fable + 4), "   —", "{}", wide[2]);
        assert_eq!(
            columns(&wide[2], spend, spend + 13),
            format!("{}—", " ".repeat(12)),
            "{}",
            wide[2]
        );
        assert!(wide.iter().all(|l| width_of(l) <= 160));
        let narrow = body_of(&state, 120, 40, NOW_MS);
        assert!(
            !narrow[0].contains("FABLE") && !narrow[0].contains("SPEND"),
            "{}",
            narrow[0]
        );
        assert!(!narrow[1].contains("€0 of €20"), "{}", narrow[1]);
    }

    #[test]
    fn a_narrow_table_shrinks_the_name_then_the_gauges() {
        // T3-15's order, below the TUI's 80 columns.
        let accounts = three();
        let snap = snapshot(accounts.clone());
        let forecast = &snap.providers[0].forecast;
        let theme = Theme::new(true);
        let draw = |width: usize| {
            let table = Table::new(&accounts, forecast, None, width, NOW_S, NOW_S);
            let head = text(&table.header(theme));
            let rows: Vec<String> = accounts
                .iter()
                .map(|v| text(&table.row(v, NOW_S, theme)))
                .collect();
            for row in &rows {
                assert!(width_of(row) <= width, "{width}: {row}");
                assert!(row.contains('%'), "{width}: {row}");
            }
            (head, rows)
        };
        // At 72: the name down to 16, then the 7D gauge to 6; 5H keeps its 9.
        let (head, rows) = draw(72);
        assert_eq!(
            [col(&head, "5H"), col(&head, "7D"), col(&head, "UPDATED")],
            [27, 27 + 16 + 2, 45 + 13 + 3],
            "{head}"
        );
        assert_eq!(
            rows[0],
            format!(
                "    1   alex@example.com   {}   0%    ━╸────  30%{}2m",
                "─".repeat(9),
                " ".repeat(10)
            )
        );
        assert!(
            rows[2].starts_with("    3   spare  alex@per…   "),
            "{}",
            rows[2]
        );
        assert!(rows[2].ends_with("14m !"), "{}", rows[2]);
        // At 60: the 5H gauge to 6 as well, and 7D to its percentage alone.
        let (head, rows) = draw(60);
        assert_eq!(
            [col(&head, "5H"), col(&head, "7D"), col(&head, "UPDATED")],
            [27, 27 + 13 + 2, 42 + 6 + 3],
            "{head}"
        );
        assert_eq!(
            rows[0],
            format!(
                "    1   alex@example.com   {}   0%     30%{}2m",
                "─".repeat(6),
                " ".repeat(10)
            )
        );
        assert_eq!(
            rows[2],
            format!(
                "    3   spare  alex@per…   {}   0%     31%{}14m !",
                "─".repeat(6),
                " ".repeat(9)
            )
        );
    }

    /// Review Focus 3: names wider than their column are cut by display width, never by bytes,
    /// and the table never overflows or panics.
    #[test]
    fn wide_names_are_cut_by_display_width_and_never_overflow() {
        let mut long = account(1, "alex@example.com", read(60, 9.0, 12.0, false));
        long.row.alias = Some("a".repeat(200));
        let mut wide_alias = account(4, "a4@x.co", read(60, 9.0, 12.0, false));
        wide_alias.row.alias = Some("仕事用アカウント😀😀😀😀".into());
        let mut empty_alias = account(5, "sam@corp.example", read(60, 9.0, 12.0, false));
        empty_alias.row.alias = Some(String::new());
        let accounts = vec![
            long,
            account(2, "例のアカウント@example.com", read(840, 9.0, 12.0, false)),
            account(
                3,
                "😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀@x.co",
                read(60, 9.0, 12.0, false),
            ),
            wide_alias,
            empty_alias,
        ];
        for w in [80, 100, 120] {
            let state = loaded(accounts.clone());
            let lines = draw(&state, w, 24);
            for line in &lines {
                assert!(width_of(line) <= usize::from(w), "{w}: {line}");
            }
            // Every row's UPDATED still sits under its head.
            let body = body_of(&state, w, 24, NOW_MS);
            let updated = col(&body[0], "UPDATED");
            for (row, age) in body[1..6].iter().zip(["1m", "14m", "1m", "1m", "1m"]) {
                assert_eq!(
                    columns(row, updated, updated + 7).trim_start(),
                    age,
                    "{w}: {row}"
                );
            }
            // The old reading's mark shows.
            let mark = if w < 100 { " !" } else { "  ! old reading" };
            assert!(body[2].ends_with(mark), "{w}: {}", body[2]);
        }
        let narrow = body_of(&loaded(accounts), 80, 24, NOW_MS);
        assert!(narrow[1].contains("aaaa…"), "{}", narrow[1]);
        assert!(
            narrow[2].starts_with("    2   例のアカウント@"),
            "{}",
            narrow[2]
        );
        // The 23-column name: a wide alias cut by display width to half of it, two spaces and
        // the email; an empty alias leaves its column blank.
        let name = |row: &str| columns(row, 8, 8 + 23);
        assert_eq!(
            name(&narrow[4]),
            format!("仕事用アカ…  a4@x.co{}", " ".repeat(3))
        );
        assert_eq!(width_of(&name(&narrow[4])), 23);
        assert_eq!(name(&narrow[5]), format!("{}sam@corp.…", " ".repeat(13)));
    }

    /// Review Focus 3: with no store and no accounts, the empty state (redesign §5.7).
    #[test]
    fn no_accounts_shows_the_empty_state() {
        let state = loaded(vec![]);
        let lines = draw(&state, 80, 24);
        for text in [
            "No accounts yet",
            "Log in with claude, then press A to add that login.",
        ] {
            assert!(lines.iter().any(|l| l.trim() == text), "{text}: {lines:?}");
        }
        assert_eq!(state.accounts.selected(), None);
        assert!(state.accounts.shown_ids().is_empty());
        let unloaded = draw(&fresh(Screen::Accounts), 80, 24);
        assert!(unloaded.iter().any(|l| l.trim() == "Loading accounts…"));
    }

    #[test]
    fn with_one_account_the_table_says_how_to_add_a_second() {
        let state = loaded(many(1));
        let body = body_of(&state, 80, 24, NOW_MS);
        let hint = " Add a second account to switch: log in to Claude Code as it, then press A.";
        assert_eq!(body[2], hint, "{body:?}");
        assert!(body[1].contains("a1@x.co"), "{body:?}");
        let two = body_of(&loaded(many(2)), 80, 24, NOW_MS);
        assert!(!two.iter().any(|l| l.contains("Add a second")), "{two:?}");
    }

    #[test]
    fn the_words_name_each_provider_by_its_own_names() {
        let mut other = account(1, "o@x.co", read(60, 50.0, 60.0, false));
        other.row.provider = ProviderId::new("fake-agent");
        other.row.id = AccountId::from_string("other-1");
        let mut snap = snapshot(vec![]);
        snap.providers.push(ProviderAccounts {
            provider: ProviderId::new("fake-agent"),
            active_position: None,
            forecast: tagteam_engine::views::forecast_of(
                std::slice::from_ref(&other),
                &LiveLogin::None,
                &[],
            ),
            accounts: vec![other],
            threshold: 90.0,
            strategy: tagteam_core::autoswitch::AutoStrategy::Best,
            names: tagteam_engine::views::ProviderNames {
                display_name: "FakeAgent",
                launch_command: "fakeagent",
            },
            models: vec![],
            live: LiveLogin::None,
        });
        set_live(
            &mut snap.providers[1],
            LiveLogin::Unmanaged {
                email: "s@x.co".into(),
            },
        );
        let mut state = fresh(Screen::Accounts);
        state.provider = Some(ProviderId::new("fake-agent"));
        state.accounts.apply(snap);
        // The one account, so the table asks for a second.
        let lines = draw(&state, 120, 24);
        assert!(
            lines.iter().any(|l| l.trim_end()
                == " Add a second account to switch: log in to FakeAgent as it, then press A."),
            "{lines:?}"
        );
        // The unmanaged login is the one of the provider the TUI follows.
        assert_eq!(
            lines[2],
            " ! FakeAgent is logged in as s@x.co, which tagteam doesn't manage · press A to add it"
        );
        // The remove question names it too.
        update(&mut state, ch('x'), NOW_MS);
        let Some(crate::overlay::Overlay::Confirm { question, .. }) = &state.overlay else {
            panic!("{:?}", state.overlay);
        };
        assert_eq!(
            question,
            "Remove 1 o@x.co? Its stored login is deleted; FakeAgent is not changed."
        );
        // No accounts for it: the launch command.
        let mut empty = snapshot(vec![]);
        empty.providers[0].provider = ProviderId::new("fake-agent");
        empty.providers[0].names = tagteam_engine::views::ProviderNames {
            display_name: "FakeAgent",
            launch_command: "fakeagent",
        };
        let mut state = fresh(Screen::Accounts);
        state.provider = Some(ProviderId::new("fake-agent"));
        state.accounts.apply(empty);
        let lines = draw(&state, 80, 24);
        assert!(
            lines
                .iter()
                .any(|l| l.trim() == "Log in with fakeagent, then press A to add that login."),
            "{lines:?}"
        );
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
    fn several_providers_are_grouped_under_their_display_names() {
        let mut other = account(1, "o@x.co", read(60, 50.0, 60.0, false));
        other.row.provider = ProviderId::new("fake-agent");
        other.row.id = AccountId::from_string("other-1");
        let mut snap = snapshot(many(1));
        snap.providers.push(ProviderAccounts {
            provider: ProviderId::new("fake-agent"),
            active_position: None,
            forecast: tagteam_engine::views::forecast_of(
                std::slice::from_ref(&other),
                &LiveLogin::None,
                &[],
            ),
            accounts: vec![other],
            threshold: 90.0,
            strategy: tagteam_core::autoswitch::AutoStrategy::Best,
            names: tagteam_engine::views::ProviderNames {
                display_name: "FakeAgent",
                launch_command: "fakeagent",
            },
            models: vec![],
            live: LiveLogin::None,
        });
        for theme in TIERS {
            let mut state = fresh(Screen::Accounts);
            state.theme = theme;
            state.accounts.apply(snap.clone());
            let body = body_of(&state, 80, 24, NOW_MS);
            assert_eq!(body[0], " Claude Code");
            assert!(body[1].starts_with("    #   ACCOUNT"), "{}", body[1]);
            assert!(body[2].contains("a1@x.co"));
            assert_eq!(body[3], " FakeAgent");
            assert!(body[4].starts_with("    #   ACCOUNT"));
            assert!(body[5].contains("o@x.co"));
            assert!(body.iter().all(|l| width_of(l) <= 80), "{body:?}");
            let buf = draw_buffer(&state, 80, 24, NOW_MS);
            let y = body_y(&state, 80, 24);
            for row in [y, y + 3] {
                let letters: String = roles(&buf, theme, row)
                    .chars()
                    .filter(|c| *c != ' ')
                    .collect();
                let muted = canon(theme, "m").repeat(letters.chars().count());
                assert_eq!(letters, muted, "{theme:?} {row}");
            }
        }
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snap);
        update(&mut state, ch('j'), NOW_MS);
        assert_eq!(
            state.accounts.selected().unwrap().provider,
            ProviderId::new("fake-agent")
        );
    }

    #[test]
    fn a_run_shell_or_an_unmanaged_live_login_raises_a_banner() {
        let mut snap = snapshot(many(1));
        set_live(
            &mut snap.providers[0],
            LiveLogin::Unmanaged {
                email: "s@x.co".into(),
            },
        );
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snap.clone());
        assert_eq!(
            draw(&state, 80, 24)[2..4],
            [
                " ! Claude Code is logged in as s@x.co, which tagteam doesn't manage · press A to",
                "   add it",
            ]
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
        set_live(
            &mut snap.providers[0],
            LiveLogin::Unmanaged {
                email: long.clone(),
            },
        );
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snap);
        let lines = draw(&state, 80, 24);
        assert_eq!(
            lines[2..5],
            [
                " ! Claude Code is logged in as".to_owned(),
                format!("   {long}, which tagteam"),
                "   doesn't manage · press A to add it".to_owned(),
            ]
        );
        let body = body_of(&state, 80, 24, NOW_MS);
        assert!(body[0].starts_with("    #   ACCOUNT"), "{}", body[0]);
    }

    /// FakeAgent's view holding `accounts`, its live login `live`.
    fn fake_agent(accounts: Vec<AccountView>, live: LiveLogin) -> ProviderAccounts {
        let forecast = tagteam_engine::views::forecast_of(&accounts, &live, &[]);
        ProviderAccounts {
            provider: ProviderId::new("fake-agent"),
            active_position: None,
            accounts,
            threshold: 90.0,
            strategy: tagteam_core::autoswitch::AutoStrategy::Best,
            names: tagteam_engine::views::ProviderNames {
                display_name: "FakeAgent",
                launch_command: "fakeagent",
            },
            models: vec![],
            live,
            forecast,
        }
    }

    /// FakeAgent's account at `position`.
    fn fake_account(position: u32, email: &str) -> AccountView {
        let mut v = account(position, email, read(60, 50.0, 60.0, false));
        v.row.provider = ProviderId::new("fake-agent");
        v.row.id = AccountId::from_string(format!("fake-{position}"));
        v
    }

    #[test]
    fn the_unmanaged_login_banner_follows_the_selected_account_s_provider() {
        // T3-7: the banner names the unmanaged login of the provider focus is on.
        let mut snap = snapshot(many(1));
        snap.providers.push(fake_agent(
            vec![fake_account(1, "a1@x.co")],
            LiveLogin::Unmanaged {
                email: "s@x.co".into(),
            },
        ));
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snap);
        assert_eq!(state.accounts.selected().unwrap().provider, cc());
        assert_eq!(banner(&state), None, "only FakeAgent's login is unmanaged");
        update(&mut state, ch('j'), NOW_MS);
        assert_eq!(
            banner(&state).as_deref(),
            Some(
                "FakeAgent is logged in as s@x.co, which tagteam doesn't manage · press A to add it"
            )
        );
    }

    #[test]
    fn with_one_account_in_the_provider_followed_the_table_says_how_to_add_a_second() {
        // T3-7: the hint shows when focus's provider has exactly one account, and names it.
        let mut snap = snapshot(many(1));
        snap.providers.push(fake_agent(
            vec![fake_account(1, "a1@x.co"), fake_account(2, "a2@x.co")],
            LiveLogin::None,
        ));
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snap);
        let hint = " Add a second account to switch: log in to Claude Code as it, then press A.";
        let lines = draw(&state, 120, 30);
        assert!(lines.iter().any(|l| l.trim_end() == hint), "{lines:?}");
        update(&mut state, ch('j'), NOW_MS);
        assert_eq!(
            state.accounts.selected().unwrap().provider,
            ProviderId::new("fake-agent")
        );
        let lines = draw(&state, 120, 30);
        assert!(
            !lines.iter().any(|l| l.contains("Add a second")),
            "{lines:?}"
        );
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
        let state = loaded(vec![key, off]);
        let wide = body_of(&state, 120, 40, NOW_MS);
        assert!(wide[1].ends_with("  api key"), "{}", wide[1]);
        assert!(wide[2].ends_with("  ○ disabled"), "{}", wide[2]);
        // At 80 columns only the disabled glyph follows UPDATED.
        let body = body_of(&state, 80, 24, NOW_MS);
        assert!(body[1].ends_with("  —"), "{}", body[1]);
        assert!(body[2].ends_with("1m ○"), "{}", body[2]);
    }

    #[test]
    fn the_detail_pane_announces_no_poll_the_status_rules_out() {
        // A quarantined account keeps its stored plan, but no poll comes (core §8.3).
        let mut accounts = detail_three();
        accounts[2].usage.next_poll_at = Some(NOW_S + 180);
        let mut state = loaded(accounts);
        update(&mut state, key(KeyCode::End), NOW_MS);
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
                .any(|l| l == " usage: rate-limited (retry 15m)"),
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
        // Below 160 columns the scoped and spend columns give way; the status keeps its glyph.
        let lines = draw(&loaded(crowded("http-429")), 80, 24);
        assert!(lines.iter().all(|l| width_of(l) <= 80), "{lines:?}");
        let body = body_of(&loaded(crowded("http-429")), 80, 24, NOW_MS);
        let head = &body[0];
        assert!(head.contains("5H") && head.contains("7D"), "{head}");
        assert!(head.ends_with("UPDATED"), "{head}");
        assert!(
            !head.contains("SONNET") && !head.contains("SPEND"),
            "{head}"
        );
        assert!(body[2].ends_with("2m ○"), "{}", body[2]);
        assert!(body[3].ends_with("2m !"), "{}", body[3]);
        // At 160 columns every column and every phrase fit.
        let lines = draw(&loaded(crowded("http-429")), 160, 40);
        assert!(lines.iter().all(|l| width_of(l) <= 160), "{lines:?}");
        let wide = body_of(&loaded(crowded("http-429")), 160, 40, NOW_MS);
        assert!(wide[0].contains("CLAUDE SONNET 4.5"), "{}", wide[0]);
        assert!(
            wide[0].contains("SPEND") && wide[0].ends_with("STATUS"),
            "{}",
            wide[0]
        );
        assert!(wide[2].ends_with("  ○ disabled"), "{}", wide[2]);
        assert!(wide[3].ends_with("  ! rate-limited"), "{}", wide[3]);
    }

    #[test]
    fn a_note_too_long_for_its_room_ends_in_an_ellipsis() {
        // A row without a reading says its status across the window columns, cut to them.
        let long = "x".repeat(70);
        let mut accounts = crowded(&long);
        accounts[2].usage.windows = None;
        let lines = draw(&loaded(accounts.clone()), 80, 24);
        assert!(lines.iter().all(|l| width_of(l) <= 80), "{lines:?}");
        let body = body_of(&loaded(accounts), 80, 24, NOW_MS);
        let from = col(&body[0], "5H");
        let to = col(&body[0], "UPDATED") - 3;
        let group = columns(&body[3], from, to);
        assert!(group.ends_with('…'), "{}", body[3]);
        assert!(
            group.starts_with("! usage check failed (xxx"),
            "{}",
            body[3]
        );
        assert!(body[3].ends_with("      — !"), "{}", body[3]);
    }
}
