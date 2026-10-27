//! The selected account's card (redesign §6.4, S§3.4): its title, its facts, a row per window
//! (thick bars for the account in use), its problem and remedy, and its keys; and the window
//! row, the facts row and the problem rows the full list shares (S§4.1).

use ratatui::text::{Line, Span};
use tagteam_core::{Pace, Window, WindowKind};
use tagteam_engine::store::AccountRow;
use tagteam_engine::views::{AccountView, HistoryWindow, UsageStatus};
use tagteam_engine::words;

use super::status::{self, IN_USE_WORDS, THIS_SESSION};
use super::table::age_now;
use crate::autoline;
use crate::chrome::{WIDE, fit_spans};
use crate::gauge::{lead_bar, line_gauge};
use crate::state::State;
use crate::theme::{Role, Theme};
use crate::view::{fit, pad, width_of, wrap_rows};

/// The bar's cells below `WIDE` columns, and from it (S§4.1).
const BAR: usize = 16;
const BAR_WIDE: usize = 26;
/// The card's window rows start this far in; its facts and keys rows, under the position.
const CARD_INDENT: usize = 2;
const FACTS_INDENT: usize = 10;
/// Between a row's parts (§4.3).
pub(super) const GAP: &str = "  ";
/// Between the keys row's keys.
const KEYS_GAP: &str = "   ";
pub(super) const FACT_SEP: &str = " · ";
/// The rows a problem's sentence, and its remedy, take at most.
const PROBLEM_ROWS: usize = 2;
/// The label column's least width; it takes a quarter of the card's at most (T3-20).
const LABEL_LEAST: usize = 4;
/// A name keeps this many columns, or its whole width when narrower, before what follows it is
/// cut: T3-15's least for the table's name.
const NAME_LEAST: usize = 16;
/// A percentage is bold from here (T3-18).
const BOLD_FROM: i64 = 70;
/// A login valid for less than this is warn, and says to log in again (S§3.4, T3-10).
const SOON_S: i64 = 7 * 86_400;
const LOG_IN_AGAIN: &str = "log in again before then";

/// An account's windows, as its reading lists them; none without a reading.
pub(super) fn windows_of(v: &AccountView) -> &[(Window, Pace)] {
    v.usage.windows.as_deref().unwrap_or_default()
}

/// The card's lines at `width`, its windows from `from` on, in at most `rows` (T3-20, T3-22), and
/// its keys row when `keys`. When `rows` is less than `min_height`, it returns `min_height` lines
/// all the same: the title, the facts, one window and the problem rows are never dropped. When
/// not every window fits and two rows or more are left for them, the last of those says which
/// show and how to scroll (`J K: windows 3–9 of 24`, muted).
pub(crate) fn lines(
    state: &State,
    v: &AccountView,
    width: u16,
    rows: usize,
    from: usize,
    keys: bool,
    now_ms: i64,
) -> Vec<Line<'static>> {
    let theme = state.theme;
    let now_s = now_ms.div_euclid(1000);
    let windows = windows_of(v);
    let n = windows.len();
    let problems = problem_rows(state, v, CARD_INDENT, width, now_ms);
    let none = no_windows_row(v, CARD_INDENT, width, now_s, theme);
    let fixed = 2 + problems.len() + usize::from(none.is_some()) + usize::from(keys);
    let shown = Shown::of(rows, fixed, n);
    let from = from.min(n - shown.windows);
    let at = row_layout(windows.iter().map(|(w, _)| w), CARD_INDENT, width);
    let mut lines = vec![
        title(state, v, width),
        facts_row(state, v, FACTS_INDENT, width, now_ms),
    ];
    lines.extend(
        windows[from..from + shown.windows]
            .iter()
            .map(|(w, p)| window_line(w, p, &at, v.active, now_s, theme)),
    );
    if shown.hint {
        let (a, b) = (from + 1, from + shown.windows);
        lines.push(Line::from(vec![
            Span::raw(" ".repeat(CARD_INDENT)),
            Span::styled(
                fit(
                    &format!("J K: windows {a}–{b} of {n}"),
                    usize::from(width).saturating_sub(CARD_INDENT),
                ),
                theme.role(Role::Muted),
            ),
        ]));
    }
    lines.extend(none);
    lines.extend(problems);
    if keys {
        lines.push(keys_row(v, width, theme));
    }
    lines
}

/// The first window `J` scrolls the card to at most, when `rows` lines hold it (and its keys row
/// when `keys`): from there on every window shows, the last one included.
pub(super) fn furthest(
    state: &State,
    v: &AccountView,
    width: u16,
    rows: usize,
    keys: bool,
    now_ms: i64,
) -> usize {
    let n = windows_of(v).len();
    let fixed = min_height(state, v, width, now_ms) - usize::from(n > 0) + usize::from(keys);
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

/// `v`'s first short and first long window, in that order: the Now strip's (§5.4).
pub(crate) fn short_and_long(v: &AccountView) -> impl Iterator<Item = &(Window, Pace)> {
    let windows = windows_of(v);
    [WindowKind::Short, WindowKind::Long]
        .into_iter()
        .filter_map(move |kind| windows.iter().find(|(w, _)| w.kind == kind))
}

/// The card's lines at `width` with every window and its keys row: what the layout gives it when
/// it fits.
pub(crate) fn height(state: &State, v: &AccountView, width: u16, now_ms: i64) -> usize {
    min_height(state, v, width, now_ms) + windows_of(v).len().saturating_sub(1) + 1
}

/// The fewest lines the card is drawn in (T3-22, S§5.4): its title, its facts, one window row
/// (or the row that says why it has none, when no problem does) and its problem rows. Its keys
/// row goes before the rules do.
pub(crate) fn min_height(state: &State, v: &AccountView, width: u16, now_ms: i64) -> usize {
    let first = !windows_of(v).is_empty() || words::problem(v).is_none();
    2 + usize::from(first) + problem_rows(state, v, CARD_INDENT, width, now_ms).len()
}

/// A problem and its remedy as rows (§6.2, §6.4): `✕ Needs a new login: …` in its role from
/// `indent`, the remedy two columns further in, in text. Each wraps at its words, in
/// `PROBLEM_ROWS` rows at most, so that what to do is never cut at 80 columns.
pub(crate) fn problem_rows(
    state: &State,
    v: &AccountView,
    indent: usize,
    width: u16,
    now_ms: i64,
) -> Vec<Line<'static>> {
    let Some(problem) = words::problem(v) else {
        return vec![];
    };
    let width = usize::from(width);
    let indent = " ".repeat(indent);
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
        Some(glyph) => (format!("{glyph} "), format!("{indent}{GAP}")),
        None => (String::new(), indent.clone()),
    };
    let style = state.theme.role(role);
    let room = width.saturating_sub(width_of(&under));
    let mut rows: Vec<Line<'static>> = wrap_rows(&sentence, room, PROBLEM_ROWS)
        .into_iter()
        .enumerate()
        .map(|(i, text)| {
            let lead = if i == 0 {
                vec![Span::raw(indent.clone()), Span::styled(head.clone(), style)]
            } else {
                vec![Span::raw(under.clone())]
            };
            Line::from([lead, vec![Span::styled(text, style)]].concat())
        })
        .collect();
    let names = state.names_of(&v.row.provider);
    let now_s = now_ms.div_euclid(1000);
    if let Some(remedy) = words::remedy(problem, v, &names, now_s) {
        let under = format!("{indent}{GAP}");
        let room = width.saturating_sub(width_of(&under));
        rows.extend(
            wrap_rows(&remedy, room, PROBLEM_ROWS)
                .into_iter()
                .map(|text| Line::from(vec![Span::raw(under.clone()), Span::raw(text)])),
        );
    }
    rows
}

/// The title (S§3.4): `SELECTED  {pos} {name}[ · {org}]`, cut to the width.
fn title(state: &State, v: &AccountView, width: u16) -> Line<'static> {
    let theme = state.theme;
    let mut spans = vec![
        Span::styled("SELECTED", theme.role(Role::Muted)),
        Span::raw(format!("{GAP}{} ", v.row.position)),
    ];
    spans.extend(name_spans(&v.row, width, true, theme));
    Line::from(fit_spans(spans, usize::from(width)))
}

/// A card's name (§6.2, §6.4): the alias bold, two spaces and the email muted, or the email
/// bold (`strong`, else plain); the org muted from 120 columns.
pub(super) fn name_spans(
    row: &AccountRow,
    width: u16,
    strong: bool,
    theme: Theme,
) -> Vec<Span<'static>> {
    let muted = theme.role(Role::Muted);
    let email = words::email(row);
    let main = |text: String| {
        if strong {
            Span::styled(text, theme.strong())
        } else {
            Span::raw(text)
        }
    };
    let mut name = match &row.alias {
        Some(alias) => vec![
            main(alias.clone()),
            Span::raw(GAP),
            Span::styled(email, muted),
        ],
        None => vec![main(email)],
    };
    if let Some(org) = row.org_name.as_deref().filter(|_| width >= WIDE) {
        name.push(Span::styled(format!("{FACT_SEP}{org}"), muted));
    }
    name
}

/// A card's facts, each in its role, joined by a muted ` · `.
fn fact_spans(facts: Vec<(String, Role)>, theme: Theme) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (i, (text, role)) in facts.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(FACT_SEP, theme.role(Role::Muted)));
        }
        spans.push(Span::styled(text, theme.role(role)));
    }
    spans
}

fn spans_width(spans: &[Span<'static>]) -> usize {
    spans.iter().map(|s| width_of(&s.content)).sum()
}

/// The fewest columns `floored` draws `head`, `name`, `gap` and `rest` in whole, the name cut to
/// its floor.
pub(crate) fn floored_least(
    head: &[Span<'static>],
    name: &[Span<'static>],
    gap: &str,
    rest: &[Span<'static>],
) -> usize {
    let rest_w = spans_width(rest);
    let gap_w = if rest_w > 0 { width_of(gap) } else { 0 };
    spans_width(head) + spans_width(name).min(NAME_LEAST) + gap_w + rest_w
}

/// `{head}{name}{gap}{rest}` in `room` columns, the shared rule of a name beside what follows it
/// (the Now strip). The name gives way first, down to `NAME_LEAST` columns (its whole width when
/// narrower); then `rest` is cut, from its end. Only when no room is left for it does it go, and
/// the name below its floor.
pub(crate) fn floored(
    head: Vec<Span<'static>>,
    name: Vec<Span<'static>>,
    gap: &'static str,
    rest: Vec<Span<'static>>,
    room: usize,
) -> Vec<Span<'static>> {
    let (head_w, name_w, rest_w) = (spans_width(&head), spans_width(&name), spans_width(&rest));
    let floor = name_w.min(NAME_LEAST);
    let after_head = room.saturating_sub(head_w);
    let gap_w = width_of(gap);
    let name_room = after_head.saturating_sub(gap_w + rest_w);
    let mut spans = head;
    if rest.is_empty() {
        spans.extend(fit_spans(name, after_head));
    } else if name_room >= floor {
        spans.extend(fit_spans(name, name_room));
        spans.push(Span::raw(gap));
        spans.extend(rest);
    } else {
        let rest_room = after_head.saturating_sub(floor + gap_w);
        if rest_room > 0 {
            spans.extend(fit_spans(name, floor));
            spans.push(Span::raw(gap));
            spans.extend(fit_spans(rest, rest_room));
        } else {
            spans.extend(fit_spans(name, after_head));
        }
    }
    fit_spans(spans, room)
}

/// A window rows' label column (T3-19, T3-20): the widest label, at least `LABEL_LEAST`, and a
/// quarter of `width` at most, where a longer one is cut.
fn label_width<'a>(windows: impl IntoIterator<Item = &'a Window>, width: u16) -> usize {
    windows
        .into_iter()
        .map(|w| width_of(&w.label))
        .max()
        .unwrap_or(0)
        .max(LABEL_LEAST)
        .min(usize::from(width) / 4)
}

/// The facts row, from `indent` (S§3.4): the facts, each in its role, cut at the end.
pub(super) fn facts_row(
    state: &State,
    v: &AccountView,
    indent: usize,
    width: u16,
    now_ms: i64,
) -> Line<'static> {
    let mut spans = vec![Span::raw(" ".repeat(indent))];
    spans.extend(fact_spans(facts_of(state, v, width, now_ms), state.theme));
    Line::from(fit_spans(spans, usize::from(width)))
}

/// The facts and their roles (S§3.4): `IN USE` accent, the rest muted, the reading's age warn
/// when it is old, the login warn when it is soon over; `next update` last, from `WIDE`.
fn facts_of(state: &State, v: &AccountView, width: u16, now_ms: i64) -> Vec<(String, Role)> {
    let now_s = now_ms.div_euclid(1000);
    let (row, u) = (&v.row, &v.usage);
    let snap = state.accounts.snapshot();
    let mut facts = vec![if v.active {
        (IN_USE_WORDS.to_uppercase(), Role::Accent)
    } else {
        (format!("not {IN_USE_WORDS}"), Role::Muted)
    }];
    let mut muted: Vec<String> = v.kind.display.map(str::to_owned).into_iter().collect();
    if v.in_session {
        muted.push("in a session".to_owned());
    }
    if snap.and_then(|s| s.this.as_ref()) == Some(&row.id) {
        muted.push(format!("{} {THIS_SESSION}", words::IN_SESSION));
    }
    if row.disabled {
        muted.push("disabled".to_owned());
    }
    facts.extend(muted.into_iter().map(|f| (f, Role::Muted)));
    match u.age_s {
        Some(age) => {
            let read_at_s = snap.map_or(now_s, |s| s.read_at_s);
            let old = !windows_of(v).is_empty() && !u.decision_grade;
            let role = if old { Role::Warn } else { Role::Muted };
            facts.push((words::updated(age_now(age, read_at_s, now_s)), role));
        }
        // Only an account never checked waits for a first reading: an API key has none to
        // take, and a problem says why there is none.
        None if words::problem(v) == Some(words::Problem::NotChecked) => {
            facts.push((words::NOT_CHECKED.to_owned(), Role::Muted));
        }
        None => {}
    }
    if let Some(at_ms) = row.login_expires_at {
        // Under `SOON_S` left it is warn and says to log in again (S§3.4); once passed it is
        // `login expired`, warn, alone.
        let left = at_ms.div_euclid(1000).saturating_sub(now_s);
        let login = words::login_valid(at_ms, now_s);
        let role = if left < SOON_S {
            Role::Warn
        } else {
            Role::Muted
        };
        facts.push((login, role));
        if (1..SOON_S).contains(&left) {
            facts.push((LOG_IN_AGAIN.to_owned(), Role::Warn));
        }
    }
    let next = u
        .next_poll_at
        .filter(|_| u.status == UsageStatus::Ok && width >= WIDE);
    if let Some(at) = next {
        facts.push((words::next_update(at, now_s), Role::Muted));
    }
    facts
}

/// What every window row on a screen shares (S§4.1).
pub(super) struct RowLayout {
    /// 2 in the card, 7 in the full list.
    pub(super) indent: usize,
    /// `label_width` over every window on the screen, plus two.
    pub(super) label_w: usize,
    /// 4, or the widest spend value on the screen.
    pub(super) value_w: usize,
    pub(super) width: u16,
}

/// The layout of the window rows of `windows`, every one on the screen.
pub(super) fn row_layout<'a>(
    windows: impl IntoIterator<Item = &'a Window>,
    indent: usize,
    width: u16,
) -> RowLayout {
    let windows: Vec<&Window> = windows.into_iter().collect();
    let value_w = windows
        .iter()
        .filter_map(|w| words::spend_text(w))
        .map(|text| width_of(&text))
        .fold(4, usize::max);
    RowLayout {
        indent,
        label_w: label_width(windows.iter().copied(), width) + GAP.len(),
        value_w,
        width,
    }
}

/// One window's row (S§4.1): `{indent}{label}{bar} {value} {▲ or space}  {tail}`, cut to the
/// width. The bar is thick for the account in use. `▲` shows on a long or scoped window ahead of
/// pace (C-8), and its tail ends with `{NN}% expected by now` when the whole row then fits.
pub(super) fn window_line(
    w: &Window,
    p: &Pace,
    at: &RowLayout,
    thick: bool,
    now_s: i64,
    theme: Theme,
) -> Line<'static> {
    let width = usize::from(at.width);
    let wide = at.width >= WIDE;
    let cells = if wide { BAR_WIDE } else { BAR };
    let label = pad(&w.label, at.label_w.saturating_sub(GAP.len()));
    let mut spans = vec![Span::raw(format!("{}{label}{GAP}", " ".repeat(at.indent)))];
    spans.extend(if thick {
        lead_bar(w.pct, cells, theme)
    } else {
        line_gauge(w.pct, cells, theme)
    });
    let mut value = value_cell(w, theme);
    value.content = format!("{:>1$}", value.content, at.value_w).into();
    if w.pct.round() as i64 >= BOLD_FROM {
        value.style = value.style.patch(theme.strong());
    }
    let ahead = p.ahead == Some(true) && matches!(w.kind, WindowKind::Long | WindowKind::Scoped);
    spans.extend([
        Span::raw(" "),
        value,
        Span::raw(" "),
        if ahead {
            Span::styled(AHEAD_MARK, theme.role(Role::Warn))
        } else {
            Span::raw(" ")
        },
    ]);
    let mut tail = tail(w, p, wide, now_s, theme);
    if let Some(expected) = p.expected_pct.filter(|_| ahead) {
        let words = words::expected_words(expected);
        let drawn = spans_width(&spans) + GAP.len() + tail_width(&tail);
        let sep = if tail.is_empty() {
            0
        } else {
            width_of(FACT_SEP)
        };
        if drawn + sep + width_of(&words) <= width {
            tail.push(Span::raw(words));
        }
    }
    for (i, part) in tail.into_iter().enumerate() {
        spans.push(Span::raw(if i == 0 { GAP } else { FACT_SEP }));
        spans.push(part);
    }
    Line::from(fit_spans(spans, width))
}

/// The columns `parts` take once joined by `FACT_SEP`.
fn tail_width(parts: &[Span<'static>]) -> usize {
    spans_width(parts) + parts.len().saturating_sub(1) * width_of(FACT_SEP)
}

/// The parts a window's row ends with (S§4.1), to be joined by ` · `. The reset is `resets in
/// {span}`, `reset` once passed, or `not started` for a short or long window without one. The
/// rate and the projection show only with a rate (`projection_clause` says `not enough data yet`
/// without one): the projection as its last clause below 120 columns, and in warn when the window
/// fills before it resets.
fn tail(w: &Window, p: &Pace, wide: bool, now_s: i64, theme: Theme) -> Vec<Span<'static>> {
    let reset = match (w.resets_at, w.kind) {
        (Some(at), _) if at > now_s => Some(format!("resets in {}", words::countdown(at, now_s))),
        (Some(at), _) => Some(words::countdown(at, now_s)),
        (None, WindowKind::Short | WindowKind::Long) => Some(NOT_STARTED.to_owned()),
        (None, WindowKind::Scoped | WindowKind::Spend) => None,
    };
    let mut parts: Vec<Span<'static>> = reset.into_iter().map(Span::raw).collect();
    if let Some(rate) = p.rate_per_hour {
        let hw = HistoryWindow {
            window: w.clone(),
            samples: vec![],
            pace: *p,
        };
        let clause = words::projection_clause(&hw, now_s, !wide);
        let role = if p.will_last_to_reset == Some(false) {
            Role::Warn
        } else {
            Role::Text
        };
        parts.push(Span::styled(
            format!("{}%/h{FACT_SEP}{clause}", words::rate_signed(rate)),
            theme.role(role),
        ));
    }
    parts
}

/// The ahead-of-pace mark.
pub(super) const AHEAD_MARK: &str = "▲";
/// A window's value cell, in its severity's role (T3-19, T3-20): its spend (`€12.40 of €50`),
/// else its percentage right-aligned in four columns.
pub(crate) fn value_cell(w: &Window, theme: Theme) -> Span<'static> {
    let pct = w.pct.round() as i64;
    let value = words::spend_text(w).unwrap_or_else(|| format!("{:>4}", format!("{pct}%")));
    Span::styled(value, theme.severity(words::severity(pct)))
}
/// A short or long window that has no reset yet (T3-20).
const NOT_STARTED: &str = "not started";

/// An account with no windows and no problem (an API key, an unsupported provider, an empty
/// list), from `indent`: the words the table puts across its window columns, in their role
/// (S§3.4). One with a problem says it in `problem_rows`.
pub(super) fn no_windows_row(
    v: &AccountView,
    indent: usize,
    width: u16,
    now_s: i64,
    theme: Theme,
) -> Option<Line<'static>> {
    if !windows_of(v).is_empty() || words::problem(v).is_some() {
        return None;
    }
    let (text, role) = status::unread_words(v, now_s);
    let spans = vec![
        Span::raw(" ".repeat(indent)),
        Span::styled(text, theme.role(role)),
    ];
    Some(Line::from(fit_spans(spans, usize::from(width))))
}

/// The keys for this account (§6.4, S§3.4), from `FACTS_INDENT`: `s switch   ⏎ history   e
/// disable   a alias   x remove`, the keys in accent and their labels muted. `s` is left out for
/// the account in use.
fn keys_row(v: &AccountView, width: u16, theme: Theme) -> Line<'static> {
    let toggle = if v.row.disabled { "enable" } else { "disable" };
    let keys = [
        ("s", "switch", !v.active),
        ("⏎", "history", true),
        ("e", toggle, true),
        ("a", "alias", true),
        ("x", "remove", true),
    ];
    let mut spans = vec![Span::raw(" ".repeat(FACTS_INDENT))];
    for (i, (key, label, _)) in keys.into_iter().filter(|k| k.2).enumerate() {
        if i > 0 {
            spans.push(Span::raw(KEYS_GAP));
        }
        spans.push(Span::styled(key, theme.role(Role::Accent)));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(label, theme.role(Role::Muted)));
    }
    Line::from(fit_spans(spans, usize::from(width)))
}

#[cfg(test)]
mod tests {
    use ratatui::style::Modifier;
    use serde_json::json;

    use super::*;
    use crate::testutil::*;
    use crate::view::width_of;

    fn line_text(line: Line<'static>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// The display column `needle` starts at in `line`.
    fn col(line: &str, needle: &str) -> usize {
        let at = line
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} in {line:?}"));
        width_of(&line[..at])
    }

    fn ahead(expected: f64, rate: f64) -> Pace {
        Pace {
            ahead: Some(true),
            expected_pct: Some(expected),
            rate_per_hour: Some(rate),
            will_last_to_reset: Some(false),
            exhaustion_at: Some(NOW_S + 28_020),
            ..Pace::default()
        }
    }

    /// The card's layout at `width`: indent 2, the label column 7 wide, values in 4.
    fn layout(width: u16) -> RowLayout {
        RowLayout {
            indent: 2,
            label_w: 7,
            value_w: 4,
            width,
        }
    }

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

    #[test]
    fn a_window_row_has_its_parts_at_fixed_columns() {
        let long = window(
            "7d",
            "7d",
            WindowKind::Long,
            82.0,
            Some(5 * 86_400 + 4 * 3_600),
        );
        let at = layout(80);
        let theme = Theme::new(false);
        let text = line_text(window_line(
            &long,
            &ahead(26.0, 2.3),
            &at,
            true,
            NOW_S,
            theme,
        ));
        // indent 2 + label 7 + 16 cells + 1, then ` 82%` right-aligned in 4
        assert_eq!(&text[..9], "  7d     ");
        assert_eq!(
            (
                col(&text, "82%"),
                col(&text, "▲"),
                col(&text, "resets in 5d04h")
            ),
            (27, 31, 34)
        );
        // The rate is kept at 80.
        assert!(
            text.contains("resets in 5d04h · +2.3%/h · full in 7h47m"),
            "{text}"
        );
        // 75 + 22 > 80.
        assert!(!text.contains("expected by now"));
        let wide = layout(120);
        let text = line_text(window_line(
            &long,
            &ahead(26.0, 2.3),
            &wide,
            true,
            NOW_S,
            theme,
        ));
        // 26 cells.
        assert_eq!(col(&text, "82%"), 37);
        assert!(
            text.ends_with("+2.3%/h · full in 7h47m · 26% expected by now"),
            "{text}"
        );
    }

    #[test]
    fn pace_words_show_at_80_when_they_fit_and_never_on_short_or_spend_windows() {
        let theme = Theme::new(false);
        let row =
            |w: &Window, p: Pace| line_text(window_line(w, &p, &layout(80), true, NOW_S, theme));
        let reset = Some(5 * 86_400 + 4 * 3_600);
        let long = window("7d", "7d", WindowKind::Long, 82.0, reset);
        // No rate: `  7d     {bar}  82% ▲  resets in 5d04h · 26% expected by now`, 71 columns.
        let no_rate = Pace {
            rate_per_hour: None,
            ..ahead(26.0, 0.0)
        };
        let text = row(&long, no_rate);
        assert!(
            text.ends_with("82% ▲  resets in 5d04h · 26% expected by now"),
            "{text}"
        );
        assert_eq!(width_of(&text), 71);
        assert!(!text.contains("not enough data yet"), "{text}");
        // With a rate the row would reach 97 columns, so the words go and the rate stays.
        let text = row(&long, ahead(26.0, 2.3));
        assert!(text.contains("· +2.3%/h · full in 7h47m"), "{text}");
        assert!(!text.contains("expected"), "{text}");
        // The same pace on a short window, and on a spend one: no `▲`, no words.
        let short = window("5h", "5h", WindowKind::Short, 82.0, reset);
        let mut spend = window("spend", "Spend", WindowKind::Spend, 82.0, reset);
        spend.detail = Some(json!({"currency": "EUR", "used": 41.0, "limit": 50.0}));
        for w in [&short, &spend] {
            let text = row(w, no_rate);
            assert!(!text.contains('▲') && !text.contains("expected"), "{text}");
        }
    }

    #[test]
    fn a_window_without_a_reset_starts_its_tail_with_its_rate() {
        let theme = Theme::new(false);
        let scoped = window("scoped:Fable", "Fable", WindowKind::Scoped, 28.0, None);
        let pace = Pace {
            rate_per_hour: Some(0.9),
            will_last_to_reset: Some(false),
            exhaustion_at: Some(NOW_S + 3 * 86_400 + 8 * 3_600),
            ..Pace::default()
        };
        let text = line_text(window_line(
            &scoped,
            &pace,
            &layout(80),
            false,
            NOW_S,
            theme,
        ));
        // The reset's column, with no ` · ` before the rate.
        assert_eq!(col(&text, "+0.9%/h"), 34, "{text}");
        assert!(text.ends_with("  +0.9%/h · full in 3d08h"), "{text}");
        let text = line_text(window_line(
            &scoped,
            &Pace::default(),
            &layout(80),
            false,
            NOW_S,
            theme,
        ));
        assert_eq!(width_of(text.trim_end()), 30, "{text}");
        assert!(text.trim_end().ends_with("28%"), "{text}");
    }

    #[test]
    fn bars_are_thick_for_the_account_in_use_and_thin_otherwise() {
        let w = window("7d", "7d", WindowKind::Long, 54.0, Some(3_600));
        for theme in TIERS {
            for (thick, cells) in [(true, "▆▁"), (false, "━╸─·")] {
                let text = line_text(window_line(
                    &w,
                    &Pace::default(),
                    &layout(80),
                    thick,
                    NOW_S,
                    theme,
                ));
                let bar: String = text.chars().skip(9).take(16).collect();
                assert!(
                    bar.chars().all(|c| cells.contains(c)) && bar.chars().any(|c| c != '▁'),
                    "{theme:?} {thick}: {text}"
                );
            }
        }
    }

    #[test]
    fn a_percentage_is_bold_from_70() {
        for theme in TIERS {
            for thick in [true, false] {
                for (pct, bold) in [(69.0, false), (70.0, true)] {
                    let w = window("7d", "7d", WindowKind::Long, pct, Some(3_600));
                    let line = window_line(&w, &Pace::default(), &layout(80), thick, NOW_S, theme);
                    let span = line
                        .spans
                        .iter()
                        .find(|s| s.content.trim() == format!("{pct}%"))
                        .unwrap_or_else(|| panic!("{pct}% in {line:?}"));
                    assert_eq!(
                        span.style.add_modifier.contains(Modifier::BOLD),
                        bold,
                        "{theme:?} {thick} {pct}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_spend_value_widens_the_value_column_for_every_row() {
        let theme = Theme::new(false);
        let five = window("5h", "5h", WindowKind::Short, 44.0, Some(3_600));
        let mut spend = window("spend", "Spend", WindowKind::Spend, 25.0, Some(7_200));
        spend.detail = Some(json!({"currency": "EUR", "used": 12.4, "limit": 50.0}));
        let at = row_layout([&five, &spend], 2, 80);
        assert_eq!(at.value_w, 13);
        let row = |w| line_text(window_line(w, &Pace::default(), &at, false, NOW_S, theme));
        let (a, b) = (row(&five), row(&spend));
        assert_eq!(col(&a, "44%") + 3, col(&b, "€50") + 3, "{a}\n{b}");
        assert_eq!(col(&a, "resets in"), col(&b, "resets in"), "{a}\n{b}");
    }

    #[test]
    fn the_label_column_is_capped_at_a_quarter_of_the_width() {
        let theme = Theme::new(false);
        let w = window("k", &"x".repeat(40), WindowKind::Scoped, 28.0, None);
        let at = row_layout([&w], 2, 80);
        assert_eq!(at.label_w, 22);
        let text = line_text(window_line(&w, &Pace::default(), &at, false, NOW_S, theme));
        assert!(
            text.starts_with(&format!("  {}…  ", "x".repeat(19))),
            "{text}"
        );
        assert!(text.contains("28%"), "{text}");
    }
}
