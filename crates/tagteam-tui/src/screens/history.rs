//! The History screen (spec §5.4): the accounts on the left, the selected account's windows on
//! the right, each with a sparkline of its samples across the range, its latest `pct`, and
//! `history`'s rate and projection. It never fetches (core §13.4).

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use tagteam_core::{AccountId, Sample};
use tagteam_engine::views::{AccountView, HistoryView, HistoryWindow};
use tagteam_engine::words;

use crate::msg::Cmd;
use crate::state::{Screen, State};
use crate::view::{fit, flow, furthest_scroll, pad, scrolled_windows, select, width_of, wrap_rows};

pub const NO_HISTORY: &str = "No usage history.";
pub const NO_ACCOUNTS: &str = "No accounts yet.";
pub const LOADING: &str = "loading…";
/// Eight levels from 0 to 100 %, as `history` draws them.
const SPARK: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
/// The account list's width, and the gap after it.
const LIST_W: u16 = 26;
/// The rows a long label takes above its sparkline, at most (R20).
const LABEL_ROWS: usize = 2;

/// The ranges `[` and `]` cycle through (spec §5.4); 7 days is `history`'s default.
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

    fn step(self, by: isize) -> Range {
        let at = Self::ALL.iter().position(|r| *r == self).unwrap_or(1) as isize;
        Self::ALL[(at + by).rem_euclid(Self::ALL.len() as isize) as usize]
    }
}

#[derive(Default)]
pub struct HistoryState {
    /// The account whose history is shown, by ID.
    account: Option<AccountId>,
    range: Range,
    /// The last history read for `account`.
    view: Option<HistoryView>,
    /// The first window shown (`J`/`K`), when they do not all fit.
    from: usize,
}

impl HistoryState {
    #[cfg(test)]
    pub fn account(&self) -> Option<&AccountId> {
        self.account.as_ref()
    }

    /// Now less the selected range: 1, 7, 14 or 30 days, 7 by default.
    pub fn since_s(&self, now_s: i64) -> i64 {
        now_s - self.range.days() * 86_400
    }

    pub fn apply(&mut self, view: HistoryView) {
        self.view = Some(view);
    }

    /// Shows `account`'s history; another account's view is dropped until it is read.
    pub fn select(&mut self, account: AccountId) {
        if self.account.as_ref() != Some(&account) {
            self.view = None;
            self.from = 0;
        }
        self.account = Some(account);
    }
}

/// Every account in the accounts view, in its order.
fn accounts(state: &State) -> Vec<&AccountView> {
    state
        .accounts
        .snapshot()
        .map(|s| s.providers.iter().flat_map(|p| &p.accounts).collect())
        .unwrap_or_default()
}

/// A read of the shown account's history over the selected range, under a new generation.
pub(crate) fn reload(state: &mut State, now_ms: i64) -> Option<Cmd> {
    let account = state.history.account.clone()?;
    let since = state.history.since_s(now_ms.div_euclid(1000));
    Some(state.load_history(account, since))
}

/// History opens on its account, or on the first account when it has none or its account is
/// gone, and reads it.
pub(crate) fn on_enter(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    let ids: Vec<AccountId> = accounts(state).iter().map(|v| v.row.id.clone()).collect();
    match &state.history.account {
        Some(id) if ids.contains(id) => {}
        _ => match ids.first() {
            Some(first) => state.history.select(first.clone()),
            None => {
                state.history.account = None;
                state.history.view = None;
                state.gens.history += 1;
                return vec![];
            }
        },
    }
    reload(state, now_ms).into_iter().collect()
}

/// A new `accounts` read: while History is open, an account that is gone, or none chosen yet,
/// moves it to the first account.
pub(crate) fn on_accounts(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    if state.screen != Screen::History {
        return vec![];
    }
    let present = state
        .history
        .account
        .as_ref()
        .is_some_and(|id| accounts(state).iter().any(|v| &v.row.id == id));
    if present && state.history.view.is_some() {
        return vec![];
    }
    on_enter(state, now_ms)
}

pub fn handle_key(state: &mut State, key: KeyEvent, now_ms: i64) -> Vec<Cmd> {
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => step_account(state, 1, now_ms),
        KeyCode::Char('k') | KeyCode::Up => step_account(state, -1, now_ms),
        KeyCode::Char('[') => {
            state.history.range = state.history.range.step(-1);
            reload(state, now_ms).into_iter().collect()
        }
        KeyCode::Char(']') => {
            state.history.range = state.history.range.step(1);
            reload(state, now_ms).into_iter().collect()
        }
        KeyCode::Char('J') => scroll(state, 1, now_ms),
        KeyCode::Char('K') => scroll(state, -1, now_ms),
        _ => vec![],
    }
}

/// `J`/`K`: the windows move `by` one, stopping once the last one shows, or at the first.
fn scroll(state: &mut State, by: isize, now_ms: i64) -> Vec<Cmd> {
    let body = crate::view::body_at(state);
    let width = usize::from(body.width.saturating_sub(LIST_W.min(body.width) + 1));
    let heights: Vec<usize> = window_blocks(state, width, now_ms.div_euclid(1000))
        .iter()
        .map(Vec::len)
        .collect();
    let furthest = furthest_scroll(&heights, usize::from(body.height).saturating_sub(1));
    let h = &mut state.history;
    // From what shows: a resize or a new read may have left the stored scroll past the end.
    let from = h.from.min(furthest) as isize;
    h.from = (from + by).clamp(0, furthest as isize) as usize;
    vec![]
}

/// Moves to the account `by` rows away, stopping at either end, and reads its history.
fn step_account(state: &mut State, by: isize, now_ms: i64) -> Vec<Cmd> {
    let ids: Vec<AccountId> = accounts(state).iter().map(|v| v.row.id.clone()).collect();
    if ids.is_empty() {
        return vec![];
    }
    let at = state
        .history
        .account
        .as_ref()
        .and_then(|id| ids.iter().position(|i| i == id))
        .unwrap_or(0) as isize;
    let to = &ids[(at + by).clamp(0, ids.len() as isize - 1) as usize];
    if state.history.account.as_ref() == Some(to) {
        return vec![];
    }
    state.history.select(to.clone());
    reload(state, now_ms).into_iter().collect()
}

/// One bucket per column from `since_s` to `now_s`, each the highest `pct` sampled in it; an
/// empty bucket stays blank, and a reset shows as a drop (spec §5.4).
fn sparkline(samples: &[Sample], since_s: i64, now_s: i64, columns: usize) -> String {
    if columns == 0 {
        return String::new();
    }
    let span = (now_s - since_s).max(1);
    let mut peaks: Vec<Option<f64>> = vec![None; columns];
    for s in samples {
        if s.fetched_at < since_s || s.fetched_at > now_s {
            continue;
        }
        let i = (((s.fetched_at - since_s) as i128 * columns as i128) / span as i128) as usize;
        let slot = &mut peaks[i.min(columns - 1)];
        *slot = Some(slot.map_or(s.pct, |p| p.max(s.pct)));
    }
    peaks
        .into_iter()
        .map(|p| match p {
            Some(pct) => SPARK[(pct.clamp(0.0, 100.0) / 100.0 * 7.0).round() as usize],
            None => ' ',
        })
        .collect()
}

pub fn render(state: &State, frame: &mut Frame, area: Rect, now_ms: i64) {
    let now_s = now_ms.div_euclid(1000);
    let list = Rect::new(area.x, area.y, LIST_W.min(area.width), area.height);
    let right = Rect::new(
        area.x + list.width + 1,
        area.y,
        area.width.saturating_sub(list.width + 1),
        area.height,
    );
    frame.render_widget(
        Paragraph::new(account_list(state, usize::from(list.height))),
        list,
    );
    frame.render_widget(Paragraph::new(windows(state, right, now_s)), right);
}

/// ` ACCOUNTS`, then each account as `* 1 name`, the shown one in reverse video, under its
/// provider's ID when there are several (spec §5.1). A list longer than `rows` scrolls so that
/// the shown account stays in view.
fn account_list(state: &State, rows: usize) -> Vec<Line<'static>> {
    let w = usize::from(LIST_W);
    let head = Line::from(Span::styled(" ACCOUNTS", state.theme.strong()));
    let groups: Vec<_> = state
        .accounts
        .snapshot()
        .map(|s| {
            s.providers
                .iter()
                .filter(|p| !p.accounts.is_empty())
                .collect()
        })
        .unwrap_or_default();
    if groups.is_empty() {
        let words = if state.accounts.snapshot().is_none() {
            LOADING
        } else {
            NO_ACCOUNTS
        };
        return vec![head, Line::from(format!(" {words}"))];
    }
    let several = groups.len() > 1;
    let mut lines = Vec::new();
    let mut shown_at = 0;
    for group in groups {
        if several {
            lines.push(Line::from(Span::styled(
                format!(" {}", group.provider),
                state.theme.strong(),
            )));
        }
        for v in &group.accounts {
            let marker = if v.active { '*' } else { ' ' };
            let lead = format!(" {marker} {} ", v.row.position);
            let name = pad(&words::name(&v.row), w.saturating_sub(width_of(&lead)));
            // Column 0 is the selection column; the name is the span the selection bolds.
            let line = Line::from(vec![
                Span::raw(" "),
                Span::raw(lead[1..].to_owned()),
                Span::raw(name),
            ]);
            if state.history.account.as_ref() == Some(&v.row.id) {
                shown_at = lines.len();
                lines.push(select(line, Some(2), w, state.theme));
            } else {
                lines.push(line);
            }
        }
    }
    let room = rows.saturating_sub(1).max(1);
    let skip = (shown_at + 1).saturating_sub(room);
    std::iter::once(head)
        .chain(lines.into_iter().skip(skip))
        .collect()
}

/// The shown account's windows (`window_lines`), then the range. Windows that do not fit are
/// scrolled to (`J`/`K`), whole, which the range line says.
fn windows(state: &State, area: Rect, now_s: i64) -> Vec<Line<'static>> {
    let h = &state.history;
    let width = usize::from(area.width);
    let mut lines = Vec::new();
    let mut more = String::new();
    match (&h.account, &h.view) {
        (None, _) if state.accounts.snapshot().is_none() => lines.push(Line::from(LOADING)),
        (None, _) => lines.push(Line::from(NO_HISTORY)),
        (Some(_), None) => lines.push(Line::from(LOADING)),
        (Some(_), Some(view)) if view.windows.is_empty() => lines.push(Line::from(NO_HISTORY)),
        (Some(_), Some(_)) => {
            let blocks = window_blocks(state, width, now_s);
            let heights: Vec<usize> = blocks.iter().map(Vec::len).collect();
            let rows = usize::from(area.height).saturating_sub(1);
            let range = scrolled_windows(&heights, h.from, rows);
            if range != (0..blocks.len()) {
                let (from, to) = (range.start + 1, range.end);
                more = format!("  J K: windows {from}–{to} of {}", blocks.len());
            }
            for block in blocks.into_iter().take(range.end).skip(range.start) {
                lines.extend(block);
            }
        }
    }
    let footer_at = usize::from(area.height).saturating_sub(1);
    while lines.len() < footer_at {
        lines.push(Line::default());
    }
    lines.truncate(footer_at);
    lines.push(Line::from(fit(
        &format!("{}  [ ] range{more}", h.range.label()),
        width,
    )));
    lines
}

/// Each window's lines in a panel `width` columns wide. The label column is a third of the
/// width at most, so the sparkline, the latest pct and the projection always have room (R20).
fn window_blocks(state: &State, width: usize, now_s: i64) -> Vec<Vec<Line<'static>>> {
    let Some(view) = &state.history.view else {
        return vec![];
    };
    let label_w = view
        .windows
        .iter()
        .map(|w| width_of(&w.window.label))
        .max()
        .unwrap_or(0)
        .min(width / 3);
    let columns = width.saturating_sub(label_w + 2 + 2 + 4);
    view.windows
        .iter()
        .map(|hw| window_lines(state, hw, label_w, columns, width, now_s))
        .collect()
}

/// `5h  ▁▂▃…  9%`, then the rate and projection in `history`'s words (which say whether it
/// lasts to its reset) under the sparkline, going on to more lines there when they do not fit
/// `width` (R20). A label wider than its column takes rows of its own above: `LABEL_ROWS` at
/// most, the rest cut.
fn window_lines(
    state: &State,
    hw: &HistoryWindow,
    label_w: usize,
    columns: usize,
    width: usize,
    now_s: i64,
) -> Vec<Line<'static>> {
    let w = &hw.window;
    let since = state.history.since_s(now_s);
    let pct = w.pct.round() as i64;
    let style = state.theme.severity(words::severity(pct));
    let mut lines: Vec<Line<'static>> = Vec::new();
    let label = if width_of(&w.label) > label_w {
        let rows = wrap_rows(&w.label, width, LABEL_ROWS);
        lines.extend(rows.into_iter().map(Line::from));
        ""
    } else {
        w.label.as_str()
    };
    lines.push(Line::from(vec![
        Span::raw(format!("{}  ", pad(label, label_w))),
        Span::raw(sparkline(&hw.samples, since, now_s, columns)),
        Span::styled(format!("  {:>4}", format!("{pct}%")), style),
    ]));
    let indent = label_w + 2;
    let under = vec![Span::raw(" ".repeat(indent))];
    lines.extend(flow(
        under,
        vec![words::projection_text(hw, now_s)],
        indent,
        width,
    ));
    lines
}
#[cfg(test)]
mod tests {
    /// `line` from column 27, where the right-hand panel starts (by columns: the selected
    /// account's `▌` is three bytes), as `str::get(27..)` shows it: none when `line` is shorter.
    fn right_of(line: &str) -> Option<&str> {
        match line.char_indices().nth(27) {
            Some((at, _)) => Some(&line[at..]),
            None => (line.chars().count() == 27).then_some(""),
        }
    }

    use ratatui::style::{Color, Modifier};
    use tagteam_core::{Pace, ProjectionMethod, WindowKind};
    use tagteam_engine::views::UsageStatus;

    use super::*;
    use crate::msg::{Cmd, Msg};
    use crate::state::BannerKind;
    use crate::state::Screen;
    use crate::testutil::*;
    use crate::theme::Theme;
    use crate::update::update;

    fn on_history(accounts: Vec<AccountView>) -> State {
        let mut state = fresh(Screen::History);
        state.accounts.apply(snapshot(accounts));
        state
    }

    #[test]
    fn the_shown_account_has_the_bar_and_a_bold_name_and_no_reverse_video() {
        for theme in TIERS {
            let mut state = on_history(many(2));
            state.theme = theme;
            state.history.select(id(2));
            let buf = draw_buffer(&state, 80, 24, NOW_MS);
            let (x, y) = locate(&buf, "▌  2 a2@x.co").unwrap();
            assert_eq!(x, 0);
            for i in 5..12 {
                assert!(
                    buf[(i, y)].modifier.contains(Modifier::BOLD),
                    "{theme:?} {i}"
                );
            }
            let (_, other) = locate(&buf, "   1 a1@x.co").unwrap();
            for i in 0..26 {
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

    /// 5h with three samples and no rate; 7d running out in 12 hours by regression.
    fn view_of(account: AccountView) -> HistoryView {
        history_of(
            account,
            vec![
                history_window(
                    window("5h", "5h", WindowKind::Short, 9.0, Some(9_600)),
                    vec![
                        sample(6 * HOUR, 5.0),
                        sample(4 * HOUR, 7.0),
                        sample(2 * HOUR, 9.0),
                    ],
                    Pace::default(),
                ),
                history_window(
                    window("7d", "7d", WindowKind::Long, 95.0, Some(266_400)),
                    vec![sample(3 * 86_400, 10.0), sample(HOUR, 95.0)],
                    Pace {
                        rate_per_hour: Some(5.0),
                        method: Some(ProjectionMethod::Regression),
                        exhaustion_at: Some(NOW_S + 43_200),
                        will_last_to_reset: Some(false),
                        ..Pace::default()
                    },
                ),
            ],
        )
    }

    #[test]
    fn the_range_defaults_to_seven_days_and_cycles() {
        let mut state = on_history(many(1));
        state.history.select(id(1));
        assert_eq!(state.history.since_s(NOW_S), NOW_S - 7 * 86_400);
        let mut seen = vec![];
        for _ in 0..4 {
            let cmds = update(&mut state, ch(']'), NOW_MS);
            let [Cmd::LoadHistory { since_s, .. }] = cmds.as_slice() else {
                panic!("{cmds:?}");
            };
            seen.push((NOW_S - since_s) / 86_400);
        }
        assert_eq!(seen, [14, 30, 1, 7]);
        update(&mut state, ch('['), NOW_MS);
        assert_eq!(state.history.since_s(NOW_S), NOW_S - 86_400);
    }

    #[test]
    fn a_range_or_account_change_reads_under_a_new_generation() {
        let mut state = on_history(many(3));
        let opened = crate::update::show(&mut state, Screen::Accounts, NOW_MS);
        assert!(opened.is_empty());
        let cmds = crate::update::show(&mut state, Screen::History, NOW_MS);
        assert_eq!(
            cmds,
            [Cmd::LoadHistory {
                r#gen: 1,
                account: id(1),
                since_s: NOW_S - 7 * 86_400,
            }]
        );
        assert_eq!(
            update(&mut state, ch('j'), NOW_MS),
            [Cmd::LoadHistory {
                r#gen: 2,
                account: id(2),
                since_s: NOW_S - 7 * 86_400,
            }]
        );
        assert_eq!(
            update(&mut state, ch('['), NOW_MS),
            [Cmd::LoadHistory {
                r#gen: 3,
                account: id(2),
                since_s: NOW_S - 86_400,
            }]
        );
        for _ in 0..5 {
            update(&mut state, ch('j'), NOW_MS);
        }
        assert_eq!(state.history.account(), Some(&id(3)), "j stops at the end");
        assert!(update(&mut state, ch('j'), NOW_MS).is_empty());
    }

    #[test]
    fn the_windows_show_a_sparkline_the_latest_pct_and_the_projection() {
        let mut state = on_history(many(2));
        state.history.select(id(1));
        state
            .history
            .apply(view_of(account(1, "a1@x.co", read(60, 9.0, 95.0, false))));
        let lines = body_of(&state, 80, 24, NOW_MS);
        assert!(lines[0].starts_with(" ACCOUNTS "), "{}", lines[0]);
        assert!(lines[1].starts_with("▌  1 a1@x.co"), "{}", lines[1]);
        let right: Vec<&str> = lines[0..6]
            .iter()
            .map(|l| right_of(l).unwrap_or(""))
            .collect();
        let spark = 80 - 27 - 2 - 2 - 2 - 4;
        assert_eq!(
            right[0].chars().count(),
            2 + 2 + spark + 2 + 4,
            "{:?}",
            right[0]
        );
        assert!(right[0].starts_with("5h  "), "{:?}", right[0]);
        assert!(right[0].ends_with("    9%"), "{:?}", right[0]);
        assert_eq!(right[1], "    no rate yet");
        assert!(right[2].ends_with("   95%"), "{:?}", right[2]);
        assert_eq!(right[3], "    +5.0 pts/h · runs out in 12h00m (regression)");
        let last = lines.last().unwrap();
        assert_eq!(
            right_of(last).unwrap_or(""),
            "7 days  [ ] range",
            "on the body's last row"
        );
    }

    #[test]
    fn a_projection_too_wide_for_the_panel_wraps_under_the_sparkline() {
        // Spec §5.1: 80×24 is enough; spec §5.4: each window's rate and projection. At 80
        // columns the panel is 53 wide, and a long model label leaves 34 for the projection.
        let mut state = on_history(many(1));
        state.history.select(id(1));
        let sonnet = "Claude Sonnet 4.5";
        let w = window(
            &format!("scoped:{sonnet}"),
            sonnet,
            WindowKind::Scoped,
            95.0,
            None,
        );
        let pace = Pace {
            rate_per_hour: Some(5.0),
            method: Some(ProjectionMethod::Regression),
            exhaustion_at: Some(NOW_S + 43_200),
            will_last_to_reset: Some(false),
            ..Pace::default()
        };
        state.history.apply(history_of(
            account(1, "a1@x.co", read(60, 1.0, 95.0, false)),
            vec![history_window(w, vec![sample(HOUR, 95.0)], pace)],
        ));
        let right = |width: u16| -> Vec<String> {
            body_of(&state, width, 24, NOW_MS)[0..4]
                .iter()
                .map(|l| right_of(l).unwrap_or("").trim_end().to_owned())
                .collect()
        };
        let under = " ".repeat(19);
        let narrow = right(80);
        assert!(narrow[0].starts_with("Claude Sonnet 4.5  "), "{narrow:?}");
        assert!(narrow[0].ends_with("  95%"), "{narrow:?}");
        assert_eq!(
            narrow[1..],
            [
                format!("{under}+5.0 pts/h"),
                format!("{under}runs out in 12h00m (regression)"),
                String::new(),
            ]
        );
        let wide = right(120);
        assert_eq!(
            wide[1..3],
            [
                format!("{under}+5.0 pts/h · runs out in 12h00m (regression)"),
                String::new(),
            ]
        );
    }

    #[test]
    fn an_oversized_label_goes_above_its_sparkline_and_the_column_stays_capped() {
        // An 80-column model name: the label column is a third of the 53-column panel at most,
        // so the sparkline, the latest pct and the projection keep room.
        let mut state = on_history(many(1));
        state.history.select(id(1));
        let long =
            "Claude Opus 4.5 with extended thinking, for long-running agentic coding sessions";
        let pace = Pace {
            rate_per_hour: Some(5.0),
            method: Some(ProjectionMethod::Regression),
            exhaustion_at: Some(NOW_S + 43_200),
            will_last_to_reset: Some(false),
            ..Pace::default()
        };
        let five = window("5h", "5h", WindowKind::Short, 9.0, None);
        let wide = window(
            &format!("scoped:{long}"),
            long,
            WindowKind::Scoped,
            95.0,
            None,
        );
        state.history.apply(history_of(
            account(1, "a1@x.co", read(60, 9.0, 95.0, false)),
            vec![
                history_window(five, vec![sample(HOUR, 9.0)], Pace::default()),
                history_window(wide, vec![sample(HOUR, 95.0)], pace),
            ],
        ));
        let right: Vec<String> = body_of(&state, 80, 24, NOW_MS)[0..7]
            .iter()
            .map(|l| right_of(l).unwrap_or("").trim_end().to_owned())
            .collect();
        assert!(
            right[0].starts_with(&format!("{:<17}  ", "5h")),
            "{right:?}"
        );
        assert!(right[0].ends_with("    9%"), "{right:?}");
        assert_eq!(right[1], format!("{:19}no rate yet", ""));
        assert_eq!([right[2].as_str(), right[3].as_str()].join(" "), long);
        assert!(right[4].starts_with(&" ".repeat(19)), "{right:?}");
        assert!(right[4].ends_with("   95%"), "{right:?}");
        assert_eq!(
            right[5..7],
            [
                format!("{:19}+5.0 pts/h", ""),
                format!("{:19}runs out in 12h00m (regression)", ""),
            ]
        );
    }

    #[test]
    fn a_1000_character_label_leaves_its_window_reachable_at_80x24_under_a_full_banner() {
        // A label takes two rows at most; under a 3-row banner and a 3-row toast the panel has
        // 16 rows, and a window at most five (R20).
        let mut state = on_history(many(1));
        state.history.select(id(1));
        let long = "Claude ".repeat(143).trim_end().to_owned();
        let pace = Pace {
            rate_per_hour: Some(5.0),
            method: Some(ProjectionMethod::Regression),
            exhaustion_at: Some(NOW_S + 43_200),
            will_last_to_reset: Some(false),
            ..Pace::default()
        };
        let five = window("5h", "5h", WindowKind::Short, 9.0, None);
        let wide = window(
            &format!("scoped:{long}"),
            &long,
            WindowKind::Scoped,
            95.0,
            None,
        );
        state.history.apply(history_of(
            account(1, "a1@x.co", read(60, 9.0, 95.0, false)),
            vec![
                history_window(five, vec![sample(HOUR, 9.0)], Pace::default()),
                history_window(wide, vec![sample(HOUR, 95.0)], pace),
            ],
        ));
        state.raise_banner(
            BannerKind::Refusal,
            ["the store could not be read"; 12].join("; "),
        );
        state.toast(["Switched to a@x.co (position 1)"; 8].join(" "), NOW_MS);
        let right: Vec<String> = body_of(&state, 80, 24, NOW_MS)[0..16]
            .iter()
            .map(|l| right_of(l).unwrap_or("").trim_end().to_owned())
            .collect();
        let label = right
            .iter()
            .position(|r| r.starts_with("Claude Claude"))
            .unwrap_or_else(|| panic!("{right:?}"));
        assert!(
            right[label + 1].ends_with('…'),
            "two rows at most: {right:?}"
        );
        assert!(right[label + 2].ends_with("   95%"), "{right:?}");
        assert_eq!(
            right[label + 3..label + 5],
            [
                format!("{:19}+5.0 pts/h", ""),
                format!("{:19}runs out in 12h00m (regression)", ""),
            ]
        );
    }

    #[test]
    fn a_resize_clamps_the_window_scroll_so_the_next_key_moves_at_once() {
        let mut state = on_history(many(1));
        state.history.select(id(1));
        let windows = (1..=20)
            .map(|i| {
                let (key, label) = (format!("scoped:M{i}"), format!("M{i}"));
                let w = window(&key, &label, WindowKind::Scoped, 10.0, None);
                history_window(w, vec![], Pace::default())
            })
            .collect();
        state.history.apply(history_of(
            account(1, "a1@x.co", read(60, 1.0, 1.0, false)),
            windows,
        ));
        // 80×25 and 80×32 give the body the rows 80×24 and 80×30 gave it before the auto line
        // and the rule. The panel's footer is the body's last row.
        state.size = (80, 25);
        let footer = |state: &State, h: u16| {
            right_of(body_of(state, 80, h, NOW_MS).last().unwrap())
                .unwrap()
                .to_owned()
        };
        for _ in 0..20 {
            update(&mut state, ch('J'), NOW_MS);
        }
        assert!(footer(&state, 25).ends_with("J K: windows 11–20 of 20"));
        // Taller: the last window shows from further up, and `K` moves from what shows.
        update(
            &mut state,
            Msg::Resize {
                width: 80,
                height: 32,
            },
            NOW_MS,
        );
        assert!(footer(&state, 32).ends_with("J K: windows 8–20 of 20"));
        update(&mut state, ch('K'), NOW_MS);
        assert!(
            footer(&state, 32).ends_with("J K: windows 7–19 of 20"),
            "{}",
            footer(&state, 32)
        );
    }

    #[test]
    fn windows_beyond_the_screen_scroll_into_view_with_shift_j_and_k() {
        // Spec §5.4: the account's windows, every one. 80×25 holds ten of two lines each, as
        // 80×24 did before the auto line.
        let mut state = on_history(many(2));
        state.size = (80, 25);
        state.history.select(id(1));
        let twelve = |position| {
            let windows = (1..=12)
                .map(|i| {
                    let (key, label) = (format!("scoped:M{i}"), format!("M{i}"));
                    let w = window(&key, &label, WindowKind::Scoped, 10.0, None);
                    history_window(w, vec![], Pace::default())
                })
                .collect();
            let email = format!("a{position}@x.co");
            history_of(
                account(position, &email, read(60, 1.0, 1.0, false)),
                windows,
            )
        };
        state.history.apply(twelve(1));
        let labels = |state: &State| -> Vec<String> {
            body_of(state, 80, 25, NOW_MS)[0..21]
                .iter()
                .filter_map(|l| right_of(l))
                .filter(|r| r.starts_with('M'))
                .map(|r| r.split_whitespace().next().unwrap().to_owned())
                .collect()
        };
        let footer = |state: &State| {
            right_of(&body_of(state, 80, 25, NOW_MS)[21])
                .unwrap()
                .to_owned()
        };
        let first: Vec<String> = (1..=10).map(|i| format!("M{i}")).collect();
        assert_eq!(labels(&state), first);
        assert_eq!(footer(&state), "7 days  [ ] range  J K: windows 1–10 of 12");
        for _ in 0..5 {
            assert!(
                update(&mut state, ch('J'), NOW_MS).is_empty(),
                "nothing is read"
            );
        }
        let last: Vec<String> = (3..=12).map(|i| format!("M{i}")).collect();
        assert_eq!(labels(&state), last, "J stops at the last window");
        assert_eq!(footer(&state), "7 days  [ ] range  J K: windows 3–12 of 12");
        update(&mut state, ch('K'), NOW_MS);
        assert_eq!(labels(&state)[0], "M2");
        // Another account's windows start at its first.
        update(&mut state, ch('j'), NOW_MS);
        state.history.apply(twelve(2));
        assert_eq!(labels(&state), first);
        // A taller terminal shows them all: 42 rows give the body the 38 that 40 gave it.
        assert!(right_of(&body_of(&state, 80, 42, NOW_MS)[37]).unwrap() == "7 days  [ ] range");
        // Help lists the keys.
        update(&mut state, ch('?'), NOW_MS);
        let help = draw(&state, 80, 24);
        assert!(
            help.iter()
                .any(|l| l.contains("J K") && l.contains("scroll the windows")),
            "{help:?}"
        );
    }

    #[test]
    fn a_bucket_shows_its_peak_and_an_empty_one_stays_blank() {
        let since = NOW_S - 10 * HOUR;
        // Ten buckets of an hour: a peak in the third, nothing in the fourth, a reset in the
        // eighth.
        let samples = [
            Sample {
                fetched_at: since + 2 * HOUR + 10,
                pct: 20.0,
                resets_at: None,
            },
            Sample {
                fetched_at: since + 2 * HOUR + 20,
                pct: 100.0,
                resets_at: None,
            },
            Sample {
                fetched_at: since + 2 * HOUR + 30,
                pct: 40.0,
                resets_at: None,
            },
            Sample {
                fetched_at: since + 6 * HOUR,
                pct: 90.0,
                resets_at: None,
            },
            Sample {
                fetched_at: since + 7 * HOUR,
                pct: 0.0,
                resets_at: None,
            },
            Sample {
                fetched_at: NOW_S,
                pct: 14.3,
                resets_at: None,
            },
            Sample {
                fetched_at: since - 1,
                pct: 100.0,
                resets_at: None,
            },
        ];
        assert_eq!(sparkline(&samples, since, NOW_S, 10), "  █   ▇▁ ▂");
        assert_eq!(sparkline(&[], since, NOW_S, 4), "    ");
        assert_eq!(sparkline(&samples, since, NOW_S, 0), "");
    }

    #[test]
    fn the_latest_pct_takes_its_severity() {
        let mut state = on_history(many(1));
        state.theme = Theme::new(true);
        state.history.select(id(1));
        state
            .history
            .apply(view_of(account(1, "a1@x.co", read(60, 9.0, 95.0, false))));
        let buf = draw_buffer(&state, 80, 24, NOW_MS);
        let (x, y) = locate(&buf, "95%").unwrap();
        assert_eq!(buf[(x, y)].fg, Color::Red);
        state.theme = Theme::new(false);
        let buf = draw_buffer(&state, 80, 24, NOW_MS);
        assert!(buf[(x, y)].modifier.contains(Modifier::BOLD));
        assert!(buf[(x, y)].modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn several_providers_are_grouped_and_a_long_list_keeps_the_shown_account_in_view() {
        let mut other = account(1, "o@x.co", read(60, 1.0, 1.0, false));
        other.row.provider = tagteam_core::ProviderId::new("fake-agent");
        other.row.id = AccountId::from_string("other-1");
        let mut snap = snapshot(many(1));
        snap.providers
            .push(tagteam_engine::views::ProviderAccounts {
                provider: tagteam_core::ProviderId::new("fake-agent"),
                active_position: None,
                accounts: vec![other],
                threshold: 90.0,
                strategy: tagteam_core::autoswitch::Strategy::Best,
                names: tagteam_engine::views::ProviderNames {
                    display_name: "FakeAgent",
                    launch_command: "fakeagent",
                },
            });
        let mut state = fresh(Screen::History);
        state.accounts.apply(snap);
        // 80×25: the 22 body rows 80×24 gave before the auto line.
        let left = |state: &State| -> Vec<String> {
            body_of(state, 80, 25, NOW_MS)[0..22]
                .iter()
                .map(|l| l.chars().take(26).collect::<String>().trim_end().to_owned())
                .collect()
        };
        assert_eq!(
            &left(&state)[..5],
            [
                " ACCOUNTS",
                " claude-code",
                "   1 a1@x.co",
                " fake-agent",
                "   1 o@x.co"
            ]
        );
        let mut long = on_history(many(40));
        long.history.select(id(35));
        let shown = left(&long);
        assert_eq!(shown[0], " ACCOUNTS");
        assert_eq!(shown.last().unwrap(), "▌  35 a35@x.co", "{shown:?}");
    }

    #[test]
    fn both_panels_say_loading_until_the_first_accounts_read() {
        let mut state = fresh(Screen::History);
        let lines = body_of(&state, 80, 24, NOW_MS);
        assert_eq!(lines[0].trim_end(), " ACCOUNTS                  loading…");
        assert_eq!(lines[1], " loading…");
        state.accounts.apply(snapshot(vec![]));
        let lines = body_of(&state, 80, 24, NOW_MS);
        assert!(lines[0].ends_with(NO_HISTORY), "{lines:?}");
        assert_eq!(lines[1], " No accounts yet.");
    }

    #[test]
    fn the_empty_states_say_so() {
        let state = on_history(vec![]);
        let lines = body_of(&state, 80, 24, NOW_MS);
        assert_eq!(
            lines[0].trim_end(),
            " ACCOUNTS                  No usage history."
        );
        assert_eq!(lines[1], " No accounts yet.");
        let mut read = on_history(many(1));
        read.history.select(id(1));
        assert!(body_of(&read, 80, 24, NOW_MS)[0].ends_with(LOADING));
        read.history.apply(history_of(
            account(1, "a1@x.co", unread(UsageStatus::Unavailable, None, None)),
            vec![],
        ));
        assert!(body_of(&read, 80, 24, NOW_MS)[0].ends_with(NO_HISTORY));
    }

    #[test]
    fn an_accounts_read_moves_history_off_an_account_that_is_gone() {
        let mut state = on_history(many(2));
        let week = NOW_S - 7 * 86_400;
        assert_eq!(
            on_accounts(&mut state, NOW_MS),
            [Cmd::LoadHistory {
                r#gen: 1,
                account: id(1),
                since_s: week,
            }],
            "none chosen yet: the first"
        );
        state
            .history
            .apply(view_of(account(1, "a1@x.co", read(60, 1.0, 1.0, false))));
        assert!(
            on_accounts(&mut state, NOW_MS).is_empty(),
            "still there, already read"
        );
        state.history.select(id(2));
        state
            .history
            .apply(view_of(account(2, "a2@x.co", read(60, 1.0, 1.0, false))));
        state.accounts.apply(snapshot(many(1)));
        assert_eq!(
            on_accounts(&mut state, NOW_MS),
            [Cmd::LoadHistory {
                r#gen: 2,
                account: id(1),
                since_s: week,
            }]
        );
        assert_eq!(state.history.account(), Some(&id(1)));
        state.accounts.apply(snapshot(vec![]));
        assert!(on_accounts(&mut state, NOW_MS).is_empty());
        assert_eq!(state.history.account(), None);
        state.screen = Screen::Accounts;
        state.accounts.apply(snapshot(many(1)));
        assert!(
            on_accounts(&mut state, NOW_MS).is_empty(),
            "only while History is open"
        );
    }

    #[test]
    fn no_accounts_drops_a_history_read_still_in_flight() {
        let mut state = on_history(many(1));
        on_accounts(&mut state, NOW_MS);
        let before = state.gens.history;
        state.accounts.apply(snapshot(vec![]));
        assert!(on_accounts(&mut state, NOW_MS).is_empty());
        assert_eq!(
            state.gens.history,
            before + 1,
            "the read in flight is stale"
        );
    }

    #[test]
    fn reload_views_reads_the_accounts_and_on_history_the_account_shown() {
        let mut state = fresh(Screen::Accounts);
        let g = state.gens.accounts;
        assert_eq!(
            state.reload_views(NOW_MS),
            [Cmd::LoadAccounts { r#gen: g + 1 }]
        );

        let mut state = on_history(many(2));
        state.history.select(id(2));
        let (g, h) = (state.gens.accounts, state.gens.history);
        assert_eq!(
            state.reload_views(NOW_MS),
            [
                Cmd::LoadAccounts { r#gen: g + 1 },
                Cmd::LoadHistory {
                    r#gen: h + 1,
                    account: id(2),
                    since_s: NOW_S - 7 * 86_400,
                },
            ]
        );

        let mut state = fresh(Screen::History);
        let g = state.gens.accounts;
        assert_eq!(
            state.reload_views(NOW_MS),
            [Cmd::LoadAccounts { r#gen: g + 1 }],
            "no account shown yet"
        );
    }
}
