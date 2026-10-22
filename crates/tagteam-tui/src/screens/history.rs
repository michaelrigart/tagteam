//! The History screen (redesign §7): focus's provider's timeline, of every account or one
//! (§7.2), over 1, 7, 14 or 30 days, read on the views lane (§7.7). Its scope row, then
//! `loading…` before a view or the empty state when nothing is in range (§5.7). It never
//! fetches (core §13.4).

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::timeline::{Scope, TimelineView};

use crate::autoline;
use crate::chrome::{WIDE, fit_spans};
use crate::daemon;
use crate::keys::RIGHT_INSET;
use crate::msg::Cmd;
use crate::source::SourceError;
use crate::state::{BannerKind, Screen, State};
use crate::theme::Role;
use crate::update;
use crate::view::{centered, centred_block, width_of};

/// T4-21: drawn under the scope row until the first view of a scope and range arrives.
const LOADING: &str = "loading…";
/// The empty state's sentence (§5.7), left out at 30 days.
const LONGER: &str = "[ ] picks a longer range.";
/// The scope row's name for every account (§7.2).
const ALL_ACCOUNTS: &str = "all accounts";
/// A scoped account the accounts view no longer lists.
const AN_ACCOUNT: &str = "an account";
/// The ranges, muted beside the current one from `WIDE` columns (§7.2).
const CHOICES: &str = "1 · 7 · 14 · 30";
/// Between the scope row's groups (§4.3's group gap).
const GAP: &str = "   ";
/// T4-19: a resize re-reads the timeline this long after the last one.
pub(crate) const RESIZE_DEBOUNCE_MS: i64 = 250;
/// T4-10: the marker column, the 8-column gutter, and the column kept at the right.
const CHART_MARGIN: u16 = 10;

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

pub struct HistoryState {
    /// Whose timeline (§7.2): every account's until `⏎` on Accounts scopes it.
    pub(crate) scope: Scope,
    range: Range,
    /// Focus's provider when the timeline was last asked for (T4-18); `None` before then.
    provider: Option<ProviderId>,
    /// The last timeline read; `None` from an open, a scope or a range change until the read.
    view: Option<TimelineView>,
    /// When the terminal was last resized on History, until the re-read it is due (T4-19).
    resized_ms: Option<i64>,
}

impl Default for HistoryState {
    fn default() -> Self {
        HistoryState {
            scope: Scope::All,
            range: Range::default(),
            provider: None,
            view: None,
            resized_ms: None,
        }
    }
}

impl HistoryState {
    /// Now less the range: 1, 7, 14 or 30 days, 7 by default.
    fn since_s(&self, now_s: i64) -> i64 {
        now_s - self.range.days() * 86_400
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
    let since = h.since_s(now_ms.div_euclid(1000));
    let scope = h.scope.clone();
    let columns = columns(state.size.0);
    Some(state.load_timeline(provider, scope, since, columns))
}

/// History opens on `loading…` and reads its timeline (T4-19).
pub(crate) fn on_enter(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    state.history.view = None;
    reload(state, now_ms).into_iter().collect()
}

/// A new accounts read, while History shows: the first one reads the timeline a TUI started on
/// History waited for, and one that moves focus to another provider reads that provider's
/// (T4-18, T4-19). Any other leaves the timeline to its own re-reads.
pub(crate) fn on_accounts(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    if state.screen != Screen::History {
        return vec![];
    }
    let focus = daemon::focus(state);
    match &state.history.provider {
        Some(p) if *p == focus => return vec![],
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

/// T4-19: the re-read a resize is due, once it has settled, while History shows.
pub(crate) fn on_tick(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    let due = state
        .history
        .resized_ms
        .is_some_and(|at| now_ms >= at + RESIZE_DEBOUNCE_MS);
    if !due || state.screen != Screen::History || state.quitting.is_some() {
        return vec![];
    }
    state.history.resized_ms = None;
    reload(state, now_ms).into_iter().collect()
}

/// `⏎` on Accounts: History, scoped to `account` (§7.2).
pub(crate) fn scope_to(state: &mut State, account: AccountId, now_ms: i64) -> Vec<Cmd> {
    state.history.scope = Scope::Account(account);
    update::show(state, Screen::History, now_ms)
}

pub fn handle_key(state: &mut State, key: KeyEvent, now_ms: i64) -> Vec<Cmd> {
    let by = match key.code {
        KeyCode::Char('[') => -1,
        KeyCode::Char(']') => 1,
        _ => return vec![],
    };
    let h = &mut state.history;
    h.range = h.range.step(by);
    h.view = None;
    reload(state, now_ms).into_iter().collect()
}

/// The scope row, then `loading…` before a view, or the empty state when nothing is in range
/// (T4-21).
pub fn render(state: &State, frame: &mut Frame, area: Rect, _now_ms: i64) {
    if area.is_empty() {
        return;
    }
    let scope_row = Rect::new(area.x, area.y, area.width, 1);
    frame.render_widget(Paragraph::new(scope_line(state, area.width)), scope_row);
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
        Some(_) => {}
    }
}

/// No lanes and no chart value (T4-21).
fn nothing_in_range(view: &TimelineView) -> bool {
    view.lanes.is_empty()
        && view
            .charts
            .iter()
            .all(|s| s.peaks.iter().all(Option::is_none))
}

/// ` f {scope}   [ ] {range}`, the choices beside it from `WIDE` columns, and `cursor now` on
/// the right, one column in (§7.1, §7.2): the keys accent, the words muted, the time bold.
fn scope_line(state: &State, width: u16) -> Line<'static> {
    let theme = state.theme;
    let (accent, muted) = (theme.role(Role::Accent), theme.role(Role::Muted));
    let mut left = vec![
        Span::raw(" "),
        Span::styled("f", accent),
        Span::raw(" "),
        Span::styled(scope_name(state), muted),
        Span::raw(GAP),
        Span::styled("[ ]", accent),
        Span::raw(" "),
        Span::styled(state.history.range.label(), muted),
    ];
    if width >= WIDE {
        left.push(Span::raw(GAP));
        left.push(Span::styled(CHOICES, muted));
    }
    let right = [
        Span::styled("cursor", muted),
        Span::raw(" "),
        Span::styled("now", theme.strong()),
    ];
    let used = |spans: &[Span]| spans.iter().map(|s| width_of(&s.content)).sum::<usize>();
    let gap = usize::from(width).saturating_sub(used(&left) + used(&right) + RIGHT_INSET);
    left.push(Span::raw(" ".repeat(gap)));
    left.extend(right);
    Line::from(fit_spans(left, usize::from(width)))
}

/// `all accounts`, or the scoped account's short name in focus's accounts (§7.2).
fn scope_name(state: &State) -> String {
    let Scope::Account(id) = &state.history.scope else {
        return ALL_ACCOUNTS.to_owned();
    };
    let view = state.view_of(&daemon::focus(state));
    view.and_then(|v| v.accounts.iter().find(|a| &a.row.id == id))
        .map_or_else(
            || AN_ACCOUNT.to_owned(),
            |a| autoline::named(view, a.row.position),
        )
}

#[cfg(test)]
mod tests {
    use tagteam_engine::timeline::{IN_USE, Series};

    use super::*;
    use crate::msg::Msg;
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
}
