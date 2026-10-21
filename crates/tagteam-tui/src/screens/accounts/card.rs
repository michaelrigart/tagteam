//! The selected account's card (redesign §6.4, T3-20, T3-22): its title and facts, a row per
//! window with a sparkline from the card's history read, its problem and remedy, and its keys.
//! The lead card (§6.2) reuses its problem rows and `lead_keys`.

use ratatui::text::{Line, Span};
use tagteam_core::{Pace, Sample, Window, WindowKind};
use tagteam_engine::views::{AccountView, HistoryWindow, UsageStatus};
use tagteam_engine::words;

use super::status;
use super::table::age_now;
use crate::autoline;
use crate::chrome::fit_spans;
use crate::gauge::sparkline;
use crate::state::State;
use crate::theme::{Role, Theme};
use crate::view::{fit, pad, width_of, wrap_rows};

/// The card's history read waits this long after the selection, or its reading, changes
/// (T3-21).
pub(crate) const CARD_DEBOUNCE_MS: i64 = 250;
/// How far back the card's history read reaches: its long windows' sparklines span it (§6.4).
pub(crate) const CARD_SINCE_S: i64 = 7 * 86_400;
/// A short window's sparkline spans the last day (§6.4).
const SHORT_SINCE_S: i64 = 86_400;
/// The width from which the sparkline takes `SPARK_WIDE` cells, `▲` its words, the projection
/// its rate, the title the org, and the card its keys row (§4.5, §6.4, T3-20).
const WIDE_FROM: u16 = 120;
const SPARK_WIDE: usize = 32;
const SPARK: usize = 16;
/// Every row of the card starts this far in.
const INDENT: &str = "    ";
/// Between a row's parts (§4.3).
const GAP: &str = "  ";
/// Between the title's name and its facts.
const TITLE_GAP: &str = "    ";
/// Between the keys row's keys.
const KEYS_GAP: &str = "   ";
const FACT_SEP: &str = " · ";
/// The rows a problem's sentence, and its remedy, take at most.
const PROBLEM_ROWS: usize = 2;
/// The label column's least width; it takes a quarter of the card's at most (T3-20).
const LABEL_LEAST: usize = 4;

/// An account's windows, as its reading lists them; none without a reading.
fn windows_of(v: &AccountView) -> &[(Window, Pace)] {
    v.usage.windows.as_deref().unwrap_or_default()
}

/// The card's lines at `width`, its windows from `from` on, in at most `rows` (T3-20, T3-22).
/// When `rows` is less than `min_height`, it returns `min_height` lines all the same: the
/// title, one window, the problem rows and the keys row are never dropped. When not every
/// window fits and two rows or more are left for them, the last of those says which show and
/// how to scroll (`J K: windows 3–9 of 24`, muted). `on_lead` names the windows the lead card
/// shows for the account in use (T3-32): those rows leave out their reset and projection. It
/// is empty for any other account.
pub(crate) fn lines(
    state: &State,
    v: &AccountView,
    width: u16,
    rows: usize,
    from: usize,
    on_lead: &[String],
    now_ms: i64,
) -> Vec<Line<'static>> {
    let problems = problem_rows(state, v, width, now_ms);
    let keys = keys_row(v, width, state.theme);
    let n = windows_of(v).len();
    let shown = Shown::of(rows, 1 + problems.len() + keys.len(), n);
    let from = from.min(n - shown.windows);
    let mut lines = vec![title(state, v, width, now_ms)];
    lines.extend(window_rows(
        state,
        v,
        width,
        from..from + shown.windows,
        on_lead,
        now_ms,
    ));
    if shown.hint {
        let (a, b) = (from + 1, from + shown.windows);
        lines.push(Line::from(vec![
            Span::raw(INDENT),
            Span::styled(
                fit(
                    &format!("J K: windows {a}–{b} of {n}"),
                    usize::from(width).saturating_sub(INDENT.len()),
                ),
                state.theme.role(Role::Muted),
            ),
        ]));
    }
    lines.extend(problems);
    lines.extend(keys);
    lines
}

/// The first window `J` scrolls the card to at most, when `rows` lines hold it: from there on
/// every window shows, the last one included.
pub(super) fn furthest(
    state: &State,
    v: &AccountView,
    width: u16,
    rows: usize,
    now_ms: i64,
) -> usize {
    let n = windows_of(v).len();
    let fixed = min_height(state, v, width, now_ms) - usize::from(n > 0);
    n.saturating_sub(Shown::of(rows, fixed, n).windows)
}

/// What the card's window rows show in `rows` lines beside its `fixed` others.
struct Shown {
    /// How many windows: one at least when it has any, as T3-22 reserves.
    windows: usize,
    /// Whether the last window row says which windows show instead (`J K: windows a–b of n`):
    /// when not every window fits and two rows or more are left for them.
    hint: bool,
}

impl Shown {
    fn of(rows: usize, fixed: usize, windows: usize) -> Shown {
        let room = rows.saturating_sub(fixed).max(usize::from(windows > 0));
        let hint = windows > room && room >= 2;
        Shown {
            windows: (room - usize::from(hint)).min(windows),
            hint,
        }
    }
}

/// §6.2's windows for the lead card at `width`, before any fold, by key: the first short and
/// the first long below 120 columns, every window from 120. Until Task 13 draws the lead
/// card, `render` passes these as `on_lead`; Task 13 picks its bars from them.
pub(crate) fn lead_keys(v: &AccountView, width: u16) -> Vec<String> {
    let windows = windows_of(v);
    if width >= WIDE_FROM {
        return windows.iter().map(|(w, _)| w.key.clone()).collect();
    }
    [WindowKind::Short, WindowKind::Long]
        .into_iter()
        .filter_map(|kind| windows.iter().find(|(w, _)| w.kind == kind))
        .map(|(w, _)| w.key.clone())
        .collect()
}

/// The card's lines at `width` with every window: what the layout gives it when it fits.
pub(crate) fn height(state: &State, v: &AccountView, width: u16, now_ms: i64) -> usize {
    min_height(state, v, width, now_ms) + windows_of(v).len().saturating_sub(1)
}

/// The fewest lines the card is drawn in (T3-22): its title, one window row when it has
/// windows, its problem rows and its keys row.
pub(crate) fn min_height(state: &State, v: &AccountView, width: u16, now_ms: i64) -> usize {
    1 + usize::from(!windows_of(v).is_empty())
        + problem_rows(state, v, width, now_ms).len()
        + usize::from(width >= WIDE_FROM)
}

/// A problem and its remedy as rows (§6.2, §6.4): `✕ Needs a new login: …` in its role, the
/// remedy two columns further in, in text. Each wraps at its words, in `PROBLEM_ROWS` rows at
/// most, so that what to do is never cut at 80 columns.
pub(crate) fn problem_rows(
    state: &State,
    v: &AccountView,
    width: u16,
    now_ms: i64,
) -> Vec<Line<'static>> {
    let Some(problem) = words::problem(v) else {
        return vec![];
    };
    let width = usize::from(width);
    let when = match problem {
        words::Problem::NeedsLogin => v
            .row
            .quarantine_at
            .map(|at_ms| autoline::when(at_ms, now_ms, state.tokens.tz())),
        _ => None,
    };
    let sentence = words::problem_sentence(problem, v, when.as_deref());
    let (glyph, role) = status::mark(problem);
    let (head, under) = match glyph {
        Some(glyph) => (format!("{glyph} "), format!("{INDENT}{GAP}")),
        None => (String::new(), INDENT.to_owned()),
    };
    let style = state.theme.role(role);
    let room = width.saturating_sub(width_of(&under));
    let mut rows: Vec<Line<'static>> = wrap_rows(&sentence, room, PROBLEM_ROWS)
        .into_iter()
        .enumerate()
        .map(|(i, text)| {
            let lead = if i == 0 {
                vec![Span::raw(INDENT), Span::styled(head.clone(), style)]
            } else {
                vec![Span::raw(under.clone())]
            };
            Line::from([lead, vec![Span::styled(text, style)]].concat())
        })
        .collect();
    let names = state.names_of(&v.row.provider);
    let now_s = now_ms.div_euclid(1000);
    if let Some(remedy) = words::remedy(problem, v, &names, now_s) {
        let under = format!("{INDENT}{GAP}");
        let room = width.saturating_sub(width_of(&under));
        rows.extend(
            wrap_rows(&remedy, room, PROBLEM_ROWS)
                .into_iter()
                .map(|text| Line::from(vec![Span::raw(under.clone()), Span::raw(text)])),
        );
    }
    rows
}

/// The title (T3-20): `    {pos} {name}[ · {org}]    {facts}`, the name cut before the facts.
fn title(state: &State, v: &AccountView, width: u16, now_ms: i64) -> Line<'static> {
    let theme = state.theme;
    let muted = theme.role(Role::Muted);
    let row = &v.row;
    let email = words::email(row);
    let mut name = match &row.alias {
        Some(alias) => vec![
            Span::styled(alias.clone(), theme.strong()),
            Span::raw(GAP),
            Span::styled(email, muted),
        ],
        None => vec![Span::styled(email, theme.strong())],
    };
    if let Some(org) = row.org_name.as_deref().filter(|_| width >= WIDE_FROM) {
        name.push(Span::styled(format!("{FACT_SEP}{org}"), muted));
    }
    let mut facts: Vec<Span<'static>> = Vec::new();
    for (i, (text, role)) in facts_of(state, v, now_ms).into_iter().enumerate() {
        if i > 0 {
            facts.push(Span::styled(FACT_SEP, muted));
        }
        facts.push(Span::styled(text, theme.role(role)));
    }
    let head = format!("{INDENT}{} ", row.position);
    let facts_w: usize = facts.iter().map(|s| width_of(&s.content)).sum();
    let room = usize::from(width)
        .saturating_sub(width_of(&head) + TITLE_GAP.len() + facts_w)
        .max(1);
    let mut spans = vec![Span::raw(head)];
    spans.extend(fit_spans(name, room));
    spans.push(Span::raw(TITLE_GAP));
    spans.extend(facts);
    Line::from(fit_spans(spans, usize::from(width)))
}

/// The title's facts and their roles (T3-20): muted, the reading's age warn when it is old.
fn facts_of(state: &State, v: &AccountView, now_ms: i64) -> Vec<(String, Role)> {
    let now_s = now_ms.div_euclid(1000);
    let (row, u) = (&v.row, &v.usage);
    let snap = state.accounts.snapshot();
    let mut facts = vec![if v.active { "in use" } else { "not in use" }.to_owned()];
    facts.extend(v.kind.display.map(str::to_owned));
    if v.in_session {
        facts.push("in a session".to_owned());
    }
    if snap.and_then(|s| s.this.as_ref()) == Some(&row.id) {
        facts.push(format!("{} this session", words::IN_SESSION));
    }
    if row.disabled {
        facts.push("disabled".to_owned());
    }
    let mut facts: Vec<(String, Role)> = facts.into_iter().map(|f| (f, Role::Muted)).collect();
    match u.age_s {
        Some(age) => {
            let read_at_s = snap.map_or(now_s, |s| s.read_at_s);
            let old = !windows_of(v).is_empty() && !u.decision_grade;
            let role = if old { Role::Warn } else { Role::Muted };
            facts.push((words::updated(age_now(age, read_at_s, now_s)), role));
        }
        None => facts.push((words::NOT_CHECKED.to_owned(), Role::Muted)),
    }
    if let Some(at) = u.next_poll_at.filter(|_| u.status == UsageStatus::Ok) {
        facts.push((words::next_update(at, now_s), Role::Muted));
    }
    if let Some(at_ms) = row.login_expires_at {
        facts.push((words::login_valid(at_ms, now_s), Role::Muted));
    }
    facts
}

/// `v`'s windows in `shown` as a row each (T3-20); the label column fits every window's.
fn window_rows(
    state: &State,
    v: &AccountView,
    width: u16,
    shown: std::ops::Range<usize>,
    on_lead: &[String],
    now_ms: i64,
) -> Vec<Line<'static>> {
    let windows = windows_of(v);
    let label_w = windows
        .iter()
        .map(|(w, _)| width_of(&w.label))
        .max()
        .unwrap_or(0)
        .max(LABEL_LEAST)
        .min(usize::from(width) / 4);
    let card = state
        .accounts
        .card
        .as_ref()
        .filter(|c| c.account.row.id == v.row.id);
    windows[shown.start.min(windows.len())..shown.end.min(windows.len())]
        .iter()
        .map(|(w, p)| {
            let samples = card
                .and_then(|c| c.windows.iter().find(|hw| hw.window.key == w.key))
                .map_or(&[][..], |hw| hw.samples.as_slice());
            let row = Row {
                window: w,
                pace: p,
                samples,
                label_w,
                width,
                on_lead: on_lead.contains(&w.key),
            };
            row.line(now_ms.div_euclid(1000), state.theme)
        })
        .collect()
}

/// One window's row and what it is drawn from.
struct Row<'a> {
    window: &'a Window,
    pace: &'a Pace,
    samples: &'a [Sample],
    label_w: usize,
    width: u16,
    /// The lead card shows this window: no reset and no projection (T3-32).
    on_lead: bool,
}

impl Row<'_> {
    /// `    {label}  {spark} {tag}  {pct:>4}[ ▲]  {reset}[ · {projection}]` (T3-20), cut to
    /// the width. From 120 columns `▲` takes its words when the row still fits.
    fn line(&self, now_s: i64, theme: Theme) -> Line<'static> {
        let width = usize::from(self.width);
        let (mut spans, mark) = self.spans(now_s, theme);
        if let Some(at) = mark.filter(|_| self.width >= WIDE_FROM) {
            let words = words::ahead_words(self.pace.expected_pct);
            let drawn: usize = spans.iter().map(|s| width_of(&s.content)).sum();
            if drawn - width_of(AHEAD) + width_of(&words) <= width {
                spans[at].content = words.into();
                return Line::from(spans);
            }
        }
        Line::from(fit_spans(spans, width))
    }

    /// The row's spans with `▲` alone, and which span `▲` is when the window is ahead.
    fn spans(&self, now_s: i64, theme: Theme) -> (Vec<Span<'static>>, Option<usize>) {
        let (w, p) = (self.window, self.pace);
        let wide = self.width >= WIDE_FROM;
        let (since, tag) = match w.kind {
            WindowKind::Short => (SHORT_SINCE_S, "24h"),
            WindowKind::Long | WindowKind::Scoped | WindowKind::Spend => (CARD_SINCE_S, "7d"),
        };
        let cells = if wide { SPARK_WIDE } else { SPARK };
        let pct = w.pct.round() as i64;
        let value = words::spend_text(w).unwrap_or_else(|| format!("{:>4}", format!("{pct}%")));
        let mut spans = vec![
            Span::raw(format!("{INDENT}{}{GAP}", pad(&w.label, self.label_w))),
            sparkline(self.samples, now_s - since, now_s + 1, cells, theme),
            Span::raw(" "),
            Span::styled(format!("{tag:<3}"), theme.role(Role::Faint)),
            Span::raw(GAP),
            Span::styled(value, theme.severity(words::severity(pct))),
        ];
        let mut mark = None;
        if p.ahead == Some(true) {
            spans.push(Span::raw(" "));
            mark = Some(spans.len());
            spans.push(Span::styled(AHEAD, theme.role(Role::Warn)));
        }
        if self.on_lead {
            return (spans, mark);
        }
        let reset = match (w.resets_at, w.kind) {
            (Some(at), _) if at > now_s => {
                Some(format!("resets in {}", words::countdown(at, now_s)))
            }
            (Some(at), _) => Some(words::countdown(at, now_s)),
            (None, WindowKind::Short | WindowKind::Long) => Some(NOT_STARTED.to_owned()),
            (None, WindowKind::Scoped | WindowKind::Spend) => None,
        };
        let projection = p.rate_per_hour.map(|_| {
            let hw = HistoryWindow {
                window: w.clone(),
                samples: vec![],
                pace: *p,
            };
            let text = if wide {
                words::projection_text(&hw, now_s)
            } else {
                words::projection_clause(&hw, now_s, true)
            };
            let role = if p.will_last_to_reset == Some(false) {
                Role::Warn
            } else {
                Role::Text
            };
            Span::styled(text, theme.role(role))
        });
        if let Some(reset) = &reset {
            spans.push(Span::raw(GAP));
            spans.push(Span::raw(reset.clone()));
        }
        if let Some(projection) = projection {
            spans.push(Span::raw(if reset.is_some() { FACT_SEP } else { GAP }));
            spans.push(projection);
        }
        (spans, mark)
    }
}

/// The ahead-of-pace mark on its own, below 120 columns or where its words do not fit.
const AHEAD: &str = "▲";
/// A short or long window that has no reset yet (T3-20).
const NOT_STARTED: &str = "not started";

/// The keys for this account (§6.4), from 120 columns: `⏎ history of spare   e disable   a
/// alias   x remove`, the keys in accent and their labels muted.
fn keys_row(v: &AccountView, width: u16, theme: Theme) -> Vec<Line<'static>> {
    if width < WIDE_FROM {
        return vec![];
    }
    let email = words::email(&v.row);
    let name = v
        .row
        .alias
        .clone()
        .unwrap_or_else(|| email.split('@').next().unwrap_or(&email).to_owned());
    let toggle = if v.row.disabled { "enable" } else { "disable" };
    let keys = [
        ("⏎", format!("history of {name}")),
        ("e", toggle.to_owned()),
        ("a", "alias".to_owned()),
        ("x", "remove".to_owned()),
    ];
    let mut spans = vec![Span::raw(INDENT)];
    for (i, (key, label)) in keys.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(KEYS_GAP));
        }
        spans.push(Span::styled(key, theme.role(Role::Accent)));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(label, theme.role(Role::Muted)));
    }
    vec![Line::from(fit_spans(spans, usize::from(width)))]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_windows_give_their_last_row_to_the_hint_when_two_rows_are_left() {
        let shown = |rows, fixed, n| {
            let s = Shown::of(rows, fixed, n);
            (s.windows, s.hint)
        };
        // Everything fits: every window, no hint.
        assert_eq!(shown(10, 3, 7), (7, false));
        assert_eq!(shown(10, 3, 0), (0, false));
        // Two rows or more left and windows hidden: one fewer window, and the hint.
        assert_eq!(shown(10, 3, 24), (6, true));
        assert_eq!(shown(5, 3, 24), (1, true));
        // One row left, or none: the window T3-22 reserves, and no hint.
        assert_eq!(shown(4, 3, 24), (1, false));
        assert_eq!(shown(2, 3, 24), (1, false));
    }
}
