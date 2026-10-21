//! The Accounts screen (redesign §6): the lead card with the account in use (§6.2), the table
//! of accounts (§6.3), and the selected account's card (§6.4), stacked from the top (§6.1).

pub(crate) mod card;
pub(crate) mod lead;
pub(crate) mod status;
pub(crate) mod table;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use tagteam_core::AccountId;
use tagteam_engine::switch::LiveLogin;
use tagteam_engine::views::{AccountView, HistoryView, ProviderAccounts};

use self::table::{NAME_SPAN, Table};

use crate::daemon;
use crate::host::AccountRef;
use crate::msg::Cmd;
use crate::source::{AccountsSnapshot, SourceError};
use crate::state::{Screen, State};
use crate::theme::Role;
use crate::update;
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
/// The blank row under the lead card, and the one under the table (§6.1).
const BLANK: u16 = 1;
/// The lead card's least rows (T3-22).
const LEAD_LEAST: u16 = 2;

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
    /// The card's last history read (T3-21), for the account it names.
    card: Option<HistoryView>,
    /// The selected account, and its reading's `fetched_at`, that the card's read was last
    /// scheduled for.
    card_for: Option<(AccountId, Option<i64>)>,
    /// When the card's read is due (T3-21).
    card_due_ms: Option<i64>,
}

impl AccountsState {
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

    /// The card's half of a new selection or read (T3-21): when the selected account, or its
    /// reading's `fetched_at`, is not what the card's read was last scheduled for, that read
    /// is due `CARD_DEBOUNCE_MS` from `now_ms`. A change before then puts it off again.
    pub(crate) fn schedule_card(&mut self, now_ms: i64) {
        let now = self
            .selected_view()
            .map(|v| (v.row.id.clone(), v.usage.fetched_at));
        if now != self.card_for {
            self.card_due_ms = now.is_some().then_some(now_ms + card::CARD_DEBOUNCE_MS);
            self.card_for = now;
        }
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
            self.card_from = 0;
        }
        self.selected = Some(id);
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

/// T3-21: the card's read once it is due, while Accounts shows; it waits for Accounts
/// otherwise.
pub(crate) fn on_tick(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    // Whatever changed the selection, its card is read; a no-op when nothing did.
    state.accounts.schedule_card(now_ms);
    let due = state.accounts.card_due_ms.is_some_and(|at| at <= now_ms);
    if !due || state.screen != Screen::Accounts || state.quitting.is_some() {
        return vec![];
    }
    state.accounts.card_due_ms = None;
    let Some(id) = state.accounts.selected.clone() else {
        return vec![];
    };
    let since = now_ms.div_euclid(1000) - card::CARD_SINCE_S;
    vec![state.load_card(id, since)]
}

/// T3-21: the card's read for the selected account, under the card's latest generation; any
/// other is dropped. A failed read keeps the last view and raises no banner: the accounts
/// read's own banner covers a broken store.
pub(crate) fn on_card(
    state: &mut State,
    r#gen: u64,
    account: AccountId,
    result: Result<HistoryView, SourceError>,
) {
    if r#gen != state.gens.card || state.accounts.selected.as_ref() != Some(&account) {
        return;
    }
    if let Ok(view) = result {
        state.accounts.card = Some(view);
        state.dirty = true;
    }
}

pub fn handle_key(state: &mut State, key: KeyEvent, now_ms: i64) -> Vec<Cmd> {
    let body = crate::view::body_at(state);
    let page = visible_rows(state, body, now_ms).max(1) as isize;
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
            state.history.select(id);
            return update::show(state, Screen::History, now_ms);
        }
        _ => return vec![],
    }
    state.accounts.schedule_card(now_ms);
    let lines = layout(&state.accounts).len();
    let at = line_of(&state.accounts);
    let rows = visible_rows(state, body, now_ms);
    state.accounts.offset = keep_visible(state.accounts.offset, at, at, rows, lines);
    vec![]
}

/// `J`/`K`: the card's windows move `by` one, stopping once the last one shows, or at the
/// first.
fn scroll_card(state: &mut State, body: Rect, now_ms: i64, by: isize) {
    let Some(v) = state.accounts.selected_view() else {
        return;
    };
    let rows = usize::from(parts(state, body, now_ms).card.height);
    let furthest = card::furthest(state, v, body.width, rows, now_ms);
    // From what shows: a resize or a new read may have left the stored scroll past the end.
    let st = &mut state.accounts;
    let from = st.card_from.min(furthest) as isize;
    st.card_from = (from + by).clamp(0, furthest as isize) as usize;
}

/// The body's parts, top down (§6.1): the lead card, a blank row, the table, a blank row, the
/// selected card; spare rows stay at the bottom (VIS-5).
struct Parts {
    lead: Rect,
    table: Rect,
    card: Rect,
}

/// T3-22: the table's least lines (`TABLE_LEAST`), the blank rows and the card's least are
/// reserved first. The lead card has its lines, at most what that leaves (T3-32 folds the rest)
/// and never fewer than `LEAD_LEAST`; the card has its lines, at most what the lead card, the
/// blank rows and the table's least leave, and never fewer than its own least (its windows
/// scroll by `J`/`K`); the table takes the rest, at most its lines, and scrolls with the
/// selection. A part the body has no room left for is empty.
fn parts(state: &State, body: Rect, now_ms: i64) -> Parts {
    let rows = |n: usize| u16::try_from(n).unwrap_or(u16::MAX);
    let width = body.width;
    let Some(v) = state.accounts.selected_view() else {
        let none = |y| Rect::new(body.x, y, width, 0);
        return Parts {
            lead: none(body.y),
            table: body,
            card: none(body.y + body.height),
        };
    };
    let h = body.height;
    let lines = rows(table_lines(state));
    let table_least = lines.min(rows(TABLE_LEAST));
    let card_most = rows(card::height(state, v, width, now_ms));
    let card_least = rows(card::min_height(state, v, width, now_ms));
    let lead_most = rows(lead::height(state, width, now_ms));
    let blanks = 2 * BLANK;
    let reserved = table_least
        .saturating_add(blanks)
        .saturating_add(card_least);
    let lead = lead_most
        .min(h.saturating_sub(reserved))
        .max(lead_most.min(LEAD_LEAST));
    let card = card_most
        .min(h.saturating_sub(lead.saturating_add(blanks).saturating_add(table_least)))
        .max(card_least);
    let table = lines.min(h.saturating_sub(lead.saturating_add(blanks).saturating_add(card)));
    let bottom = body.y + h;
    let mut y = body.y;
    let mut take = |n: u16| {
        let at = y.min(bottom);
        y = at + n.min(bottom - at);
        Rect::new(body.x, at, width, y - at)
    };
    let lead = take(lead);
    take(BLANK);
    let table = take(table);
    take(BLANK);
    let card = take(card);
    Parts { lead, table, card }
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
    let now_s = now_ms.div_euclid(1000);
    let width = usize::from(area.width);
    let parts = parts(state, area, now_ms);
    let lead = lead::lead(state, area.width, usize::from(parts.lead.height), now_ms);
    frame.render_widget(Paragraph::new(lead.lines), parts.lead);
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
    if let Some(v) = st.selected_view() {
        // The account in use leaves out what the lead card shows of it (§6.4, T3-32).
        let on_lead = if v.active { lead.shown } else { vec![] };
        let rows = usize::from(parts.card.height);
        let lines = card::lines(state, v, area.width, rows, st.card_from, &on_lead, now_ms);
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
    use tagteam_engine::views::{ProviderAccounts, UsageStatus, UsageView};

    use super::*;
    use crate::msg::Msg;
    use crate::state::BannerKind;
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

    /// The body's first row on a `w`×`h` terminal.
    fn body_y(state: &State, w: u16, h: u16) -> u16 {
        crate::view::areas(state, Rect::new(0, 0, w, h)).body.y
    }

    /// The table's first row on a `w`×`h` terminal (§6.1: under the lead card).
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
            let row = lines
                .iter()
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
            let body = table_of(&state, 120, 40, NOW_MS);
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
            let y = table_y(&state, 120, 40);
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

    /// The selected account's card read back as the views lane answers it: each window's
    /// samples one every 20 minutes over the last 7 days, at its reading's percentage.
    fn with_card(state: &mut State) {
        let v = state.accounts.selected_view().unwrap().clone();
        let windows = v
            .usage
            .windows
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(|(w, p)| {
                let samples = (0..7 * 72).map(|i| sample(i * 1200 + 600, w.pct)).collect();
                history_window(w, samples, p)
            })
            .collect();
        let account = v.row.id.clone();
        let since = NOW_S - card::CARD_SINCE_S;
        let Cmd::LoadCard { r#gen, .. } = state.load_card(account.clone(), since) else {
            unreachable!()
        };
        let result = Ok(history_of(v, windows));
        update(
            state,
            Msg::Card {
                r#gen,
                account,
                result,
            },
            NOW_MS,
        );
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
    fn the_selected_card_shows_its_facts_sparklines_and_windows() {
        let mut accounts = three();
        accounts[2].usage.next_poll_at = Some(NOW_S + 120);
        for theme in TIERS {
            let mut state = loaded(accounts.clone());
            state.theme = theme;
            update(&mut state, key(KeyCode::End), NOW_MS);
            with_card(&mut state);
            let (y, card) = card_rows(&state, 120, 40);
            assert_eq!(
                card,
                [
                    "    3 spare  alex@personal.example    not in use · updated 14m ago · next update in 2m".to_owned(),
                    format!("    5h    {} 24h    0%  resets in 2h40m", "▁".repeat(32)),
                    format!("    7d    {} 7d    31%  resets in 3d09h", "▃".repeat(32)),
                    "    ! Old reading: too old to pick the best account by.".to_owned(),
                    "      tagteam checks it within 2m.".to_owned(),
                    "    ⏎ history of spare   e disable   a alias   x remove".to_owned(),
                ],
                "{theme:?}"
            );
            let buf = draw_buffer(&state, 120, 40, NOW_MS);
            let check = |row: u16, needle: &str, role: char| {
                let (got, want) = role_of(&buf, theme, y + row, needle, role);
                assert_eq!(got, want, "{theme:?} {needle:?}");
            };
            // The title: its facts muted, the old reading's age in warn.
            check(0, "alex@personal.example", 'm');
            check(0, "not in use", 'm');
            check(0, "updated 14m ago", 'w');
            check(0, "next update in 2m", 'm');
            // The windows: the sparkline accent, its tag faint, the rest in text.
            for (row, spark, tag) in [(1, "▁", "24h"), (2, "▃", "7d")] {
                check(row, &spark.repeat(32), 'a');
                // The tag follows the sparkline's 32 cells and a space.
                let letters: String = roles(&buf, theme, y + row)
                    .chars()
                    .skip(43)
                    .take(tag.len())
                    .collect();
                assert_eq!(
                    letters,
                    canon(theme, &"f".repeat(tag.len())),
                    "{theme:?} {tag}"
                );
            }
            check(1, "  0%  resets in 2h40m", 't');
            check(2, " 31%  resets in 3d09h", 't');
            // The problem in its role, its remedy in text.
            check(
                3,
                "! Old reading: too old to pick the best account by.",
                'w',
            );
            check(4, "tagteam checks it within 2m.", 't');
            // The keys accent, their labels muted.
            for (key, label) in [
                ("⏎", "history of spare"),
                ("e", "disable"),
                ("a", "alias"),
                ("x", "remove"),
            ] {
                check(5, label, 'm');
                let at = col(&card[5], &format!("{key} {label}"));
                let letter = roles(&buf, theme, y + 5).chars().nth(at);
                assert_eq!(letter, canon(theme, "a").chars().next(), "{theme:?} {key}");
            }
            // The alias is the title's one bold, past tier 0's roles.
            if theme != TIERS[0] {
                assert_eq!(
                    bold_cells(&buf, y),
                    (6..11).collect::<Vec<_>>(),
                    "{theme:?}"
                );
            }
        }
    }

    #[test]
    fn the_card_of_the_account_in_use_leaves_out_its_reset_and_projection() {
        // T3-32: the lead card shows them for the account in use.
        let mut accounts = three();
        let windows = accounts[1].usage.windows.as_mut().unwrap();
        windows[0].1 = Pace {
            rate_per_hour: Some(15.7),
            will_last_to_reset: Some(true),
            ..Pace::default()
        };
        windows[1].1.ahead = Some(true);
        let mut state = loaded(accounts);
        assert_eq!(state.accounts.selected().map(|a| a.position), Some(2));
        let card = card_rows(&state, 120, 40).1;
        assert!(card[1].ends_with(" 53%"), "{card:?}");
        assert!(
            card[2].ends_with("  6% ▲ faster than an even pace"),
            "{card:?}"
        );
        let narrow = card_rows(&state, 80, 24).1;
        assert!(narrow[1].ends_with(" 53%"), "{narrow:?}");
        assert!(narrow[2].ends_with("  6% ▲"), "{narrow:?}");
        // Another account's card has them.
        update(&mut state, ch('k'), NOW_MS);
        let card = card_rows(&state, 120, 40).1;
        assert!(card[1].ends_with("  resets in 2h40m"), "{card:?}");
        assert!(card[2].ends_with("  resets in 3d09h"), "{card:?}");
    }

    #[test]
    fn a_window_the_lead_card_does_not_show_keeps_its_reset_on_the_in_use_card() {
        // T3-32: at 80 columns the lead card shows the short and long windows only.
        let mut accounts = three();
        accounts[1].usage.windows.as_mut().unwrap().push((
            window(
                "scoped:fable",
                "Fable",
                WindowKind::Scoped,
                40.0,
                Some(291_630),
            ),
            Pace::default(),
        ));
        let state = loaded(accounts);
        assert_eq!(state.accounts.selected().map(|a| a.position), Some(2));
        let card = card_rows(&state, 80, 24).1;
        assert!(card[1].starts_with("    5h ") && card[1].ends_with(" 53%"));
        assert!(card[2].starts_with("    7d ") && card[2].ends_with("  6%"));
        assert!(
            card[3].starts_with("    Fable ") && card[3].ends_with(" 40%  resets in 3d09h"),
            "{card:?}"
        );
        // From 120 columns the lead card shows every window.
        let wide = card_rows(&state, 120, 40).1;
        assert!(wide[3].ends_with(" 40%"), "{wide:?}");
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
                    "    3 spare  alex@personal.example    not in use",
                    "    ✕ Needs a new login: its stored login was refused (invalid_grant), Sat 09:02.",
                    "      Log in to Claude Code as alex@personal.example, then press A here.",
                    "    ⏎ history of spare   e disable   a alias   x remove",
                ],
                "{theme:?}"
            );
            let buf = draw_buffer(&state, 120, 40, NOW_MS);
            let (got, want) = role_of(&buf, theme, y + 1, &card[1][4..], 'c');
            assert_eq!(got, want, "{theme:?}");
            let (got, want) = role_of(&buf, theme, y + 2, &card[2][6..], 't');
            assert_eq!(got, want, "{theme:?}");
            // At 80 columns the sentence wraps at its words, under its text.
            let (y, card) = card_rows(&state, 80, 24);
            assert_eq!(
                card[1..],
                [
                    "    ✕ Needs a new login: its stored login was refused (invalid_grant), Sat",
                    "      09:02.",
                    "      Log in to Claude Code as alex@personal.example, then press A here.",
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
            card[0].ends_with("not in use · disabled · updated 14m ago"),
            "{card:?}"
        );
        assert_eq!(
            card[3..5],
            [
                "    ! Rate-limited: the usage check was turned away for now.",
                "      tagteam tries again in 15m.",
            ]
        );
        assert_eq!(
            card[5],
            "    ⏎ history of spare   e enable   a alias   x remove"
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
        assert!(card[1].ends_with("   9%  not started"), "{card:?}");
        // From 120 columns `▲` says what it means, when the row has room.
        assert!(
            card[2].ends_with(
                " 77% ▲ faster than an even pace (62% expected by now)  resets in 3d09h"
            ),
            "{card:?}"
        );
        // A scoped window without a reset has none to say; one whose reset passed says so.
        assert!(card[3].ends_with(" 40%"), "{card:?}");
        assert!(card[4].ends_with(" 10%  reset"), "{card:?}");
        let narrow = card_rows(&state, 80, 24).1;
        assert!(narrow[2].ends_with(" 77% ▲  resets in 3d09h"), "{narrow:?}");
    }

    #[test]
    fn the_card_title_cuts_the_name_before_the_facts() {
        // Review Focus 3: the facts are the card's to say.
        let mut v = account(1, "alex@example.com", read(120, 9.0, 12.0, false));
        v.row.alias = Some("a".repeat(200));
        let title = card_rows(&loaded(vec![v]), 80, 24).1[0].clone();
        assert!(title.starts_with("    1 aaaa"), "{title}");
        assert!(
            title.ends_with("a…    not in use · updated 2m ago"),
            "{title}"
        );
        assert_eq!(width_of(&title), 80);
    }

    #[test]
    fn the_card_title_cuts_a_long_email_before_the_facts() {
        // Whether the account is session-owned is the card's to say.
        let long = "a".repeat(52) + "@example.com";
        let mut v = account(1, &long, read(60, 9.0, 12.0, false));
        v.in_session = true;
        let title = card_rows(&loaded(vec![v]), 80, 24).1[0].clone();
        assert!(title.starts_with("    1 aaaa"), "{title}");
        assert!(
            title.ends_with("…    not in use · in a session · updated 1m ago"),
            "{title}"
        );
        assert_eq!(width_of(&title), 80);
    }

    /// The card reads `cmds` asks for: each one's account and `since_s`.
    fn card_reads(cmds: &[Cmd]) -> Vec<(AccountId, i64)> {
        cmds.iter()
            .filter_map(|c| match c {
                Cmd::LoadCard {
                    account, since_s, ..
                } => Some((account.clone(), *since_s)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_selection_change_reads_the_card_s_history_once_after_250_ms() {
        let week = 7 * 86_400;
        let mut state = loaded(three());
        assert!(card_reads(&update(&mut state, ch('k'), NOW_MS)).is_empty());
        assert!(card_reads(&update(&mut state, ch('j'), NOW_MS)).is_empty());
        let tick = |state: &mut State, at: i64| card_reads(&update(state, Msg::Tick, at));
        assert!(tick(&mut state, NOW_MS + 249).is_empty());
        assert_eq!(tick(&mut state, NOW_MS + 250), [(id(2), NOW_S - week)]);
        assert!(tick(&mut state, NOW_MS + 500).is_empty());
        // A read that leaves its reading as it was asks for nothing.
        let read_back = |state: &mut State, accounts: Vec<AccountView>, at: i64| {
            let Cmd::LoadAccounts { r#gen } = state.load_accounts() else {
                unreachable!()
            };
            let result = Ok(snapshot(accounts));
            card_reads(&update(state, Msg::Accounts { r#gen, result }, at))
        };
        assert!(read_back(&mut state, three(), NOW_MS + 600).is_empty());
        assert!(tick(&mut state, NOW_MS + 1_000).is_empty());
        // One that moved its `fetched_at`: once more, 250 ms after it arrived.
        let mut moved = three();
        moved[1].usage.fetched_at = Some(NOW_S);
        assert!(read_back(&mut state, moved, NOW_MS + 1_000).is_empty());
        assert!(tick(&mut state, NOW_MS + 1_249).is_empty());
        assert_eq!(
            tick(&mut state, NOW_MS + 1_250),
            [(id(2), NOW_S + 1 - week)]
        );
        assert!(tick(&mut state, NOW_MS + 1_500).is_empty());
        // A selection that moves again before then puts it off.
        update(&mut state, ch('j'), NOW_MS + 2_000);
        update(&mut state, ch('k'), NOW_MS + 2_200);
        assert!(tick(&mut state, NOW_MS + 2_250).is_empty());
        assert_eq!(
            tick(&mut state, NOW_MS + 2_450),
            [(id(2), NOW_S + 2 - week)]
        );
        // The first selection counts: the first accounts read is read for its card too. It is
        // the account in use (T3-23).
        let mut fresh = fresh(Screen::Accounts);
        assert!(read_back(&mut fresh, three(), NOW_MS).is_empty());
        assert_eq!(tick(&mut fresh, NOW_MS + 250), [(id(2), NOW_S - week)]);
        // Elsewhere it waits for Accounts.
        let mut away = loaded(three());
        update(&mut away, ch('k'), NOW_MS);
        update(&mut away, ch('j'), NOW_MS);
        update(&mut away, ch('2'), NOW_MS);
        assert!(tick(&mut away, NOW_MS + 250).is_empty());
        update(&mut away, ch('1'), NOW_MS + 300);
        assert_eq!(tick(&mut away, NOW_MS + 500), [(id(2), NOW_S - week)]);
    }

    #[test]
    fn a_card_read_for_another_account_or_an_older_generation_is_dropped() {
        // Review Focus 4.
        let mut state = loaded(three());
        update(&mut state, key(KeyCode::Home), NOW_MS);
        let since = NOW_S - card::CARD_SINCE_S;
        let Cmd::LoadCard { r#gen: old, .. } = state.load_card(id(1), since) else {
            unreachable!()
        };
        let Cmd::LoadCard { r#gen: new, .. } = state.load_card(id(1), since) else {
            unreachable!()
        };
        let view = |pct: f64| {
            let v = state.accounts.selected_view().unwrap().clone();
            let windows = v
                .usage
                .windows
                .clone()
                .unwrap()
                .into_iter()
                .map(|(w, p)| history_window(w, vec![sample(600, pct)], p))
                .collect();
            history_of(v, windows)
        };
        let (full, half) = (view(100.0), view(50.0));
        let card = |r#gen: u64, account: AccountId, result| Msg::Card {
            r#gen,
            account,
            result,
        };
        let before = draw(&state, 120, 40);
        update(&mut state, card(old, id(1), Ok(full.clone())), NOW_MS);
        assert_eq!(draw(&state, 120, 40), before, "an older generation");
        update(&mut state, card(new, id(2), Ok(full.clone())), NOW_MS);
        assert_eq!(draw(&state, 120, 40), before, "another account");
        update(&mut state, card(new, id(1), Ok(half)), NOW_MS);
        let shown = draw(&state, 120, 40);
        assert_ne!(shown, before);
        assert!(card_rows(&state, 120, 40).1[1].contains('▅'), "{shown:?}");
        // A failed read keeps the last view, and raises no banner.
        let Cmd::LoadCard { r#gen: failed, .. } = state.load_card(id(1), since) else {
            unreachable!()
        };
        let error = SourceError {
            kind: "store".into(),
            message: "the store could not be read".into(),
        };
        update(&mut state, card(failed, id(1), Err(error)), NOW_MS);
        assert_eq!(draw(&state, 120, 40), shown);
        assert_eq!(state.banner_text(), None);
        // A read for the account selected before is dropped once another is selected.
        let Cmd::LoadCard { r#gen: late, .. } = state.load_card(id(1), since) else {
            unreachable!()
        };
        update(&mut state, ch('j'), NOW_MS);
        let moved = draw(&state, 120, 40);
        update(&mut state, card(late, id(1), Ok(full)), NOW_MS);
        assert_eq!(draw(&state, 120, 40), moved);
        // Back on the first account, its card is still the last read it took, not the late one.
        update(&mut state, ch('k'), NOW_MS);
        let back = card_rows(&state, 120, 40).1;
        assert!(back[1].contains('▅'), "{back:?}");
        assert!(!back[1].contains('█'), "{back:?}");
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
        // Seven windows fit whole in what T3-22 leaves the card under a 3-row banner.
        let mut state = loaded(accounts(scoped(5)));
        state.raise_banner(BannerKind::Refusal, banner.clone());
        let seven = card_windows(&state, 80, 24);
        assert_eq!(seven.len(), 7, "{seven:?}");
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
        assert_eq!(card_windows(&state, 80, 24), seven);
        // Twenty-four do not: `J` until nothing moves shows the last.
        let mut state = loaded(accounts(scoped(22)));
        state.raise_banner(BannerKind::Refusal, banner);
        let first = card_windows(&state, 80, 24);
        assert_eq!(first[0], "5h");
        assert!(first.len() < 24, "{first:?}");
        // The last window row says which show, muted, in the other screens' words.
        let (y, said) = hint(&state, 24).unwrap();
        assert_eq!(said, format!("    J K: windows 1–{} of 24", first.len()));
        let buf = draw_buffer(&state, 80, 24, NOW_MS);
        let (got, want) = role_of(&buf, Theme::new(false), y, &said[4..], 'm');
        assert_eq!(got, want);
        update(&mut state, ch('J'), NOW_MS);
        let moved = format!("    J K: windows 2–{} of 24", first.len() + 1);
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
        let end = format!("    J K: windows {}–24 of 24", 25 - last.len());
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
        let clamped = format!("    J K: windows {from}–24 of 24");
        assert_eq!(hint(&state, 30).map(|h| h.1), Some(clamped));
        update(&mut state, ch('K'), NOW_MS);
        let up = card_windows(&state, 80, 30);
        assert_eq!(up[1..], taller[..taller.len() - 1], "{up:?}");
        let back = format!("    J K: windows {}–23 of 24", from - 1);
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
            // No history read yet: the sparklines' buckets are empty.
            let (y, card) = card_rows(&state, 120, 40);
            assert_eq!(
                card,
                [
                    "    1 alex@example.com    not in use · updated 2m ago · login valid 6 more days".to_owned(),
                    format!("    5h    {} 24h    9%  resets in 2h40m", " ".repeat(32)),
                    format!(
                        "    7d    {} 7d    77% ▲  resets in 3d09h · +1.2%/h · full in 2d04h",
                        " ".repeat(32)
                    ),
                    "    ⏎ history of alex   e disable   a alias   x remove".to_owned(),
                ],
                "{theme:?}"
            );
            // The window fills before it resets: its projection is warn, as `▲` is.
            let buf = draw_buffer(&state, 120, 40, NOW_MS);
            let (got, want) = role_of(&buf, theme, y + 2, "+1.2%/h · full in 2d04h", 'w');
            assert_eq!(got, want, "{theme:?}");
            let (got, want) = role_of(&buf, theme, y + 2, "▲", 'w');
            assert_eq!(got, want, "{theme:?}");
            let (got, want) = role_of(&buf, theme, y + 2, "77%", 'w');
            assert_eq!(got, want, "{theme:?}");
        }
        // At 80 columns: 16 cells, the projection's clause, and no keys row.
        let mut state = loaded(accounts);
        update(&mut state, key(KeyCode::Home), NOW_MS);
        let card = card_rows(&state, 80, 24).1;
        assert_eq!(
            card,
            [
                "    1 alex@example.com    not in use · updated 2m ago · login valid 6 more days"
                    .to_owned(),
                format!("    5h    {} 24h    9%  resets in 2h40m", " ".repeat(16)),
                format!(
                    "    7d    {} 7d    77% ▲  resets in 3d09h · full in 2d04h",
                    " ".repeat(16)
                ),
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
                "    3 spare  alex@personal.example    not in use",
                "    ✕ Needs a new login: its stored login was refused (invalid_grant).",
                "      Log in to Claude Code as alex@personal.example, then press A here.",
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
        assert_eq!(
            card_rows(&state, 120, 40).1[0],
            "    1 alex@example.com    not in use · updated 2m ago · next update in 1m · login valid 6 more days"
        );
        // Once it has come, a frame drawn before the update lands says it is close.
        assert_eq!(
            card_at(&state, 120, 40, NOW_MS + 60_000).1[0],
            "    1 alex@example.com    not in use · updated 3m ago · next update in <1m · login valid 5 more days"
        );
        // At 80 columns the name keeps its 16 columns, and the facts give way from their end.
        let title = card_rows(&state, 80, 24).1[0].clone();
        assert_eq!(
            title,
            "    1 alex@example.com    not in use · updated 2m ago · next update in 1m · log…"
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
            card[3..],
            [
                "    ! Rate-limited: the usage check was turned away for now.",
                "      tagteam tries again in 15m.",
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
            card_rows(&state, 80, 24).1[0],
            "    4 a4@x.co    not in use · api key"
        );
        let mut accounts = three();
        accounts[2].usage = unread(UsageStatus::ReloginRequired, None, None);
        accounts[2].row.quarantine_reason = Some("invalid_grant".into());
        let mut state = loaded(accounts.clone());
        update(&mut state, key(KeyCode::End), NOW_MS);
        assert_eq!(
            card_rows(&state, 80, 24).1[0],
            "    3 spare  alex@personal.example    not in use"
        );
        // Never read: it says so.
        accounts[2].usage = unread(UsageStatus::Unavailable, None, None);
        accounts[2].row.quarantine_reason = None;
        let mut state = loaded(accounts);
        update(&mut state, key(KeyCode::End), NOW_MS);
        assert_eq!(
            card_rows(&state, 80, 24).1[0],
            "    3 spare  alex@personal.example    not in use · not checked yet"
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
                card[0],
                "    1 alex@example.com    not in use · updated 2m ago · login expired"
            );
            let buf = draw_buffer(&state, 120, 40, NOW_MS);
            let (got, want) = role_of(&buf, theme, y, "login expired", 'w');
            assert_eq!(got, want, "{theme:?}");
            let (got, want) = role_of(&buf, theme, y, "updated 2m ago", 'm');
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
        assert_eq!(card.len(), 5, "a row a window: {card:?}");
        assert!(card.iter().all(|l| width_of(l) <= 80), "{card:?}");
        let label = |name: &str| format!("    {name:<17}  ");
        assert!(card[2].starts_with(&label("7d")), "{card:?}");
        assert!(
            card[2].contains(" 77% ▲  resets in 3d09h · full in ") && card[2].ends_with('…'),
            "{card:?}"
        );
        assert!(card[3].starts_with(&label("spend")), "{card:?}");
        assert!(
            card[3].ends_with("  €123.45 of €200  resets in 14d00h"),
            "{card:?}"
        );
        assert!(card[4].starts_with(&label("Claude Sonnet 4.5")), "{card:?}");
        assert!(card[4].ends_with('…'), "{card:?}");
        // A wide terminal has room for each window whole.
        let wide = card_rows(&state, 120, 40).1;
        assert!(
            wide[2].ends_with(" 77% ▲  resets in 3d09h · +1.2%/h · full in 2d04h"),
            "{wide:?}"
        );
        assert!(
            wide[4].ends_with(" 41% ▲  resets in 3d09h · +1.5%/h · full in 1d00h"),
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
            card[1].starts_with(&format!("    {:<20}  ", "5h")),
            "{card:?}"
        );
        assert!(
            card[3].starts_with(&format!("    {}  ", fit(OVERSIZED, 20))),
            "{card:?}"
        );
        let value = col(&card[1], "  9%");
        assert_eq!(col(&card[3], " 40%"), value, "{card:?}");
        assert!(card[3].contains(" 40% ▲  resets in 3d09h"), "{card:?}");
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
        let label = format!("    {}  ", fit(&thousand(), 20));
        let row = card
            .iter()
            .find(|l| l.starts_with(&label))
            .unwrap_or_else(|| panic!("{lines:?}"));
        assert!(row.contains(" 40% ▲  resets in 3d09h"), "{row}");
        assert_eq!(
            card[card.len() - 2..],
            [
                "    ! Rate-limited: the usage check was turned away for now.",
                "      tagteam tries again in 5m.",
            ]
        );
    }

    #[test]
    fn the_worst_live_stack_at_80_by_24_still_reaches_a_long_labelled_window() {
        // The header, the auto line, a 3-row banner and a 3-row toast leave a 16-row body; the
        // window's row and the problem's rows fit in what is left.
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
        assert_eq!(
            lines[5],
            "  No account in use                      best now  1 alex@example.com   b switch",
            "the body starts at row 5: {lines:?}"
        );
        assert!(
            lines[8].contains("ACCOUNT"),
            "the table under the lead card: {lines:?}"
        );
        assert!(
            lines[21..].iter().all(|l| l.contains("(position 1)")
                || l.starts_with("1) Switched")
                || l.contains("Switched to alex")),
            "{lines:?}"
        );
        let row = lines
            .iter()
            .find(|l| l.starts_with("    Claude Claude"))
            .unwrap_or_else(|| panic!("{lines:?}"));
        assert!(row.contains(" 40% ▲  resets in 3d09h"), "{lines:?}");
        assert!(
            lines
                .iter()
                .any(|l| l == "    ! Rate-limited: the usage check was turned away for now."),
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
                .filter(|l| l.starts_with("    Claude Claude"))
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
            card.iter().any(|l| l.starts_with("    Claude Claude")),
            "{card:?}"
        );
        assert!(
            card.iter()
                .any(|l| l == "    ! Rate-limited: the usage check was turned away for now."),
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
        assert_eq!(
            card[5],
            format!(
                "    Haiku   {} 7d    42% ▲  resets in 3d09h · +1.5%/h · full in 1d00h",
                " ".repeat(32)
            )
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
        // 80×28: 25 body rows, 2 for the lead card and a blank row under it, 3 for the card and a
        // blank row above it, 1 for the header: 17 account rows.
        state.size = (80, 28);
        let shown = |state: &State| {
            table_of(state, 80, 28, NOW_MS)[1..18]
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
        assert_eq!(shown(&state).first().unwrap(), "a2@x.co");
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
        // At 80×28 the table has the 18 rows 80×24 gave it before the auto line and the lead card.
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
        assert_eq!(first(&state, 28), "a14@x.co");
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

    /// The lead card's rows at `w`×`h`, and the row it starts at.
    fn lead_rows(state: &State, w: u16, h: u16) -> (u16, Vec<String>) {
        rows_of_part(state, w, h, |p| p.lead)
    }

    /// The text of `spans`.
    fn joined(spans: Option<Vec<Span<'static>>>) -> Option<String> {
        spans.map(|s| s.iter().map(|s| s.content.as_ref()).collect())
    }

    /// `fill` cells of `▆`, then `▁` to `cells`.
    fn lead_bar_of(fill: usize, cells: usize) -> String {
        format!("{}{}", "▆".repeat(fill), "▁".repeat(cells - fill))
    }

    const NO_ACCOUNT_IN_USE: [&str; 2] = [
        "  No account in use",
        "  s switches to the selected account",
    ];

    #[test]
    fn the_lead_card_shows_the_account_in_use_with_its_bars_and_the_best_pick() {
        for theme in TIERS {
            let mut state = loaded(three());
            state.theme = theme;
            let (y, lead) = lead_rows(&state, 120, 40);
            assert_eq!(lead.len(), 3, "{lead:?}");
            assert!(
                lead[0].starts_with("  ● 2 work  sam@corp.example · Example Corp    in use"),
                "{}",
                lead[0]
            );
            assert!(
                lead[0].ends_with("best now  1 alex@example.com   b switch"),
                "{}",
                lead[0]
            );
            assert_eq!(width_of(&lead[0]), 120, "{}", lead[0]);
            assert_eq!(
                lead[1],
                format!("    5h    {}   53%  resets in 2h40m", lead_bar_of(28, 52))
            );
            assert_eq!(
                lead[2],
                format!("    7d    {}    6%  resets in 3d09h", lead_bar_of(3, 52))
            );
            let buf = draw_buffer(&state, 120, 40, NOW_MS);
            let check = |row: u16, needle: &str, role: char| {
                let (got, want) = role_of(&buf, theme, y + row, needle, role);
                assert_eq!(got, want, "{theme:?} {needle:?}");
            };
            check(0, "●", 'a');
            check(0, "sam@corp.example · Example Corp", 'm');
            check(0, "in use", 'm');
            check(0, "best now", 'm');
            check(0, "1 alex@example.com", 't');
            check(0, "switch", 'm');
            let b = col(&lead[0], "b switch");
            assert_eq!(
                roles(&buf, theme, y).chars().nth(b),
                canon(theme, "a").chars().next(),
                "{theme:?}"
            );
            for x in 6..10 {
                assert!(
                    buf[(x, y)].modifier.contains(Modifier::BOLD),
                    "{theme:?} {x}"
                );
            }
            // The bars: the fill accent, the baseline faint.
            for (row, fill) in [(1, 28), (2, 3)] {
                let letters: String = roles(&buf, theme, y + row)
                    .chars()
                    .skip(10)
                    .take(52)
                    .collect();
                let want = "a".repeat(fill) + &"f".repeat(52 - fill);
                assert_eq!(letters, canon(theme, &want), "{theme:?} {row}");
            }
        }
        // At 80 columns: 30 cells, the short and long windows, and no org.
        let lead = lead_rows(&loaded(three()), 80, 24).1;
        assert_eq!(
            lead,
            [
                "  ● 2 work  sam@corp.example    in use   best now  1 alex@example.com   b switch"
                    .to_owned(),
                format!("    5h    {}   53%  resets in 2h40m", lead_bar_of(16, 30)),
                format!("    7d    {}    6%  resets in 3d09h", lead_bar_of(2, 30)),
            ]
        );
    }

    #[test]
    fn each_forecast_has_its_words_on_the_lead_card() {
        use tagteam_engine::switch::{BestForecast, NoPick};
        let cases = [
            (
                BestForecast::Pick { order: vec![1] },
                Some("best now  1 alex@example.com   b switch"),
                Some("best now 1 alex@example.com"),
            ),
            (
                BestForecast::Pick { order: vec![3, 1] },
                Some("best now  3 spare   b switch"),
                Some("best now 3 spare"),
            ),
            (
                BestForecast::AlreadyBest,
                Some("on the best account"),
                Some("on the best account"),
            ),
            (
                BestForecast::None(NoPick::UsageUnknown),
                Some("b checks usage first, then picks"),
                None,
            ),
            (BestForecast::None(NoPick::TooFewCandidates), None, None),
            (BestForecast::None(NoPick::UnmanagedLogin), None, None),
            (BestForecast::None(NoPick::CandidatesExhausted), None, None),
        ];
        for theme in TIERS {
            for (forecast, keyed, bare) in cases.clone() {
                let mut snap = snapshot(three());
                snap.providers[0].forecast = forecast.clone();
                let mut state = fresh(Screen::Accounts);
                state.theme = theme;
                state.accounts.apply(snap);
                let (y, lead) = lead_rows(&state, 120, 40);
                match keyed {
                    Some(words) => {
                        assert!(lead[0].ends_with(words), "{forecast:?}: {}", lead[0]);
                        assert_eq!(width_of(&lead[0]), 120, "{}", lead[0]);
                    }
                    None => assert!(lead[0].ends_with("    in use"), "{forecast:?}: {}", lead[0]),
                }
                let view = &state.accounts.snapshot().unwrap().providers[0];
                assert_eq!(
                    joined(lead::pick_words(&state, view, true)).as_deref(),
                    keyed
                );
                assert_eq!(
                    joined(lead::pick_words(&state, view, false)).as_deref(),
                    bare
                );
                let buf = draw_buffer(&state, 120, 40, NOW_MS);
                match forecast {
                    BestForecast::AlreadyBest => {
                        let (got, want) = role_of(&buf, theme, y, "on the best account", 'm');
                        assert_eq!(got, want, "{theme:?}");
                    }
                    BestForecast::None(NoPick::UsageUnknown) => {
                        let (got, want) =
                            role_of(&buf, theme, y, "checks usage first, then picks", 'm');
                        assert_eq!(got, want, "{theme:?}");
                        let b = col(&lead[0], "b checks");
                        assert_eq!(
                            roles(&buf, theme, y).chars().nth(b),
                            canon(theme, "a").chars().next(),
                            "{theme:?}"
                        );
                    }
                    _ => {}
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
        "    ✕ Needs a new login: its stored login was refused (invalid_grant).",
        "      Log in to Claude Code as sam@corp.example, then press A here.",
    ];

    #[test]
    fn a_problem_with_the_account_in_use_shows_under_its_bars() {
        let accounts = quarantined_in_use(three());
        for theme in TIERS {
            let mut state = loaded(accounts.clone());
            state.theme = theme;
            let (y, lead) = lead_rows(&state, 120, 40);
            assert_eq!(lead.len(), 5, "{lead:?}");
            assert!(lead[0].starts_with("  ● 2 work"), "{lead:?}");
            assert!(lead[1].starts_with("    5h    ▆"), "{lead:?}");
            assert!(lead[2].starts_with("    7d    ▆"), "{lead:?}");
            assert_eq!(lead[3..], NEEDS_LOGIN_ROWS, "{lead:?}");
            let buf = draw_buffer(&state, 120, 40, NOW_MS);
            let (got, want) = role_of(&buf, theme, y + 3, &NEEDS_LOGIN_ROWS[0][4..], 'c');
            assert_eq!(got, want, "{theme:?}");
            let (got, want) = role_of(&buf, theme, y + 4, &NEEDS_LOGIN_ROWS[1][6..], 't');
            assert_eq!(got, want, "{theme:?}");
            // The table's STATUS says it too, whatever is selected.
            update(&mut state, key(KeyCode::End), NOW_MS);
            let table = rows_of_part(&state, 120, 40, |p| p.table).1;
            assert!(table[2].ends_with("  ✕ new login"), "{table:?}");
            assert_eq!(lead_rows(&state, 120, 40).1[3..], NEEDS_LOGIN_ROWS);
        }
    }

    #[test]
    fn a_login_close_to_expiry_is_warn_and_says_to_log_in_again() {
        let mut accounts = three();
        accounts[1].row.login_expires_at = Some((NOW_S + 6 * 86_400 + 3 * HOUR) * 1000);
        for theme in TIERS {
            let mut state = loaded(accounts.clone());
            state.theme = theme;
            // 160 columns hold the name, the org, the facts and the pick whole.
            let (y, lead) = lead_rows(&state, 160, 40);
            let facts = "    in use · login valid 6 more days · log in again before then";
            assert!(lead[0].contains(facts), "{}", lead[0]);
            assert!(
                lead[0].ends_with("best now  1 alex@example.com   b switch"),
                "{}",
                lead[0]
            );
            let buf = draw_buffer(&state, 160, 40, NOW_MS);
            for (needle, role) in [
                ("in use", 'm'),
                ("login valid 6 more days", 'w'),
                ("log in again before then", 'w'),
            ] {
                let (got, want) = role_of(&buf, theme, y, needle, role);
                assert_eq!(got, want, "{theme:?} {needle}");
            }
        }
        // At 120 the name keeps its 16 columns and the facts give way from their end.
        let lead = lead_rows(&loaded(accounts.clone()), 120, 40).1;
        assert!(
            lead[0].starts_with(
                "  ● 2 work  sam@corp.…    in use · login valid 6 more days · log in again bef…   best now"
            ),
            "{}",
            lead[0]
        );
        // Once passed: `login expired` in warn, and nothing to do before then.
        accounts[1].row.login_expires_at = Some((NOW_S - HOUR) * 1000);
        for theme in TIERS {
            let mut state = loaded(accounts.clone());
            state.theme = theme;
            let (y, lead) = lead_rows(&state, 120, 40);
            assert!(
                lead[0].contains("    in use · login expired   "),
                "{}",
                lead[0]
            );
            assert!(!lead[0].contains("log in again"), "{}", lead[0]);
            let buf = draw_buffer(&state, 120, 40, NOW_MS);
            let (got, want) = role_of(&buf, theme, y, "login expired", 'w');
            assert_eq!(got, want, "{theme:?}");
        }
        accounts[1].row.login_expires_at = Some((NOW_S + 27 * 86_400) * 1000);
        let state = loaded(accounts);
        let (y, lead) = lead_rows(&state, 120, 40);
        assert!(
            lead[0].contains("    in use · login valid 27 more days   "),
            "{}",
            lead[0]
        );
        assert!(!lead[0].contains("log in again"), "{}", lead[0]);
        let buf = draw_buffer(&state, 120, 40, NOW_MS);
        let (got, want) = role_of(&buf, state.theme, y, "login valid 27 more days", 'm');
        assert_eq!(got, want);
    }

    #[test]
    fn from_120_columns_the_lead_card_shows_every_window_and_spend_as_amounts() {
        let mut accounts = three();
        let windows = accounts[1].usage.windows.as_mut().unwrap();
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
        let mut spend = window("spend", "spend", WindowKind::Spend, 24.8, Some(1_209_630));
        spend.detail = Some(r#"{"used": 12.4, "limit": 50, "currency": "EUR"}"#.parse().unwrap());
        windows.push((spend, Pace::default()));
        let state = loaded(accounts);
        let lead = lead_rows(&state, 120, 40).1;
        assert_eq!(lead.len(), 5, "{lead:?}");
        // The label column is the widest label shown.
        assert!(lead[1].starts_with("    5h     ▆"), "{lead:?}");
        assert!(lead[3].starts_with("    Fable  ▆"), "{lead:?}");
        assert!(lead[3].ends_with("   40%  resets in 3d09h"), "{lead:?}");
        assert!(lead[4].starts_with("    spend  ▆"), "{lead:?}");
        assert!(
            lead[4].ends_with("  €12.40 of €50  resets in 14d00h"),
            "{lead:?}"
        );
        assert_eq!(
            col(&lead[4], "€12.40 of €50"),
            col(&lead[1], " 53%"),
            "{lead:?}"
        );
        // At 80 columns the short and long windows only.
        let narrow = lead_rows(&state, 80, 24).1;
        assert_eq!(narrow.len(), 3, "{narrow:?}");
        assert!(
            !narrow
                .iter()
                .any(|l| l.contains("Fable") || l.contains('€'))
        );
    }

    #[test]
    fn with_no_account_in_use_the_lead_card_says_so() {
        for theme in TIERS {
            let mut state = loaded(many(3));
            state.theme = theme;
            let (y, lead) = lead_rows(&state, 80, 24);
            let pick = "best now  1 a1@x.co   b switch";
            let gap = " ".repeat(80 - width_of(NO_ACCOUNT_IN_USE[0]) - pick.len());
            assert_eq!(lead[0], format!("{}{gap}{pick}", NO_ACCOUNT_IN_USE[0]));
            assert_eq!(lead[1], NO_ACCOUNT_IN_USE[1]);
            let buf = draw_buffer(&state, 80, 24, NOW_MS);
            let title: Vec<u16> = (2..19).filter(|x| ![4, 12, 15].contains(x)).collect();
            let bold: Vec<u16> = bold_cells(&buf, y)
                .into_iter()
                .filter(|x| *x < 19)
                .collect();
            assert_eq!(bold, title, "{theme:?}");
            let (got, want) = role_of(
                &buf,
                theme,
                y + 1,
                "s switches to the selected account",
                'm',
            );
            assert_eq!(got, want, "{theme:?}");
        }
    }

    #[test]
    fn with_several_providers_the_lead_card_names_its_provider() {
        let mut snap = snapshot(three());
        let mut fake = fake_agent(
            vec![fake_account(1, "a1@x.co"), fake_account(2, "a2@x.co")],
            LiveLogin::Managed { position: 2 },
        );
        fake.accounts[1].active = true;
        snap.providers.push(fake);
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snap);
        for theme in TIERS {
            state.theme = theme;
            let (y, lead) = lead_rows(&state, 120, 40);
            assert!(
                lead[0].starts_with(
                    "  ● 2 work  sam@corp.example · Example Corp    in use · Claude Code   "
                ),
                "{lead:?}"
            );
            let buf = draw_buffer(&state, 120, 40, NOW_MS);
            let (got, want) = role_of(&buf, theme, y, "in use · Claude Code", 'm');
            assert_eq!(got, want, "{theme:?}");
        }
        update(&mut state, key(KeyCode::End), NOW_MS);
        let lead = lead_rows(&state, 120, 40).1;
        assert!(
            lead[0].starts_with("  ● 2 a2@x.co    in use · FakeAgent   "),
            "{lead:?}"
        );
        // With one provider, nothing names it.
        let lead = lead_rows(&loaded(three()), 120, 40).1;
        assert!(
            lead[0].starts_with("  ● 2 work  sam@corp.example · Example Corp    in use   "),
            "{lead:?}"
        );
    }

    #[test]
    fn with_no_account_in_use_the_lead_card_carries_the_best_pick() {
        let mut accounts = three();
        accounts[1].active = false;
        for theme in TIERS {
            let mut state = loaded(accounts.clone());
            state.theme = theme;
            for w in [80, 120] {
                let (y, lead) = lead_rows(&state, w, 24);
                let pick = "best now  1 alex@example.com   b switch";
                let gap = " ".repeat(usize::from(w) - width_of(NO_ACCOUNT_IN_USE[0]) - pick.len());
                assert_eq!(
                    lead,
                    [
                        format!("{}{gap}{pick}", NO_ACCOUNT_IN_USE[0]),
                        NO_ACCOUNT_IN_USE[1].to_owned()
                    ],
                    "{theme:?} {w}"
                );
                let buf = draw_buffer(&state, w, 24, NOW_MS);
                let (got, want) = role_of(&buf, theme, y, "best now", 'm');
                assert_eq!(got, want, "{theme:?}");
                let (got, want) = role_of(&buf, theme, y, "1 alex@example.com", 't');
                assert_eq!(got, want, "{theme:?}");
            }
            // The pick is never cut: it goes where it would leave the left part under 16
            // columns.
            let mut state = loaded(accounts.clone());
            state.theme = theme;
            let title_at = |w: u16| -> String {
                super::lead::lead(&state, w, 2, NOW_MS).lines[0]
                    .spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect()
            };
            assert_eq!(
                title_at(61),
                "  No account in use   best now  1 alex@example.com   b switch",
                "{theme:?}"
            );
            assert_eq!(
                title_at(58),
                "  No account in…   best now  1 alex@example.com   b switch",
                "{theme:?}"
            );
            assert_eq!(title_at(57), "  No account in use", "{theme:?}");
            assert_eq!(title_at(40), "  No account in use", "{theme:?}");
        }
    }

    #[test]
    fn with_several_providers_the_lead_card_follows_the_selected_account_s_provider() {
        // Review Focus 2.
        let mut snap = snapshot(three());
        snap.providers.push(fake_agent(
            vec![fake_account(1, "a1@x.co"), fake_account(2, "a2@x.co")],
            LiveLogin::None,
        ));
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snap);
        let lead = lead_rows(&state, 120, 40).1;
        assert!(lead[0].starts_with("  ● 2 work"), "{lead:?}");
        update(&mut state, key(KeyCode::End), NOW_MS);
        assert_eq!(
            state.accounts.selected().unwrap().provider,
            ProviderId::new("fake-agent")
        );
        let lead = lead_rows(&state, 120, 40).1;
        let pick = "best now  1 a1@x.co   b switch";
        let gap = " ".repeat(120 - width_of(NO_ACCOUNT_IN_USE[0]) - pick.len());
        assert_eq!(lead[0], format!("{}{gap}{pick}", NO_ACCOUNT_IN_USE[0]));
        assert_eq!(lead[1], NO_ACCOUNT_IN_USE[1]);
    }

    #[test]
    fn a_long_name_on_the_lead_card_keeps_the_best_pick() {
        // Review Focus 3: the name is cut before the facts, and the pick is never cut.
        let mut accounts = three();
        accounts[1].row.alias = Some("a".repeat(200));
        accounts[1].row.email = Some("例のアカウント@example.com".into());
        let state = loaded(accounts);
        for (w, h) in [(80, 24), (120, 40)] {
            let lead = lead_rows(&state, w, h).1;
            let title = &lead[0];
            assert!(title.starts_with("  ● 2 aaaa"), "{title}");
            assert!(title.contains("…    in use   "), "{title}");
            assert!(
                title.ends_with("best now  1 alex@example.com   b switch"),
                "{title}"
            );
            assert_eq!(width_of(title), usize::from(w), "{title}");
            assert!(
                lead.iter().all(|l| width_of(l) <= usize::from(w)),
                "{lead:?}"
            );
        }
    }

    #[test]
    fn the_body_stacks_lead_table_and_card_from_the_top_and_leaves_spare_rows_at_the_bottom() {
        // VIS-5.
        let state = loaded(three());
        let body = body_of(&state, 80, 24, NOW_MS);
        assert_eq!(body.len(), 21, "{body:?}");
        assert!(body[0].starts_with("  ● 2 work"), "{body:?}");
        assert!(body[1].starts_with("    5h    ▆"), "{body:?}");
        assert!(body[2].starts_with("    7d    ▆"), "{body:?}");
        assert_eq!(body[3], "", "{body:?}");
        assert!(body[4].starts_with("    #   ACCOUNT"), "{body:?}");
        for (row, email) in [
            (5, "alex@example.com"),
            (6, "sam@corp.example"),
            (7, "alex@personal."),
        ] {
            assert!(body[row].contains(email), "{body:?}");
        }
        assert_eq!(body[8], "", "{body:?}");
        assert!(
            body[9].starts_with("    2 work  sam@corp.example"),
            "{body:?}"
        );
        assert!(body[10].starts_with("    5h "), "{body:?}");
        assert!(body[11].starts_with("    7d "), "{body:?}");
        assert!(body[12..].iter().all(String::is_empty), "{body:?}");
    }

    #[test]
    fn with_sixteen_accounts_at_80_by_24_the_table_scrolls_between_the_lead_and_the_card() {
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
        let shown = rows(&state);
        assert!(shown.len() < 16, "{shown:?}");
        assert_eq!(shown[0], "a1@x.co");
        let (lead_y, lead) = lead_rows(&state, 80, 24);
        let (card_y, card) = card_rows(&state, 80, 24);
        assert!(lead[0].starts_with("  ● 1 a1@x.co"), "{lead:?}");
        assert!(card[0].starts_with("    1 a1@x.co"), "{card:?}");
        update(&mut state, key(KeyCode::End), NOW_MS);
        assert_eq!(state.accounts.selected().unwrap().position, 16);
        let shown = rows(&state);
        assert_eq!(
            shown.last().map(String::as_str),
            Some("a16@x.co"),
            "{shown:?}"
        );
        let (y, lead) = lead_rows(&state, 80, 24);
        assert_eq!(y, lead_y);
        assert!(lead[0].starts_with("  ● 1 a1@x.co"), "{lead:?}");
        let (y, card) = card_rows(&state, 80, 24);
        assert_eq!(y, card_y);
        assert!(card[0].starts_with("    16 a16@x.co"), "{card:?}");
        let lines = draw(&state, 80, 24);
        assert!(lines[usize::from(lead_y)].starts_with("  ● 1 a1@x.co"));
        assert!(lines[usize::from(card_y)].starts_with("    16 a16@x.co"));
    }

    /// `three()` with 20 decision-grade windows on row 2, in use: 5h, 7d and `m1` … `m18`.
    fn twenty() -> Vec<AccountView> {
        let mut accounts = three();
        let windows = accounts[1].usage.windows.as_mut().unwrap();
        for i in 1..=18 {
            windows.push((
                window(
                    &format!("scoped:m{i}"),
                    &format!("m{i}"),
                    WindowKind::Scoped,
                    10.0 + i as f64,
                    Some(291_630),
                ),
                Pace::default(),
            ));
        }
        accounts
    }

    /// The first word of each row: a window row's label.
    fn labels(rows: &[String]) -> Vec<&str> {
        rows.iter()
            .map(|l| l.split_whitespace().next().unwrap_or(""))
            .collect()
    }

    fn m(range: std::ops::RangeInclusive<usize>) -> Vec<String> {
        range.map(|i| format!("m{i}")).collect()
    }

    #[test]
    fn twenty_windows_on_the_account_in_use_fold_the_lead_card_and_keep_the_table_and_the_card() {
        // T3-22, T3-32.
        let mut state = loaded(twenty());
        state.size = (120, 24);
        assert_eq!(state.accounts.selected().unwrap().position, 2);
        let body = body_of(&state, 120, 24, NOW_MS);
        assert_eq!(body.len(), 21, "{body:?}");
        assert!(body[0].starts_with("  ● 2 work"), "{body:?}");
        let mut bars = vec!["5h".to_owned(), "7d".to_owned()];
        bars.extend(m(1..=8));
        assert_eq!(labels(&body[1..11]), bars, "{body:?}");
        assert_eq!(
            body[11], "    +10 more windows · in the card below",
            "{body:?}"
        );
        let y = body_y(&state, 120, 24);
        let buf = draw_buffer(&state, 120, 24, NOW_MS);
        let (got, want) = role_of(&buf, state.theme, y + 11, &body[11][4..], 'm');
        assert_eq!(got, want);
        assert_eq!(body[12], "", "{body:?}");
        assert!(body[13].starts_with("    #   ACCOUNT"), "{body:?}");
        assert!(body[14].contains("alex@example.com"), "{body:?}");
        assert!(body[15].contains("sam@corp.example"), "{body:?}");
        assert!(body[16].contains("alex@personal.example"), "{body:?}");
        assert_eq!(body[17], "", "{body:?}");
        assert!(
            body[18].starts_with("    2 work  sam@corp.example"),
            "{body:?}"
        );
        assert!(body[19].starts_with("    5h "), "{body:?}");
        assert!(!body[19].contains("resets in"), "{body:?}");
        assert!(body[20].starts_with("    ⏎ history of work"), "{body:?}");
        let table = body[13..17].to_vec();
        for _ in 0..10 {
            update(&mut state, ch('J'), NOW_MS);
        }
        let body = body_of(&state, 120, 24, NOW_MS);
        assert!(body[19].starts_with("    m9 "), "{body:?}");
        assert!(body[19].ends_with("resets in 3d09h"), "{body:?}");
        assert_eq!(body[13..17], table);
        loop {
            let before = body_of(&state, 120, 24, NOW_MS);
            update(&mut state, ch('J'), NOW_MS);
            let after = body_of(&state, 120, 24, NOW_MS);
            assert_eq!(after[13..17], table);
            if after == before {
                break;
            }
        }
        let body = body_of(&state, 120, 24, NOW_MS);
        assert!(body[19].starts_with("    m18 "), "{body:?}");
        // With row 1 selected, the fold line says where the rest are.
        update(&mut state, key(KeyCode::Home), NOW_MS);
        let body = body_of(&state, 120, 24, NOW_MS);
        assert_eq!(
            body[11], "    +10 more windows · select it for its card",
            "{body:?}"
        );
        // At 80×24: §6.2's short and long windows, no fold; the card has the rest of the body.
        let mut state = loaded(twenty());
        let body = body_of(&state, 80, 24, NOW_MS);
        assert_eq!(body.len(), 21, "{body:?}");
        assert!(body[0].starts_with("  ● 2 work"), "{body:?}");
        assert_eq!(labels(&body[1..3]), ["5h", "7d"], "{body:?}");
        assert_eq!(body[3], "", "{body:?}");
        assert!(!body.iter().any(|l| l.contains("more windows")), "{body:?}");
        assert!(body[4].starts_with("    #   ACCOUNT"), "{body:?}");
        assert!(body[7].contains("alex@personal."), "{body:?}");
        assert_eq!(body[8], "", "{body:?}");
        let card = &body[9..];
        assert!(
            card[0].starts_with("    2 work  sam@corp.example"),
            "{card:?}"
        );
        let mut windows = vec!["5h".to_owned(), "7d".to_owned()];
        windows.extend(m(1..=8));
        assert_eq!(labels(&card[1..11]), windows, "{card:?}");
        assert_eq!(card[11], "    J K: windows 1–10 of 20", "{card:?}");
        assert!(
            !card[1].contains("resets in") && !card[2].contains("resets in"),
            "{card:?}"
        );
        assert!(card[3].ends_with("resets in 3d09h"), "{card:?}");
        let table = body[4..8].to_vec();
        loop {
            let before = body_of(&state, 80, 24, NOW_MS);
            update(&mut state, ch('J'), NOW_MS);
            let after = body_of(&state, 80, 24, NOW_MS);
            assert_eq!(after[4..8], table);
            if after == before {
                break;
            }
        }
        let card = body_of(&state, 80, 24, NOW_MS)[9..].to_vec();
        assert_eq!(labels(&card[10..11]), ["m18"], "{card:?}");
    }

    #[test]
    fn a_folded_lead_card_keeps_its_problem_under_its_bars() {
        // §6.2, T3-32.
        let state = loaded(quarantined_in_use(twenty()));
        let body = body_of(&state, 120, 24, NOW_MS);
        assert_eq!(body.len(), 21, "{body:?}");
        assert!(body[0].starts_with("  ● 2 work"), "{body:?}");
        let mut bars = vec!["5h".to_owned(), "7d".to_owned()];
        bars.extend(m(1..=4));
        assert_eq!(labels(&body[1..7]), bars, "{body:?}");
        assert_eq!(
            body[7], "    +14 more windows · in the card below",
            "{body:?}"
        );
        assert_eq!(body[8..10], NEEDS_LOGIN_ROWS, "{body:?}");
        let y = body_y(&state, 120, 24);
        let buf = draw_buffer(&state, 120, 24, NOW_MS);
        let (got, want) = role_of(&buf, state.theme, y + 8, &NEEDS_LOGIN_ROWS[0][4..], 'c');
        assert_eq!(got, want);
        assert_eq!(body[10], "", "{body:?}");
        assert!(body[11].starts_with("    #   ACCOUNT"), "{body:?}");
        assert!(body[13].contains("sam@corp.example"), "{body:?}");
        assert!(body[13].ends_with("  ✕ new login"), "{body:?}");
        assert_eq!(body[15], "", "{body:?}");
        let card = &body[16..];
        assert!(
            card[0].starts_with("    2 work  sam@corp.example"),
            "{card:?}"
        );
        assert!(card[1].starts_with("    5h "), "{card:?}");
        assert_eq!(card[2..4], NEEDS_LOGIN_ROWS, "{card:?}");
        assert!(card[4].starts_with("    ⏎ history of work"), "{card:?}");
        // Two windows only: the title, both bars, then the problem.
        let lead = lead_rows(&loaded(quarantined_in_use(three())), 120, 40).1;
        assert_eq!(labels(&lead[1..3]), ["5h", "7d"], "{lead:?}");
        assert_eq!(lead[3..], NEEDS_LOGIN_ROWS, "{lead:?}");
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
                let rows = text_of(&buf);
                let check = |y: u16, needle: &str, role: char| {
                    let (got, want) = role_of(&buf, theme, y, needle, role);
                    assert_eq!(got, want, "{theme:?} {w}: {needle:?}");
                };
                // The lead card: `●` and `b` accent, the alias bold, the rest muted, the bars
                // accent on faint.
                let y = p.lead.y;
                check(y, "●", 'a');
                check(y, "sam@corp.example", 'm');
                check(y, "best now", 'm');
                let b = col(&rows[usize::from(y)], "b switch");
                assert_eq!(
                    roles(&buf, theme, y).chars().nth(b),
                    canon(theme, "a").chars().next()
                );
                assert!(
                    buf[(6, y)].modifier.contains(Modifier::BOLD),
                    "{theme:?} {w}"
                );
                let cells = if w >= 120 { 52 } else { 30 };
                let fill = if w >= 120 { 28 } else { 16 };
                let bar: String = roles(&buf, theme, y + 1)
                    .chars()
                    .skip(10)
                    .take(cells)
                    .collect();
                let want = "a".repeat(fill) + &"f".repeat(cells - fill);
                assert_eq!(bar, canon(theme, &want), "{theme:?} {w}");
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
                // The card: the alias bold, the facts muted, the sparkline's tag faint.
                let y = p.card.y;
                for x in 6..10 {
                    assert!(
                        buf[(x, y)].modifier.contains(Modifier::BOLD),
                        "{theme:?} {w} {x}"
                    );
                }
                check(y, "in use", 'm');
                check(y + 1, "24h", 'f');
                let tag = 10 + if w >= 120 { 32 } else { 16 } + 1;
                let letters: String = roles(&buf, theme, y + 2)
                    .chars()
                    .skip(tag)
                    .take(2)
                    .collect();
                assert_eq!(letters, canon(theme, "ff"), "{theme:?} {w}");
            }
        }
    }

    #[test]
    fn a_title_keeps_its_name_readable_before_its_facts() {
        // A name keeps min(its width, 16) columns on both cards' titles; the facts give way
        // from their end first.
        let titles = |email: &str| {
            let mut in_use = account(1, email, read(60, 53.0, 6.0, false));
            in_use.active = true;
            in_use.usage.next_poll_at = Some(NOW_S + 120);
            in_use.row.login_expires_at = Some((NOW_S + 6 * 86_400) * 1000);
            let other = account(2, "sam@corp.example", read(60, 0.0, 10.0, false));
            let state = loaded(vec![in_use, other]);
            assert_eq!(state.accounts.selected().unwrap().position, 1);
            let lead = lead_rows(&state, 80, 24).1[0].clone();
            let card = card_rows(&state, 80, 24).1[0].clone();
            for title in [&lead, &card] {
                assert_eq!(width_of(title), 80, "{title}");
                assert!(title.contains("    in use · "), "{title}");
            }
            assert!(lead.ends_with("   b switch"), "{lead}");
            (lead, card)
        };
        let (lead, card) = titles("alex@example.com");
        assert!(
            lead.starts_with("  ● 1 alex@example.com    in use"),
            "{lead}"
        );
        assert!(
            card.starts_with("    1 alex@example.com    in use"),
            "{card}"
        );
        assert!(card.ends_with('…'), "the facts give way: {card}");
        let (lead, card) = titles("alex@personal.example");
        assert!(
            lead.starts_with("  ● 1 alex@personal.e…    in use"),
            "{lead}"
        );
        assert!(
            card.starts_with("    1 alex@personal.e…    in use"),
            "{card}"
        );
        assert_eq!(width_of("alex@personal.e…"), 16);
    }

    #[test]
    fn a_lead_bar_s_percentage_is_bold_from_70_percent() {
        // T3-18: the lead card's percentage is bold from 70 %, in its severity.
        let mut accounts = three();
        let windows = accounts[1].usage.windows.as_mut().unwrap();
        windows[0].0.pct = 75.0;
        windows[1].0.pct = 69.0;
        for theme in TIERS {
            let mut state = loaded(accounts.clone());
            state.theme = theme;
            let (y, lead) = lead_rows(&state, 120, 40);
            let buf = draw_buffer(&state, 120, 40, NOW_MS);
            let bold = |row: u16, needle: &str| {
                let at = col(&lead[usize::from(row)], needle) as u16;
                (at..at + needle.len() as u16)
                    .all(|x| buf[(x, y + row)].modifier.contains(Modifier::BOLD))
            };
            assert!(bold(1, "75%"), "{theme:?}");
            let (got, want) = role_of(&buf, theme, y + 1, "75%", 'w');
            assert_eq!(got, want, "{theme:?}");
            assert!(!bold(2, "69%"), "{theme:?}");
            let (got, want) = role_of(&buf, theme, y + 2, "69%", 't');
            assert_eq!(got, want, "{theme:?}");
        }
    }

    #[test]
    fn only_long_and_scoped_lead_bars_have_a_pace_mark() {
        // C-8: a short window and a spend have no pace mark, whatever their expected share.
        let mut accounts = three();
        let windows = accounts[1].usage.windows.as_mut().unwrap();
        for (w, p) in windows.iter_mut() {
            w.pct = 40.0;
            p.expected_pct = Some(50.0);
        }
        let pace = Pace {
            expected_pct: Some(50.0),
            ..Pace::default()
        };
        windows.push((
            window(
                "scoped:fable",
                "Fable",
                WindowKind::Scoped,
                40.0,
                Some(291_630),
            ),
            pace,
        ));
        let mut spend = window("spend", "spend", WindowKind::Spend, 40.0, Some(1_209_630));
        spend.detail = Some(r#"{"used": 20, "limit": 50, "currency": "EUR"}"#.parse().unwrap());
        windows.push((spend, pace));
        let lead = lead_rows(&loaded(accounts), 120, 40).1;
        // The label column is 5 wide: the bars are columns 11 to 63; the mark is cell 26.
        let bar = |row: &str| columns(row, 11, 63);
        let marked = format!("{}│{}", "▆".repeat(21) + &"▁".repeat(5), "▁".repeat(25));
        assert_eq!(
            bar(&lead[1]),
            "▆".repeat(21) + &"▁".repeat(31),
            "5h: {lead:?}"
        );
        assert_eq!(bar(&lead[2]), marked, "7d: {lead:?}");
        assert_eq!(bar(&lead[3]), marked, "Fable: {lead:?}");
        assert_eq!(
            bar(&lead[4]),
            "▆".repeat(21) + &"▁".repeat(31),
            "spend: {lead:?}"
        );
    }

    #[test]
    fn ahead_of_pace_a_lead_bar_shows_its_mark_in_warn() {
        let mut accounts = three();
        accounts[1].usage.windows.as_mut().unwrap()[1].1.ahead = Some(true);
        for theme in TIERS {
            let mut state = loaded(accounts.clone());
            state.theme = theme;
            let (y, lead) = lead_rows(&state, 120, 40);
            assert!(lead[1].ends_with("   53%  resets in 2h40m"), "{lead:?}");
            assert!(lead[2].ends_with("    6% ▲  resets in 3d09h"), "{lead:?}");
            let buf = draw_buffer(&state, 120, 40, NOW_MS);
            let (got, want) = role_of(&buf, theme, y + 2, "▲", 'w');
            assert_eq!(got, want, "{theme:?}");
        }
    }

    #[test]
    fn a_lead_bar_s_projection_is_its_text_from_120_columns_and_its_clause_at_80() {
        let mut accounts = three();
        let windows = accounts[1].usage.windows.as_mut().unwrap();
        windows[0].1 = Pace {
            rate_per_hour: Some(15.7),
            will_last_to_reset: Some(true),
            ..Pace::default()
        };
        windows[1].1 = Pace {
            rate_per_hour: Some(1.2),
            method: Some(tagteam_core::ProjectionMethod::Regression),
            exhaustion_at: Some(NOW_S + 187_200),
            will_last_to_reset: Some(false),
            ..Pace::default()
        };
        for theme in TIERS {
            let mut state = loaded(accounts.clone());
            state.theme = theme;
            for (w, h, lasts, fills) in [
                (
                    120,
                    40,
                    "resets in 2h40m · +15.7%/h · lasts until it resets",
                    "resets in 3d09h · +1.2%/h · full in 2d04h",
                ),
                (
                    80,
                    24,
                    "resets in 2h40m · lasts",
                    "resets in 3d09h · full in 2d04h",
                ),
            ] {
                let (y, lead) = lead_rows(&state, w, h);
                assert!(lead[1].ends_with(lasts), "{w}: {lead:?}");
                assert!(lead[2].ends_with(fills), "{w}: {lead:?}");
                let buf = draw_buffer(&state, w, h, NOW_MS);
                let lasting = lasts.split(" · ").skip(1).collect::<Vec<_>>().join(" · ");
                let (got, want) = role_of(&buf, theme, y + 1, &lasting, 't');
                assert_eq!(got, want, "{theme:?} {w}");
                let filling = fills.split(" · ").skip(1).collect::<Vec<_>>().join(" · ");
                let (got, want) = role_of(&buf, theme, y + 2, &filling, 'w');
                assert_eq!(got, want, "{theme:?} {w}");
            }
        }
    }

    #[test]
    fn the_best_pick_is_dropped_when_it_would_leave_the_title_under_16_columns() {
        // T3-19: the pick is never cut. Its 83 columns and a 3-column gap leave the title's left
        // part 16 columns at 102, and 15 at 101.
        let mut accounts = three();
        accounts[0].row.alias = Some("p".repeat(60));
        let state = loaded(accounts);
        let pick = format!("best now  1 {}   b switch", "p".repeat(60));
        assert_eq!(width_of(&pick), 83);
        for w in [102u16, 160] {
            let title = lead_rows(&state, w, 40).1[0].clone();
            assert!(title.ends_with(&pick), "{w}: {title}");
            assert_eq!(width_of(&title), usize::from(w), "{title}");
        }
        for w in [80u16, 101] {
            let title = lead_rows(&state, w, 40).1[0].clone();
            assert!(!title.contains("best now"), "{w}: {title}");
            assert!(
                title.starts_with("  ● 2 work  sam@corp.example"),
                "{w}: {title}"
            );
            assert!(title.ends_with("    in use"), "{w}: {title}");
        }
    }

    #[test]
    fn the_lead_card_s_label_column_is_a_quarter_of_its_width_at_most() {
        let mut accounts = three();
        let long = "x".repeat(100);
        accounts[1].usage.windows.as_mut().unwrap().push((
            window(
                "scoped:long",
                &long,
                WindowKind::Scoped,
                40.0,
                Some(291_630),
            ),
            Pace::default(),
        ));
        let lead = lead_rows(&loaded(accounts), 120, 40).1;
        assert_eq!(lead.len(), 4, "{lead:?}");
        assert!(
            lead[1].starts_with(&format!("    {:<30}  ▆", "5h")),
            "{lead:?}"
        );
        assert!(
            lead[3].starts_with(&format!("    {}  ▆", fit(&long, 30))),
            "{lead:?}"
        );
        assert!(lead.iter().all(|l| width_of(l) <= 120), "{lead:?}");
    }

    #[test]
    fn a_second_provider_s_lead_card_shows_its_own_account_in_use_and_pick() {
        use tagteam_engine::switch::BestForecast;
        let mut snap = snapshot(three());
        let mut fake = fake_agent(
            vec![fake_account(1, "a1@x.co"), fake_account(2, "a2@x.co")],
            LiveLogin::Managed { position: 2 },
        );
        fake.accounts[1].active = true;
        fake.forecast = BestForecast::Pick { order: vec![1] };
        snap.providers.push(fake);
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snap);
        let title = lead_rows(&state, 120, 40).1[0].clone();
        assert!(title.starts_with("  ● 2 work"), "{title}");
        assert!(
            title.ends_with("best now  1 alex@example.com   b switch"),
            "{title}"
        );
        update(&mut state, key(KeyCode::End), NOW_MS);
        let lead = lead_rows(&state, 120, 40).1;
        assert!(lead[0].starts_with("  ● 2 a2@x.co    in use"), "{lead:?}");
        assert!(
            lead[0].ends_with("best now  1 a1@x.co   b switch"),
            "{lead:?}"
        );
        assert!(lead[1].starts_with("    5h    ▆"), "{lead:?}");
    }
}
