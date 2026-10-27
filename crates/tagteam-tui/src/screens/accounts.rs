//! The Accounts screen (redesign §6, S§3.1): under the chrome's Now strip, the table of accounts
//! (§6.3) between two rules, and the selected account's card (§6.4), stacked from the top.

pub(crate) mod card;
pub(crate) mod lead;
mod list;
pub(crate) mod status;
pub(crate) mod table;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use tagteam_core::AccountId;
use tagteam_engine::switch::LiveLogin;
use tagteam_engine::views::{AccountView, ProviderAccounts};

use self::table::{NAME_SPAN, Table};

use crate::chrome;
use crate::daemon;
use crate::host::AccountRef;
use crate::msg::Cmd;
use crate::source::AccountsSnapshot;
use crate::state::State;
use crate::theme::Role;
use crate::view::{centred_block, fit, keep_visible, select};

/// Redesign §5.7: drawn until the first `accounts` result arrives.
const LOADING: &str = "Loading accounts…";
/// The title of the empty state: no store, or no accounts.
const NO_ACCOUNTS: &str = "No accounts yet";
/// The banners the accounts view raises (spec §5.1).
pub const RUN_SHELL: &str = "inside a `tagteam run` session: account changes are refused here";
/// The table keeps this many lines at least, its header and the selected row among them, when
/// the cards grow to hold their windows (T3-22).
const TABLE_LEAST: usize = 4;

#[derive(Default)]
pub struct AccountsState {
    snapshot: Option<AccountsSnapshot>,
    /// The selected account, by ID, so a refresh that reorders rows keeps it (Review Focus 5).
    selected: Option<AccountId>,
    /// The first table line shown.
    offset: usize,
    /// The first of the selected account's windows the card shows (`J`/`K`), when they do not
    /// all fit.
    card_from: usize,
    /// Compact, or every account in full (`v`); it lasts for the session.
    view: View,
    /// The first block the full list shows, as last kept whole around the selection.
    list_from: usize,
}

/// What the Accounts screen shows (S§6): the table and the selected card, or every account.
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum View {
    #[default]
    Compact,
    List,
}

impl AccountsState {
    pub(crate) fn view(&self) -> View {
        self.view
    }

    pub fn snapshot(&self) -> Option<&AccountsSnapshot> {
        self.snapshot.as_ref()
    }

    /// Takes a new read. With no selection yet, it starts on the first account in use in table
    /// order, else on row 1 (§6.5, T3-23). The selection stays on the same account; when that
    /// account is gone it moves to the row now at its old place, or the last row (Review Focus
    /// 5).
    pub fn apply(&mut self, snap: AccountsSnapshot) {
        let old = self.index();
        let rows = rows_of(&snap);
        let ids: Vec<AccountId> = rows.iter().map(|v| v.row.id.clone()).collect();
        let selected = match &self.selected {
            Some(id) if ids.contains(id) => Some(id.clone()),
            _ if ids.is_empty() => None,
            None => Some(
                rows.iter()
                    .find(|v| v.active)
                    .map_or_else(|| ids[0].clone(), |v| v.row.id.clone()),
            ),
            Some(_) => Some(ids[old.unwrap_or(0).min(ids.len() - 1)].clone()),
        };
        if selected != self.selected {
            self.card_from = 0;
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
        self.select(id);
    }

    fn select(&mut self, id: AccountId) {
        if self.selected.as_ref() != Some(&id) {
            self.card_from = 0;
        }
        self.selected = Some(id);
    }

    /// Selects `id`, when the table shows it: History's scope chose it (T4-17).
    pub(crate) fn select_id(&mut self, id: &AccountId) -> bool {
        let shown = self.rows().iter().any(|v| &v.row.id == id);
        if shown {
            self.select(id.clone());
        }
        shown
    }
}

/// Every provider's accounts, in the order `list` prints them.
fn rows_of(snap: &AccountsSnapshot) -> Vec<&AccountView> {
    snap.providers.iter().flat_map(|p| &p.accounts).collect()
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
    let list = state.accounts.view == View::List;
    if list {
        // From what is on screen now: a resize or a read may have left the stored origin stale.
        list::follow(state, body, now_ms);
    }
    let page = if list {
        list::page(state, body, now_ms)
    } else {
        visible_rows(state, body, now_ms)
    }
    .max(1) as isize;
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
        KeyCode::Char('v') => {
            let st = &mut state.accounts;
            st.view = match st.view {
                View::Compact => View::List,
                View::List => View::Compact,
            };
            st.list_from = 0;
            if st.view == View::List {
                list::follow(state, body, now_ms);
            }
            return vec![];
        }
        KeyCode::Char('J') => {
            scroll_card(state, body, now_ms, 1);
            return vec![];
        }
        KeyCode::Char('K') => {
            scroll_card(state, body, now_ms, -1);
            return vec![];
        }
        KeyCode::Enter => {
            let Some(id) = state.accounts.selected.clone() else {
                return vec![];
            };
            return crate::screens::history::scope_to(state, id, now_ms);
        }
        _ => return vec![],
    }
    if list {
        list::follow(state, body, now_ms);
    } else {
        let lines = layout(&state.accounts).len();
        let at = line_of(&state.accounts);
        let rows = visible_rows(state, body, now_ms);
        state.accounts.offset = keep_visible(state.accounts.offset, at, at, rows, lines);
    }
    crate::screens::history::follow(state);
    vec![]
}

/// `J`/`K`: the card's windows move `by` one, stopping once the last one shows, or at the
/// first.
fn scroll_card(state: &mut State, body: Rect, now_ms: i64, by: isize) {
    let Some(v) = state.accounts.selected_view() else {
        return;
    };
    let parts = parts(state, body, now_ms);
    let rows = usize::from(parts.card.height);
    let furthest = card::furthest(state, v, body.width, rows, parts.keys, now_ms);
    // From what shows: a resize or a new read may have left the stored scroll past the end.
    let st = &mut state.accounts;
    let from = st.card_from.min(furthest) as isize;
    st.card_from = (from + by).clamp(0, furthest as isize) as usize;
}

/// The body's parts, top down (S§3.1, §5.2): `top`, the table, `under` and the selected card;
/// spare rows stay at the bottom (VIS-5). `top` and `under` are a blank row, a rule and a blank
/// row (3 rows; 2 once the blank under the rule is dropped, the rule then last; 1 for the rule
/// alone; 0 when dropped). `keys` is whether the card's last row is its keys.
struct Parts {
    top: Rect,
    table: Rect,
    under: Rect,
    card: Rect,
    keys: bool,
}

/// T3-22, S§5.4: the table's least lines (`TABLE_LEAST`) and the card's least are reserved
/// first. The rows left go, in the reverse of the order in which a short body loses them: to the
/// two rules, the card's keys row, the blanks of `top`, the blanks of `under`, the card's other
/// windows (they scroll by `J`/`K`), and last the table's other lines (it scrolls with the
/// selection). So a short body cuts the table first, then the card's windows, then the
/// separators' blanks, the lower ones before the upper, then the keys row, and last the rules. A
/// part the body has no room left for is empty.
fn parts(state: &State, body: Rect, now_ms: i64) -> Parts {
    let rows = |n: usize| u16::try_from(n).unwrap_or(u16::MAX);
    let width = body.width;
    let Some(v) = state.accounts.selected_view() else {
        let none = |y| Rect::new(body.x, y, width, 0);
        return Parts {
            top: none(body.y),
            table: body,
            under: none(body.y + body.height),
            card: none(body.y + body.height),
            keys: false,
        };
    };
    let lines = rows(table_lines(state));
    let table_least = lines.min(rows(TABLE_LEAST));
    let card_most = rows(card::height(state, v, width, now_ms));
    let card_least = rows(card::min_height(state, v, width, now_ms));
    let mut spare = body
        .height
        .saturating_sub(table_least.saturating_add(card_least));
    let mut give = |most: u16| {
        let n = most.min(spare);
        spare -= n;
        n
    };
    let (top_rule, under_rule) = (give(1), give(1));
    let keys = give(1);
    let top = top_rule + give(2);
    let under = under_rule + give(2);
    let card = card_least + keys + give(card_most - card_least - 1);
    let table = table_least + give(lines - table_least);
    let bottom = body.y + body.height;
    let mut y = body.y;
    let mut take = |n: u16| {
        let at = y.min(bottom);
        y = at + n.min(bottom - at);
        Rect::new(body.x, at, width, y - at)
    };
    let top = take(top);
    let table = take(table);
    let under = take(under);
    let card = take(card);
    Parts {
        top,
        table,
        under,
        card,
        keys: keys == 1,
    }
}

/// A separator of `rows` rows (`Parts::top`, `Parts::under`): blank, rule, blank; the rule last
/// with 2 rows, alone with 1.
fn separator(state: &State, width: u16, rows: u16) -> Vec<Line<'static>> {
    let rule = chrome::rule(state, width);
    match rows {
        0 => vec![],
        1 => vec![rule],
        2 => vec![Line::default(), rule],
        _ => vec![Line::default(), rule, Line::default()],
    }
}

/// The table's lines: its pinned header, with one provider, its scrolling lines, and the hint
/// to add a second account when focus's provider has one (§5.7, T3-7).
fn table_lines(state: &State) -> usize {
    let st = &state.accounts;
    layout(st).len() + usize::from(sections(st).len() == 1) + usize::from(one_account(state))
}

/// Whether focus's provider has exactly one account: the table says how to add a second.
fn one_account(state: &State) -> bool {
    state
        .view_of(&daemon::focus(state))
        .is_some_and(|p| p.accounts.len() == 1)
}

/// How many scrolling table lines fit in `body`: one provider's header is pinned above them.
fn visible_rows(state: &State, body: Rect, now_ms: i64) -> usize {
    let pinned = sections(&state.accounts).len() == 1;
    usize::from(parts(state, body, now_ms).table.height).saturating_sub(usize::from(pinned))
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
    if st.view == View::List {
        list::render(state, frame, area, now_ms);
        return;
    }
    let now_s = now_ms.div_euclid(1000);
    let width = usize::from(area.width);
    let parts = parts(state, area, now_ms);
    let top = separator(state, area.width, parts.top.height);
    frame.render_widget(Paragraph::new(top), parts.top);
    let table_area = parts.table;
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
    let rows = visible_rows(state, area, now_ms);
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
    if one_account(state) {
        let room = usize::from(table_area.height).saturating_sub(shown.len());
        if room > 0 {
            let display = state.names_of(&daemon::focus(state)).display_name;
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
    let under = separator(state, area.width, parts.under.height);
    frame.render_widget(Paragraph::new(under), parts.under);
    if let Some(v) = st.selected_view() {
        let rows = usize::from(parts.card.height);
        let from = st.card_from;
        let lines = card::lines(state, v, area.width, rows, from, parts.keys, now_ms);
        frame.render_widget(Paragraph::new(lines), parts.card);
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyCode;
    use ratatui::buffer::Buffer;
    use ratatui::style::{Color, Modifier};
    use tagteam_core::{Pace, ProviderId, WindowKind};
    use tagteam_engine::switch::LiveLogin;
    use tagteam_engine::timeline::Scope;
    use tagteam_engine::views::{ProviderAccounts, UsageStatus, UsageView};

    use super::*;
    use crate::msg::Msg;
    use crate::state::{BannerKind, Screen};
    use crate::testutil::*;
    use crate::theme::Theme;
    use crate::update::update;
    use crate::view::width_of;

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

    /// `three()` in `theme`, the selection on row 2, where it starts: the account in use.
    fn on_work(theme: Theme) -> State {
        let mut state = loaded(three());
        state.theme = theme;
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

    /// The table's first row on a `w`×`h` terminal (S§3.1: under the rule and its blank rows).
    fn table_y(state: &State, w: u16, h: u16) -> u16 {
        rows_of_part(state, w, h, |p| p.table).0
    }

    /// What the table draws at `w`×`h` and `now_ms`, trailing spaces trimmed.
    fn table_of(state: &State, w: u16, h: u16, now_ms: i64) -> Vec<String> {
        let body = crate::view::areas(state, Rect::new(0, 0, w, h)).body;
        let at = parts(state, body, now_ms).table;
        let rows = draw_at(state, w, h, now_ms);
        rows[usize::from(at.y)..usize::from(at.y + at.height)].to_vec()
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
        let mut state = loaded(three());
        update(&mut state, key(KeyCode::Home), NOW_MS);
        let state = state;
        let at = |seconds: i64| draw_at(&state, 120, 24, NOW_MS + seconds * 1000);
        // UPDATED, right-aligned under its head.
        let row_age = |lines: &[String]| {
            let head = lines.iter().find(|l| l.contains("UPDATED"))?;
            let from = col(head, "UPDATED");
            // The table's row, not the Now strip's pick above it.
            let row = lines
                .iter()
                .skip_while(|l| !l.contains("UPDATED"))
                .find(|l| l.contains("alex@example.com") && l.contains('%'));
            row.map(|l| columns(l, from, from + 7).trim().to_owned())
        };
        // The card's title, the first row selected.
        let card = |lines: &[String]| {
            let line = lines.iter().find(|l| l.contains("updated "))?;
            let age = line.split("updated ").nth(1)?.split(" ago").next()?;
            Some(age.to_owned())
        };
        let now = at(0);
        assert_eq!(row_age(&now).as_deref(), Some("2m"), "{now:?}");
        assert_eq!(card(&now).as_deref(), Some("2m"), "{now:?}");
        let later = at(185);
        assert_eq!(row_age(&later).as_deref(), Some("5m"), "{later:?}");
        assert_eq!(card(&later).as_deref(), Some("5m"), "{later:?}");
        // A clock behind the read never makes an age shorter than the reading's own.
        let earlier = at(-500);
        assert_eq!(row_age(&earlier).as_deref(), Some("2m"), "{earlier:?}");
        assert_eq!(card(&earlier).as_deref(), Some("2m"), "{earlier:?}");
    }

    #[test]
    fn the_table_at_120_columns_has_14_cell_gauges_and_a_status_column() {
        assert_eq!(
            snapshot(three()).providers[0].forecast,
            tagteam_engine::switch::BestForecast::Pick { order: vec![1] }
        );
        for theme in TIERS {
            let state = on_work(theme);
            let body = table_of(&state, 120, 40, NOW_MS);
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
            let y = table_y(&state, 120, 40);
            let heads: String = roles(&buf, theme, y)
                .chars()
                .filter(|c| *c != ' ')
                .collect();
            let muted = canon(theme, "m").repeat(heads.chars().count());
            assert_eq!(heads, muted, "{theme:?}: the heads are muted");
            // Tier 0 draws the tracks as `·` (§4.1).
            let track = if theme.colourless() { "·" } else { "─" };
            assert_eq!(
                body[2],
                format!(
                    "▌   2 ● work   sam@corp.example{}━━━━━━━{}  53%{}╸{}   6%{}1m  in use",
                    " ".repeat(8),
                    track.repeat(7),
                    " ".repeat(4),
                    track.repeat(13),
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
        let body = table_of(&state, 80, 24, NOW_MS);
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
        let mut off = account(5, "a5@x.co", read(840, 9.0, 12.0, false));
        off.row.disabled = true;
        let accounts = vec![
            account(1, "alex@example.com", read(60, 9.0, 12.0, false)),
            account(2, "sam@corp.example", read(840, 9.0, 12.0, false)),
            account(3, "alex@personal.example", retrying),
            account(4, "a4@x.co", limited),
            off,
        ];
        for theme in TIERS {
            let mut state = loaded(accounts.clone());
            state.theme = theme;
            let body = table_of(&state, 120, 40, NOW_MS);
            let updated = col(&body[0], "UPDATED");
            let buf = draw_buffer(&state, 120, 40, NOW_MS);
            let y = table_y(&state, 120, 40);
            let rows = [
                ("1m", "m", "best"),
                ("14m", "w", "! old reading"),
                ("14m", "w", "! retrying"),
                ("1m", "m", "! rate-limited"),
                // M-8: disabled over the old reading, its age still in warn.
                ("14m", "w", "○ disabled"),
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
        // Under 100 columns the disabled row's glyph is `○`, not its old reading's `!`.
        let body = table_of(&loaded(accounts), 80, 24, NOW_MS);
        assert!(body[2].ends_with("14m !"), "{}", body[2]);
        assert!(body[5].ends_with("14m ○"), "{}", body[5]);
    }

    #[test]
    fn the_table_draws_no_pace_mark() {
        // The expected shares draw no mark in the table, on a long window or a short one.
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
            let body = table_of(&state, 120, 40, NOW_MS);
            assert!(
                body.iter().all(|row| !row.contains(['┿', '┼'])),
                "{theme:?}: {body:?}"
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
            let body = table_of(&state, 120, 40, NOW_MS);
            assert!(body[1].ends_with("  ✕ new login"), "{}", body[1]);
            assert_eq!(columns(&body[1], 6, 7), "●", "{}", body[1]);
            assert!(body[2].ends_with("  ✕ new login"), "{}", body[2]);
            assert!(body[3].ends_with("  ○ disabled"), "{}", body[3]);
            let buf = draw_buffer(&state, 120, 40, NOW_MS);
            let y = table_y(&state, 120, 40) + 1;
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
        // Each problem comes before this session, in use and best. Disabled comes after the
        // `✕` problems and before every `!` one (M-8).
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
        let disabled = ("○ disabled".to_owned(), Role::Muted);
        for (usage, want) in problems {
            let mut v = account(1, "a1@x.co", usage);
            v.active = true;
            let own = want.map(|(p, r)| (p.to_owned(), r));
            // Enabled: its own phrase, over this session, in use and best.
            if let Some(own) = &own {
                assert_eq!(status::phrase(&v, true, true).as_ref(), Some(own));
            }
            v.row.disabled = true;
            let want = match own {
                Some((p, r)) if r == Role::Crit => (p, r),
                _ => disabled.clone(),
            };
            assert_eq!(
                status::phrase(&v, true, true),
                Some(want.clone()),
                "{want:?}"
            );
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
            let body = table_of(&state, 120, 40, NOW_MS);
            assert_eq!(
                body[1],
                format!(
                    "▌   1   alex@example.com   ! usage check failed (a-token-from-a-newer-build)   {:>7}  ! retrying",
                    "—"
                ),
                "{theme:?}"
            );
            let buf = draw_buffer(&state, 120, 40, NOW_MS);
            let row = roles(&buf, theme, table_y(&state, 120, 40) + 1);
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
                let body = table_of(&state, w, 40, NOW_MS);
                let head = &body[0];
                let from = col(head, "5H");
                let to = col(head, "UPDATED") - 3;
                assert_eq!(to - from, if w == 80 { 34 } else { 44 }, "{head}");
                let updated = col(head, "UPDATED");
                let buf = draw_buffer(&state, w, 40, NOW_MS);
                let y = table_y(&state, w, 40);
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
        let body = table_of(&loaded(accounts), 80, 24, NOW_MS);
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
            let body = table_of(&state, 120, 40, NOW_MS);
            let marks: Vec<String> = body[1..5].iter().map(|l| columns(l, 6, 7)).collect();
            assert_eq!(marks, ["▶", "●", "○", " "], "{body:?}");
            let buf = draw_buffer(&state, 120, 40, NOW_MS);
            let y = table_y(&state, 120, 40);
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

    /// The card's rows at `w`×`h` and `now_ms`, under the table, and the row it starts at.
    fn card_at(state: &State, w: u16, h: u16, now_ms: i64) -> (u16, Vec<String>) {
        let body = crate::view::areas(state, Rect::new(0, 0, w, h)).body;
        let pane = parts(state, body, now_ms).card;
        let rows = draw_at(state, w, h, now_ms);
        let (from, to) = (usize::from(pane.y), usize::from(pane.y + pane.height));
        (pane.y, rows[from..to].to_vec())
    }

    fn card_rows(state: &State, w: u16, h: u16) -> (u16, Vec<String>) {
        card_at(state, w, h, NOW_MS)
    }

    /// The first word of each of the card's window rows: its label's.
    fn card_windows(state: &State, w: u16, h: u16) -> Vec<String> {
        let v = state.accounts.selected_view().unwrap();
        let labels: Vec<&str> = v
            .usage
            .windows
            .iter()
            .flatten()
            .filter_map(|(w, _)| w.label.split_whitespace().next())
            .collect();
        card_rows(state, w, h)
            .1
            .iter()
            .filter_map(|l| l.split_whitespace().next())
            .filter(|word| labels.contains(word))
            .map(str::to_owned)
            .collect()
    }

    /// The role letters `roles` reads under `needle` in row `y` of `buf`, and the ones `role`
    /// draws it in.
    fn role_of(buf: &Buffer, theme: Theme, y: u16, needle: &str, role: char) -> (String, String) {
        let line = &text_of(buf)[usize::from(y)];
        let at = col(line, needle);
        let n = needle.chars().count();
        let got: String = roles(buf, theme, y).chars().skip(at).take(n).collect();
        let want: String = needle
            .chars()
            .map(|c| if c == ' ' { ' ' } else { role })
            .collect();
        (got, canon(theme, &want))
    }

    #[test]
    fn the_selected_card_shows_its_facts_windows_problem_and_keys() {
        let mut accounts = three();
        accounts[2].usage.next_poll_at = Some(NOW_S + 120);
        for theme in TIERS {
            let mut state = loaded(accounts.clone());
            state.theme = theme;
            update(&mut state, key(KeyCode::End), NOW_MS);
            let (y, card) = card_rows(&state, 120, 40);
            let track = if theme.colourless() { "·" } else { "─" };
            // Windows at 2, 26-cell thin gauges, the label column `7d` + 4 wide.
            assert_eq!(
                card,
                [
                    "SELECTED  3 spare  alex@personal.example".to_owned(),
                    "          not in use · updated 14m ago · next update in 2m".to_owned(),
                    format!("  5h    {}   0%    resets in 2h40m", track.repeat(26)),
                    format!(
                        "  7d    {}{}  31%    resets in 3d09h",
                        "━".repeat(8),
                        track.repeat(18)
                    ),
                    "  ! Old reading: too old to pick the best account by.".to_owned(),
                    "    tagteam checks it within 2m.".to_owned(),
                    "          s switch   ⏎ history   e disable   a alias   x remove".to_owned(),
                ],
                "{theme:?}"
            );
            let buf = draw_buffer(&state, 120, 40, NOW_MS);
            let check = |row: u16, needle: &str, role: char| {
                let (got, want) = role_of(&buf, theme, y + row, needle, role);
                assert_eq!(got, want, "{theme:?} {needle:?}");
            };
            // The title: SELECTED and the email muted.
            check(0, "SELECTED", 'm');
            check(0, "alex@personal.example", 'm');
            // The facts: muted, the old reading's age in warn.
            check(1, "not in use", 'm');
            check(1, "updated 14m ago", 'w');
            check(1, "next update in 2m", 'm');
            // The windows: the fill in accent on a faint track, the reset in text.
            check(3, &"━".repeat(8), 'a');
            check(3, &track.repeat(18), 'f');
            check(3, "resets in 3d09h", 't');
            // The problem in its role, its remedy in text.
            check(
                4,
                "! Old reading: too old to pick the best account by.",
                'w',
            );
            check(5, "tagteam checks it within 2m.", 't');
            // The keys accent, their labels muted.
            for (key, label) in [
                ("s", "switch"),
                ("⏎", "history"),
                ("e", "disable"),
                ("a", "alias"),
                ("x", "remove"),
            ] {
                check(6, label, 'm');
                let at = col(&card[6], &format!("{key} {label}"));
                let letter = roles(&buf, theme, y + 6).chars().nth(at);
                assert_eq!(letter, canon(theme, "a").chars().next(), "{theme:?} {key}");
            }
            // The alias is the title's one bold, past tier 0's roles.
            if theme != TIERS[0] {
                assert_eq!(
                    bold_cells(&buf, y),
                    (12..17).collect::<Vec<_>>(),
                    "{theme:?}"
                );
            }
        }
    }

    /// `loaded(mock())`, moved with `j`/`k` onto the account at `position`.
    fn state_selecting(position: u32) -> State {
        let mut state = loaded(mock());
        let at = |state: &State| state.accounts.selected().unwrap().position;
        while at(&state) < position {
            update(&mut state, ch('j'), NOW_MS);
        }
        while at(&state) > position {
            update(&mut state, ch('k'), NOW_MS);
        }
        state
    }

    #[test]
    fn the_card_reads_selected_then_facts_windows_and_keys() {
        let (_, rows) = card_rows(&state_selecting(1), 80, 24);
        assert!(
            rows[0].starts_with("SELECTED  1 alex@example.com"),
            "{rows:?}"
        );
        assert_eq!(col(&rows[1], "not in use"), 10, "{rows:?}");
        assert!(rows[2].starts_with("  5h     "), "{rows:?}");
        assert!(rows[4].starts_with("  Fable  "), "{rows:?}");
        let keys = rows.last().unwrap();
        assert_eq!(col(keys, "s switch"), 10, "{rows:?}");
        assert!(keys.contains("s switch   ⏎ history   e disable   a alias   x remove"));
    }

    #[test]
    fn the_card_of_an_account_not_in_use_draws_thin_bars() {
        // The on-screen half of the card's bars: only `━`, `╸` and the track, no thick cells.
        let (_, rows) = card_rows(&state_selecting(1), 80, 24);
        for row in &rows[2..5] {
            let bar: String = row.chars().skip(9).take(16).collect();
            assert!(bar.chars().all(|c| "━╸─·".contains(c)), "{row}");
        }
    }

    #[test]
    fn the_in_use_card_says_in_use_and_hides_s_but_s_still_switches() {
        let mut state = state_selecting(2);
        let (y, rows) = card_rows(&state, 80, 24);
        assert!(rows[1].starts_with("          IN USE"), "{rows:?}");
        assert!(
            rows.last().unwrap().starts_with("          ⏎ history"),
            "{rows:?}"
        );
        let buf = draw_buffer(&state, 80, 24, NOW_MS);
        let (got, want) = role_of(&buf, state.theme, y + 1, "IN USE", 'a');
        assert_eq!(got, want);
        let cmds = update(&mut state, ch('s'), NOW_MS);
        let switch = crate::host::Action::Switch(state.accounts.selected().unwrap());
        assert_eq!(cmds, [crate::msg::Cmd::Act(switch)]);
    }

    #[test]
    fn an_account_without_windows_shows_the_table_s_words_or_its_problem() {
        // An API key: the title, the facts, one muted row from column 2, and the keys.
        let mut api = account(4, "a4@x.co", unread(UsageStatus::ApiKey, None, None));
        api.kind = API_KEY;
        let mut accounts = three();
        accounts.push(api);
        let mut state = loaded(accounts);
        update(&mut state, key(KeyCode::End), NOW_MS);
        let (y, rows) = card_rows(&state, 80, 24);
        assert_eq!(rows.len(), 4, "{rows:?}");
        assert_eq!(rows[2], "  api key · has no usage limits to show");
        let buf = draw_buffer(&state, 80, 24, NOW_MS);
        let (got, want) = role_of(
            &buf,
            state.theme,
            y + 2,
            "api key · has no usage limits to show",
            'm',
        );
        assert_eq!(got, want);
        // A problem says why there is no reading: its rows once, and no row of the table's words.
        let mut accounts = three();
        accounts[2].usage = unread(UsageStatus::ReloginRequired, None, None);
        accounts[2].row.quarantine_reason = Some("invalid_grant".into());
        let mut state = loaded(accounts);
        update(&mut state, key(KeyCode::End), NOW_MS);
        let rows = card_rows(&state, 80, 24).1;
        assert_eq!(rows.len(), 5, "{rows:?}");
        assert!(rows[2].starts_with("  ✕ Needs a new login"), "{rows:?}");
        assert!(rows[3].starts_with("    Log in to Claude Code"), "{rows:?}");
    }

    #[test]
    fn compact_shows_exactly_one_selected_block_in_every_tier_at_80_by_24() {
        for theme in TIERS {
            let mut state = loaded(mock());
            state.theme = theme;
            let rows = draw(&state, 80, 24);
            let selected: Vec<usize> = (0..rows.len())
                .filter(|&i| rows[i].contains("SELECTED"))
                .collect();
            assert_eq!(selected, [14], "{theme:?}: {rows:?}");
        }
    }

    #[test]
    fn next_update_ends_the_facts_from_120_columns_only() {
        let mut accounts = three();
        accounts[0].usage.next_poll_at = Some(NOW_S + 60);
        let mut state = loaded(accounts);
        update(&mut state, key(KeyCode::Home), NOW_MS);
        assert!(card_rows(&state, 120, 40).1[1].ends_with("· next update in 1m"));
        assert!(!card_rows(&state, 80, 24).1[1].contains("next update"));
    }

    #[test]
    fn a_short_body_drops_the_keys_row_after_the_blank_rows() {
        // S§5.4: with the table and the card's windows at their least and both pairs of blank
        // rows gone (10), the keys row goes (9), before the rules (8, 7).
        let state = loaded(mock());
        let at = |h| parts(&state, Rect::new(0, 0, 80, h), NOW_MS);
        let p = at(10);
        assert!(p.keys && p.top.height == 1 && p.under.height == 1);
        let p = at(9);
        assert!(!p.keys && p.top.height == 1 && p.under.height == 1);
        assert_eq!(at(8).under.height, 0);
    }

    #[test]
    fn a_problem_shows_its_sentence_and_remedy_under_the_windows() {
        let mut accounts = three();
        let spare = &mut accounts[2];
        spare.usage = unread(UsageStatus::ReloginRequired, None, None);
        spare.row.quarantine_reason = Some("invalid_grant".into());
        // Saturday 09:02, two days before `NOW_S` (a Monday, 14:13:20 UTC).
        spare.row.quarantine_at = Some((NOW_S - 2 * 86_400 - (5 * HOUR + 11 * 60 + 20)) * 1000);
        for theme in TIERS {
            let mut state = loaded(accounts.clone());
            state.theme = theme;
            update(&mut state, key(KeyCode::End), NOW_MS);
            let (y, card) = card_rows(&state, 120, 40);
            assert_eq!(
                card,
                [
                    "SELECTED  3 spare  alex@personal.example",
                    "          not in use",
                    "  ✕ Needs a new login: its stored login was refused (invalid_grant), Sat 09:02.",
                    "    Log in to Claude Code as alex@personal.example, then press A here.",
                    "          s switch   ⏎ history   e disable   a alias   x remove",
                ],
                "{theme:?}"
            );
            let buf = draw_buffer(&state, 120, 40, NOW_MS);
            let (got, want) = role_of(&buf, theme, y + 2, &card[2][2..], 'c');
            assert_eq!(got, want, "{theme:?}");
            let (got, want) = role_of(&buf, theme, y + 3, &card[3][4..], 't');
            assert_eq!(got, want, "{theme:?}");
            // At 80 columns the sentence, from column 2, still fits one row.
            let (y, card) = card_rows(&state, 80, 24);
            assert_eq!(
                card[2..4],
                [
                    "  ✕ Needs a new login: its stored login was refused (invalid_grant), Sat 09:02.",
                    "    Log in to Claude Code as alex@personal.example, then press A here.",
                ],
                "{theme:?}"
            );
            let buf = draw_buffer(&state, 80, 24, NOW_MS);
            let (got, want) = role_of(&buf, theme, y + 2, "09:02.", 'c');
            assert_eq!(got, want, "{theme:?}");
        }
        // M-8: a disabled account's table says `○ disabled`; its card still says what is wrong.
        let mut accounts = three();
        let spare = &mut accounts[2];
        spare.row.disabled = true;
        spare.usage.status = UsageStatus::Unavailable;
        spare.usage.error = Some("http-429".into());
        spare.usage.retry_at = Some(NOW_S + 900);
        let mut state = loaded(accounts);
        update(&mut state, key(KeyCode::End), NOW_MS);
        let body = table_of(&state, 120, 40, NOW_MS);
        assert!(body[3].ends_with("  ○ disabled"), "{}", body[3]);
        let card = card_rows(&state, 120, 40).1;
        assert!(
            card[1].ends_with("not in use · disabled · updated 14m ago"),
            "{card:?}"
        );
        assert_eq!(
            card[4..6],
            [
                "  ! Rate-limited: the usage check was turned away for now.",
                "    tagteam tries again in 15m.",
            ]
        );
        assert_eq!(
            card[6],
            "          s switch   ⏎ history   e enable   a alias   x remove"
        );
    }

    #[test]
    fn a_window_without_a_reset_says_not_started() {
        let mut usage = read(60, 9.0, 77.0, false);
        let windows = usage.windows.as_mut().unwrap();
        windows[0].0.resets_at = None;
        windows[1].1 = Pace {
            expected_pct: Some(62.0),
            ahead: Some(true),
            ..Pace::default()
        };
        windows.push((
            window("scoped:fable", "Fable", WindowKind::Scoped, 40.0, None),
            Pace::default(),
        ));
        windows.push((
            window("scoped:opus", "Opus", WindowKind::Scoped, 10.0, Some(-60)),
            Pace::default(),
        ));
        let state = loaded(vec![account(1, "alex@example.com", usage)]);
        let card = card_rows(&state, 120, 40).1;
        assert!(card[2].ends_with("   9%    not started"), "{card:?}");
        // `▲` puts what it means last in the tail, when the row has room: it has at 80 too.
        let pace = " 77% ▲  resets in 3d09h · 62% expected by now";
        assert!(card[3].ends_with(pace), "{card:?}");
        // A scoped window without a reset has none to say; one whose reset passed says so.
        assert!(card[4].ends_with(" 40%"), "{card:?}");
        assert!(card[5].ends_with(" 10%    reset"), "{card:?}");
        let narrow = card_rows(&state, 80, 24).1;
        assert!(narrow[3].ends_with(pace), "{narrow:?}");
    }

    #[test]
    fn the_card_title_cuts_the_name_and_leaves_the_facts_their_own_row() {
        // Review Focus 3: the facts are the card's to say.
        let mut v = account(1, "alex@example.com", read(120, 9.0, 12.0, false));
        v.row.alias = Some("a".repeat(200));
        let card = card_rows(&loaded(vec![v]), 80, 24).1;
        assert!(card[0].starts_with("SELECTED  1 aaaa"), "{}", card[0]);
        assert!(card[0].ends_with('…'), "{}", card[0]);
        assert_eq!(width_of(&card[0]), 80);
        assert_eq!(card[1], "          not in use · updated 2m ago");
    }

    #[test]
    fn the_card_title_cuts_a_long_email_and_the_facts_row_says_what_the_session_owns() {
        // Whether the account is session-owned is the card's to say.
        let long = "a".repeat(70) + "@example.com";
        let mut v = account(1, &long, read(60, 9.0, 12.0, false));
        v.in_session = true;
        let card = card_rows(&loaded(vec![v]), 80, 24).1;
        assert!(card[0].starts_with("SELECTED  1 aaaa"), "{}", card[0]);
        assert!(card[0].ends_with('…'), "{}", card[0]);
        assert_eq!(width_of(&card[0]), 80);
        assert_eq!(
            card[1],
            "          not in use · in a session · updated 1m ago"
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

    /// `M1` … `Mn`.
    fn models(n: usize) -> Vec<String> {
        (1..=n).map(|i| format!("M{i}")).collect()
    }

    /// `with_windows` for `models(n)`: 5h, 7d and `n` scoped windows.
    fn scoped(n: usize) -> UsageView {
        let models = models(n);
        with_windows(&models.iter().map(String::as_str).collect::<Vec<_>>())
    }

    #[test]
    fn every_window_is_reachable_by_shift_j_and_k_at_80x24_under_a_full_banner() {
        let banner = ["the store could not be read"; 12].join("; ");
        let accounts = |usage: UsageView| {
            vec![
                account(1, "alex@example.com", usage),
                account(2, "sam@corp.example", read(60, 9.0, 12.0, false)),
                account(3, "alex@personal.example", read(60, 9.0, 12.0, false)),
            ]
        };
        // Four windows fit whole in what the 17-row body leaves the card under a 3-row banner
        // (7 rows: the title, the facts, the keys and four windows): the table and both
        // separators keep their rows before a card window does.
        let mut state = loaded(accounts(scoped(2)));
        state.raise_banner(BannerKind::Refusal, banner.clone());
        let shown = card_windows(&state, 80, 24);
        assert_eq!(shown.len(), 4, "{shown:?}");
        // With nothing hidden there is nothing to hint at.
        let hint = |state: &State, h: u16| {
            let (y, card) = card_rows(state, 80, h);
            let at = card
                .iter()
                .position(|l| l.trim_start().starts_with("J K:"))?;
            Some((y + at as u16, card[at].clone()))
        };
        assert_eq!(hint(&state, 24), None);
        update(&mut state, ch('J'), NOW_MS);
        assert_eq!(card_windows(&state, 80, 24), shown);
        // Twenty-four do not: `J` until nothing moves shows the last.
        let mut state = loaded(accounts(scoped(22)));
        state.raise_banner(BannerKind::Refusal, banner);
        let first = card_windows(&state, 80, 24);
        assert_eq!(first[0], "5h");
        assert!(first.len() < 24, "{first:?}");
        // The last window row says which show, muted, in the other screens' words.
        let (y, said) = hint(&state, 24).unwrap();
        assert_eq!(said, format!("  J K: windows 1–{} of 24", first.len()));
        let buf = draw_buffer(&state, 80, 24, NOW_MS);
        let (got, want) = role_of(&buf, Theme::new(false), y, &said[2..], 'm');
        assert_eq!(got, want);
        update(&mut state, ch('J'), NOW_MS);
        let moved = format!("  J K: windows 2–{} of 24", first.len() + 1);
        assert_eq!(hint(&state, 24).map(|h| h.1), Some(moved));
        loop {
            let before = card_windows(&state, 80, 24);
            update(&mut state, ch('J'), NOW_MS);
            if card_windows(&state, 80, 24) == before {
                break;
            }
        }
        let last = card_windows(&state, 80, 24);
        assert_eq!(last.last().map(String::as_str), Some("M22"), "{last:?}");
        assert_eq!(last.len(), first.len(), "whole rows, the same count");
        let end = format!("  J K: windows {}–24 of 24", 25 - last.len());
        assert_eq!(hint(&state, 24).map(|h| h.1), Some(end));
        assert!(draw(&state, 80, 24).iter().all(|l| width_of(l) <= 80));
        // Taller: the scroll is clamped, so the last window shows from further up, and `K`
        // moves at once.
        update(
            &mut state,
            Msg::Resize {
                width: 80,
                height: 30,
            },
            NOW_MS,
        );
        let taller = card_windows(&state, 80, 30);
        assert_eq!(taller.last().map(String::as_str), Some("M22"), "{taller:?}");
        assert!(taller.len() > last.len(), "{taller:?}");
        let from = 25 - taller.len();
        let clamped = format!("  J K: windows {from}–24 of 24");
        assert_eq!(hint(&state, 30).map(|h| h.1), Some(clamped));
        update(&mut state, ch('K'), NOW_MS);
        let up = card_windows(&state, 80, 30);
        assert_eq!(up[1..], taller[..taller.len() - 1], "{up:?}");
        let back = format!("  J K: windows {}–23 of 24", from - 1);
        assert_eq!(hint(&state, 30).map(|h| h.1), Some(back));
    }

    #[test]
    fn the_card_shows_the_selected_account() {
        let mut accounts = three();
        accounts[0].usage = read(120, 9.0, 77.0, false);
        accounts[0].usage.windows.as_mut().unwrap()[1].1 = Pace {
            expected_pct: Some(62.0),
            ahead: Some(true),
            rate_per_hour: Some(1.2),
            method: Some(tagteam_core::ProjectionMethod::Regression),
            exhaustion_at: Some(NOW_S + 187_200),
            will_last_to_reset: Some(false),
        };
        accounts[0].row.login_expires_at = Some((NOW_S + 6 * 86_400 + 3 * HOUR) * 1000);
        for theme in TIERS {
            let mut state = loaded(accounts.clone());
            state.theme = theme;
            update(&mut state, key(KeyCode::Home), NOW_MS);
            let (y, card) = card_rows(&state, 120, 40);
            let track = if theme.colourless() { "·" } else { "─" };
            assert_eq!(
                card,
                [
                    "SELECTED  1 alex@example.com".to_owned(),
                    "          not in use · updated 2m ago · login valid 6 more days · log in again before then".to_owned(),
                    format!("  5h    {}{}   9%    resets in 2h40m", "━".repeat(2), track.repeat(24)),
                    format!(
                        "  7d    {}{}  77% ▲  resets in 3d09h · +1.2%/h · full in 2d04h · 62% expected by now",
                        "━".repeat(20),
                        track.repeat(6)
                    ),
                    "          s switch   ⏎ history   e disable   a alias   x remove".to_owned(),
                ],
                "{theme:?}"
            );
            // The window fills before it resets: its projection is warn, as `▲` is.
            let buf = draw_buffer(&state, 120, 40, NOW_MS);
            let (got, want) = role_of(&buf, theme, y + 3, "+1.2%/h · full in 2d04h", 'w');
            assert_eq!(got, want, "{theme:?}");
            let (got, want) = role_of(&buf, theme, y + 3, "▲", 'w');
            assert_eq!(got, want, "{theme:?}");
            let (got, want) = role_of(&buf, theme, y + 3, "77%", 'w');
            assert_eq!(got, want, "{theme:?}");
        }
        // At 80 columns: 16 cells, and the row has no room left for `expected by now`; the
        // facts row gives way from its end.
        let mut state = loaded(accounts);
        update(&mut state, key(KeyCode::Home), NOW_MS);
        let card = card_rows(&state, 80, 24).1;
        assert_eq!(
            card,
            [
                "SELECTED  1 alex@example.com".to_owned(),
                "          not in use · updated 2m ago · login valid 6 more days · log in again …"
                    .to_owned(),
                format!("  5h    {}{}   9%    resets in 2h40m", "━", "·".repeat(15)),
                format!(
                    "  7d    {}{}  77% ▲  resets in 3d09h · +1.2%/h · full in 2d04h",
                    "━".repeat(12),
                    "·".repeat(4)
                ),
                "          s switch   ⏎ history   e disable   a alias   x remove".to_owned(),
            ]
        );
    }

    #[test]
    fn a_row_without_a_reading_says_why_in_the_card() {
        let mut accounts = three();
        accounts[2].usage = unread(UsageStatus::ReloginRequired, None, None);
        accounts[2].row.quarantine_reason = Some("invalid_grant".into());
        let mut state = loaded(accounts);
        update(&mut state, key(KeyCode::End), NOW_MS);
        // No quarantine time: the sentence ends at its reason.
        assert_eq!(
            card_rows(&state, 80, 24).1,
            [
                "SELECTED  3 spare  alex@personal.example",
                "          not in use",
                "  ✕ Needs a new login: its stored login was refused (invalid_grant).",
                "    Log in to Claude Code as alex@personal.example, then press A here.",
                "          s switch   ⏎ history   e disable   a alias   x remove",
            ]
        );
    }

    #[test]
    fn the_card_counts_down_to_the_next_update_while_one_is_planned() {
        let mut accounts = three();
        accounts[0].usage.next_poll_at = Some(NOW_S + 60);
        accounts[0].row.login_expires_at = Some((NOW_S + 6 * 86_400) * 1000);
        let mut state = loaded(accounts);
        update(&mut state, key(KeyCode::Home), NOW_MS);
        // `next update` is the last fact.
        assert_eq!(
            card_rows(&state, 140, 40).1[1],
            "          not in use · updated 2m ago · login valid 6 more days · log in again before then · next update in 1m"
        );
        // Once it has come, a frame drawn before the update lands says it is close.
        assert_eq!(
            card_at(&state, 140, 40, NOW_MS + 60_000).1[1],
            "          not in use · updated 3m ago · login valid 5 more days · log in again before then · next update in <1m"
        );
        // Below 120 columns it is not said, and the facts give way from their end.
        let facts = card_rows(&state, 80, 24).1[1].clone();
        assert_eq!(
            facts,
            "          not in use · updated 2m ago · login valid 6 more days · log in again …"
        );
    }

    #[test]
    fn the_card_announces_no_update_the_status_rules_out() {
        // A quarantined account keeps its stored plan, but no poll comes (core §8.3).
        let mut accounts = three();
        accounts[2].usage = unread(UsageStatus::ReloginRequired, None, None);
        accounts[2].row.quarantine_reason = Some("invalid_grant".into());
        accounts[2].usage.next_poll_at = Some(NOW_S + 180);
        let mut state = loaded(accounts);
        update(&mut state, key(KeyCode::End), NOW_MS);
        let lines = draw(&state, 80, 24);
        assert!(
            !lines.iter().any(|l| l.contains("next update")),
            "{lines:?}"
        );
        // A backoff later than the plan: the remedy's retry is the only time given.
        let mut backed_off = read(120, 9.0, 77.0, false);
        backed_off.status = UsageStatus::Unavailable;
        backed_off.error = Some("http-429".into());
        backed_off.retry_at = Some(NOW_S + 900);
        backed_off.next_poll_at = Some(NOW_S + 120);
        let state = loaded(vec![account(1, "alex@example.com", backed_off)]);
        let card = card_rows(&state, 80, 24).1;
        assert!(!card.iter().any(|l| l.contains("next update")), "{card:?}");
        assert_eq!(
            card[4..6],
            [
                "  ! Rate-limited: the usage check was turned away for now.",
                "    tagteam tries again in 15m.",
            ]
        );
    }

    #[test]
    fn only_an_account_never_checked_says_not_checked_yet() {
        // An API key has no reading to wait for, and a problem says why there is none.
        let mut api = account(4, "a4@x.co", unread(UsageStatus::ApiKey, None, None));
        api.kind = API_KEY;
        let mut accounts = three();
        accounts.push(api);
        let mut state = loaded(accounts);
        update(&mut state, key(KeyCode::End), NOW_MS);
        assert_eq!(
            card_rows(&state, 80, 24).1[..2],
            ["SELECTED  4 a4@x.co", "          not in use · api key"]
        );
        let mut accounts = three();
        accounts[2].usage = unread(UsageStatus::ReloginRequired, None, None);
        accounts[2].row.quarantine_reason = Some("invalid_grant".into());
        let mut state = loaded(accounts.clone());
        update(&mut state, key(KeyCode::End), NOW_MS);
        assert_eq!(
            card_rows(&state, 80, 24).1[..2],
            [
                "SELECTED  3 spare  alex@personal.example",
                "          not in use"
            ]
        );
        // Never read: it says so.
        accounts[2].usage = unread(UsageStatus::Unavailable, None, None);
        accounts[2].row.quarantine_reason = None;
        let mut state = loaded(accounts);
        update(&mut state, key(KeyCode::End), NOW_MS);
        assert_eq!(
            card_rows(&state, 80, 24).1[..2],
            [
                "SELECTED  3 spare  alex@personal.example",
                "          not in use · not checked yet"
            ]
        );
    }

    #[test]
    fn a_selected_card_s_expired_login_is_warn() {
        let mut accounts = three();
        accounts[0].row.login_expires_at = Some((NOW_S - HOUR) * 1000);
        for theme in TIERS {
            let mut state = loaded(accounts.clone());
            state.theme = theme;
            update(&mut state, key(KeyCode::Home), NOW_MS);
            let (y, card) = card_rows(&state, 120, 40);
            assert_eq!(
                card[1],
                "          not in use · updated 2m ago · login expired"
            );
            let buf = draw_buffer(&state, 120, 40, NOW_MS);
            let (got, want) = role_of(&buf, theme, y + 1, "login expired", 'w');
            assert_eq!(got, want, "{theme:?}");
            let (got, want) = role_of(&buf, theme, y + 1, "updated 2m ago", 'm');
            assert_eq!(got, want, "{theme:?}");
        }
    }

    /// The widest rows a card draws: a long scoped-model label, a spend value, and a reset, pace
    /// and projection on one window.
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
    fn a_window_too_wide_for_80_columns_is_cut_to_its_one_row() {
        // §4.5: 80×24 is enough; a window keeps one row and gives up its last words.
        let state = loaded(vec![account(1, "alex@example.com", widest())]);
        let card = card_rows(&state, 80, 24).1;
        assert_eq!(
            card_windows(&state, 80, 24),
            ["5h", "7d", "spend", "Claude"]
        );
        // The title, the facts, a row a window and the keys.
        assert_eq!(card.len(), 7, "a row a window: {card:?}");
        assert!(card.iter().all(|l| width_of(l) <= 80), "{card:?}");
        let label = |name: &str| format!("  {name:<17}  ");
        assert!(card[3].starts_with(&label("7d")), "{card:?}");
        assert!(
            card[3].contains(" 77% ▲  resets in 3d09h · +1.2") && card[3].ends_with('…'),
            "{card:?}"
        );
        assert!(card[4].starts_with(&label("spend")), "{card:?}");
        assert!(
            card[4].ends_with(" €123.45 of €200    resets in 14d00h"),
            "{card:?}"
        );
        assert!(card[5].starts_with(&label("Claude Sonnet 4.5")), "{card:?}");
        assert!(card[5].ends_with('…'), "{card:?}");
        // A wide terminal has room for each window whole, bar `expected by now`, which only
        // joins the tail when the whole row still fits.
        let wide = card_rows(&state, 120, 40).1;
        assert!(
            wide[3].ends_with(" 77% ▲  resets in 3d09h · +1.2%/h · full in 2d04h"),
            "{wide:?}"
        );
        assert!(
            wide[5].ends_with(" 41% ▲  resets in 3d09h · +1.5%/h · full in 1d00h"),
            "{wide:?}"
        );
        assert!(wide.iter().all(|l| !l.contains('…')), "{wide:?}");
    }

    /// An 80-column model name: any non-empty `display_name` is a label (the provider's §8.2).
    const OVERSIZED: &str =
        "Claude Opus 4.5 with extended thinking, for long-running agentic coding sessions";

    #[test]
    fn an_oversized_label_is_cut_to_its_column_and_the_column_stays_capped() {
        // The label column is a quarter of the card at most (T3-20), so every value and what
        // follows it keep their room.
        let mut usage = with_windows(&["Opus"]);
        usage.windows.as_mut().unwrap()[2].0.label = OVERSIZED.to_owned();
        let state = loaded(vec![account(1, "alex@example.com", usage)]);
        let card = card_rows(&state, 80, 40).1;
        assert!(card.iter().all(|l| width_of(l) <= 80), "{card:?}");
        assert!(
            card[2].starts_with(&format!("  {:<20}  ", "5h")),
            "{card:?}"
        );
        assert!(
            card[4].starts_with(&format!("  {}  ", fit(OVERSIZED, 20))),
            "{card:?}"
        );
        let value = col(&card[2], "  9%");
        assert_eq!(col(&card[4], " 40%"), value, "{card:?}");
        assert!(card[4].contains(" 40% ▲  resets in 3d09h"), "{card:?}");
    }

    /// A 1,000-character model name, as the provider accepts any non-empty `display_name`.
    fn thousand() -> String {
        "Claude ".repeat(143).trim_end().to_owned()
    }

    #[test]
    fn a_1000_character_label_leaves_its_window_reachable_at_80x24_under_a_full_banner() {
        // The smallest card 80×24 gives: a 3-row banner, a 3-row toast, and a problem.
        let mut usage = with_windows(&["Opus"]);
        usage.status = UsageStatus::Unavailable;
        usage.error = Some("http-429".into());
        usage.retry_at = Some(NOW_S + 300);
        usage.windows.as_mut().unwrap()[2].0.label = thousand();
        let mut state = loaded(vec![account(1, "alex@example.com", usage)]);
        state.raise_banner(
            BannerKind::Refusal,
            ["the store could not be read"; 12].join("; "),
        );
        state.toast(
            crate::state::ToastKind::Info,
            ["Switched to alex@example.com (position 1)"; 8].join(" "),
            NOW_MS,
        );
        for _ in 0..3 {
            update(&mut state, ch('J'), NOW_MS);
        }
        let lines = draw(&state, 80, 24);
        assert!(lines.iter().all(|l| width_of(l) <= 80), "{lines:?}");
        let card = card_rows(&state, 80, 24).1;
        let label = format!("  {}  ", fit(&thousand(), 20));
        let row = card
            .iter()
            .find(|l| l.starts_with(&label))
            .unwrap_or_else(|| panic!("{lines:?}"));
        assert!(row.contains(" 40% ▲  resets in 3d09h"), "{row}");
        assert_eq!(
            card[card.len() - 3..card.len() - 1],
            [
                "  ! Rate-limited: the usage check was turned away for now.",
                "    tagteam tries again in 5m.",
            ]
        );
    }

    #[test]
    fn the_worst_live_stack_at_80_by_24_still_reaches_a_long_labelled_window() {
        // The header, the auto line, the Now strip, a 3-row banner and a 3-row toast leave a
        // 15-row body; the window's row and the problem's rows fit in what is left.
        let mut usage = with_windows(&["Opus"]);
        usage.status = UsageStatus::Unavailable;
        usage.error = Some("http-429".into());
        usage.windows.as_mut().unwrap()[2].0.label = thousand();
        let mut state = loaded(vec![account(1, "alex@example.com", usage)]);
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
            ["Switched to alex@example.com (position 1)"; 8].join(" "),
            NOW_MS,
        );
        for _ in 0..3 {
            update(&mut state, ch('J'), NOW_MS);
        }
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
        assert_eq!(
            lines[2], " no account in use · s switches to the selected account",
            "{lines:?}"
        );
        assert!(
            lines[3..6]
                .iter()
                .all(|l| l.contains("the store could not be")),
            "{lines:?}"
        );
        assert!(
            lines[9].contains("ACCOUNT"),
            "the body starts at row 6, its table under the blank and rule: {lines:?}"
        );
        assert!(
            lines[21..].iter().all(|l| l.contains("(position 1)")
                || l.starts_with("1) Switched")
                || l.contains("Switched to alex")),
            "{lines:?}"
        );
        let row = lines
            .iter()
            .find(|l| l.starts_with("  Claude Claude"))
            .unwrap_or_else(|| panic!("{lines:?}"));
        assert!(row.contains(" 40% ▲  resets in 3d09h"), "{lines:?}");
        assert!(
            lines
                .iter()
                .any(|l| l == "  ! Rate-limited: the usage check was turned away for now."),
            "{lines:?}"
        );
    }

    #[test]
    fn a_window_takes_one_row_whatever_its_words() {
        // A 1,000-character label, a spend beyond any number, a rate and a projection 400 days
        // past: one row at any width, cut to it.
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
        let mut usage = read(120, 9.0, 77.0, false);
        usage.status = UsageStatus::Unavailable;
        usage.error = Some("http-429".into());
        usage.windows.as_mut().unwrap().push((w, p));
        let mut state = loaded(vec![account(1, "alex@example.com", usage)]);
        for (width, height) in [(80, 24), (120, 40)] {
            let card = card_rows(&state, width, height).1;
            let rows: Vec<&String> = card
                .iter()
                .filter(|l| l.starts_with("  Claude Claude"))
                .collect();
            assert_eq!(rows.len(), 1, "{width}: {card:?}");
            assert_eq!(width_of(rows[0]), usize::from(width), "{width}: {card:?}");
            assert!(rows[0].ends_with('…'), "{width}: {card:?}");
            assert!(rows[0].contains('€'), "the spend shows: {width}: {card:?}");
        }
        // The card reaches it at 80×24 under a 3-row banner, a 3-row toast and a problem.
        state.raise_banner(
            BannerKind::Refusal,
            ["the store could not be read"; 12].join("; "),
        );
        state.toast(
            crate::state::ToastKind::Info,
            ["Switched to alex@example.com (position 1)"; 8].join(" "),
            NOW_MS,
        );
        for _ in 0..2 {
            update(&mut state, ch('J'), NOW_MS);
        }
        let card = card_rows(&state, 80, 24).1;
        assert!(
            card.iter().any(|l| l.starts_with("  Claude Claude")),
            "{card:?}"
        );
        assert!(
            card.iter()
                .any(|l| l == "  ! Rate-limited: the usage check was turned away for now."),
            "{card:?}"
        );
    }

    #[test]
    fn a_resize_clamps_the_card_scroll_so_the_next_key_moves_at_once() {
        let mut state = loaded(vec![account(1, "alex@example.com", scoped(22))]);
        for _ in 0..30 {
            update(&mut state, ch('J'), NOW_MS);
        }
        let at_24 = card_windows(&state, 80, 24);
        assert_eq!(at_24.last().map(String::as_str), Some("M22"), "{at_24:?}");
        // Taller: the last window shows from further up, and `K` moves from what shows.
        update(
            &mut state,
            Msg::Resize {
                width: 80,
                height: 30,
            },
            NOW_MS,
        );
        let at_30 = card_windows(&state, 80, 30);
        assert_eq!(at_30.last().map(String::as_str), Some("M22"), "{at_30:?}");
        assert!(at_24.len() < at_30.len() && at_30.len() < 24, "{at_30:?}");
        update(&mut state, ch('K'), NOW_MS);
        let up = card_windows(&state, 80, 30);
        assert_eq!(up[1..], at_30[..at_30.len() - 1], "{up:?}");
    }

    #[test]
    fn the_card_grows_to_show_every_window() {
        // §6.4: a row per window, all of them. 5h, 7d and three scoped windows are five.
        let state = loaded(vec![account(
            1,
            "alex@example.com",
            with_windows(&["Opus", "Sonnet", "Haiku"]),
        )]);
        for (w, h) in [(80, 24), (120, 40)] {
            assert_eq!(
                card_windows(&state, w, h),
                ["5h", "7d", "Opus", "Sonnet", "Haiku"],
                "{w}×{h}"
            );
        }
        let card = card_rows(&state, 120, 40).1;
        assert!(card[6].starts_with("  Haiku  "), "{card:?}");
        assert!(
            card[6].ends_with(
                " 42% ▲  resets in 3d09h · +1.5%/h · full in 1d00h · 20% expected by now"
            ),
            "{card:?}"
        );
    }

    #[test]
    fn windows_beyond_the_card_scroll_into_view_with_shift_j_and_k() {
        // 24 windows: more than 80×24's card holds beside the table, so it scrolls them, a row
        // each.
        let mut state = loaded(vec![
            account(1, "alex@example.com", scoped(22)),
            account(2, "sam@corp.example", scoped(22)),
        ]);
        let first = card_windows(&state, 80, 24);
        assert_eq!(first[..3], ["5h", "7d", "M1"]);
        let shown = first.len();
        assert!(shown < 24, "{first:?}");
        for _ in 0..30 {
            update(&mut state, ch('J'), NOW_MS);
        }
        let last = card_windows(&state, 80, 24);
        assert_eq!(last.len(), shown, "{last:?}");
        assert_eq!(
            last.last().map(String::as_str),
            Some("M22"),
            "J stops at the last window"
        );
        update(&mut state, ch('K'), NOW_MS);
        let up = card_windows(&state, 80, 24);
        assert_eq!(up[1..], last[..shown - 1], "{up:?}");
        // Another account's card starts at its first window.
        update(&mut state, ch('j'), NOW_MS);
        assert_eq!(card_windows(&state, 80, 24), first);
        // A taller terminal shows them all.
        assert_eq!(card_windows(&state, 120, 40).len(), 24);
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
            let body = table_of(&state, 120, 40, NOW_MS);
            let five = col(&body[0], "5H");
            let seven = col(&body[0], "7D");
            let buf = draw_buffer(&state, 120, 40, NOW_MS);
            let y = table_y(&state, 120, 40);
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
            let y = table_y(&state, 80, 24) + 2;
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
            let y = table_y(&state, 120, 40);
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
        let y = table_y(&state, 120, 40) + 2;
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
            let y = table_y(&state, 120, 40) + 1;
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
            update(&mut state, key(KeyCode::Home), NOW_MS);
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
        let wide = table_of(&state, 160, 40, NOW_MS);
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
        let narrow = table_of(&state, 120, 40, NOW_MS);
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
            let body = table_of(&state, w, 24, NOW_MS);
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
        let narrow = table_of(&loaded(accounts), 80, 24, NOW_MS);
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
        // No lead card: the empty block is the whole body.
        let body = body_of(&state, 80, 24, NOW_MS);
        assert!(
            !body
                .iter()
                .any(|l| l.contains("No account in use") || l.contains("●")),
            "{body:?}"
        );
        let unloaded = draw(&fresh(Screen::Accounts), 80, 24);
        assert!(unloaded.iter().any(|l| l.trim() == "Loading accounts…"));
        assert!(
            !unloaded.iter().any(|l| l.contains("No account in use")),
            "{unloaded:?}"
        );
    }

    #[test]
    fn with_one_account_the_table_says_how_to_add_a_second() {
        let state = loaded(many(1));
        let body = table_of(&state, 80, 24, NOW_MS);
        let hint = " Add a second account to switch: log in to Claude Code as it, then press A.";
        assert_eq!(body[2], hint, "{body:?}");
        assert!(body[1].contains("a1@x.co"), "{body:?}");
        let two = table_of(&loaded(many(2)), 80, 24, NOW_MS);
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
            lines[3],
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
        // 80×28: 24 body rows, 3 for each separator, 5 for the card (title, facts, two windows,
        // keys), 1 for the header: 12 account rows.
        state.size = (80, 28);
        let shown = |state: &State| {
            table_of(state, 80, 28, NOW_MS)[1..]
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
        for _ in 0..17 {
            update(&mut state, ch('j'), NOW_MS);
        }
        assert_eq!(state.accounts.selected().unwrap().position, 18);
        assert_eq!(shown(&state).first().unwrap(), "a7@x.co");
        assert_eq!(shown(&state).last().unwrap(), "a18@x.co");
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
        for _ in 0..16 {
            update(&mut state, ch('k'), NOW_MS);
        }
        assert_eq!(shown(&state).first().unwrap(), "a14@x.co");
        update(&mut state, ch('k'), NOW_MS);
        assert_eq!(shown(&state).first().unwrap(), "a13@x.co");
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
        // At 80×28 the table has 12 account rows: the last 12 of the 30 start at a19.
        let mut state = loaded(many(30));
        state.size = (80, 28);
        update(&mut state, key(KeyCode::End), NOW_MS);
        let first = |state: &State, h: u16| {
            table_of(state, 80, h, NOW_MS)[1]
                .split_whitespace()
                .find(|w| w.contains('@'))
                .unwrap()
                .to_owned()
        };
        assert_eq!(first(&state, 28), "a19@x.co");
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
        accounts[1].active = true;
        // The first selection is the account in use (T3-23).
        st.apply(snapshot(accounts.clone()));
        assert_eq!(st.selected().unwrap().id, id(2));
        st.step(1);
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
        // Everything gone, then back: the account in use, else the first row.
        st.apply(snapshot(vec![]));
        assert_eq!(st.selected(), None);
        st.apply(snapshot(many(2)));
        assert_eq!(st.selected().unwrap().id, id(1));
        st.apply(snapshot(vec![]));
        let mut back = many(2);
        back[1].active = true;
        st.apply(snapshot(back));
        assert_eq!(st.selected().unwrap().id, id(2));
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
    fn enter_on_accounts_opens_history_scoped_to_the_selected_account() {
        let mut state = loaded(many(3));
        update(&mut state, ch('j'), NOW_MS);
        let cmds = update(&mut state, key(KeyCode::Enter), NOW_MS);
        assert_eq!(state.screen, Screen::History);
        assert_eq!(state.history.scope, Scope::Account(id(2)));
        assert_eq!(
            cmds,
            [Cmd::LoadTimeline {
                r#gen: state.gens.history,
                provider: cc(),
                scope: Scope::Account(id(2)),
                since_s: NOW_S - 7 * 86_400,
                columns: 70,
            }]
        );
        let row = body_of(&state, 80, 24, NOW_MS).remove(0);
        assert!(row.starts_with(" f 2 a2   [ ] 7 days"), "{row:?}");
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
            let body = table_of(&state, 80, 24, NOW_MS);
            assert_eq!(body[0], " Claude Code");
            assert!(body[1].starts_with("    #   ACCOUNT"), "{}", body[1]);
            assert!(body[2].contains("a1@x.co"));
            assert_eq!(body[3], " FakeAgent");
            assert!(body[4].starts_with("    #   ACCOUNT"));
            assert!(body[5].contains("o@x.co"));
            assert!(body.iter().all(|l| width_of(l) <= 80), "{body:?}");
            let buf = draw_buffer(&state, 80, 24, NOW_MS);
            let y = table_y(&state, 80, 24);
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
    fn a_provider_heading_wider_than_the_terminal_is_cut_to_it() {
        const WIDE_NAME: &str =
            "FakeAgent, a provider whose display name is far wider than any eighty-column terminal";
        let mut snap = snapshot(many(1));
        let mut fake = fake_agent(vec![fake_account(1, "a1@x.co")], LiveLogin::None);
        fake.names.display_name = WIDE_NAME;
        snap.providers.push(fake);
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snap);
        let body = table_of(&state, 80, 24, NOW_MS);
        assert_eq!(body[0], " Claude Code");
        assert!(width_of(WIDE_NAME) > 80);
        assert_eq!(body[3], format!(" {}", fit(WIDE_NAME, 79)), "{body:?}");
        assert!(body[3].ends_with('…'), "{body:?}");
        assert_eq!(width_of(&body[3]), 80, "{body:?}");
        assert!(body[4].starts_with("    #   ACCOUNT"), "{body:?}");
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
            draw(&state, 80, 24)[3..5],
            [
                " ! Claude Code is logged in as s@x.co, which tagteam doesn't manage · press A to",
                "   add it",
            ]
        );
        snap.in_run_shell = true;
        state.accounts.apply(snap);
        assert_eq!(
            draw(&state, 80, 24)[3],
            " ! inside a `tagteam run` session: account changes are refused here"
        );
        state.raise_banner(BannerKind::Refusal, "the store is locked");
        assert_eq!(draw(&state, 80, 24)[3], " ! the store is locked");
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
            lines[3..6],
            [
                " ! Claude Code is logged in as".to_owned(),
                format!("   {long}, which tagteam"),
                "   doesn't manage · press A to add it".to_owned(),
            ]
        );
        let body = table_of(&state, 80, 24, NOW_MS);
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
        let wide = table_of(&state, 120, 40, NOW_MS);
        assert!(wide[1].ends_with("  api key"), "{}", wide[1]);
        assert!(wide[2].ends_with("  ○ disabled"), "{}", wide[2]);
        // At 80 columns only the disabled glyph follows UPDATED.
        let body = table_of(&state, 80, 24, NOW_MS);
        assert!(body[1].ends_with("  —"), "{}", body[1]);
        assert!(body[2].ends_with("1m ○"), "{}", body[2]);
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
        let body = table_of(&loaded(crowded("http-429")), 80, 24, NOW_MS);
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
        let wide = table_of(&loaded(crowded("http-429")), 160, 40, NOW_MS);
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
        let body = table_of(&loaded(accounts), 80, 24, NOW_MS);
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

    /// The rows of `area` (one of `parts`) at `w`×`h`, and the row it starts at.
    fn rows_of_part(state: &State, w: u16, h: u16, area: fn(&Parts) -> Rect) -> (u16, Vec<String>) {
        let body = crate::view::areas(state, Rect::new(0, 0, w, h)).body;
        let at = area(&parts(state, body, NOW_MS));
        let rows = draw(state, w, h);
        let (from, to) = (usize::from(at.y), usize::from(at.y + at.height));
        (at.y, rows[from..to].to_vec())
    }

    /// The text of `spans`.
    fn joined(spans: Option<Vec<Span<'static>>>) -> Option<String> {
        spans.map(|s| s.iter().map(|s| s.content.as_ref()).collect())
    }

    /// `fill` cells of `▆`, then `▁` to `cells`.
    fn lead_bar_of(fill: usize, cells: usize) -> String {
        format!("{}{}", "▆".repeat(fill), "▁".repeat(cells - fill))
    }

    /// S§3.1's mock in shape: its accounts, names, windows and rows, so columns and row indices
    /// match it; not its figures (`read` gives other resets and no rate, `▲`, age or login), so no
    /// test built on it asserts a percentage, reset or fact from the mock.
    /// Four accounts, every reading fresh (60 s, so no `!` status column), each with
    /// 5h, 7d and a scoped `Fable` window; `work` (2) in use and selected. Tasks 5-7 reuse it.
    fn mock() -> Vec<AccountView> {
        let with_fable = |five, seven| {
            let mut u = read(60, five, seven, false);
            u.windows.as_mut().unwrap().push((
                window(
                    "scoped:Fable",
                    "Fable",
                    WindowKind::Scoped,
                    28.0,
                    Some(291_630),
                ),
                Pace::default(),
            ));
            u
        };
        let one = account(1, "alex@example.com", with_fable(0.0, 30.0));
        let mut work = account(2, "sam@corp.example", with_fable(44.0, 82.0));
        work.row.alias = Some("work".into());
        work.row.org_name = Some("Example Corp".into());
        work.active = true;
        let mut spare = account(3, "alex@personal.example", with_fable(0.0, 31.0));
        spare.row.alias = Some("spare".into());
        let kim = account(4, "kim@example.org", with_fable(99.0, 13.0));
        vec![one, work, spare, kim]
    }

    #[test]
    fn compact_at_80_by_24_has_no_lead_card_and_rules_between_blank_rows() {
        // Rows: 0 header, 1 auto line, 2 the Now strip; the body from 3: blank, rule, blank
        // (3-5), the table's head and four accounts (6-10), blank, rule, blank (11-13), the card
        // (14-19: the title, the facts, three windows and the keys); the footer on 23.
        for theme in TIERS {
            let mut state = loaded(mock());
            state.theme = theme;
            let rows = draw(&state, 80, 24);
            assert!(
                rows[2].starts_with(" ● 2 work   5h"),
                "{theme:?} {}",
                rows[2]
            );
            for blank in [3, 5, 11, 13] {
                assert_eq!(rows[blank], "", "{theme:?} {blank}");
            }
            for rule in [4, 12] {
                let c = rows[rule].chars().next().unwrap();
                assert!(
                    c != ' '
                        && rows[rule].chars().all(|x| x == c)
                        && rows[rule].chars().count() == 80,
                    "{theme:?} {rule}"
                );
            }
            assert!(rows[6].starts_with("    #   ACCOUNT"), "{theme:?}");
            assert!(rows[14].starts_with("SELECTED  2 work"), "{theme:?}");
            assert!(rows[15].starts_with("          IN USE"), "{theme:?}");
            assert!(rows[19].contains("⏎ history"), "{theme:?}");
            let header = &rows[6];
            assert_eq!(
                (col(header, "5H"), col(header, "7D"), col(header, "UPDATED")),
                (36, 54, 73)
            );
        }
    }

    #[test]
    fn compact_at_120_by_40_puts_the_columns_where_the_spec_does() {
        let rows = draw(&loaded(mock()), 120, 40);
        let header = &rows[6];
        assert_eq!(
            (col(header, "5H"), col(header, "7D"), col(header, "UPDATED")),
            (39, 62, 86)
        );
        assert!(header.contains("STATUS"));
    }

    #[test]
    fn the_in_use_card_draws_thick_bars() {
        // `mock()` at 80×24: the card's window rows (after the title and the facts) carry
        // `lead_bar`'s 16 cells, `▆` for the fill on `▁`; 44 %, 82 % and 28 % fill 7, 13 and 4.
        let card = card_rows(&loaded(mock()), 80, 24).1;
        for (row, fill) in [(2, 7), (3, 13), (4, 4)] {
            assert!(card[row].contains(&lead_bar_of(fill, 16)), "{card:?}");
        }
    }

    #[test]
    fn ten_accounts_at_80_by_24_scroll_the_table_and_keep_both_rules() {
        // `mock()` and positions 5-10: the body (rows 3-22) holds the separators (6), the card
        // (6: the title, the facts, three windows and the keys) and 8 table lines of the 11, so
        // the table scrolls.
        let mut accounts = mock();
        accounts.extend(many(10).into_iter().skip(4));
        let mut state = loaded(accounts);
        let rows = draw(&state, 80, 24);
        assert!(!rows.iter().any(|l| l.contains("a10@x.co")), "{rows:?}");
        for blank in [3, 5, 14, 16] {
            assert_eq!(rows[blank], "", "{blank}");
        }
        for rule in [4, 15] {
            assert!(
                rows[rule].chars().all(|c| c == '─'),
                "{rule}: {}",
                rows[rule]
            );
        }
        assert!(rows[17].starts_with("SELECTED  2 work"), "{rows:?}");
        // The last account's card has no third window, so the table then holds more of its lines.
        update(&mut state, key(KeyCode::End), NOW_MS);
        let rows = draw(&state, 80, 24);
        assert!(
            rows.iter().any(|l| l.starts_with("▌  10   a10@x.co")),
            "{rows:?}"
        );
    }

    #[test]
    fn a_short_body_scrolls_the_table_and_the_card_before_it_drops_a_separator() {
        // S§5.4's order, on the layout itself (the frame below 80×24 shows only the too-small
        // notice, `view.rs`): `parts` for `mock()` at 80 columns over body heights 20 down to 7.
        // The table has 5 lines (its least is 4) and the card 6 (its title, facts, three windows
        // and keys; its least is 3 with the keys row, 4). From the top of the list: nothing is cut
        // until 17 rows; then the table scrolls (16), then the card's windows go (15, 14); then
        // the lower separator loses its blank rows (13, 12), the upper its blanks (11, 10), the
        // card its keys row (9), and the rules go lower first (8, 7).
        let state = loaded(mock());
        // (body height, table, card, top, under)
        let want = [
            (20, 5, 6, 3, 3),
            (17, 5, 6, 3, 3),
            (16, 4, 6, 3, 3),
            (15, 4, 5, 3, 3),
            (14, 4, 4, 3, 3),
            (13, 4, 4, 3, 2),
            (12, 4, 4, 3, 1),
            (11, 4, 4, 2, 1),
            (10, 4, 4, 1, 1),
            (9, 4, 3, 1, 1),
            (8, 4, 3, 1, 0),
            (7, 4, 3, 0, 0),
        ];
        for (h, table, card, top, under) in want {
            let p = parts(&state, Rect::new(0, 0, 80, h), NOW_MS);
            let got = (p.table.height, p.card.height, p.top.height, p.under.height);
            assert_eq!(got, (table, card, top, under), "h={h}");
        }
        // Every height between holds the same order: none grows back as the body shrinks.
        let mut last = (u16::MAX, u16::MAX, u16::MAX, u16::MAX);
        for h in (7..=20).rev() {
            let p = parts(&state, Rect::new(0, 0, 80, h), NOW_MS);
            let got = (p.table.height, p.card.height, p.top.height, p.under.height);
            assert!(
                got.0 <= last.0 && got.1 <= last.1 && got.2 <= last.2 && got.3 <= last.3,
                "h={h}: {got:?} after {last:?}"
            );
            last = got;
        }
        let rows = draw(&state, 80, 23);
        assert!(
            rows.iter().any(|l| l.contains("tagteam needs 80×24")),
            "{rows:?}"
        );
        assert!(!rows.iter().any(|l| l.contains("ACCOUNT")), "{rows:?}");
    }

    #[test]
    fn on_accounts_with_no_account_in_use_the_strip_says_s_switches_to_the_selected_account() {
        // Ports the lead card's words. `three()` with no account active: on Accounts the strip
        // reads the lead card's hint, muted; on Tokens it still says `s on Accounts switches
        // to one`. At 80 columns the best pick would cut the hint, so it gives way; at 120
        // both show.
        let mut accounts = three();
        accounts[1].active = false;
        let hint = " no account in use · s switches to the selected account";
        let pick = "best now 1 alex@example.com";
        for theme in TIERS {
            let mut state = loaded(accounts.clone());
            state.theme = theme;
            for (w, h, want) in [
                (80u16, 24u16, hint.to_owned()),
                (
                    120,
                    40,
                    format!(
                        "{hint}{}{pick}",
                        " ".repeat(120 - 1 - width_of(hint) - width_of(pick))
                    ),
                ),
            ] {
                let buf = draw_buffer(&state, w, h, NOW_MS);
                assert_eq!(text_of(&buf)[2], want, "{theme:?} {w}");
                let (got, want) = role_of(&buf, theme, 2, hint.trim_start(), 'm');
                assert_eq!(got, want, "{theme:?} {w}");
            }
            state.screen = Screen::Tokens;
            assert_eq!(
                draw(&state, 80, 24)[2],
                " no account in use · s on Accounts switches to one"
            );
        }
    }

    #[test]
    fn on_accounts_a_problem_in_use_does_not_say_see_accounts() {
        // Its card and the table's STATUS are on the same screen; Tokens points here instead.
        let mut state = loaded(quarantined_in_use(three()));
        let strip = draw(&state, 80, 24)[2].clone();
        assert!(
            strip.starts_with(" ● 2 work   ✕ needs a new login"),
            "{strip}"
        );
        assert!(!strip.contains("see Accounts"), "{strip}");
        state.screen = Screen::Tokens;
        let strip = draw(&state, 80, 24)[2].clone();
        assert!(
            strip.contains("✕ needs a new login · see Accounts"),
            "{strip}"
        );
    }

    #[test]
    fn a_login_under_seven_days_is_warn_and_says_to_log_in_again() {
        // Ports the lead card's: on the card's facts, which 160 columns hold whole.
        let mut accounts = three();
        accounts[1].row.login_expires_at = Some((NOW_S + 3 * 86_400 + HOUR) * 1000);
        for theme in TIERS {
            let mut state = loaded(accounts.clone());
            state.theme = theme;
            let (y, card) = card_rows(&state, 160, 40);
            assert!(
                card[1].contains("login valid 3 more days · log in again before then"),
                "{}",
                card[1]
            );
            let buf = draw_buffer(&state, 160, 40, NOW_MS);
            for needle in ["login valid 3 more days", "log in again before then"] {
                let (got, want) = role_of(&buf, theme, y + 1, needle, 'w');
                assert_eq!(got, want, "{theme:?} {needle}");
            }
        }
        // Once passed: `login expired` in warn, and nothing to do before then.
        accounts[1].row.login_expires_at = Some((NOW_S - HOUR) * 1000);
        let state = loaded(accounts);
        let (y, card) = card_rows(&state, 160, 40);
        assert!(card[1].contains("login expired"), "{}", card[1]);
        assert!(!card[1].contains("log in again"), "{}", card[1]);
        let (got, want) = role_of(
            &draw_buffer(&state, 160, 40, NOW_MS),
            state.theme,
            y + 1,
            "login expired",
            'w',
        );
        assert_eq!(got, want);
    }

    #[test]
    fn the_in_use_card_says_when_each_window_resets_and_shows_its_problem() {
        // Replaces the three tests that had the lead card show them (M-9): `work` is in use and
        // selected, and every window row says when it resets; quarantined, its problem sentence
        // and remedy sit under its windows.
        for (w, h) in [(80, 24), (120, 40)] {
            let state = loaded(mock());
            let card = card_rows(&state, w, h).1;
            assert!(
                card[2..5].iter().all(|l| l.contains("resets in")),
                "{w}: {card:?}"
            );
            let state = loaded(quarantined_in_use(mock()));
            let card = card_rows(&state, w, h).1;
            assert!(
                card[2..5].iter().all(|l| l.contains("resets in")),
                "{w}: {card:?}"
            );
            assert_eq!(card[5..7], NEEDS_LOGIN_ROWS, "{w}: {card:?}");
        }
    }

    #[test]
    fn each_forecast_has_its_words_on_the_now_strip() {
        use tagteam_engine::switch::{BestForecast, NoPick};
        let cases = [
            (
                BestForecast::Pick { order: vec![1] },
                Some("best now 1 alex@example.com"),
            ),
            (
                BestForecast::Pick { order: vec![3, 1] },
                Some("best now 3 spare"),
            ),
            (BestForecast::AlreadyBest, Some("on the best account")),
            (BestForecast::None(NoPick::UsageUnknown), None),
            (BestForecast::None(NoPick::TooFewCandidates), None),
            (BestForecast::None(NoPick::UnmanagedLogin), None),
            (BestForecast::None(NoPick::CandidatesExhausted), None),
        ];
        for theme in TIERS {
            for (forecast, words) in cases.clone() {
                let mut snap = snapshot(three());
                snap.providers[0].forecast = forecast.clone();
                let mut state = fresh(Screen::Accounts);
                state.theme = theme;
                state.accounts.apply(snap);
                let view = &state.accounts.snapshot().unwrap().providers[0];
                assert_eq!(joined(lead::pick_words(&state, view)).as_deref(), words);
                let buf = draw_buffer(&state, 120, 40, NOW_MS);
                let strip = &text_of(&buf)[2];
                match words {
                    Some(words) => {
                        assert!(strip.ends_with(words), "{forecast:?}: {strip}");
                        // One column in from the edge.
                        assert_eq!(width_of(strip), 119, "{strip}");
                    }
                    None => assert!(strip.ends_with("3d09h"), "{forecast:?}: {strip}"),
                }
                if matches!(forecast, BestForecast::AlreadyBest) {
                    let (got, want) = role_of(&buf, theme, 2, "on the best account", 'm');
                    assert_eq!(got, want, "{theme:?}");
                }
            }
        }
    }

    /// `three()` with row 2, in use, quarantined: its stored login refused (`invalid_grant`).
    fn quarantined_in_use(mut accounts: Vec<AccountView>) -> Vec<AccountView> {
        accounts[1].usage.status = UsageStatus::ReloginRequired;
        accounts[1].row.quarantine_reason = Some("invalid_grant".into());
        accounts
    }

    const NEEDS_LOGIN_ROWS: [&str; 2] = [
        "  ✕ Needs a new login: its stored login was refused (invalid_grant).",
        "    Log in to Claude Code as sam@corp.example, then press A here.",
    ];

    #[test]
    fn the_strip_drops_the_best_pick_rather_than_cut_its_left_part() {
        // Ports T3-19's rule (the pick is never cut) to the strip. alex's alias is 60 columns, so
        // the pick takes 71; with the 3-column gap and the column's inset it needs 75 beside the
        // left part, which is whole below 120 columns (no gauges).
        let mut accounts = three();
        accounts[0].row.alias = Some("p".repeat(60));
        let state = loaded(accounts);
        let pick = format!("best now 1 {}", "p".repeat(60));
        assert_eq!(width_of(&pick), 71);
        let left = draw(&state, 80, 24)[2].clone();
        assert!(!left.contains("best now"), "{left}");
        let least = width_of(&left) + 3 + 71 + 1;
        for w in [least as u16, 160] {
            let strip = draw(&state, w, 24)[2].clone();
            assert!(strip.ends_with(&pick), "{w}: {strip}");
        }
        let strip = draw(&state, least as u16 - 1, 24)[2].clone();
        assert_eq!(strip, left, "{strip}");
    }

    #[test]
    fn on_accounts_the_strip_follows_the_selected_account_s_provider() {
        // Review Focus 2: with several providers, the strip is the selected account's, and does
        // not name it.
        use tagteam_engine::switch::BestForecast;
        let with_fake = |live: LiveLogin, active: bool| {
            let mut snap = snapshot(three());
            let mut fake = fake_agent(
                vec![fake_account(1, "a1@x.co"), fake_account(2, "a2@x.co")],
                live,
            );
            fake.accounts[1].active = active;
            fake.forecast = BestForecast::Pick { order: vec![1] };
            snap.providers.push(fake);
            let mut state = fresh(Screen::Accounts);
            state.accounts.apply(snap);
            state
        };
        let strip = |state: &State| draw(state, 120, 40)[2].clone();
        // Its own account in use and pick.
        let mut state = with_fake(LiveLogin::Managed { position: 2 }, true);
        assert!(strip(&state).starts_with(" ● 2 work"), "{}", strip(&state));
        assert!(strip(&state).ends_with("best now 1 alex@example.com"));
        update(&mut state, key(KeyCode::End), NOW_MS);
        assert_eq!(
            state.accounts.selected().unwrap().provider,
            ProviderId::new("fake-agent")
        );
        assert!(
            strip(&state).starts_with(" ● 2 a2@x.co"),
            "{}",
            strip(&state)
        );
        assert!(strip(&state).ends_with("best now 1 a1@x.co"));
        // Or no account in use, which says so for the selected account's provider.
        let mut state = with_fake(LiveLogin::None, false);
        update(&mut state, key(KeyCode::End), NOW_MS);
        let want = " no account in use · s switches to the selected account";
        assert!(strip(&state).starts_with(want), "{}", strip(&state));
        assert!(strip(&state).ends_with("best now 1 a1@x.co"));
    }

    #[test]
    fn the_body_stacks_the_table_between_two_rules_and_the_card_and_leaves_spare_rows_at_the_bottom()
     {
        // VIS-5, S§3.1: the body (rows 3-22) is blank, rule, blank, the table's head and three
        // accounts, blank, rule, blank, the card's title, facts, two windows and keys, then 5
        // spare rows.
        let state = loaded(three());
        let body = body_of(&state, 80, 24, NOW_MS);
        assert_eq!(body.len(), 20, "{body:?}");
        for rule in [1, 8] {
            assert!(body[rule].chars().all(|c| c == '─'), "{body:?}");
            assert_eq!(body[rule].chars().count(), 80);
        }
        for blank in [0, 2, 7, 9] {
            assert_eq!(body[blank], "", "{body:?}");
        }
        assert!(body[3].starts_with("    #   ACCOUNT"), "{body:?}");
        for (row, email) in [
            (4, "alex@example.com"),
            (5, "sam@corp.example"),
            (6, "alex@personal."),
        ] {
            assert!(body[row].contains(email), "{body:?}");
        }
        assert!(
            body[10].starts_with("SELECTED  2 work  sam@corp.example"),
            "{body:?}"
        );
        assert!(body[11].starts_with("          IN USE"), "{body:?}");
        assert!(body[12].starts_with("  5h "), "{body:?}");
        assert!(body[13].starts_with("  7d "), "{body:?}");
        assert!(body[14].contains("⏎ history"), "{body:?}");
        assert!(body[15..].iter().all(String::is_empty), "{body:?}");
    }

    #[test]
    fn with_sixteen_accounts_at_80_by_24_the_table_scrolls_between_the_rules_and_the_card() {
        // The body's 20 rows: 6 separators, the card's 5 (title, facts, 5h, 7d, keys), so the
        // table has 9 of its 17 lines: its head and 8 accounts.
        let mut accounts = many(16);
        accounts[0].active = true;
        let mut state = loaded(accounts);
        let rows = |state: &State| {
            rows_of_part(state, 80, 24, |p| p.table)
                .1
                .iter()
                .filter_map(|l| {
                    l.split_whitespace()
                        .find(|w| w.contains('@'))
                        .map(str::to_owned)
                })
                .collect::<Vec<_>>()
        };
        let strip = |state: &State| draw(state, 80, 24)[2].clone();
        let shown = rows(&state);
        assert_eq!(shown.len(), 8, "{shown:?}");
        assert_eq!(shown[0], "a1@x.co");
        let (card_y, card) = card_rows(&state, 80, 24);
        assert!(
            strip(&state).starts_with(" ● 1 a1@x.co"),
            "{}",
            strip(&state)
        );
        assert!(card[0].starts_with("SELECTED  1 a1@x.co"), "{card:?}");
        update(&mut state, key(KeyCode::End), NOW_MS);
        assert_eq!(state.accounts.selected().unwrap().position, 16);
        let shown = rows(&state);
        assert_eq!(
            shown.last().map(String::as_str),
            Some("a16@x.co"),
            "{shown:?}"
        );
        assert!(
            strip(&state).starts_with(" ● 1 a1@x.co"),
            "{}",
            strip(&state)
        );
        let (y, card) = card_rows(&state, 80, 24);
        assert_eq!(y, card_y);
        assert!(card[0].starts_with("SELECTED  16 a16@x.co"), "{card:?}");
        let lines = draw(&state, 80, 24);
        assert!(lines[usize::from(card_y)].starts_with("SELECTED  16 a16@x.co"));
    }

    #[test]
    fn selection_starts_on_the_account_in_use() {
        // UX-2: the account in use, else row 1.
        let selected = |state: &State| state.accounts.selected().map(|a| a.id);
        let mut state = loaded(three());
        assert_eq!(selected(&state), Some(id(2)));
        assert_eq!(selected(&loaded(many(3))), Some(id(1)));
        // In table order, whatever its provider.
        let mut snap = snapshot(many(2));
        let mut fake = fake_agent(
            vec![fake_account(1, "a1@x.co"), fake_account(2, "a2@x.co")],
            LiveLogin::Managed { position: 2 },
        );
        fake.accounts[1].active = true;
        snap.providers.push(fake);
        let mut several = fresh(Screen::Accounts);
        several.accounts.apply(snap);
        assert_eq!(selected(&several), Some(AccountId::from_string("fake-2")));
        // A refresh keeps the selection by ID.
        update(&mut state, ch('k'), NOW_MS);
        state.accounts.apply(snapshot(three()));
        assert_eq!(selected(&state), Some(id(1)));
    }

    #[test]
    fn the_accounts_screen_s_roles_in_each_tier() {
        // §16.
        for theme in TIERS {
            for (w, h) in [(80u16, 24u16), (120, 40)] {
                let mut state = loaded(three());
                state.theme = theme;
                let buf = draw_buffer(&state, w, h, NOW_MS);
                let body = crate::view::areas(&state, Rect::new(0, 0, w, h)).body;
                let p = parts(&state, body, NOW_MS);
                let check = |y: u16, needle: &str, role: char| {
                    let (got, want) = role_of(&buf, theme, y, needle, role);
                    assert_eq!(got, want, "{theme:?} {w}: {needle:?}");
                };
                // The Now strip: `●` accent, the alias bold.
                check(2, "●", 'a');
                for x in 5..9 {
                    assert!(
                        buf[(x, 2)].modifier.contains(Modifier::BOLD),
                        "{theme:?} {w} {x}"
                    );
                }
                // Both rules, the row after the blank above each: faint.
                for part in [p.top, p.under] {
                    assert_eq!(part.height, 3);
                    assert_eq!(
                        roles(&buf, theme, part.y + 1),
                        canon(theme, &"f".repeat(usize::from(w))),
                        "{theme:?} {w}"
                    );
                }
                // The table: muted heads, the selected row's `▌` accent and `sel-bg` behind it.
                let y = p.table.y;
                let heads: String = roles(&buf, theme, y)
                    .chars()
                    .filter(|c| *c != ' ')
                    .collect();
                assert_eq!(
                    heads,
                    canon(theme, "m").repeat(heads.chars().count()),
                    "{theme:?}"
                );
                let selected = y + 2;
                assert_eq!(buf[(0, selected)].symbol(), "▌");
                check(selected, "▌", 'a');
                let tint = theme.sel_bg().bg.unwrap_or(Color::Reset);
                for x in 0..w {
                    assert_eq!(buf[(x, selected)].bg, tint, "{theme:?} {w} {x}");
                    assert_eq!(buf[(x, selected - 1)].bg, Color::Reset, "{theme:?} {w} {x}");
                }
                // The card: the title muted with the alias bold, `IN USE` accent and the rest
                // of the facts muted, the window's percentage and reset in text, the keys muted.
                let y = p.card.y;
                for x in 12..16 {
                    assert!(
                        buf[(x, y)].modifier.contains(Modifier::BOLD),
                        "{theme:?} {w} {x}"
                    );
                }
                check(y, "SELECTED", 'm');
                check(y + 1, "IN USE", 'a');
                check(y + 1, "updated 1m ago", 'm');
                check(y + 2, "53%", 't');
                check(y + 2, "resets in 2h40m", 't');
                check(y + 4, "history", 'm');
            }
        }
    }

    fn resize(state: &mut State, width: u16, height: u16) {
        update(state, Msg::Resize { width, height }, NOW_MS);
    }

    /// The first row of the body (from row 4, under the chrome and the Now strip) that holds
    /// `needle`.
    fn row_of(rows: &[String], needle: &str) -> usize {
        (4..rows.len())
            .find(|&y| rows[y].contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} in {rows:?}"))
    }

    /// The footer row at 80×24.
    fn footer_80(state: &State) -> String {
        draw(state, 80, 24)[23].clone()
    }

    #[test]
    fn v_toggles_the_full_list_and_keeps_the_selection() {
        let mut state = loaded(three());
        update(&mut state, key(KeyCode::Down), NOW_MS);
        let before = state.accounts.selected();
        update(&mut state, ch('v'), NOW_MS);
        assert_eq!(state.accounts.view(), View::List);
        assert_eq!(state.accounts.selected(), before);
        assert!(draw(&state, 80, 24)[23].contains("v compact"));
        update(&mut state, ch('v'), NOW_MS);
        assert_eq!(state.accounts.view(), View::Compact);
        assert!(draw(&state, 80, 24)[23].contains("v full list"));
    }

    #[test]
    fn the_full_list_at_80_by_24_matches_the_spec() {
        // S§6.1's mock: Task 4's `mock()` (three windows each), account 1 selected (`k`), then
        // `v`, in every tier (tier 0, `fresh`'s default, has `·` separators).
        for theme in TIERS {
            let mut state = loaded(mock());
            state.theme = theme;
            update(&mut state, ch('k'), NOW_MS);
            update(&mut state, ch('v'), NOW_MS);
            let rows = draw(&state, 80, 24);
            let dot = if theme == TIERS[0] { "·" } else { "┈" };
            assert_eq!(rows[3], "");
            assert_eq!(rows[5], "");
            assert!(
                rows[4]
                    .chars()
                    .all(|c| c == rows[4].chars().next().unwrap())
            );
            assert!(rows[6].starts_with("▌  1   alex@example.com"));
            for r in &rows[6..11] {
                assert!(r.starts_with('▌'), "{r}");
            }
            assert!(rows[11..].iter().all(|r| !r.starts_with('▌')));
            assert_eq!(rows[12], format!("   {}", dot.repeat(74))); // ends three columns in
            assert!(rows[14].starts_with("   2 ● work  sam@corp.example"));
            assert_eq!(col(&rows[15], "IN USE"), 7);
            assert!(rows[16].starts_with("       5h     ▆")); // thick, from column 7
            assert!(rows[8].starts_with("▌      5h     ") && !rows[8].contains('▆')); // thin
            assert_eq!(col(&rows[8], "%"), col(&rows[16], "%")); // one % column
            assert!(rows[22].starts_with("   3   spare")); // cut at the body's end
            assert!(
                !rows
                    .iter()
                    .any(|r| r.contains("x remove") && !r.contains("q quit"))
            ); // no keys row
        }
    }

    #[test]
    fn the_full_list_at_120_by_40_lines_every_block_up_in_every_tier() {
        // `loaded(mock())`, View::List, for every tier in TIERS: every window row's `%` column is
        // the same in every block; 26-cell bars; `Example Corp` on `work`'s title; thick bars only
        // on `work`'s rows; the separator rows are `┈` from tier 1 and `·` in tier 0.
        for theme in TIERS {
            let mut state = loaded(mock());
            state.theme = theme;
            update(&mut state, ch('v'), NOW_MS);
            let rows = draw(&state, 120, 40);
            let dot = if theme == TIERS[0] { "·" } else { "┈" };
            let emails = [
                "alex@example.com",
                "sam@corp.example",
                "alex@personal.example",
                "kim@example.org",
            ];
            for (i, email) in emails.iter().enumerate() {
                let title = row_of(&rows, email);
                assert_eq!(rows[title].contains("Example Corp"), i == 1, "{theme:?}");
                for row in &rows[title + 2..title + 5] {
                    // Bars start at column 14 and take 26 cells, then a space and the value.
                    assert_eq!(col(row, "%"), 44, "{theme:?} {row}");
                    assert_eq!(row.contains('▁'), i == 1, "{theme:?} {row}");
                }
            }
            let rules = rows
                .iter()
                .filter(|r| !r.trim().is_empty() && r.trim().chars().all(|c| c.to_string() == dot))
                .count();
            assert_eq!(rules, 3, "{theme:?}");
        }
    }

    #[test]
    fn blocks_without_windows_match_the_compact_card() {
        // An ApiKey account and a ReloginRequired one: the first block is exactly title, facts and
        // `api key · has no usage limits to show` from column 7; the second is exactly title, facts
        // and the compact card's problem rows (each once, the same text), the sentence's `✕` at
        // column 7 like the window rows and the remedy at 9.
        let mut api = account(1, "a1@x.co", unread(UsageStatus::ApiKey, None, None));
        api.kind = API_KEY;
        let mut relogin = account(
            2,
            "a2@x.co",
            unread(UsageStatus::ReloginRequired, None, None),
        );
        relogin.row.quarantine_reason = Some("invalid_grant".into());
        let mut state = loaded(vec![api, relogin]);
        update(&mut state, key(KeyCode::End), NOW_MS);
        let card = card_rows(&state, 80, 24).1;
        update(&mut state, ch('v'), NOW_MS);
        let rows = draw(&state, 80, 24);
        assert!(rows[6].contains("a1@x.co") && rows[7].contains("not in use"));
        assert_eq!(rows[8], "       api key · has no usage limits to show");
        assert_eq!(rows[9], "");
        assert!(rows[12].contains("a2@x.co") && rows[13].contains("not in use"));
        // The selected block's rows start with `▌`; the text is the card's, five columns in.
        assert_eq!(rows[14], format!("▌    {}", card[2]));
        assert_eq!(rows[15], format!("▌    {}", card[3]));
        assert_eq!(col(&rows[14], "✕"), 7);
        assert_eq!(col(&rows[15], "Log in"), 9);
        assert_eq!(rows[16], "");
    }

    #[test]
    fn a_title_keeps_its_name_at_column_7_whatever_its_position() {
        // `many(12)` with account 10 in use (so selected), View::List at 80×40: block 10's title
        // is `▌ 10 ● a10@x.co` (a space between `10` and `●`); `j` to 11, then `Home`: block 1's
        // title is `▌  1   a1@x.co` and block 10's `  10 ● a10@x.co` (column 0 blank, `10` in columns
        // 2-3); in every title the name
        // starts at column 7.
        let mut accounts = many(12);
        accounts[9].active = true;
        let mut state = loaded(accounts);
        resize(&mut state, 80, 40);
        update(&mut state, ch('v'), NOW_MS);
        let rows = draw(&state, 80, 40);
        let title = row_of(&rows, "a10@x.co");
        assert!(
            rows[title].starts_with("▌ 10 ● a10@x.co"),
            "{}",
            rows[title]
        );
        assert_eq!(col(&rows[title], "a10@x.co"), 7);
        // Block 10 does not fit in the same frame as block 1, so it is read after `j`.
        update(&mut state, ch('j'), NOW_MS);
        let rows = draw(&state, 80, 40);
        let title = row_of(&rows, "a10@x.co");
        assert!(
            rows[title].starts_with("  10 ● a10@x.co"),
            "{}",
            rows[title]
        );
        assert_eq!(col(&rows[title], "a10@x.co"), 7);
        update(&mut state, key(KeyCode::Home), NOW_MS);
        let rows = draw(&state, 80, 40);
        let title = row_of(&rows, "a1@x.co");
        assert!(rows[title].starts_with("▌  1   a1@x.co"), "{}", rows[title]);
        assert_eq!(col(&rows[title], "a1@x.co"), 7);
    }

    #[test]
    fn a_disabled_block_is_dim_on_every_row() {
        // `mock()` with account 3 disabled (`row.disabled = true`), View::List at 120×40, in every
        // tier: every non-blank cell of block 3's rows carries `Modifier::DIM`. Muted carries
        // DIM in tiers 0 and 1, faint also in tier 0 (`theme.rs:71`, :77-78), so for block 1 (enabled)
        // assert instead that its cells' styles equal block 1's in the same draw of `mock()` with
        // no account disabled, and that its title's name cell has no DIM in any tier.
        for theme in TIERS {
            let mut accounts = mock();
            accounts[2].row.disabled = true;
            let mut off = loaded(accounts);
            let mut on = loaded(mock());
            for state in [&mut off, &mut on] {
                state.theme = theme;
                update(state, ch('v'), NOW_MS);
            }
            let (off_buf, on_buf) = (
                draw_buffer(&off, 120, 40, NOW_MS),
                draw_buffer(&on, 120, 40, NOW_MS),
            );
            let rows = text_of(&off_buf);
            let first = row_of(&rows, "alex@example.com") as u16;
            let third = row_of(&rows, "alex@personal.example") as u16;
            for y in first..first + 5 {
                for x in 0..120 {
                    assert_eq!(off_buf[(x, y)].style(), on_buf[(x, y)].style(), "{theme:?}");
                }
            }
            assert!(!off_buf[(7, first)].modifier.contains(Modifier::DIM));
            for y in third..third + 5 {
                for x in 0..120 {
                    let cell = &off_buf[(x, y)];
                    assert!(
                        cell.symbol() == " " || cell.modifier.contains(Modifier::DIM),
                        "{theme:?} {x},{y}"
                    );
                }
            }
        }
    }

    #[test]
    fn moving_past_the_last_whole_block_scrolls_one_block() {
        // `loaded(mock())` (blocks of 5 rows, 8 with the separator above), View::List at 80×24
        // (17 rows below the pinned three), account 1 selected: `j` → 2 (no scroll); `j` → 3:
        // rows[6] is block 2's title. `k`, `k` back to 1: rows[6] is block 1's title again.
        let mut state = loaded(mock());
        update(&mut state, ch('k'), NOW_MS);
        update(&mut state, ch('v'), NOW_MS);
        update(&mut state, ch('j'), NOW_MS);
        assert!(draw(&state, 80, 24)[6].contains("alex@example.com"));
        update(&mut state, ch('j'), NOW_MS);
        assert!(draw(&state, 80, 24)[6].contains("sam@corp.example"));
        update(&mut state, ch('k'), NOW_MS);
        update(&mut state, ch('k'), NOW_MS);
        assert!(draw(&state, 80, 24)[6].contains("alex@example.com"));
    }

    #[test]
    fn page_keys_move_by_the_whole_blocks_shown() {
        // `many(8)` (4-row blocks; two whole blocks fit in the 17-row viewport at 80×24): from 1,
        // PgDn selects 3, PgDn 5; PgUp 3. One block shown: eight accounts with four windows each
        // (`read` plus two scoped windows, as `with_windows(&["M1", "M2"])` builds; 6-row blocks,
        // two need 6 + 3 + 6 = 15 rows) under a three-row banner (a 14-row viewport): PgDn moves
        // exactly one account.
        let position = |state: &State| state.accounts.selected().unwrap().position;
        let mut state = loaded(many(8));
        update(&mut state, ch('v'), NOW_MS);
        update(&mut state, key(KeyCode::PageDown), NOW_MS);
        assert_eq!(position(&state), 3);
        update(&mut state, key(KeyCode::PageDown), NOW_MS);
        assert_eq!(position(&state), 5);
        update(&mut state, key(KeyCode::PageUp), NOW_MS);
        assert_eq!(position(&state), 3);
        let accounts = (1..=8)
            .map(|p| account(p, &format!("a{p}@x.co"), with_windows(&["M1", "M2"])))
            .collect();
        let mut state = loaded(accounts);
        state.raise_banner(
            BannerKind::Refusal,
            ["the store could not be read"; 12].join("; "),
        );
        update(&mut state, ch('v'), NOW_MS);
        update(&mut state, key(KeyCode::PageDown), NOW_MS);
        assert_eq!(position(&state), 2);
    }

    #[test]
    fn v_into_the_list_starts_at_the_top_and_scrolls_the_selection_into_view() {
        // `many(8)`, account 7 selected in compact; `v`: the list's first block is the fewest moves
        // from block 1 that make block 7 whole (rows[6] is block 6's title); `v`, `v` again from
        // account 1: rows[6] is block 1's title.
        let mut state = loaded(many(8));
        assert!(state.accounts.select_id(&id(7)));
        update(&mut state, ch('v'), NOW_MS);
        assert!(draw(&state, 80, 24)[6].contains("a6@x.co"));
        update(&mut state, ch('v'), NOW_MS);
        update(&mut state, key(KeyCode::Home), NOW_MS);
        update(&mut state, ch('v'), NOW_MS);
        assert!(draw(&state, 80, 24)[6].contains("a1@x.co"));
    }

    #[test]
    fn v_before_the_first_read_or_with_no_accounts_shows_the_same_state() {
        // `fresh(Screen::Accounts)`: `v` then draw: `Loading accounts…` centred; `loaded(vec![])`:
        // `No accounts yet`; `v` again: compact, same text. In both views and both states the
        // footer has no `v` hint (VIS-10, S§8): at 80 and 120 it reads `r refresh` before the
        // pinned keys, as `with_no_accounts_the_footer_hides_the_account_keys` expects.
        for (mut state, words) in [
            (fresh(Screen::Accounts), LOADING),
            (loaded(vec![]), NO_ACCOUNTS),
        ] {
            let first = draw(&state, 80, 24);
            for want in [View::List, View::Compact] {
                update(&mut state, ch('v'), NOW_MS);
                assert_eq!(state.accounts.view(), want);
                assert_eq!(draw(&state, 80, 24), first);
                assert!(first.iter().any(|r| r.contains(words)));
                for (w, h) in [(80, 24), (120, 40)] {
                    let rows = draw(&state, w, h);
                    assert_eq!(
                        rows[usize::from(h) - 1],
                        footer_row("r refresh", usize::from(w))
                    );
                }
            }
        }
    }

    #[test]
    fn after_a_resize_a_key_moves_the_list_from_what_is_on_screen() {
        // `many(8)` at 120×40, View::List, from account 1 `j` to account 6 (a 33-row viewport:
        // the stored first block is 2); `Msg::Resize` to 80×24: the frame shows block 5 first
        // (rows[6]; 17 rows hold blocks 5 and 6, not 4). Then `k`
        // to account 5: rows[6] is still block 5's title (it was whole, nothing moves). The same
        // set-up, then `PgUp`: the selection moves by the whole blocks shown, and rows[6] is the
        // new selection's title or the fewest moves from block 5 that make it whole.
        let at_6 = || {
            let mut state = loaded(many(8));
            resize(&mut state, 120, 40);
            update(&mut state, ch('v'), NOW_MS);
            for _ in 0..5 {
                update(&mut state, ch('j'), NOW_MS);
            }
            resize(&mut state, 80, 24);
            assert!(draw(&state, 80, 24)[6].contains("a5@x.co"));
            state
        };
        let mut state = at_6();
        update(&mut state, ch('k'), NOW_MS);
        assert!(draw(&state, 80, 24)[6].contains("a5@x.co"));
        let mut state = at_6();
        update(&mut state, key(KeyCode::PageUp), NOW_MS);
        assert_eq!(state.accounts.selected().unwrap().position, 4);
        assert!(draw(&state, 80, 24)[6].contains("a4@x.co"));
    }

    #[test]
    fn a_refresh_that_removes_accounts_clamps_the_list() {
        // `many(8)`, View::List, account 8 selected (list scrolled); apply `snapshot(many(3))`: the
        // selection moves to account 3 (as `apply` does) and the next frame shows block 3 whole.
        let mut state = loaded(many(8));
        update(&mut state, ch('v'), NOW_MS);
        update(&mut state, key(KeyCode::End), NOW_MS);
        state.accounts.apply(snapshot(many(3)));
        assert_eq!(state.accounts.selected().unwrap().position, 3);
        let rows = draw(&state, 80, 24);
        assert!(rows[6].starts_with("▌  3   a3@x.co"), "{}", rows[6]);
        assert!(rows[9].contains("7d"), "{rows:?}");
    }

    #[test]
    fn s_has_no_footer_label_for_the_account_in_use() {
        // three(): with `work` selected the footer at 80 is
        // `b best  n next  ⏎ history  P pause auto  v full list  r refresh` then the pinned keys;
        // with account 1 selected it is `s switch  b best  n next  ⏎ history  P pause auto  v full
        // list` then the pinned keys (no `r refresh`, S§8). The same in View::List with `v compact`.
        let mut state = loaded(three());
        for (view, label) in [("full list", "v full list"), ("compact", "v compact")] {
            let work = format!("b best  n next  ⏎ history  P pause auto  {label}  r refresh");
            let one = format!("s switch  b best  n next  ⏎ history  P pause auto  {label}");
            assert_eq!(footer_80(&state), footer_row(&work, 80), "{view}");
            update(&mut state, ch('k'), NOW_MS);
            assert_eq!(footer_80(&state), footer_row(&one, 80), "{view}");
            update(&mut state, ch('j'), NOW_MS);
            update(&mut state, ch('v'), NOW_MS);
        }
    }
}
