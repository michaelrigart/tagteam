//! The History screen (redesign §7): focus's provider's timeline, of every account or one
//! (§7.2), over 1, 7, 14 or 30 days, read on the views lane (§7.7). Its scope row, then
//! `loading…` before a view or the empty state when nothing is in range (§5.7). A time cursor
//! moves by column (§7.5), and the scope stays in step with the Accounts selection (T4-17). It
//! never fetches (core §13.4).

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::timeline::{AccountRef, Scope, TimelineView};
use tagteam_engine::words;

use crate::autoline;
use crate::chrome::{WIDE, fit_spans};
use crate::daemon;
use crate::keys::RIGHT_INSET;
use crate::msg::Cmd;
use crate::overlay::{ChooseThen, Overlay};
use crate::source::SourceError;
use crate::state::{BannerKind, Screen, State};
use crate::theme::Role;
use crate::update;
use crate::view::{body_at, centered, centred_block, fit, width_of};

mod chart;
mod lanes;
mod layout;

/// T4-21: drawn under the scope row until the first view of a scope and range arrives.
const LOADING: &str = "loading…";
/// The empty state's sentence (§5.7), left out at 30 days.
const LONGER: &str = "[ ] picks a longer range.";
/// The scope row's name for every account, and the scope dialog's first option (§7.2).
const ALL_ACCOUNTS: &str = "all accounts";
/// `f`'s question (T4-17).
const SCOPE_QUESTION: &str = "Show History for";
/// The ranges, muted beside the current one from `WIDE` columns (§7.2).
const CHOICES: &str = "1 · 7 · 14 · 30";
/// Between the scope row's groups (§4.3's group gap).
const GAP: &str = "   ";
/// T4-19: a resize re-reads the timeline this long after the last one.
pub(crate) const RESIZE_DEBOUNCE_MS: i64 = 250;
/// T4-19: a paced re-read comes at most this often.
pub(crate) const PACED_MS: i64 = 30_000;
/// T4-10: the marker column, the 8-column gutter, and the column kept at the right.
const CHART_MARGIN: u16 = layout::CHART_X + 1;

/// The ranges `[` and `]` cycle through (§7.2); 7 days is `history`'s default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Range {
    Day,
    #[default]
    Week,
    Fortnight,
    Month,
}

impl Range {
    const ALL: [Range; 4] = [Range::Day, Range::Week, Range::Fortnight, Range::Month];

    fn days(self) -> i64 {
        match self {
            Range::Day => 1,
            Range::Week => 7,
            Range::Fortnight => 14,
            Range::Month => 30,
        }
    }

    fn label(self) -> String {
        match self.days() {
            1 => "1 day".to_owned(),
            n => format!("{n} days"),
        }
    }

    /// The empty state's title (§5.7, T4-21).
    fn nothing(self) -> String {
        match self.days() {
            1 => "Nothing in the last day".to_owned(),
            n => format!("Nothing in the last {n} days"),
        }
    }

    fn step(self, by: isize) -> Range {
        let at = Self::ALL.iter().position(|r| *r == self).unwrap_or(1) as isize;
        Self::ALL[(at + by).rem_euclid(Self::ALL.len() as isize) as usize]
    }
}

/// T4-14: where the time cursor is: now, in the last column, or at an instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Cursor {
    #[default]
    Now,
    At(i64),
}

pub struct HistoryState {
    /// Whose timeline (§7.2): every account's until `⏎` on Accounts or `f` scopes it.
    pub(crate) scope: Scope,
    range: Range,
    /// Focus's provider when the timeline was last asked for (T4-18); `None` before then.
    provider: Option<ProviderId>,
    /// The last timeline read; `None` from an open, a scope or a range change until the read.
    view: Option<TimelineView>,
    /// When the terminal was last resized on History, until the re-read it is due (T4-19).
    resized_ms: Option<i64>,
    pub(crate) cursor: Cursor,
    /// The first of one account's charts shown (`J`/`K`), when they do not all fit (T4-8).
    chart_from: usize,
    /// History was opened by `⏎` on Accounts: `Esc` goes back there (T4-17).
    from_accounts: bool,
    /// When the timeline was last asked for (T4-19).
    read_ms: Option<i64>,
    /// When the paced re-read that came inside `PACED_MS` of the last read is due (T4-19).
    paced_due_ms: Option<i64>,
}

impl Default for HistoryState {
    fn default() -> Self {
        HistoryState {
            scope: Scope::All,
            range: Range::default(),
            provider: None,
            view: None,
            resized_ms: None,
            cursor: Cursor::Now,
            chart_from: 0,
            from_accounts: false,
            read_ms: None,
            paced_due_ms: None,
        }
    }
}

impl HistoryState {
    /// Now less the range: 1, 7, 14 or 30 days, 7 by default.
    fn since_s(&self, now_s: i64) -> i64 {
        now_s - self.range.days() * 86_400
    }

    /// A new scope: the view goes until its read (T4-19), and one account's charts start from
    /// the first. False when it is the scope already.
    fn set_scope(&mut self, scope: Scope) -> bool {
        if self.scope == scope {
            return false;
        }
        self.scope = scope;
        self.view = None;
        self.chart_from = 0;
        true
    }
}

/// The cursor's column in `view` (T4-14): the last for now, column 0 for a time before the
/// range.
fn cursor_column(cursor: Cursor, view: &TimelineView) -> usize {
    let last = view.columns.saturating_sub(1);
    match cursor {
        Cursor::Now => last,
        Cursor::At(ms) => {
            let grid = view.grid();
            if ms < grid.since_ms {
                0
            } else {
                grid.column_of(ms).unwrap_or(last)
            }
        }
    }
}

/// The cursor on `column` (T4-14): 1 ms before the column ends, or now on the last.
fn at_column(view: &TimelineView, column: usize) -> Cursor {
    if column + 1 >= view.columns {
        Cursor::Now
    } else {
        Cursor::At(view.grid().end_ms(column) - 1)
    }
}

/// T4-10: the chart's columns at `width`.
pub(crate) fn columns(width: u16) -> usize {
    usize::from(width.saturating_sub(CHART_MARGIN))
}

/// A read for focus's provider, the scope, the range and the width, under a new generation;
/// `None` before the first accounts read (T4-19). A resize waiting for its re-read needs none
/// after it.
pub(crate) fn reload(state: &mut State, now_ms: i64) -> Option<Cmd> {
    state.accounts.snapshot()?;
    let provider = daemon::focus(state);
    let h = &mut state.history;
    h.provider = Some(provider.clone());
    h.resized_ms = None;
    h.read_ms = Some(now_ms);
    h.paced_due_ms = None;
    let since = h.since_s(now_ms.div_euclid(1000));
    let scope = h.scope.clone();
    let columns = columns(state.size.0);
    Some(state.load_timeline(provider, scope, since, columns))
}

/// T4-19: `reload`, unless the last read was asked for under `PACED_MS` ago, in which case
/// `on_tick` asks for it `PACED_MS` after that one.
pub(crate) fn reload_paced(state: &mut State, now_ms: i64) -> Option<Cmd> {
    let h = &mut state.history;
    match h.read_ms {
        Some(at) if now_ms < at + PACED_MS => {
            h.paced_due_ms = Some(at + PACED_MS);
            None
        }
        _ => reload(state, now_ms),
    }
}

/// History opens on `loading…` and reads its timeline (T4-19). Only `⏎` on Accounts opens it
/// from there (`scope_to`).
pub(crate) fn on_enter(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    state.history.view = None;
    state.history.from_accounts = false;
    reload(state, now_ms).into_iter().collect()
}

/// A new accounts read. A scoped account it no longer lists returns the scope to all accounts
/// (T4-17). While History shows, the first read reads the timeline a TUI started on History
/// waited for, and one that moves focus to another provider, or ends the scope, reads again
/// (T4-18, T4-19). Any other leaves the timeline to its own re-reads.
pub(crate) fn on_accounts(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    let gone = match &state.history.scope {
        Scope::Account(id) => !state.accounts.shown_ids().contains(id),
        Scope::All => false,
    };
    if gone {
        state.history.set_scope(Scope::All);
    }
    if state.screen != Screen::History {
        return vec![];
    }
    let focus = daemon::focus(state);
    match &state.history.provider {
        Some(p) if *p == focus && !gone => return vec![],
        Some(_) => state.history.view = None,
        None => {}
    }
    reload(state, now_ms).into_iter().collect()
}

/// The timeline read, under History's latest generation; any other is dropped. A failed one
/// raises the History banner and keeps the last view (T4-21).
pub(crate) fn on_timeline(
    state: &mut State,
    r#gen: u64,
    result: Result<TimelineView, SourceError>,
) {
    if r#gen != state.gens.history {
        return;
    }
    state.dirty = true;
    match result {
        Ok(view) => {
            state.history.view = Some(view);
            state.clear_banner(BannerKind::History);
        }
        Err(e) => state.raise_banner(BannerKind::History, e.message),
    }
}

/// A resize on History: its timeline is re-read `RESIZE_DEBOUNCE_MS` after the last one, the
/// old view drawn meanwhile (T4-19).
pub(crate) fn on_resize(state: &mut State, now_ms: i64) {
    if state.screen == Screen::History {
        state.history.resized_ms = Some(now_ms);
    }
}

/// T4-19: the re-read a resize is due once it has settled, or a paced one once it is due,
/// while History shows.
pub(crate) fn on_tick(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    let h = &state.history;
    let resized = h
        .resized_ms
        .is_some_and(|at| now_ms >= at + RESIZE_DEBOUNCE_MS);
    let paced = h.paced_due_ms.is_some_and(|at| now_ms >= at);
    if !(resized || paced) || state.screen != Screen::History || state.quitting.is_some() {
        return vec![];
    }
    state.history.resized_ms = None;
    state.history.paced_due_ms = None;
    reload(state, now_ms).into_iter().collect()
}

/// `⏎` on Accounts: History, scoped to `account` (§7.2), opened from Accounts (T4-17).
pub(crate) fn scope_to(state: &mut State, account: AccountId, now_ms: i64) -> Vec<Cmd> {
    state.history.set_scope(Scope::Account(account));
    let cmds = update::show(state, Screen::History, now_ms);
    state.history.from_accounts = true;
    cmds
}

/// T4-17: the scope follows the Accounts selection while it names an account. Only a move on
/// Accounts changes the selection, so History reads the new scope as it opens.
pub(crate) fn follow(state: &mut State) {
    let Scope::Account(scoped) = &state.history.scope else {
        return;
    };
    let Some(selected) = state.accounts.selected() else {
        return;
    };
    if selected.id != *scoped {
        state.history.set_scope(Scope::Account(selected.id));
    }
}

/// The option a digit picks in `f`'s dialog (T4-17): `0` all accounts, else the account of
/// focus's provider at that position.
pub(crate) fn scope_of_digit(state: &State, scopes: &[Scope], digit: u32) -> Option<usize> {
    let want = match digit {
        0 => Scope::All,
        _ => {
            let p = state.view_of(&daemon::focus(state))?;
            let a = p.accounts.iter().find(|a| a.row.position == digit)?;
            Scope::Account(a.row.id.clone())
        }
    };
    scopes.iter().position(|s| *s == want)
}

/// `f`'s choice (T4-17): the scope, and an account chosen selected on Accounts too.
pub(crate) fn choose(state: &mut State, scope: Scope, now_ms: i64) -> Vec<Cmd> {
    if let Scope::Account(id) = &scope {
        state.accounts.select_id(id);
    }
    if !state.history.set_scope(scope) {
        return vec![];
    }
    reload(state, now_ms).into_iter().collect()
}

pub fn handle_key(state: &mut State, key: KeyEvent, now_ms: i64) -> Vec<Cmd> {
    match key.code {
        KeyCode::Char('[') => step_range(state, -1, now_ms),
        KeyCode::Char(']') => step_range(state, 1, now_ms),
        KeyCode::Left => move_cursor(state, -1),
        KeyCode::Right => move_cursor(state, 1),
        KeyCode::Home => {
            // T4-14: the oldest event, or column 0 with none.
            if let Some(view) = &state.history.view {
                state.history.cursor = at_column(view, 0);
            }
            vec![]
        }
        KeyCode::End => {
            state.history.cursor = Cursor::Now;
            vec![]
        }
        KeyCode::Char('f') => {
            open_scopes(state);
            vec![]
        }
        KeyCode::Char('J') => scroll_charts(state, 1),
        KeyCode::Char('K') => scroll_charts(state, -1),
        KeyCode::Esc => escape(state, now_ms),
        _ => vec![],
    }
}

fn step_range(state: &mut State, by: isize, now_ms: i64) -> Vec<Cmd> {
    let h = &mut state.history;
    h.range = h.range.step(by);
    h.view = None;
    reload(state, now_ms).into_iter().collect()
}

/// `←`/`→`: the cursor one column, stopping at either end (T4-14).
fn move_cursor(state: &mut State, by: isize) -> Vec<Cmd> {
    let h = &mut state.history;
    if let Some(view) = &h.view {
        let last = view.columns.saturating_sub(1) as isize;
        let at = cursor_column(h.cursor, view) as isize;
        h.cursor = at_column(view, (at + by).clamp(0, last) as usize);
    }
    vec![]
}

/// `f`: a choose dialog of all accounts and focus's provider's accounts (T4-17, T4-18), the
/// scope preselected.
fn open_scopes(state: &mut State) {
    let mut options = vec![ALL_ACCOUNTS.to_owned()];
    let mut scopes = vec![Scope::All];
    if let Some(p) = state.view_of(&daemon::focus(state)) {
        for a in &p.accounts {
            options.push(format!("{} {}", a.row.position, words::name(&a.row)));
            scopes.push(Scope::Account(a.row.id.clone()));
        }
    }
    let selected = scopes
        .iter()
        .position(|s| *s == state.history.scope)
        .unwrap_or(0);
    state.overlay = Some(Overlay::Choose {
        question: SCOPE_QUESTION.to_owned(),
        options,
        selected,
        then: ChooseThen::Scope(scopes),
    });
}

/// `J`/`K`: one account's charts by one, stopping once the last one shows, or at the first
/// (T4-8).
fn scroll_charts(state: &mut State, by: isize) -> Vec<Cmd> {
    let body = body_at(state);
    let h = &mut state.history;
    let (Scope::Account(_), Some(view)) = (&h.scope, &h.view) else {
        return vec![];
    };
    // From what shows: a resize or a new read may have left the stored scroll past the end.
    let furthest = layout::layout(view, &h.scope, body, state.size.1, usize::MAX)
        .charts
        .first()
        .map_or(0, |(i, _)| *i);
    let from = h.chart_from.min(furthest) as isize;
    h.chart_from = (from + by).clamp(0, furthest as isize) as usize;
    vec![]
}

/// `Esc` (§10, T4-17): History opened from Accounts goes back there, the scope back to all
/// accounts; a scoped History returns to all accounts.
fn escape(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    if std::mem::take(&mut state.history.from_accounts) {
        state.history.set_scope(Scope::All);
        return update::show(state, Screen::Accounts, now_ms);
    }
    if !state.history.set_scope(Scope::All) {
        return vec![];
    }
    reload(state, now_ms).into_iter().collect()
}

/// The scope row, then `loading…` before a view, the empty state when nothing is in range
/// (T4-21), or the charts, the lanes and the axis in their rows (T4-8).
pub fn render(state: &State, frame: &mut Frame, area: Rect, now_ms: i64) {
    if area.is_empty() {
        return;
    }
    let scope_row = Rect::new(area.x, area.y, area.width, 1);
    let line = scope_line(state, area.width, now_ms);
    frame.render_widget(Paragraph::new(line), scope_row);
    let under = Rect::new(area.x, area.y + 1, area.width, area.height - 1);
    let h = &state.history;
    match &h.view {
        None => {
            let line = Line::from(Span::styled(LOADING, state.theme.role(Role::Muted)));
            frame.render_widget(
                Paragraph::new(line.centered()),
                centered(under, under.width, 1),
            );
        }
        Some(view) if nothing_in_range(view) => {
            let sentence = (h.range != Range::Month).then_some(LONGER);
            centred_block(frame, under, &h.range.nothing(), sentence, state.theme);
        }
        Some(view) => draw_view(state, frame, view, area),
    }
}

/// `view`'s charts under their titles, its lanes, the more row and the axis (T4-8 to T4-12).
/// A view read at another width is drawn from the left and clipped until its re-read (T4-19).
fn draw_view(state: &State, frame: &mut Frame, view: &TimelineView, body: Rect) {
    let theme = state.theme;
    let h = &state.history;
    let all = h.scope == Scope::All;
    let at = layout::layout(view, &h.scope, body, frame.area().height, h.chart_from);
    let line_pct = if all {
        view.threshold
    } else {
        chart::ONE_ACCOUNT_LINE
    };
    let cursor = cursor_column(h.cursor, view);
    for (i, rect) in &at.charts {
        let series = &view.charts[*i];
        let mut lines = vec![chart::title_row(state, series, all, body.width)];
        let rows = rect.height.saturating_sub(1);
        lines.extend(chart::chart_rows(
            &series.peaks,
            rows,
            line_pct,
            cursor,
            theme,
        ));
        frame.render_widget(Paragraph::new(lines), *rect);
    }
    for (i, rect) in &at.lanes {
        let lane = &view.lanes[*i];
        let line = if all {
            lanes::lane_row(lane, cursor, theme)
        } else {
            lanes::in_use_row(lane, cursor, theme)
        };
        frame.render_widget(Paragraph::new(line), *rect);
    }
    if let Some((rect, n)) = at.more {
        frame.render_widget(Paragraph::new(lanes::more_row(n, theme)), rect);
    }
    let axis = lanes::axis_row(view.grid(), h.range.days(), state.tokens.tz(), theme);
    frame.render_widget(Paragraph::new(axis), at.axis);
}

/// No lanes and no chart value (T4-21).
fn nothing_in_range(view: &TimelineView) -> bool {
    view.lanes.is_empty()
        && view
            .charts
            .iter()
            .all(|s| s.peaks.iter().all(Option::is_none))
}

/// ` f {scope}   [ ] {range}`, the choices beside it from `WIDE` columns, and `cursor now` or
/// `cursor {when}` on the right, one column in (§7.1, §7.2, T4-14): the keys accent, the words
/// muted, the time bold. The scope's name alone is cut to what the rest leaves, a group gap
/// before `cursor` kept, so the range and the right part always keep their place.
fn scope_line(state: &State, width: u16, now_ms: i64) -> Line<'static> {
    let theme = state.theme;
    let (accent, muted) = (theme.role(Role::Accent), theme.role(Role::Muted));
    let mut range = vec![
        Span::raw(GAP),
        Span::styled("[ ]", accent),
        Span::raw(" "),
        Span::styled(state.history.range.label(), muted),
    ];
    if width >= WIDE {
        range.push(Span::raw(GAP));
        range.push(Span::styled(CHOICES, muted));
    }
    let when = match state.history.cursor {
        Cursor::Now => "now".to_owned(),
        Cursor::At(ms) => autoline::when(ms, now_ms, state.tokens.tz()),
    };
    let right = [
        Span::styled("cursor", muted),
        Span::raw(" "),
        Span::styled(when, theme.strong()),
    ];
    let used = |spans: &[Span]| spans.iter().map(|s| width_of(&s.content)).sum::<usize>();
    let head = [Span::raw(" "), Span::styled("f", accent), Span::raw(" ")];
    let fixed = used(&head) + used(&range) + width_of(GAP) + used(&right) + RIGHT_INSET;
    let name = fit(&scope_name(state), usize::from(width).saturating_sub(fixed));
    let mut left: Vec<Span<'static>> = head.into();
    left.push(Span::styled(name, muted));
    left.extend(range);
    let gap = usize::from(width).saturating_sub(used(&left) + used(&right) + RIGHT_INSET);
    left.push(Span::raw(" ".repeat(gap)));
    left.extend(right);
    Line::from(fit_spans(left, usize::from(width)))
}

/// `all accounts`, or the scoped account named as the timeline names it (§7.2, T4-28): by
/// its short name while focus's accounts list it.
fn scope_name(state: &State) -> String {
    let Scope::Account(id) = &state.history.scope else {
        return ALL_ACCOUNTS.to_owned();
    };
    let view = state.view_of(&daemon::focus(state));
    let row = view
        .and_then(|v| v.accounts.iter().find(|a| &a.row.id == id))
        .map(|a| &a.row);
    words::account_name(&AccountRef {
        id: id.clone(),
        position: row.map(|r| r.position),
        alias: row.and_then(|r| r.alias.clone()),
        email: row.map(words::email),
        removed: row.is_none(),
    })
}

/// `lines` drawn by a `Paragraph` on a buffer `width` wide and as tall as they are.
#[cfg(test)]
fn buffer_of(lines: Vec<Line<'static>>, width: u16) -> ratatui::buffer::Buffer {
    use ratatui::widgets::Widget;
    let area = Rect::new(0, 0, width, u16::try_from(lines.len()).unwrap_or(u16::MAX));
    let mut buf = ratatui::buffer::Buffer::empty(area);
    Paragraph::new(lines).render(area, &mut buf);
    buf
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyCode;
    use tagteam_engine::timeline::{IN_USE, Series};

    use super::*;
    use crate::msg::Msg;
    use crate::overlay::Overlay;
    use crate::testutil::*;
    use crate::theme::Theme;
    use crate::update::update;
    use crate::view::width_of;

    const WEEK: i64 = 7 * 86_400;

    fn accounts_msg(r#gen: u64, n: u32) -> Msg {
        Msg::Accounts {
            r#gen,
            result: Ok(snapshot(many(n))),
        }
    }

    /// History at 80×24, started there, once the first accounts read of `n` accounts arrived:
    /// and the reads that read asked for.
    fn opened(n: u32) -> (State, Vec<Cmd>) {
        let mut state = fresh(Screen::History);
        state.start();
        let cmds = update(&mut state, accounts_msg(1, n), NOW_MS);
        (state, cmds)
    }

    /// The timeline read of Claude Code's `scope` since `since_s` over `columns`, under `gen`.
    fn read_of(r#gen: u64, scope: Scope, since_s: i64, columns: usize) -> Cmd {
        Cmd::LoadTimeline {
            r#gen,
            provider: cc(),
            scope,
            since_s,
            columns,
        }
    }

    /// `view` as the answer to the latest timeline read.
    fn arrived(state: &mut State, view: TimelineView) {
        let r#gen = state.gens.history;
        update(
            state,
            Msg::Timeline {
                r#gen,
                result: Ok(view),
            },
            NOW_MS,
        );
    }

    /// Nothing in range: an all-accounts chart with no value, and no lanes.
    fn nothing(columns: usize) -> TimelineView {
        let chart = Series {
            label: IN_USE.into(),
            peaks: vec![None; columns],
        };
        timeline_view(columns, vec![chart], vec![])
    }

    /// `text` centred across 80 columns, as `centred_block` draws it and `text_of` reads it.
    fn centred(text: &str) -> String {
        format!("{}{text}", " ".repeat((80 - width_of(text)) / 2))
    }

    #[test]
    fn history_reads_the_timeline_for_focus_s_provider_scope_range_and_width() {
        let (mut state, cmds) = opened(3);
        assert_eq!(cmds, [read_of(1, Scope::All, NOW_S - 604_800, 70)]);
        assert_eq!(
            update(&mut state, ch(']'), NOW_MS),
            [read_of(2, Scope::All, NOW_S - 14 * 86_400, 70)]
        );
        let view = timeline_view(
            70,
            vec![],
            vec![lane_of(1, vec![Some(5.0); 70], vec![true; 70])],
        );
        let older = Msg::Timeline {
            r#gen: 1,
            result: Ok(view.clone()),
        };
        assert!(update(&mut state, older, NOW_MS).is_empty());
        assert_eq!(state.history.view, None, "the older read changes nothing");
        arrived(&mut state, view.clone());
        assert_eq!(state.history.view.as_ref(), Some(&view));
        let failed = Msg::Timeline {
            r#gen: 2,
            result: Err(SourceError {
                kind: "store".into(),
                message: "the store is locked".into(),
            }),
        };
        update(&mut state, failed, NOW_MS);
        assert_eq!(state.banner_text(), Some("the store is locked"));
        assert_eq!(
            state.history.view.as_ref(),
            Some(&view),
            "the last view stays"
        );
        arrived(&mut state, view);
        assert_eq!(state.banner_text(), None, "the next good read clears it");
    }

    #[test]
    fn a_tui_started_on_history_reads_the_timeline_once_the_accounts_arrive() {
        let mut state = fresh(Screen::History);
        assert_eq!(state.start(), [Cmd::LoadAccounts { r#gen: 1 }]);
        assert_eq!(
            update(&mut state, accounts_msg(1, 2), NOW_MS),
            [read_of(1, Scope::All, NOW_S - WEEK, 70)]
        );
        let Cmd::LoadAccounts { r#gen } = state.load_accounts() else {
            unreachable!()
        };
        assert!(
            update(&mut state, accounts_msg(r#gen, 2), NOW_MS).is_empty(),
            "a later accounts read asks for no timeline of its own"
        );
    }

    #[test]
    fn a_resize_re_reads_the_timeline_250_ms_after_it_settles() {
        let (mut state, _) = opened(3);
        arrived(&mut state, nothing(70));
        let resize = |width, height| Msg::Resize { width, height };
        assert!(update(&mut state, resize(120, 40), NOW_MS).is_empty());
        assert!(update(&mut state, Msg::Tick, NOW_MS + 249).is_empty());
        assert_eq!(
            update(&mut state, Msg::Tick, NOW_MS + RESIZE_DEBOUNCE_MS),
            [read_of(2, Scope::All, NOW_S - WEEK, 110)]
        );
        assert!(
            update(&mut state, Msg::Tick, NOW_MS + 500).is_empty(),
            "read once"
        );
        // A second resize 100 ms after the first restarts the wait.
        let at = NOW_MS + 1_000;
        update(&mut state, resize(100, 30), at);
        update(&mut state, resize(90, 30), at + 100);
        assert!(update(&mut state, Msg::Tick, at + 250).is_empty());
        assert!(update(&mut state, Msg::Tick, at + 349).is_empty());
        assert_eq!(
            update(&mut state, Msg::Tick, at + 350),
            [read_of(3, Scope::All, NOW_S + 1 - WEEK, 80)]
        );
        // On Accounts a resize reads nothing.
        update(&mut state, ch('1'), at + 400);
        update(&mut state, resize(120, 40), at + 500);
        for t in [at + 750, at + 1_000] {
            let cmds = update(&mut state, Msg::Tick, t);
            assert!(
                !cmds.iter().any(|c| matches!(c, Cmd::LoadTimeline { .. })),
                "{cmds:?}"
            );
        }
    }

    #[test]
    fn the_scope_row_names_the_scope_the_range_and_the_cursor() {
        let (mut state, _) = opened(3);
        let at80 = " f all accounts   [ ] 7 days";
        let row = body_of(&state, 80, 24, NOW_MS).remove(0);
        assert_eq!(
            row,
            format!("{at80}{}cursor now", " ".repeat(79 - width_of(at80) - 10))
        );
        assert_eq!(width_of(&row), 79, "cursor now ends at column 78");
        let at120 = " f all accounts   [ ] 7 days   1 · 7 · 14 · 30";
        let row = body_of(&state, 120, 40, NOW_MS).remove(0);
        assert_eq!(
            row,
            format!(
                "{at120}{}cursor now",
                " ".repeat(119 - width_of(at120) - 10)
            )
        );
        // Tier 1: the keys accent, the scope, the range, the choices and `cursor` muted, the
        // time bold.
        let theme = Theme::new(true);
        state.theme = theme;
        let y = crate::view::areas(&state, ratatui::layout::Rect::new(0, 0, 120, 40))
            .body
            .y;
        let letters = |parts: &[(&str, char)]| -> String {
            parts
                .iter()
                .flat_map(|(text, role)| {
                    text.chars()
                        .map(move |c| if c == ' ' { ' ' } else { *role })
                })
                .collect()
        };
        let left = letters(&[
            (" ", 't'),
            ("f", 'a'),
            (" ", 't'),
            ("all accounts", 'm'),
            ("   ", 't'),
            ("[ ]", 'a'),
            (" ", 't'),
            ("7 days", 'm'),
            ("   ", 't'),
            ("1 · 7 · 14 · 30", 'm'),
        ]);
        let right = letters(&[("cursor", 'm'), (" ", 't'), ("now", 'B')]);
        let want = format!(
            "{left}{}{right} ",
            " ".repeat(119 - left.len() - right.len())
        );
        let buf = draw_buffer(&state, 120, 40, NOW_MS);
        assert_eq!(roles(&buf, theme, y), canon(theme, &want));
    }

    #[test]
    fn nothing_in_range_says_so_and_how_to_pick_a_longer_one() {
        let (mut state, _) = opened(3);
        let body = |state: &State| body_of(state, 80, 24, NOW_MS);
        assert!(
            body(&state).iter().any(|l| l.trim() == "loading…"),
            "before a view: {:?}",
            body(&state)
        );
        arrived(&mut state, nothing(70));
        // 80×24 leaves the body 20 rows: the scope row, then the block in the middle of the 19
        // under it.
        let lines = body(&state);
        assert!(lines[0].starts_with(" f all accounts"), "{lines:?}");
        assert_eq!(
            lines[9..11],
            [
                centred("Nothing in the last 7 days"),
                centred("[ ] picks a longer range.")
            ],
            "{lines:?}"
        );
        assert!(!lines.iter().any(|l| l.contains("loading")), "{lines:?}");
        // 1 day.
        update(&mut state, ch('['), NOW_MS);
        assert!(body(&state).iter().any(|l| l.trim() == "loading…"));
        arrived(&mut state, nothing(70));
        assert_eq!(
            body(&state)[9..11],
            [
                centred("Nothing in the last day"),
                centred("[ ] picks a longer range.")
            ]
        );
        // 30 days: there is no longer one.
        update(&mut state, ch('['), NOW_MS);
        arrived(&mut state, nothing(70));
        let lines = body(&state);
        assert!(
            lines.contains(&centred("Nothing in the last 30 days")),
            "{lines:?}"
        );
        assert!(!lines.iter().any(|l| l.contains("picks")), "{lines:?}");
        // Usage in range: neither.
        let used = timeline_view(70, vec![], vec![lane_of(1, vec![None; 70], vec![true; 70])]);
        arrived(&mut state, used);
        let lines = body(&state);
        assert!(
            !lines
                .iter()
                .any(|l| l.contains("Nothing") || l.contains("loading")),
            "{lines:?}"
        );
    }

    #[test]
    fn the_range_defaults_to_seven_days_and_cycles() {
        let (mut state, _) = opened(1);
        let mut seen = vec![];
        for _ in 0..4 {
            let cmds = update(&mut state, ch(']'), NOW_MS);
            let [Cmd::LoadTimeline { since_s, .. }] = cmds.as_slice() else {
                panic!("{cmds:?}");
            };
            seen.push((NOW_S - since_s) / 86_400);
        }
        assert_eq!(seen, [14, 30, 1, 7]);
        update(&mut state, ch('['), NOW_MS);
        assert_eq!(state.history.since_s(NOW_S), NOW_S - 86_400);
    }

    #[test]
    fn reload_views_reads_the_accounts_and_on_history_the_timeline() {
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snapshot(many(2)));
        let g = state.gens.accounts;
        assert_eq!(
            state.reload_views(NOW_MS),
            [Cmd::LoadAccounts { r#gen: g + 1 }]
        );

        let (mut state, _) = opened(2);
        let (g, h) = (state.gens.accounts, state.gens.history);
        assert_eq!(
            state.reload_views(NOW_MS),
            [
                Cmd::LoadAccounts { r#gen: g + 1 },
                read_of(h + 1, Scope::All, NOW_S - WEEK, 70),
            ]
        );

        let mut state = fresh(Screen::History);
        let g = state.gens.accounts;
        assert_eq!(
            state.reload_views(NOW_MS),
            [Cmd::LoadAccounts { r#gen: g + 1 }],
            "before the first accounts read (T4-19)"
        );
    }

    #[test]
    fn a_long_scoped_name_is_cut_and_the_range_and_the_cursor_keep_their_place() {
        let alias = "work".repeat(15);
        let mut accounts = many(3);
        accounts[1].row.alias = Some(alias.clone());
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snapshot(accounts));
        update(&mut state, ch('j'), NOW_MS);
        update(&mut state, key(KeyCode::Enter), NOW_MS);
        let row = body_of(&state, 80, 24, NOW_MS).remove(0);
        // 80 less ` f `, `   [ ] 7 days`, the gap before `cursor now`, it and the inset: 50.
        let name = crate::view::fit(&format!("2 {alias}"), 50);
        assert!(name.ends_with('…') && width_of(&name) == 50, "{name:?}");
        let left = format!(" f {name}   [ ] 7 days");
        assert_eq!(
            row,
            format!("{left}{}cursor now", " ".repeat(79 - width_of(&left) - 10))
        );
        assert_eq!(width_of(&row), 79, "cursor now ends at column 78");
    }

    /// Claude Code's three accounts, then FakeAgent's one, `fake-1`.
    fn two_providers() -> crate::source::AccountsSnapshot {
        use tagteam_engine::switch::LiveLogin;
        use tagteam_engine::views::{ProviderAccounts, ProviderNames, forecast_of};
        let fake = ProviderId::new("fake-agent");
        let mut other = account(1, "ci@example.com", read(60, 10.0, 20.0, false));
        other.row.provider = fake.clone();
        other.row.id = AccountId::from_string("fake-1");
        let mut snap = snapshot(many(3));
        snap.providers.push(ProviderAccounts {
            provider: fake,
            active_position: None,
            forecast: forecast_of(std::slice::from_ref(&other), &LiveLogin::None, &[]),
            accounts: vec![other],
            threshold: 90.0,
            strategy: tagteam_core::autoswitch::AutoStrategy::Best,
            names: ProviderNames {
                display_name: "FakeAgent",
                launch_command: "fakeagent",
            },
            models: vec![],
            live: LiveLogin::None,
        });
        snap
    }

    #[test]
    fn history_reads_the_provider_focus_follows_and_a_change_of_it_reads_again() {
        // T4-18: the FakeAgent account selected on Accounts makes FakeAgent focus's provider.
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(two_providers());
        for _ in 0..3 {
            update(&mut state, ch('j'), NOW_MS);
        }
        let fake = ProviderId::new("fake-agent");
        let read_for = |r#gen, provider: &ProviderId| Cmd::LoadTimeline {
            r#gen,
            provider: provider.clone(),
            scope: Scope::All,
            since_s: NOW_S - WEEK,
            columns: 70,
        };
        assert_eq!(update(&mut state, ch('3'), NOW_MS), [read_for(1, &fake)]);
        arrived(&mut state, nothing(70));
        assert!(state.history.view.is_some());
        // An accounts read without FakeAgent's account moves the selection, and focus, to
        // Claude Code: the view goes, and Claude Code's timeline is read.
        let Cmd::LoadAccounts { r#gen } = state.load_accounts() else {
            unreachable!()
        };
        let cmds = update(&mut state, accounts_msg(r#gen, 3), NOW_MS);
        assert_eq!(cmds, [read_for(2, &cc())]);
        assert_eq!(state.history.view, None);
        let body = body_of(&state, 80, 24, NOW_MS);
        assert!(body.iter().any(|l| l.trim() == "loading…"), "{body:?}");
    }

    #[test]
    fn loading_is_muted() {
        let (mut state, _) = opened(1);
        let theme = Theme::new(true);
        state.theme = theme;
        let buf = draw_buffer(&state, 80, 24, NOW_MS);
        let (_, y) = locate(&buf, "loading…").unwrap();
        assert_eq!(roles(&buf, theme, y).trim(), canon(theme, &"m".repeat(8)));
    }

    /// Every account's three lanes in use throughout, and the in-use chart, over `columns`
    /// columns: each column's peak its index, up to 99.
    fn drawn(columns: usize) -> TimelineView {
        let peaks: Vec<Option<f64>> = (0..columns).map(|c| Some((c % 100) as f64)).collect();
        let chart = Series {
            label: IN_USE.into(),
            peaks: peaks.clone(),
        };
        let lanes = (1..=3)
            .map(|p| lane_of(p, peaks.clone(), vec![true; columns]))
            .collect();
        timeline_view(columns, vec![chart], lanes)
    }

    fn text(line: Line<'_>) -> String {
        line.to_string().trim_end().to_owned()
    }

    #[test]
    fn history_draws_its_chart_lanes_and_axis_in_their_rows() {
        let utc = jiff::tz::TimeZone::UTC;
        for theme in TIERS {
            // Width, height, columns; the title's row, the chart's rows, the first lane's row
            // and the axis's row, in the body.
            for (w, h, columns, title, rows, lane, axis) in [
                (80, 24, 70, 1, 2..=10, 12, 15),
                (120, 40, 110, 2, 3..=14, 16, 19),
            ] {
                let (mut state, _) = opened(3);
                state.theme = theme;
                let view = drawn(columns);
                arrived(&mut state, view.clone());
                let body = body_of(&state, w, h, NOW_MS);
                let series = &view.charts[0];
                assert_eq!(
                    body[title],
                    text(chart::title_row(&state, series, true, w)),
                    "{w}×{h}"
                );
                assert!(body[title].starts_with(" in use"), "{body:?}");
                let n = u16::try_from(rows.clone().count()).unwrap();
                let chart = chart::chart_rows(&series.peaks, n, 90.0, columns - 1, theme);
                assert_eq!(
                    body[rows],
                    chart.into_iter().map(text).collect::<Vec<_>>(),
                    "{w}×{h}"
                );
                for (i, name) in [" 1 a1 ", " 2 a2 ", " 3 a3 "].into_iter().enumerate() {
                    assert!(body[lane + i].starts_with(name), "{body:?}");
                    let row = lanes::lane_row(&view.lanes[i], columns - 1, theme);
                    assert_eq!(body[lane + i], text(row));
                }
                assert_eq!(
                    body[axis],
                    text(lanes::axis_row(view.grid(), 7, &utc, theme)),
                    "{w}×{h}"
                );
                assert!(body[axis].ends_with("now"), "{body:?}");
            }
        }
    }

    #[test]
    fn lanes_past_the_cap_leave_a_more_row() {
        let (mut state, _) = opened(3);
        let lanes = (1..=12)
            .map(|p| lane_of(p, vec![Some(10.0); 70], vec![true; 70]))
            .collect();
        arrived(&mut state, timeline_view(70, vec![], lanes));
        let body = body_of(&state, 80, 24, NOW_MS);
        assert!(body[8].starts_with(" 1 a1 "), "{body:?}");
        assert!(body[13].starts_with(" 6 a6 "), "{body:?}");
        assert_eq!(body[14], " +6 more · f picks one");
    }

    #[test]
    fn one_account_draws_a_chart_per_window_and_its_in_use_lane() {
        let (mut state, _) = opened(3);
        state.history.scope = Scope::Account(id(1));
        let chart = |label: &str| Series {
            label: label.into(),
            peaks: vec![Some(10.0); 70],
        };
        let in_use = (0..70).map(|c| c >= 35).collect();
        let view = timeline_view(
            70,
            vec![chart("5h"), chart("7d")],
            vec![lane_of(1, vec![Some(10.0); 70], in_use)],
        );
        arrived(&mut state, view);
        let body = body_of(&state, 80, 24, NOW_MS);
        assert!(body[1].starts_with(" 5h "), "{body:?}");
        assert!(body[1].ends_with("at cursor  10%   now  10%"), "{body:?}");
        assert!(body[7].starts_with(" 7d "), "{body:?}");
        assert!(body[7].ends_with("now  20%"), "{body:?}");
        assert_eq!(
            body[14],
            format!("{}{}{}", " ".repeat(9), "─".repeat(35), "━".repeat(35))
        );
        assert!(body[15].ends_with("now"), "{body:?}");
    }

    /// The cursor as the scope row says it, and the column the chart's top row and the first
    /// lane draw its `│` in.
    fn cursor_shown(state: &State) -> (String, usize) {
        let body = body_of(state, 80, 24, NOW_MS);
        let said = body[0]
            .rsplit("cursor ")
            .next()
            .unwrap_or_default()
            .to_owned();
        let column = |row: &str| {
            let x = row.chars().position(|c| c == '│').expect("the cursor's │");
            x - usize::from(layout::CHART_X)
        };
        let lane = body
            .iter()
            .find(|r| r.starts_with(" 1 a1 "))
            .expect("the lane");
        let (chart, lane) = (column(&body[2]), column(lane));
        assert_eq!(chart, lane, "the lane's │ in the chart's column: {body:?}");
        (said, chart)
    }

    #[test]
    fn the_cursor_moves_by_column_and_end_goes_to_now() {
        let (mut state, _) = opened(3);
        // A chart with no value and a lane never in use, so the cursor's `│` runs through each
        // of their rows.
        let chart = Series {
            label: IN_USE.into(),
            peaks: vec![None; 70],
        };
        let lane = lane_of(1, vec![None; 70], vec![false; 70]);
        arrived(&mut state, timeline_view(70, vec![chart], vec![lane]));
        assert_eq!(cursor_shown(&state), ("now".to_owned(), 69));
        // Column 68 ends 2h24m before now: the cursor reads 1 ms before that.
        for (code, said, column) in [
            (KeyCode::Left, "11:49", 68),
            (KeyCode::Right, "now", 69),
            (KeyCode::Right, "now", 69),
            (KeyCode::Home, "Sep 14 16:37", 0),
            (KeyCode::Left, "Sep 14 16:37", 0),
            (KeyCode::End, "now", 69),
        ] {
            update(&mut state, key(code), NOW_MS);
            assert_eq!(cursor_shown(&state), (said.to_owned(), column), "{code:?}");
        }
    }

    /// The open choose dialog's options and selected index.
    fn choosing(state: &State) -> (Vec<String>, usize) {
        match &state.overlay {
            Some(Overlay::Choose {
                question,
                options,
                selected,
                ..
            }) => {
                assert_eq!(question, "Show History for");
                (options.clone(), *selected)
            }
            other => panic!("no choose dialog: {other:?}"),
        }
    }

    fn selected_id(state: &State) -> Option<AccountId> {
        state.accounts.selected().map(|a| a.id)
    }

    fn scope_row(state: &State) -> String {
        body_of(state, 80, 24, NOW_MS).remove(0)
    }

    #[test]
    fn f_chooses_all_accounts_or_one_and_the_accounts_selection_follows() {
        let (mut state, _) = opened(3);
        arrived(&mut state, nothing(70));
        assert!(update(&mut state, ch('f'), NOW_MS).is_empty());
        let options = ["all accounts", "1 a1@x.co", "2 a2@x.co", "3 a3@x.co"];
        assert_eq!(choosing(&state), (options.map(String::from).to_vec(), 0));
        update(&mut state, ch('j'), NOW_MS);
        let cmds = update(&mut state, key(KeyCode::Enter), NOW_MS);
        assert!(state.overlay.is_none());
        assert_eq!(state.history.scope, Scope::Account(id(1)));
        assert_eq!(selected_id(&state), Some(id(1)));
        assert_eq!(cmds, [read_of(2, Scope::Account(id(1)), NOW_S - WEEK, 70)]);
        let row = scope_row(&state);
        assert!(row.starts_with(" f 1 a1   [ ]"), "{row:?}");
        // A digit picks the account at that position; the scope is preselected.
        update(&mut state, ch('f'), NOW_MS);
        assert_eq!(choosing(&state).1, 1, "the current scope is preselected");
        let cmds = update(&mut state, ch('2'), NOW_MS);
        assert_eq!(cmds, [read_of(3, Scope::Account(id(2)), NOW_S - WEEK, 70)]);
        assert_eq!(selected_id(&state), Some(id(2)));
        assert!(scope_row(&state).starts_with(" f 2 a2   [ ]"));
        // `0` picks all accounts, and leaves the Accounts selection where it is.
        update(&mut state, ch('f'), NOW_MS);
        let cmds = update(&mut state, ch('0'), NOW_MS);
        assert_eq!(cmds, [read_of(4, Scope::All, NOW_S - WEEK, 70)]);
        assert_eq!(state.history.scope, Scope::All);
        assert_eq!(selected_id(&state), Some(id(2)));
        assert!(scope_row(&state).starts_with(" f all accounts   [ ]"));
        update(&mut state, ch('f'), NOW_MS);
        update(&mut state, ch('3'), NOW_MS);
        assert_eq!(state.history.scope, Scope::Account(id(3)));
        // `⏎` on `all accounts` does too.
        update(&mut state, ch('f'), NOW_MS);
        for _ in 0..3 {
            update(&mut state, ch('k'), NOW_MS);
        }
        let cmds = update(&mut state, key(KeyCode::Enter), NOW_MS);
        assert_eq!(cmds, [read_of(6, Scope::All, NOW_S - WEEK, 70)]);
        assert_eq!(state.history.scope, Scope::All);
        assert_eq!(selected_id(&state), Some(id(3)));
        // `Esc` closes it and changes nothing.
        update(&mut state, ch('f'), NOW_MS);
        assert!(update(&mut state, key(KeyCode::Esc), NOW_MS).is_empty());
        assert!(state.overlay.is_none());
        assert_eq!(state.history.scope, Scope::All);
    }

    #[test]
    fn esc_after_enter_returns_to_accounts_and_all_accounts() {
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snapshot(many(3)));
        update(&mut state, ch('j'), NOW_MS);
        update(&mut state, key(KeyCode::Enter), NOW_MS);
        assert_eq!(
            (state.screen, &state.history.scope),
            (Screen::History, &Scope::Account(id(2)))
        );
        assert!(update(&mut state, key(KeyCode::Esc), NOW_MS).is_empty());
        assert_eq!(
            (state.screen, &state.history.scope),
            (Screen::Accounts, &Scope::All)
        );
        assert_eq!(
            update(&mut state, ch('3'), NOW_MS),
            [read_of(2, Scope::All, NOW_S - WEEK, 70)]
        );
    }

    #[test]
    fn esc_on_a_scoped_history_returns_to_all_accounts() {
        let (mut state, _) = opened(3);
        update(&mut state, ch('f'), NOW_MS);
        update(&mut state, ch('1'), NOW_MS);
        assert_eq!(state.history.scope, Scope::Account(id(1)));
        assert_eq!(
            update(&mut state, key(KeyCode::Esc), NOW_MS),
            [read_of(3, Scope::All, NOW_S - WEEK, 70)]
        );
        assert_eq!(
            (state.screen, &state.history.scope),
            (Screen::History, &Scope::All)
        );
        // On all accounts, `Esc` does nothing.
        arrived(&mut state, nothing(70));
        let before = body_of(&state, 80, 24, NOW_MS);
        assert!(update(&mut state, key(KeyCode::Esc), NOW_MS).is_empty());
        assert_eq!(state.screen, Screen::History);
        assert_eq!(body_of(&state, 80, 24, NOW_MS), before);
    }

    #[test]
    fn while_scoped_the_scope_follows_the_accounts_selection() {
        let (mut state, _) = opened(3);
        update(&mut state, ch('f'), NOW_MS);
        update(&mut state, ch('2'), NOW_MS);
        assert_eq!(state.history.scope, Scope::Account(id(2)));
        update(&mut state, ch('1'), NOW_MS);
        update(&mut state, ch('j'), NOW_MS);
        assert_eq!(selected_id(&state), Some(id(3)));
        assert_eq!(state.history.scope, Scope::Account(id(3)));
        // Scoped to all accounts, the selection moves alone.
        update(&mut state, ch('3'), NOW_MS);
        update(&mut state, ch('f'), NOW_MS);
        update(&mut state, ch('0'), NOW_MS);
        assert_eq!(state.history.scope, Scope::All);
        update(&mut state, ch('1'), NOW_MS);
        update(&mut state, ch('k'), NOW_MS);
        assert_eq!(selected_id(&state), Some(id(2)));
        assert_eq!(state.history.scope, Scope::All);
    }

    #[test]
    fn a_scoped_account_gone_from_the_view_returns_history_to_all_accounts() {
        let (mut state, _) = opened(3);
        update(&mut state, ch('f'), NOW_MS);
        update(&mut state, ch('3'), NOW_MS);
        assert_eq!(state.history.scope, Scope::Account(id(3)));
        arrived(&mut state, nothing(70));
        let Cmd::LoadAccounts { r#gen } = state.load_accounts() else {
            unreachable!()
        };
        let h = state.gens.history;
        let cmds = update(&mut state, accounts_msg(r#gen, 2), NOW_MS);
        assert_eq!(state.history.scope, Scope::All);
        assert_eq!(cmds, [read_of(h + 1, Scope::All, NOW_S - WEEK, 70)]);
    }

    #[test]
    fn shift_j_and_k_scroll_one_account_s_charts() {
        let (mut state, _) = opened(3);
        update(&mut state, ch('f'), NOW_MS);
        update(&mut state, ch('1'), NOW_MS);
        let chart = |label: &str| Series {
            label: label.into(),
            peaks: vec![Some(10.0); 70],
        };
        let charts = ["5h", "7d", "Fable", "Sonnet"].map(chart).to_vec();
        let lane = lane_of(1, vec![Some(10.0); 70], vec![true; 70]);
        arrived(&mut state, timeline_view(70, charts, vec![lane]));
        // At 80×24 two charts show, their titles on body rows 1 and 7 (T4-8).
        let titles = |state: &State| {
            let body = body_of(state, 80, 24, NOW_MS);
            let first = |row: &str| row.split_whitespace().next().unwrap_or_default().to_owned();
            (first(&body[1]), first(&body[7]))
        };
        let shown = |a: &str, b: &str| (a.to_owned(), b.to_owned());
        assert_eq!(titles(&state), shown("5h", "7d"));
        for (c, a, b) in [
            ('J', "7d", "Fable"),
            ('J', "Fable", "Sonnet"),
            ('J', "Fable", "Sonnet"),
            ('K', "7d", "Fable"),
        ] {
            update(&mut state, ch(c), NOW_MS);
            assert_eq!(titles(&state), shown(a, b), "{c}");
        }
    }

    #[test]
    fn a_scope_on_another_provider_s_account_reads_that_provider() {
        // Review Focus 4, T4-18.
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(two_providers());
        for _ in 0..3 {
            update(&mut state, ch('j'), NOW_MS);
        }
        let cmds = update(&mut state, key(KeyCode::Enter), NOW_MS);
        let fake = AccountId::from_string("fake-1");
        assert_eq!(
            cmds,
            [Cmd::LoadTimeline {
                r#gen: 1,
                provider: ProviderId::new("fake-agent"),
                scope: Scope::Account(fake),
                since_s: NOW_S - WEEK,
                columns: 70,
            }]
        );
        update(&mut state, ch('f'), NOW_MS);
        let options = ["all accounts", "1 ci@example.com"];
        assert_eq!(choosing(&state), (options.map(String::from).to_vec(), 1));
    }

    #[test]
    fn the_scope_dialog_names_each_account_by_its_position_alone() {
        // Minor 1: no `N. ` of the dialog's own before the positions.
        let (mut state, _) = opened(3);
        update(&mut state, ch('f'), NOW_MS);
        let rows = draw(&state, 80, 24);
        for option in ["all accounts", "1 a1@x.co", "2 a2@x.co", "3 a3@x.co"] {
            assert!(
                rows.iter().any(|r| r.contains(&format!(" {option} "))),
                "{option}: {rows:?}"
            );
        }
        assert!(!rows.iter().any(|r| r.contains("1. ")), "{rows:?}");
        // A digit with no account at that position picks nothing.
        assert!(update(&mut state, ch('7'), NOW_MS).is_empty());
        assert!(state.overlay.is_some());
    }

    #[test]
    fn the_at_cursor_readout_follows_the_cursor() {
        let (mut state, _) = opened(3);
        arrived(&mut state, drawn(70));
        let title = |state: &State| body_of(state, 80, 24, NOW_MS).remove(1);
        assert!(
            title(&state).contains("at cursor  69%"),
            "{}",
            title(&state)
        );
        update(&mut state, key(KeyCode::Left), NOW_MS);
        update(&mut state, key(KeyCode::Left), NOW_MS);
        assert!(
            title(&state).contains("at cursor  67%"),
            "{}",
            title(&state)
        );
        update(&mut state, key(KeyCode::Home), NOW_MS);
        assert!(
            title(&state).contains("at cursor   0%"),
            "{}",
            title(&state)
        );
    }

    #[test]
    fn a_long_scoped_name_beside_a_dated_cursor_keeps_the_row_within_80_columns() {
        let mut accounts = many(3);
        accounts[1].row.alias = Some("work".repeat(15));
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snapshot(accounts));
        update(&mut state, ch('j'), NOW_MS);
        update(&mut state, key(KeyCode::Enter), NOW_MS);
        arrived(&mut state, nothing(70));
        update(&mut state, key(KeyCode::Home), NOW_MS);
        let row = scope_row(&state);
        assert_eq!(width_of(&row), 79, "one column in: {row:?}");
        assert!(row.starts_with(" f 2 work"), "{row:?}");
        assert!(row.contains('…'), "the name is cut: {row:?}");
        assert!(
            row.ends_with("   [ ] 7 days   cursor Sep 14 16:37"),
            "the range and the cursor whole: {row:?}"
        );
    }

    #[test]
    fn a_resize_leaves_j_and_k_moving_at_once() {
        let (mut state, _) = opened(3);
        update(&mut state, ch('f'), NOW_MS);
        update(&mut state, ch('1'), NOW_MS);
        let chart = |label: &str| Series {
            label: label.into(),
            peaks: vec![Some(10.0); 70],
        };
        let charts = ["5h", "7d", "Fable", "Sonnet"].map(chart).to_vec();
        let lane = lane_of(1, vec![Some(10.0); 70], vec![true; 70]);
        arrived(&mut state, timeline_view(70, charts, vec![lane]));
        let resize = |state: &mut State, height| {
            update(state, Msg::Resize { width: 80, height }, NOW_MS);
        };
        // The charts' titles shown, top first, at 80 columns and the state's height.
        let titles = |state: &State| -> Vec<String> {
            body_of(state, 80, state.size.1, NOW_MS)
                .iter()
                .filter_map(|r| r.split_whitespace().next())
                .filter(|w| ["5h", "7d", "Fable", "Sonnet"].contains(w))
                .map(str::to_owned)
                .collect()
        };
        // 35 rows show three charts: `J` past the end stops at 7d–Sonnet; 24 rows show two,
        // and `K` moves from there at once.
        resize(&mut state, 35);
        for _ in 0..3 {
            update(&mut state, ch('J'), NOW_MS);
        }
        assert_eq!(titles(&state), ["7d", "Fable", "Sonnet"]);
        resize(&mut state, 24);
        assert_eq!(titles(&state), ["7d", "Fable"]);
        update(&mut state, ch('K'), NOW_MS);
        assert_eq!(titles(&state), ["5h", "7d"]);
        // Back the other way: past the end at 24 rows is past the end at 35, and one `K` moves.
        for _ in 0..3 {
            update(&mut state, ch('J'), NOW_MS);
        }
        assert_eq!(titles(&state), ["Fable", "Sonnet"]);
        resize(&mut state, 35);
        assert_eq!(titles(&state), ["7d", "Fable", "Sonnet"]);
        update(&mut state, ch('K'), NOW_MS);
        assert_eq!(titles(&state), ["5h", "7d", "Fable"]);
    }

    #[test]
    fn a_view_wider_than_the_screen_draws_clipped_until_the_re_read() {
        let (mut state, _) = opened(3);
        arrived(&mut state, drawn(110));
        let rows = draw(&state, 80, 24);
        assert!(rows.iter().all(|r| width_of(r) <= 80), "{rows:?}");
        let body = body_of(&state, 80, 24, NOW_MS);
        assert!(
            body[10].starts_with("      0  ▁▁▁"),
            "the chart starts at column 9: {body:?}"
        );
        let resize = Msg::Resize {
            width: 80,
            height: 24,
        };
        assert!(update(&mut state, resize, NOW_MS).is_empty());
        assert_eq!(
            update(&mut state, Msg::Tick, NOW_MS + RESIZE_DEBOUNCE_MS),
            [read_of(2, Scope::All, NOW_S - WEEK, 70)]
        );
    }
}
