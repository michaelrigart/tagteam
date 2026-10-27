//! The rows around the body (redesign §5.1): the header (§5.2), the auto line (§5.3), the Now
//! strip on every screen (§5.4), the faint rule under them from 30 rows on Tokens and History
//! (§4.3), the banner (§5.6), and the footer's keys (§5.5).

use ratatui::text::{Line, Span};
use tagteam_core::WindowKind;
use tagteam_engine::views::AccountView;
use tagteam_engine::words::{self, Problem};

use crate::autoline;
use crate::daemon;
use crate::daemon::words::auto_state;
use crate::gauge::line_gauge;
use crate::keys;
use crate::screens;
use crate::screens::accounts::{card, lead, status, table};
use crate::state::{BannerKind, Screen, State, Toast, ToastKind};
use crate::theme::{Role, Theme};
use crate::view::{WRAP_ROWS, fit, width_of, wrap, wrap_rows};

/// From this many rows a faint rule sits under the chrome (§4.5).
pub(crate) const RULE_FROM: u16 = 30;
/// From this many columns the long forms take over (§4.5): the header's and the auto line's,
/// the Now strip's gauges, and the Accounts screen's wide gauges and bars, org, next-update fact
/// and long projection words.
pub(crate) const WIDE: u16 = 120;

const TABS: [(Screen, &str); 3] = [
    (Screen::Accounts, "1 Accounts"),
    (Screen::Tokens, "2 Tokens"),
    (Screen::History, "3 History"),
];

/// What the banner says while the input thread drops every key (spec §4.2, T2-14).
const KEYS_HELD: &str =
    "the terminal sent an unfinished colour report · keys resume when it ends · Ctrl-C quits";

/// ` tagteam   1 Accounts   2 Tokens   3 History` on the left: the brand bold, the tabs muted,
/// the current one accent and bold. On the right, auto's state (`auto_state`), three spaces,
/// and `? keys` one column in. The state keeps a column from the tabs: a long form too wide for
/// the room the rest leaves gives way to the short one, and only what still does not fit is
/// cut. `? keys` is never dropped.
pub(crate) fn header(state: &State, width: u16) -> Line<'static> {
    let theme = state.theme;
    let mut spans = vec![Span::styled(" tagteam", theme.strong())];
    for (screen, label) in TABS {
        let style = if screen == state.screen {
            theme.role(Role::Accent).patch(theme.strong())
        } else {
            theme.role(Role::Muted)
        };
        spans.push(Span::raw("   "));
        spans.push(Span::styled(label, style));
    }
    let keys = vec![
        Span::styled("?", theme.role(Role::Accent)),
        Span::styled(" keys", theme.role(Role::Muted)),
        Span::raw(" "),
    ];
    let width = usize::from(width);
    let room = width.saturating_sub(spans_width(&spans) + 1 + 3 + spans_width(&keys));
    let wide = width >= usize::from(WIDE);
    let mut pieces = auto_state(state, wide);
    if wide && pieces.iter().map(|(text, _)| width_of(text)).sum::<usize>() > room {
        pieces = auto_state(state, false);
    }
    let mut right = Vec::new();
    if !pieces.is_empty() {
        let pieces = pieces
            .into_iter()
            .map(|(text, role)| Span::styled(text, theme.role(role)))
            .collect();
        right = fit_spans(pieces, room);
        right.push(Span::raw("   "));
    }
    right.extend(keys);
    let gap = width.saturating_sub(spans_width(&spans) + spans_width(&right));
    spans.push(Span::raw(" ".repeat(gap)));
    spans.extend(right);
    Line::from(spans)
}

/// The auto line (§5.3): ` {mark} {when}  {text}`, with no `when` ` {mark} {text}`, then
/// ` · next check HH:MM`; the mark in its role, the time, rank 3's `last switch … · ` and the
/// suffix muted. On the right, ending one column in, `w why`, `w` accent. What gives way, in
/// order: the sentence is cut; then the hint goes, when it would leave the rest under
/// `lead::LEFT_LEAST` columns (T3-19's rule); then `next check`, which is never cut. Empty
/// before the first snapshot: the row stays.
pub(crate) fn auto_line(state: &State, width: u16, now_ms: i64) -> Line<'static> {
    let theme = state.theme;
    let Some(said) = autoline::said(state, width, now_ms) else {
        return Line::default();
    };
    let mut spans = vec![Span::raw(" ")];
    spans.push(match said.mark {
        Some((mark, role)) => Span::styled(mark, theme.role(role)),
        None => Span::raw(" "),
    });
    spans.push(Span::raw(" "));
    if let Some(when) = said.when {
        spans.push(Span::styled(when, theme.role(Role::Muted)));
        spans.push(Span::raw("  "));
    }
    if let Some(lead) = said.lead {
        spans.push(Span::styled(lead, theme.role(Role::Muted)));
    }
    // The sentence needs room for at least its `…` beside `next check`.
    let least = spans_width(&spans) + 1;
    let text = if said.muted { Role::Muted } else { Role::Text };
    spans.push(Span::styled(said.text, theme.role(text)));
    let next = said
        .next
        .map(|next| Span::styled(format!(" · {next}"), theme.role(Role::Muted)));
    let next_w = next.as_ref().map_or(0, |n| width_of(&n.content));
    let keeps_next = |room: usize| next.is_none() || least + next_w <= room;
    let left = |room: usize| match &next {
        Some(next) if keeps_next(room) => {
            let mut left = fit_spans(spans.clone(), room - next_w);
            left.push(next.clone());
            left
        }
        _ => fit_spans(spans.clone(), room),
    };
    let width = usize::from(width);
    let hint = vec![
        Span::styled("w", theme.role(Role::Accent)),
        Span::styled(" why", theme.role(Role::Muted)),
    ];
    let hint_w = spans_width(&hint);
    let Some(room) = width
        .checked_sub(hint_w + lead::PICK_GAP + keys::RIGHT_INSET)
        .filter(|&room| room >= lead::LEFT_LEAST && keeps_next(room))
    else {
        return Line::from(left(width));
    };
    let mut spans = left(room);
    let gap = width - keys::RIGHT_INSET - hint_w - spans_width(&spans);
    spans.push(Span::raw(" ".repeat(gap)));
    spans.extend(hint);
    Line::from(spans)
}

/// §5.4's Now strip for focus's provider (T3-24), on every screen.
/// ` ● {pos} {alias, else email}   {label} [{gauge} ]{pct:>4} {span}   …` for its account in
/// use, its first short and first long window, the gauge from `WIDE` columns; a problem in their
/// place as `{glyph} {status words}`, then ` · see Accounts` off Accounts, in its role; with no
/// account in use `NO_ACCOUNT_IN_USE` and how to switch to one, muted. On the right, one column in, `lead::pick_words`' Now strip form.
/// The name gives way first, down to its floor (`card::floored`); then the pick is dropped,
/// rather than cut the windows or the problem, or the hint with no account in use; and only then
/// are those cut, from their end. Empty before the first snapshot and with no accounts.
pub(crate) fn now_strip(state: &State, width: u16, now_ms: i64) -> Line<'static> {
    let theme = state.theme;
    let Some(view) = state
        .view_of(&daemon::focus(state))
        .filter(|view| !view.accounts.is_empty())
    else {
        return Line::default();
    };
    let (least, left): (usize, Box<dyn Fn(usize) -> Vec<Span<'static>>>) = match lead::in_use(state)
    {
        Some((_, v)) => {
            let (head, name, rest) = in_use_parts(v, state.screen, width, now_ms, theme);
            let least = card::floored_least(&head, &name, NOW_GAP, &rest).max(lead::LEFT_LEAST);
            let left =
                move |room| card::floored(head.clone(), name.clone(), NOW_GAP, rest.clone(), room);
            (least, Box::new(left))
        }
        None => {
            let how = match state.screen {
                Screen::Accounts => lead::NO_ACCOUNT_HINT,
                Screen::Tokens | Screen::History => SWITCH_ON_ACCOUNTS,
            };
            let hint = vec![
                Span::raw(" "),
                Span::styled(format!("{NO_ACCOUNT_IN_USE}{how}"), theme.role(Role::Muted)),
            ];
            let least = spans_width(&hint);
            (least, Box::new(move |room| fit_spans(hint.clone(), room)))
        }
    };
    let width = usize::from(width);
    let pick = lead::pick_words(state, view).unwrap_or_default();
    let pick_w = spans_width(&pick);
    let room = width.checked_sub(pick_w + lead::PICK_GAP + keys::RIGHT_INSET);
    let Some(room) = room.filter(|&room| pick_w > 0 && room >= least) else {
        return Line::from(left(width));
    };
    let mut spans = left(room);
    let gap = width - keys::RIGHT_INSET - pick_w - spans_width(&spans);
    spans.push(Span::raw(" ".repeat(gap)));
    spans.extend(pick);
    Line::from(spans)
}

/// What the Now strip says with no account in use (§5.4), and how to switch to one: on
/// Accounts `lead::NO_ACCOUNT_HINT`, elsewhere `SWITCH_ON_ACCOUNTS`.
const NO_ACCOUNT_IN_USE: &str = "no account in use · ";
const SWITCH_ON_ACCOUNTS: &str = "s on Accounts switches to one";
/// What follows a problem on the Now strip, where its card is.
const SEE_ACCOUNTS: &str = " · see Accounts";
/// The Now strip's gauge cells, from `WIDE` columns (§5.4).
const NOW_GAUGE: usize = 10;
/// Between the Now strip's name and windows, and between its windows (§4.3's group gap).
const NOW_GAP: &str = "   ";

/// The parts of the Now strip's account in use (T3-24): ` ● {pos} `, its name, and its windows
/// or its problem, the gap before them left to `card::floored`. An old reading keeps its
/// windows: they are still what was read, and its status words say nothing of its age.
type NowParts = (Vec<Span<'static>>, Vec<Span<'static>>, Vec<Span<'static>>);

fn in_use_parts(
    v: &AccountView,
    screen: Screen,
    width: u16,
    now_ms: i64,
    theme: Theme,
) -> NowParts {
    let head = vec![
        Span::raw(" "),
        Span::styled(table::IN_USE, theme.role(Role::Accent)),
        Span::raw(format!(" {} ", v.row.position)),
    ];
    let name = vec![Span::styled(
        v.row.alias.clone().unwrap_or_else(|| words::email(&v.row)),
        theme.strong(),
    )];
    let mut spans = Vec::new();
    if let Some(problem) = words::problem(v).filter(|p| *p != Problem::OldReading) {
        let (glyph, role) = status::mark(problem);
        let said = words::status_words(&v.usage);
        let said = match glyph {
            Some(glyph) => format!("{glyph} {said}"),
            None => said,
        };
        spans.push(Span::styled(said, theme.role(role)));
        // Its card and the table's STATUS are on the same screen on Accounts.
        if screen != Screen::Accounts {
            spans.push(Span::styled(SEE_ACCOUNTS, theme.role(Role::Muted)));
        }
        return (head, name, spans);
    }
    let now_s = now_ms.div_euclid(1000);
    for (i, (w, p)) in card::short_and_long(v).enumerate() {
        if i > 0 {
            spans.push(Span::raw(NOW_GAP));
        }
        spans.push(Span::styled(w.label.clone(), theme.role(Role::Muted)));
        spans.push(Span::raw(" "));
        if width >= WIDE {
            spans.extend(line_gauge(w.pct, NOW_GAUGE, theme));
            spans.push(Span::raw(" "));
        }
        spans.push(card::value_cell(w, theme));
        // C-8: only a long or scoped window is ahead of pace.
        if p.ahead == Some(true) && matches!(w.kind, WindowKind::Long | WindowKind::Scoped) {
            spans.push(Span::raw(" "));
            spans.push(Span::styled("▲", theme.role(Role::Warn)));
        }
        if let Some(at) = w.resets_at {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                words::countdown(at, now_s),
                theme.role(Role::Muted),
            ));
        }
    }
    (head, name, spans)
}

/// `─` across `width`, faint: under the chrome from `RULE_FROM` rows on Tokens and History
/// (§4.3), and around the Accounts table (§5.2).
pub(crate) fn rule(state: &State, width: u16) -> Line<'static> {
    Line::from(Span::styled(
        "─".repeat(usize::from(width)),
        state.theme.role(Role::Faint),
    ))
}

/// A toast wrapped beside the right part takes at least this many columns; with fewer the
/// spinner shrinks to its glyph, and if that still leaves fewer the toast takes the full width
/// and the right part is dropped (§5.5).
const TOAST_MIN: usize = 40;

/// What sits right of a toast's last row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Beside {
    /// The spinner with its label (when one runs) and the pinned two.
    Full,
    /// The spinner's glyph alone, and the pinned two.
    Glyph,
    /// Nothing: the toast takes the full width.
    Nothing,
}

/// A toast's rows, and what sits right of the last.
pub(crate) struct ToastRows {
    pub(crate) rows: Vec<String>,
    pub(crate) right: Beside,
}

/// One footer item: its key in accent and its label muted.
fn item(theme: Theme, key: &str, label: &str) -> [Span<'static>; 2] {
    [
        Span::styled(key.to_owned(), theme.role(Role::Accent)),
        Span::styled(format!(" {label}"), theme.role(Role::Muted)),
    ]
}

/// The pinned `? keys  q quit`.
fn pinned_spans(theme: Theme) -> Vec<Span<'static>> {
    let mut pinned = Vec::new();
    for (n, (key, label)) in keys::PINNED.into_iter().enumerate() {
        if n > 0 {
            pinned.push(Span::raw("  "));
        }
        pinned.extend(item(theme, key, label));
    }
    pinned
}

/// The words the spinner shows, without the `…`: the running action's label (§9.4), else
/// `checking usage` while a person's `r` waits for its collection (§9.7).
pub(crate) fn busy_label(state: &State) -> Option<String> {
    match &state.running {
        Some(action) => Some(state.label_of(action)),
        None => state.checking.map(|_| "checking usage".to_owned()),
    }
}

/// The toast the footer shows: not once the shutdown has begun.
fn shown_toast(state: &State) -> Option<&Toast> {
    state.toast.as_ref().filter(|_| state.quitting.is_none())
}

/// `{mark} {text}` (§5.5), the mark being `✓` for `Done`, `!` for `Warn` and `✕` for `Failed`,
/// in the role the mark is drawn in; none for `Info`.
fn toast_mark(kind: ToastKind) -> Option<(&'static str, Role)> {
    match kind {
        ToastKind::Done => Some(("✓", Role::Ok)),
        ToastKind::Info => None,
        ToastKind::Warn => Some(("!", Role::Warn)),
        ToastKind::Failed => Some(("✕", Role::Crit)),
    }
}

/// The width a toast wraps in at `width` columns, and what stays right of it: the width less
/// the right part and two columns, when that leaves `TOAST_MIN`; else with the spinner cut to
/// its glyph; else all of the width.
fn toast_room(state: &State, width: usize) -> (usize, Beside) {
    let fixed = keys::pinned_width() + keys::RIGHT_INSET + 2;
    let label = busy_label(state);
    let full = label.as_ref().map_or(0, |label| width_of(label) + 3 + 2);
    let room = width.saturating_sub(fixed + full);
    if room >= TOAST_MIN {
        return (room, Beside::Full);
    }
    // The glyph and the two spaces after it.
    let room = width.saturating_sub(fixed + 3);
    if label.is_some() && room >= TOAST_MIN {
        (room, Beside::Glyph)
    } else {
        (width, Beside::Nothing)
    }
}

fn toast_words(kind: ToastKind, text: &str) -> String {
    match toast_mark(kind) {
        Some((mark, _)) => format!("{mark} {text}"),
        None => text.to_owned(),
    }
}

/// A toast's rows at `width` columns: `{mark} {text}` wrapped onto `WRAP_ROWS` rows at most, from
/// the body, with the right part on the last row when it fits (§5.5). An empty toast still takes
/// the footer's one row.
pub(crate) fn toast_rows(state: &State, kind: ToastKind, text: &str, width: u16) -> ToastRows {
    let (room, right) = toast_room(state, usize::from(width));
    let mut rows = wrap_rows(&toast_words(kind, text), room, WRAP_ROWS);
    if rows.is_empty() {
        rows.push(String::new());
    }
    ToastRows { rows, right }
}

/// Whether a toast of `kind` fits `WRAP_ROWS` rows whole at the terminal's width, or would lose
/// words to the last row's `…`.
pub(crate) fn toast_fits(state: &State, kind: ToastKind, text: &str) -> bool {
    let (room, _) = toast_room(state, usize::from(state.size.0));
    wrap(&toast_words(kind, text), room).len() <= WRAP_ROWS
}

/// How many rows the footer takes for a toast, when it shows one.
pub(crate) fn toast_height(state: &State, width: u16) -> Option<usize> {
    let toast = shown_toast(state)?;
    Some(toast_rows(state, toast.kind, &toast.text, width).rows.len())
}

/// The footer (§5.5) at `width`, `frame` being the spinner's frame now:
/// - a toast, on as many rows as it needs, `WRAP_ROWS` at most, its mark in its role;
/// - else the spinner (`{frame} {label}…`, in accent);
/// - else the screen's keys (`keys::footer_items`) from column 0, each key in accent and its
///   label muted, two spaces apart.
///
/// `? keys  q quit` sits on the right of the last row, one column in, wherever it fits. While an
/// action runs, the spinner moves right of a toast, before the pinned two.
pub(crate) fn footer(state: &State, width: u16, frame: &str) -> Vec<Line<'static>> {
    let theme = state.theme;
    let columns = usize::from(width);
    let spinner = busy_label(state)
        .map(|label| Span::styled(format!("{frame} {label}…"), theme.role(Role::Accent)));
    let pinned = pinned_spans(theme);
    // `left`, a gap, then `right` ending `RIGHT_INSET` columns from the edge.
    let beside = |mut left: Vec<Span<'static>>, right: Vec<Span<'static>>| {
        let used = spans_width(&left) + spans_width(&right) + keys::RIGHT_INSET;
        left.push(Span::raw(" ".repeat(columns.saturating_sub(used))));
        left.extend(right);
        Line::from(left)
    };
    if let Some(toast) = shown_toast(state) {
        let layout = toast_rows(state, toast.kind, &toast.text, width);
        let mark = toast_mark(toast.kind);
        let mut right = Vec::new();
        match layout.right {
            Beside::Full => right.extend(spinner.into_iter().flat_map(|s| [s, Span::raw("  ")])),
            Beside::Glyph => right.extend([
                Span::styled(frame.to_owned(), theme.role(Role::Accent)),
                Span::raw("  "),
            ]),
            Beside::Nothing => {}
        }
        right.extend(pinned);
        let last = layout.rows.len() - 1;
        return layout
            .rows
            .into_iter()
            .enumerate()
            .map(|(n, row)| {
                let marked = mark.filter(|_| n == 0).and_then(|(mark, role)| {
                    row.strip_prefix(mark)
                        .map(|rest| (Span::styled(mark, theme.role(role)), rest.to_owned()))
                });
                let left = match marked {
                    Some((mark, rest)) => vec![mark, Span::raw(rest)],
                    None => vec![Span::raw(row)],
                };
                if n == last && layout.right != Beside::Nothing {
                    beside(left, right.clone())
                } else {
                    Line::from(left)
                }
            })
            .collect();
    }
    if let Some(spinner) = spinner {
        let room = columns.saturating_sub(spans_width(&pinned) + keys::RIGHT_INSET + 2);
        let left = vec![spinner];
        // A label too long for its room is cut, never the pinned two.
        return vec![beside(fit_spans(left, room), pinned)];
    }
    let mut left = Vec::new();
    for (n, (key, label)) in keys::footer_items(state, width).into_iter().enumerate() {
        if n > 0 {
            left.push(Span::raw("  "));
        }
        left.extend(item(theme, key, label));
    }
    vec![beside(left, pinned)]
}

/// The banner's words and role (T2-14, T5-4), its rows as `banner_rows` wraps them. One shows
/// at a time, the first of: held keys, the keyboard being unusable meanwhile; a crit state banner
/// (a failed read or a lane's death); on Tokens a stale report's words, so the DIM report keeps
/// its own banner; a warn state banner (a collection's warnings or a refused hand-over); on
/// Tokens its other errors (a failed check or refresh); the start-up warnings, joined by ` · `
/// (T2-21); what the accounts view has to say (a run shell, an unmanaged live login).
pub(crate) fn banner(state: &State) -> Option<(String, Role)> {
    if state.keys_held {
        return Some((KEYS_HELD.to_owned(), Role::Warn));
    }
    let on_tokens = state.screen == Screen::Tokens;
    let crit = state.banner.as_ref().filter(|banner| match banner.kind {
        BannerKind::Accounts | BannerKind::History | BannerKind::Lane => true,
        BannerKind::Collect | BannerKind::Refusal => false,
    });
    if let Some(banner) = crit {
        return Some((banner.text.clone(), Role::Crit));
    }
    if let Some(stale) = on_tokens.then(|| state.tokens.stale_words()).flatten() {
        return Some((stale.to_owned(), Role::Warn));
    }
    if let Some(banner) = &state.banner {
        return Some((banner.text.clone(), Role::Warn));
    }
    if let Some(error) = on_tokens.then(|| state.tokens.error()).flatten() {
        return Some((error.to_owned(), Role::Warn));
    }
    if !state.startup.warnings.is_empty() {
        return Some((state.startup.warnings.join(" · "), Role::Warn));
    }
    screens::accounts::banner(state).map(|text| (text, Role::Warn))
}

/// The banner's rows at `width`, in its role: one column in, `✕` (crit) or `!` (warn), then its
/// words, wrapped under themselves onto `WRAP_ROWS` rows at most. None without a banner.
pub(crate) fn banner_rows(state: &State, width: u16) -> Vec<Line<'static>> {
    let Some((text, role)) = banner(state) else {
        return vec![];
    };
    let mark = if role == Role::Crit { "✕" } else { "!" };
    let style = state.theme.role(role);
    wrap_rows(&text, usize::from(width).saturating_sub(3), WRAP_ROWS)
        .into_iter()
        .enumerate()
        .map(|(i, row)| {
            let lead = if i == 0 {
                format!(" {mark} ")
            } else {
                "   ".to_owned()
            };
            Line::from(Span::styled(format!("{lead}{row}"), style))
        })
        .collect()
}

fn spans_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|s| width_of(&s.content)).sum()
}

/// `spans` cut to `width` columns by display width, as `fit` cuts one string: the span at the
/// edge ends in `…`, and those after it are dropped.
pub(crate) fn fit_spans(spans: Vec<Span<'static>>, width: usize) -> Vec<Span<'static>> {
    if spans_width(&spans) <= width {
        return spans;
    }
    let mut left = width;
    let mut cut = Vec::new();
    for span in spans {
        let w = width_of(&span.content);
        if w < left {
            left -= w;
            cut.push(span);
            continue;
        }
        // More follows this span, so it ends in `…` even when it would fit whole.
        cut.push(Span::styled(
            fit(&format!("{}…", span.content), left),
            span.style,
        ));
        break;
    }
    cut
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier};
    use tagteam_core::ProviderId;
    use tagteam_core::autoswitch::AutoStrategy;
    use tagteam_daemon::protocol::{Event, Status, StatusBackoff, Wire};
    use tagteam_engine::views::{AccountView, UsageStatus};

    use super::*;
    use crate::host::{AccountRef, Action};
    use crate::live::Live;
    use crate::msg::{Cmd, Msg};
    use crate::source::SourceError;
    use crate::testutil::*;
    use crate::theme::Theme;
    use crate::update::update;
    use crate::view::areas;

    /// The header's left part, as every screen has it.
    const TABS_ROW: &str = " tagteam   1 Accounts   2 Tokens   3 History";

    /// Claude Code's job in `mode`, with `autoswitch`.
    fn one_job(mode: &str, autoswitch: bool) -> Status {
        daemon_status(vec![daemon_job("claude-code", mode, autoswitch)], vec![])
    }

    fn live_event(e: Event) -> Msg {
        Msg::Live(Live::Event(e))
    }

    /// The rows `view` draws at `w` columns: 24 of them at 80, 40 from 120.
    fn drawn(state: &State, w: u16) -> Vec<String> {
        draw(state, w, if w >= WIDE { 40 } else { 24 })
    }

    /// `pieces` in role letters: each character of a piece's text as its letter, a space as ` `.
    fn spell(pieces: &[(&str, char)]) -> String {
        pieces
            .iter()
            .flat_map(|(text, letter)| {
                text.chars()
                    .map(move |c| if c == ' ' { ' ' } else { *letter })
            })
            .collect()
    }

    /// The header's letters at `width` on `screen`, with `state`'s pieces on the right.
    fn header_letters(screen: Screen, width: usize, state: &[(&str, char)]) -> String {
        let tab = |s: Screen| if s == screen { 'a' } else { 'm' };
        let mut pieces = vec![(" tagteam", 'B')];
        for (s, label) in TABS {
            pieces.push(("   ", ' '));
            pieces.push((label, tab(s)));
        }
        let shown: usize = state.iter().map(|(t, _)| width_of(t)).sum();
        let gap = " ".repeat(width - width_of(TABS_ROW) - shown - "   ? keys ".len());
        pieces.push((&gap, ' '));
        pieces.extend_from_slice(state);
        pieces.extend([("   ", ' '), ("?", 'a'), (" keys", 'm'), (" ", ' ')]);
        spell(&pieces)
    }

    /// A state on Accounts with one account read, for each row of `the_header_says…`.
    fn on_accounts() -> State {
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snapshot(many(1)));
        state
    }

    fn live(status: Status) -> State {
        let mut state = on_accounts();
        go_live(&mut state, status);
        state
    }

    fn job(mode: &str, waiting_for: Option<&str>, pid: Option<u32>) -> Status {
        let mut job = daemon_job("claude-code", mode, true);
        job.waiting_for = waiting_for.map(Wire::from_wire);
        job.holder_pid = pid;
        daemon_status(vec![job], vec![])
    }

    fn keychain_locked() -> Status {
        let mut job = daemon_job("claude-code", "auto", true);
        job.backoff = Some(StatusBackoff {
            reason: "keychain".into(),
            until: "2026-09-21T15:05:00Z".into(),
        });
        daemon_status(vec![job], vec![])
    }

    #[test]
    fn the_header_says_each_state_of_spec_5_2_with_keys_always_there() {
        let consume_first = || {
            let mut state = fresh(Screen::Accounts);
            let mut snap = snapshot(many(1));
            snap.providers[0].strategy = AutoStrategy::ConsumeFirst;
            state.accounts.apply(snap);
            go_live(&mut state, one_job("auto", true));
            state
        };
        let stopping = || {
            let mut state = live(one_job("auto", true));
            let stopping = Event::Stopping {
                reason: Wire::from_wire("upgrade"),
            };
            update(&mut state, live_event(stopping), NOW_MS);
            state
        };
        let mismatch = || {
            let mut state = on_accounts();
            update(&mut state, Msg::Live(Live::Mismatch { server: 2 }), NOW_MS);
            state
        };
        let cases: Vec<(&str, State, &str, &str)> = vec![
            (
                "auto",
                live(one_job("auto", true)),
                "● auto on · switches at 90%",
                "● auto on · at 90%",
            ),
            (
                "consume-first",
                consume_first(),
                "● auto on · uses the soonest reset first",
                "● auto on · soonest reset",
            ),
            (
                "paused",
                live(one_job("collect", false)),
                "‖ auto paused · P resumes",
                "‖ paused · P resumes",
            ),
            (
                "idle collect",
                live(one_job("collect", true)),
                "○ auto idle · needs 2 usable accounts",
                "○ needs 2 usable accounts",
            ),
            (
                "engine-lock, pid",
                live(job("waiting", Some("engine-lock"), Some(4711))),
                "● auto runs in another terminal (pid 4711)",
                "● auto in pid 4711",
            ),
            (
                "engine-lock, no pid",
                live(job("waiting", Some("engine-lock"), None)),
                "● auto runs in another terminal",
                "● auto runs elsewhere",
            ),
            (
                "schema",
                live(job("waiting", Some("schema"), None)),
                "! daemon is outdated · restarting",
                "! daemon outdated",
            ),
            (
                "idle",
                live(one_job("idle", true)),
                "○ auto off · no usable account",
                "○ no usable account",
            ),
            (
                "backoff",
                live(keychain_locked()),
                "● auto on · switches at 90% · keychain locked until 15:05",
                "! keychain locked 15:05",
            ),
            (
                "no job",
                live(daemon_status(vec![], vec![])),
                "○ auto off",
                "○ auto off",
            ),
            (
                "unknown mode",
                live(one_job("added-later", true)),
                "○ daemon added-later",
                "○ daemon added-later",
            ),
            (
                "stopping",
                stopping(),
                "! daemon stopping (upgrade)",
                "! daemon stopping",
            ),
            (
                "mismatch",
                mismatch(),
                "! daemon version unsupported",
                "! daemon unsupported",
            ),
            (
                "no daemon",
                on_accounts(),
                "○ auto off · no daemon",
                "○ auto off · no daemon",
            ),
        ];
        for (case, state, wide, narrow) in cases {
            for (w, words) in [(120, wide), (80, narrow)] {
                let row = &drawn(&state, w)[0];
                assert!(
                    row.starts_with(&format!("{TABS_ROW} ")),
                    "{case} at {w}: {row}"
                );
                assert!(
                    row.ends_with(&format!("{words}   ? keys")),
                    "{case} at {w}: {row}"
                );
            }
        }
        // T2-11: no state until the daemon's first `status`, only `? keys`.
        let mut state = on_accounts();
        state.daemon.await_daemon(NOW_MS);
        assert_eq!(
            drawn(&state, 80)[0],
            format!("{TABS_ROW}{}? keys", " ".repeat(80 - 44 - 7))
        );
        go_live(&mut state, one_job("auto", true));
        assert!(drawn(&state, 80)[0].ends_with("● auto on · at 90%   ? keys"));
        // Before the first snapshot the threshold is not known, so it is left out.
        let mut state = fresh(Screen::Accounts);
        go_live(&mut state, one_job("auto", true));
        assert!(drawn(&state, 120)[0].ends_with("● auto on   ? keys"));
    }

    #[test]
    fn the_header_s_roles_follow_its_marks() {
        let auto = [("●", 'a'), (" auto on", 't'), (" · switches at 90%", 'm')];
        let paused = [("‖", 'w'), (" auto paused", 't'), (" · P resumes", 'm')];
        let locked = [
            ("●", 'a'),
            (" auto on", 't'),
            (" · switches at 90%", 'm'),
            (" · keychain locked until 15:05", 'w'),
        ];
        for theme in TIERS {
            for (status, pieces) in [
                (one_job("auto", true), &auto[..]),
                (one_job("collect", false), &paused[..]),
                (keychain_locked(), &locked[..]),
            ] {
                let mut state = live(status);
                state.theme = theme;
                let buf = draw_buffer(&state, 120, 40, NOW_MS);
                let want = header_letters(Screen::Accounts, 120, pieces);
                assert_eq!(roles(&buf, theme, 0), canon(theme, &want), "{theme:?}");
            }
        }
    }

    #[test]
    fn the_header_shows_the_tabs_the_current_one_marked_and_the_state() {
        // The header half of `view`'s header-and-footer test: the tabs on the left, the current
        // one in accent and bold (no brackets now), the state and `? keys` on the right.
        let theme = Theme::new(true);
        for screen in [Screen::Accounts, Screen::Tokens, Screen::History] {
            let mut state = fresh(screen);
            state.theme = theme;
            let state_pieces = [("○", 'm'), (" auto off", 't'), (" · no daemon", 'm')];
            for (w, h) in [(80, 24), (120, 40)] {
                let buf = draw_buffer(&state, w, h, NOW_MS);
                let gap = usize::from(w) - 44 - 22 - 10;
                assert_eq!(
                    text_of(&buf)[0],
                    format!(
                        "{TABS_ROW}{}○ auto off · no daemon   ? keys",
                        " ".repeat(gap)
                    ),
                    "{screen:?} at {w}"
                );
                assert_eq!(
                    roles(&buf, theme, 0),
                    canon(
                        theme,
                        &header_letters(screen, usize::from(w), &state_pieces)
                    ),
                    "{screen:?} at {w}"
                );
            }
        }
    }

    #[test]
    fn in_live_mode_the_header_says_the_daemon_s_state_from_its_status_alone() {
        // Spec §5.2, §8: the provider's state; an event's fields never reach the header.
        let mut state = on_accounts();
        let link = go_live(&mut state, one_job("collect", false));
        let paused = draw(&state, 80, 24);
        assert!(
            paused[0].ends_with("3 History      ‖ paused · P resumes   ? keys"),
            "{}",
            paused[0]
        );
        let mode = Event::Mode {
            provider: "claude-code".into(),
            mode: Wire::from_wire("idle"),
            waiting_for: None,
            holder_pid: None,
            backoff: None,
        };
        update(&mut state, live_event(mode), NOW_MS);
        assert_eq!(
            draw(&state, 80, 24)[0],
            paused[0],
            "until the status says so"
        );
        let reply = Msg::DaemonStatus {
            link,
            result: Ok(one_job("idle", true)),
        };
        update(&mut state, reply, NOW_MS);
        assert!(draw(&state, 80, 24)[0].ends_with("○ no usable account   ? keys"));
    }

    #[test]
    fn with_several_providers_the_header_follows_the_selected_account_s() {
        // Spec §5.2: the selected account's provider, with its own threshold.
        let mut state = fresh(Screen::Accounts);
        let fake = tagteam_core::ProviderId::new("fake-agent");
        let mut third = account(3, "c@x.co", read(60, 10.0, 20.0, false));
        third.row.provider = fake.clone();
        let mut snap = snapshot(many(1));
        snap.providers
            .push(tagteam_engine::views::ProviderAccounts {
                provider: fake,
                active_position: None,
                forecast: tagteam_engine::views::forecast_of(
                    std::slice::from_ref(&third),
                    &tagteam_engine::switch::LiveLogin::None,
                    &[],
                ),
                accounts: vec![third],
                threshold: 75.0,
                strategy: AutoStrategy::Best,
                names: tagteam_engine::views::ProviderNames {
                    display_name: "FakeAgent",
                    launch_command: "fakeagent",
                },
                models: vec![],
                live: tagteam_engine::switch::LiveLogin::None,
            });
        state.accounts.apply(snap);
        let mut waiting = daemon_job("fake-agent", "waiting", true);
        waiting.waiting_for = Some(Wire::from_wire("engine-lock"));
        waiting.holder_pid = Some(77);
        let status = daemon_status(
            vec![daemon_job("claude-code", "auto", true), waiting],
            vec![],
        );
        go_live(&mut state, status);
        assert!(draw(&state, 80, 24)[0].ends_with("● auto on · at 90%   ? keys"));
        update(&mut state, ch('j'), NOW_MS);
        assert!(
            draw(&state, 80, 24)[0].ends_with("● auto in pid 77   ? keys"),
            "{}",
            draw(&state, 80, 24)[0]
        );
        let status = daemon_status(
            vec![
                daemon_job("claude-code", "auto", true),
                daemon_job("fake-agent", "auto", true),
            ],
            vec![],
        );
        go_live(&mut state, status);
        assert!(draw(&state, 80, 24)[0].ends_with("● auto on · at 75%   ? keys"));
    }

    #[test]
    fn a_state_too_wide_for_the_row_is_cut_and_keys_stay() {
        // R20, now on one row: the state is cut to the room left, and `? keys` never goes.
        let mut state = on_accounts();
        go_live(
            &mut state,
            one_job("a-mode-whose-name-is-longer-than-the-room", true),
        );
        let lines = draw(&state, 80, 24);
        assert_eq!(
            lines[0],
            format!("{TABS_ROW} ○ daemon a-mode-whose-na…   ? keys"),
            "a column between the tabs and the state"
        );
        assert_eq!(width_of(&lines[0]) + 1, 80, "the keys one column in");
        assert!(
            draw(&state, 120, 40)[0]
                .ends_with("○ daemon a-mode-whose-name-is-longer-than-the-room   ? keys")
        );
        go_live(&mut state, keychain_locked());
        let lines = draw(&state, 80, 24);
        assert!(
            lines[0].ends_with("3 History   ! keychain locked 15:05   ? keys"),
            "{}",
            lines[0]
        );
        assert_eq!(areas(&state, Rect::new(0, 0, 80, 24)).body.y, 3);
        // The body: a blank row, the rule, a blank row (rows 3-5), then the table (row 6).
        assert!(
            draw(&state, 80, 24)[6].contains("ACCOUNT"),
            "the table under it: {lines:?}"
        );
        assert!(draw(&state, 120, 40)[0].ends_with("keychain locked until 15:05   ? keys"));
        // A wide state too long for the room at 120 columns takes its 80-column form, whole.
        let consume_first = {
            let mut snap = snapshot(many(1));
            snap.providers[0].strategy = AutoStrategy::ConsumeFirst;
            snap
        };
        for (case, mode, autoswitch, pid, snap) in [
            ("consume-first", "auto", true, None, consume_first),
            ("idle collect", "collect", true, None, snapshot(many(1))),
            (
                "engine-lock",
                "waiting",
                true,
                Some(4711),
                snapshot(many(1)),
            ),
        ] {
            let mut job = daemon_job("claude-code", mode, autoswitch);
            job.waiting_for = (mode == "waiting").then(|| Wire::from_wire("engine-lock"));
            job.holder_pid = pid;
            job.backoff = Some(StatusBackoff {
                reason: "keychain".into(),
                until: "2026-09-21T15:05:00Z".into(),
            });
            let mut state = fresh(Screen::Accounts);
            state.accounts.apply(snap);
            go_live(&mut state, daemon_status(vec![job], vec![]));
            let gap = " ".repeat(120 - 44 - 23 - 10);
            assert_eq!(
                draw(&state, 120, 40)[0],
                format!("{TABS_ROW}{gap}! keychain locked 15:05   ? keys"),
                "{case}"
            );
        }
    }

    #[test]
    fn timer_mode_says_when_the_daemon_stopped_or_speaks_another_protocol() {
        // Spec §5.2, §8.
        let mut state = fresh(Screen::Accounts);
        go_live(&mut state, one_job("auto", true));
        let stopping = Event::Stopping {
            reason: Wire::from_wire("upgrade"),
        };
        update(&mut state, live_event(stopping), NOW_MS);
        update(&mut state, Msg::Live(Live::Down), NOW_MS);
        assert_eq!(
            draw(&state, 80, 24)[0],
            format!(
                "{TABS_ROW}{}! daemon stopping   ? keys",
                " ".repeat(80 - 44 - 17 - 10)
            ),
        );
        assert!(draw(&state, 120, 40)[0].ends_with("! daemon stopping (upgrade)   ? keys"));
        update(&mut state, Msg::Tick, NOW_MS + 30_000);
        assert!(draw(&state, 80, 24)[0].ends_with("○ auto off · no daemon   ? keys"));
        update(&mut state, Msg::Live(Live::Mismatch { server: 2 }), NOW_MS);
        assert!(
            draw(&state, 80, 24)[0].ends_with("! daemon unsupported   ? keys"),
            "{}",
            draw(&state, 80, 24)[0]
        );
        assert!(draw(&state, 120, 40)[0].ends_with("! daemon version unsupported   ? keys"));
    }

    #[test]
    fn a_foreground_auto_shows_in_timer_mode_only() {
        // C-16, T2-11: with no daemon, a live `tagteam auto` holding the engine lock is named.
        let held = |provider: ProviderId| {
            let mut snap = snapshot(many(1));
            snap.auto_holders = vec![(provider, 4711)];
            snap
        };
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(held(cc()));
        assert!(
            draw(&state, 120, 40)[0]
                .ends_with("● auto runs in another terminal (pid 4711)   ? keys")
        );
        assert!(draw(&state, 80, 24)[0].ends_with("● auto in pid 4711   ? keys"));

        state.accounts.apply(held(ProviderId::new("fake-agent")));
        assert!(
            draw(&state, 80, 24)[0].ends_with("○ auto off · no daemon   ? keys"),
            "another provider's holder is not the focus provider's"
        );

        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(held(cc()));
        go_live(&mut state, one_job("auto", true));
        let row = &draw(&state, 120, 40)[0];
        assert!(row.contains("● auto on · "), "{row}");
        assert!(!row.contains("another terminal"), "{row}");
    }

    #[test]
    fn the_rows_above_the_body_are_the_same_in_both_modes() {
        // UX-5: the body never moves when the daemon comes or goes.
        for ((w, h), body) in [
            ((80, 24), Rect::new(0, 3, 80, 20)),
            ((120, 40), Rect::new(0, 3, 120, 36)),
        ] {
            let area = Rect::new(0, 0, w, h);
            let mut state = on_accounts();
            let timer = areas(&state, area).body;
            go_live(&mut state, one_job("auto", true));
            let live = areas(&state, area).body;
            update(&mut state, Msg::Live(Live::Down), NOW_MS);
            let down = areas(&state, area).body;
            assert_eq!((timer, live, down), (body, body, body), "{w}x{h}");
        }
    }

    /// `screen` with `alex@example.com` (1) and `work` (2) read, `work` in use at 53% and 6%:
    /// `alex@example.com` is the best pick.
    fn work_in_use(screen: Screen) -> State {
        let mut state = fresh(screen);
        state.accounts.apply(snapshot(alex_and_work()));
        state
    }

    fn alex_and_work() -> Vec<AccountView> {
        let alex = account(1, "alex@example.com", read(60, 5.0, 2.0, false));
        let mut work = account(2, "sam@corp.example", read(60, 53.0, 6.0, false));
        work.row.alias = Some("work".into());
        work.active = true;
        vec![alex, work]
    }

    /// Row 2 of `state` drawn at `w` columns in `theme`, and its role letters.
    fn now_row(state: &State, w: u16, theme: Theme) -> (String, String) {
        let buf = draw_buffer(state, w, if w >= WIDE { 40 } else { 24 }, NOW_MS);
        (text_of(&buf)[2].clone(), roles(&buf, theme, 2))
    }

    /// `left`, then `right` ending one column in, across `width`.
    fn edged(left: &str, right: &str, width: usize) -> String {
        let gap = width - 1 - width_of(left) - width_of(right);
        format!("{left}{}{right}", " ".repeat(gap))
    }

    const BEST_ALEX: &str = "best now 1 alex@example.com";

    #[test]
    fn the_now_strip_shows_the_account_in_use_and_the_best_pick() {
        // §5.4, T3-24: on Tokens and History, under the auto line.
        let wide = " ● 2 work   5h ━━━━━─────  53% 2h40m   7d ╸─────────   6% 3d09h";
        let narrow = " ● 2 work   5h  53% 2h40m   7d   6% 3d09h";
        for screen in [Screen::Tokens, Screen::History] {
            let mut state = work_in_use(screen);
            state.theme = TIERS[1];
            assert_eq!(
                now_row(&state, 120, TIERS[1]).0,
                edged(wide, BEST_ALEX, 120),
                "{screen:?}"
            );
            assert_eq!(
                now_row(&state, 80, TIERS[1]).0,
                edged(narrow, BEST_ALEX, 80),
                "{screen:?}: no gauge below 120 columns"
            );
        }
        for theme in TIERS {
            let mut state = work_in_use(Screen::Tokens);
            state.theme = theme;
            let track = if theme.colourless() { "·" } else { "─" };
            let (five_track, seven_track) = (track.repeat(5), track.repeat(9));
            for w in [120, 80] {
                let gauge = w >= WIDE;
                let mut pieces = vec![(" ", ' '), ("●", 'a'), (" 2 ", 't'), ("work", 'B')];
                pieces.extend([("   ", ' '), ("5h", 'm'), (" ", ' ')]);
                if gauge {
                    pieces.extend([("━━━━━", 'a'), (five_track.as_str(), 'f'), (" ", ' ')]);
                }
                pieces.extend([(" 53%", 't'), (" ", ' '), ("2h40m", 'm')]);
                pieces.extend([("   ", ' '), ("7d", 'm'), (" ", ' ')]);
                if gauge {
                    pieces.extend([("╸", 'a'), (seven_track.as_str(), 'f'), (" ", ' ')]);
                }
                pieces.extend([("  6%", 't'), (" ", ' '), ("3d09h", 'm')]);
                let left: usize = pieces.iter().map(|(t, _)| width_of(t)).sum();
                let gap = " ".repeat(usize::from(w) - 1 - left - width_of(BEST_ALEX));
                pieces.extend([(gap.as_str(), ' '), ("best now", 'm')]);
                pieces.push((" 1 alex@example.com", 't'));
                let (text, letters) = now_row(&state, w, theme);
                assert_eq!(width_of(&text), usize::from(w) - 1, "{theme:?} {w}");
                assert_eq!(
                    letters.trim_end(),
                    canon(theme, &spell(&pieces)),
                    "{theme:?} {w}"
                );
            }
        }
        // On the best account, the right part says so.
        let mut state = fresh(Screen::Tokens);
        let mut accounts = alex_and_work();
        accounts[0].active = true;
        accounts[1].active = false;
        state.accounts.apply(snapshot(accounts));
        let row = &draw(&state, 80, 24)[2];
        assert!(
            row.starts_with(" ● 1 alex@example.com   5h   5% 2h40m"),
            "{row}"
        );
        assert!(row.ends_with("   on the best account"), "{row}");
        // Accounts has the strip too.
        let row = &draw(&work_in_use(Screen::Accounts), 80, 24)[2];
        assert_eq!(row, &edged(narrow, BEST_ALEX, 80));
    }

    /// `screen` with `work` read and in use, its 7d window ahead of pace.
    fn state_on(screen: Screen) -> State {
        let mut state = fresh(screen);
        let mut work = account(2, "sam@corp.example", read(60, 53.0, 82.0, true));
        work.row.alias = Some("work".into());
        work.active = true;
        state.accounts.apply(snapshot(vec![work]));
        state
    }

    #[test]
    fn the_now_strip_marks_an_ahead_window_after_its_percentage() {
        // Tokens and History, `work` in use with 7d ahead: `read(60, 53.0, 82.0, true)`.
        for screen in [Screen::Tokens, Screen::History] {
            for w in [80, 120] {
                let rows = draw(&state_on(screen), w, 30);
                let strip = &rows[2];
                assert!(strip.contains(" 82% ▲ "), "{strip}");
                assert!(!strip.contains('┿') && !strip.contains('┼'), "{strip}");
            }
        }
        for theme in TIERS {
            for w in [80, 120] {
                let mut state = state_on(Screen::Tokens);
                state.theme = theme;
                let buf = draw_buffer(&state, w, 30, NOW_MS);
                let (x, y) = locate(&buf, "▲").expect("the mark is drawn");
                assert_eq!(y, 2);
                let got = roles(&buf, theme, y).chars().nth(usize::from(x));
                assert_eq!(got, canon(theme, "w").chars().next(), "{theme:?} {w}");
            }
        }
    }

    #[test]
    fn a_problem_replaces_the_now_strip_s_windows() {
        let quarantined = || {
            let mut accounts = alex_and_work();
            accounts[1].usage = unread(UsageStatus::ReloginRequired, None, None);
            accounts[1].row.quarantine_reason = Some("invalid_grant".into());
            accounts
        };
        // A quarantined account is no candidate, which leaves too few for a pick: no right part.
        let left = " ● 2 work   ✕ needs a new login · see Accounts";
        for theme in TIERS {
            let mut state = fresh(Screen::History);
            state.theme = theme;
            state.accounts.apply(snapshot(quarantined()));
            for w in [80, 120] {
                let (text, letters) = now_row(&state, w, theme);
                assert_eq!(text, left, "{w}");
                let pieces = [
                    (" ", ' '),
                    ("●", 'a'),
                    (" 2 ", 't'),
                    ("work", 'B'),
                    ("   ", ' '),
                    ("✕ needs a new login", 'c'),
                    (" · see Accounts", 'm'),
                ];
                assert!(
                    letters.starts_with(&canon(theme, &spell(&pieces))),
                    "{theme:?} {w}: {letters}"
                );
            }
        }
        // M-8 governs the table's STATUS only: a disabled account in use still shows its
        // real problem here.
        let mut accounts = alex_and_work();
        accounts[1].usage = unread(UsageStatus::Unavailable, Some("http-429"), Some(120));
        accounts[1].row.disabled = true;
        for theme in TIERS {
            let mut state = fresh(Screen::Tokens);
            state.theme = theme;
            state.accounts.apply(snapshot(accounts.clone()));
            let (row, letters) = now_row(&state, 80, theme);
            assert!(
                row.starts_with(" ● 2 work   ! rate-limited · see Accounts"),
                "{row}"
            );
            let pieces = [
                (" ", ' '),
                ("●", 'a'),
                (" 2 ", 't'),
                ("work", 'B'),
                ("   ", ' '),
                ("! rate-limited", 'w'),
                (" · see Accounts", 'm'),
            ];
            assert!(
                letters.starts_with(&canon(theme, &spell(&pieces))),
                "{theme:?}: {letters}"
            );
        }
    }

    #[test]
    fn the_now_strip_cuts_the_name_to_16_columns_then_drops_the_pick_and_cuts_the_windows_last() {
        let windows = "   5h  53% 2h40m   7d   6% 3d09h";
        let with_name = |email: &str, alex_used: f64| {
            let mut state = fresh(Screen::Tokens);
            let mut accounts = alex_and_work();
            accounts[0].usage = read(60, alex_used, alex_used, false);
            accounts[1].row.alias = None;
            accounts[1].row.email = Some(email.into());
            accounts[1].row.label = email.into();
            state.accounts.apply(snapshot(accounts));
            state
        };
        let strip = |state: &State, w: u16| -> String {
            now_strip(state, w, NOW_MS)
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect()
        };
        // At 80 columns `sam@corp.example` and its windows take 53, too many beside the pick:
        // the pick goes and the windows stay whole.
        let state = with_name("sam@corp.example", 5.0);
        assert_eq!(
            now_row(&state, 80, TIERS[1]).0,
            format!(" ● 2 sam@corp.example{windows}")
        );
        for email in ["alex+work.team@example.com", "例のアカウント@example.com"] {
            let state = with_name(email, 5.0);
            assert_eq!(
                now_row(&state, 80, TIERS[1]).0,
                format!(" ● 2 {email}{windows}"),
                "{email}"
            );
            // A shorter pick stays: the name gives way to it, the windows whole.
            let best = with_name(email, 90.0);
            let row = now_row(&best, 80, TIERS[1]).0;
            assert!(
                row.ends_with(&format!("{windows}   on the best account")),
                "{row}"
            );
            assert_eq!(width_of(&row), 79, "{row}");
            // With less room the name gives way to 16 columns before the windows are cut,
            // and keeps them while they are.
            let floor = fit(email, 16);
            assert_eq!(width_of(&floor), 16, "{floor}");
            assert_eq!(
                strip(&state, 53),
                format!(" ● 2 {floor}{windows}"),
                "{email}"
            );
            let cut = strip(&state, 45);
            assert!(cut.starts_with(&format!(" ● 2 {floor}   5h  53%")), "{cut}");
            assert!(cut.ends_with('…'), "{cut}");
            assert_eq!(width_of(&cut), 45, "{cut}");
        }
    }

    #[test]
    fn with_no_account_in_use_the_now_strip_says_how_to_switch_to_one() {
        let words = " no account in use · s on Accounts switches to one";
        // The hint is never cut: at 80 columns, where the best pick would leave it 49 of its 51
        // columns, the pick gives way; at 120 both show.
        for theme in TIERS {
            let mut state = fresh(Screen::Tokens);
            state.theme = theme;
            assert_eq!(
                now_row(&state, 80, theme).0,
                "",
                "before the first snapshot"
            );
            let mut accounts = alex_and_work();
            accounts[1].active = false;
            state.accounts.apply(snapshot(accounts));
            for (w, row) in [(120, edged(words, BEST_ALEX, 120)), (80, words.to_owned())] {
                let (text, letters) = now_row(&state, w, theme);
                assert_eq!(text, row, "{w}");
                assert!(
                    letters.starts_with(&canon(theme, &spell(&[(words, 'm')]))),
                    "{theme:?} {w}: {letters}"
                );
            }
            state.accounts.apply(snapshot(vec![]));
            assert_eq!(now_row(&state, 80, theme).0, "", "no accounts");
        }
    }

    #[test]
    fn the_now_strip_follows_the_selected_account_s_provider() {
        // Review Focus 2: with several providers, the selected account's provider.
        use tagteam_engine::views::{ProviderAccounts, ProviderNames, forecast_of};
        let fake = ProviderId::new("fake-agent");
        let mut sam = account(3, "sam@corp.example", read(60, 81.0, 40.0, false));
        sam.row.provider = fake.clone();
        sam.active = true;
        let live = tagteam_engine::switch::LiveLogin::Managed { position: 3 };
        let mut snap = snapshot(alex_and_work());
        snap.providers.push(ProviderAccounts {
            provider: fake,
            active_position: Some(3),
            forecast: forecast_of(std::slice::from_ref(&sam), &live, &[]),
            accounts: vec![sam],
            threshold: 75.0,
            strategy: AutoStrategy::Best,
            names: ProviderNames {
                display_name: "FakeAgent",
                launch_command: "fakeagent",
            },
            models: vec![],
            live,
        });
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snap);
        update(&mut state, ch('j'), NOW_MS);
        update(&mut state, ch('j'), NOW_MS);
        // FakeAgent's one account leaves too few candidates for a pick: no right part.
        for screen in [Screen::Tokens, Screen::History] {
            state.screen = screen;
            assert_eq!(
                draw(&state, 80, 24)[2],
                " ● 3 sam@corp.example   5h  81% 2h40m   7d  40% 3d09h",
                "{screen:?}"
            );
        }
        state.screen = Screen::Accounts;
        update(&mut state, ch('k'), NOW_MS);
        state.screen = Screen::Tokens;
        let row = &draw(&state, 80, 24)[2];
        assert_eq!(
            row,
            &edged(" ● 2 work   5h  53% 2h40m   7d   6% 3d09h", BEST_ALEX, 80)
        );
    }

    #[test]
    fn the_rows_above_the_body_are_three_on_every_screen_and_a_rule_follows_on_two() {
        // §5.1, §7.8, UX-5: the Now strip's row never comes or goes with the daemon's mode.
        for (screen, body) in [
            (Screen::Tokens, Rect::new(0, 3, 80, 20)),
            (Screen::History, Rect::new(0, 3, 80, 20)),
            (Screen::Accounts, Rect::new(0, 3, 80, 20)),
        ] {
            let area = Rect::new(0, 0, 80, 24);
            let mut state = work_in_use(screen);
            let timer = areas(&state, area).body;
            go_live(&mut state, one_job("auto", true));
            let live = areas(&state, area).body;
            update(&mut state, Msg::Live(Live::Down), NOW_MS);
            let down = areas(&state, area).body;
            assert_eq!((timer, live, down), (body, body, body), "{screen:?}");
            let now = areas(&state, area).now;
            assert_eq!(now, Some(Rect::new(0, 2, 80, 1)), "{screen:?}");
        }
        // From 30 rows the rule sits under the strip on Tokens and History, and not on Accounts.
        let a = areas(&work_in_use(Screen::History), Rect::new(0, 0, 120, 40));
        assert_eq!(a.rule.map(|r| r.y), Some(3));
        assert_eq!(a.body, Rect::new(0, 4, 120, 35));
        let a = areas(&work_in_use(Screen::Accounts), Rect::new(0, 0, 120, 40));
        assert_eq!(a.rule, None);
        assert_eq!(a.body, Rect::new(0, 3, 120, 36));
    }

    #[test]
    fn from_30_rows_a_faint_rule_sits_under_the_chrome_on_tokens_and_history() {
        for screen in [Screen::Tokens, Screen::History] {
            for theme in TIERS {
                let mut state = fresh(screen);
                state.theme = theme;
                state.accounts.apply(snapshot(many(1)));
                let buf = draw_buffer(&state, 120, 40, NOW_MS);
                assert_eq!(text_of(&buf)[3], "─".repeat(120), "{screen:?}");
                assert_eq!(roles(&buf, theme, 3), canon(theme, &"f".repeat(120)));
            }
            let state = fresh(screen);
            for (h, rule, body) in [(24, None, 3), (29, None, 3), (30, Some(3), 4)] {
                let a = areas(&state, Rect::new(0, 0, 80, h));
                assert_eq!(a.rule.map(|r| r.y), rule, "{screen:?} 80x{h}");
                assert_eq!(a.body.y, body, "{screen:?} 80x{h}");
            }
        }
        // Accounts has none at any height: its body's own rule is at row 4.
        let state = on_accounts();
        let rows = draw(&state, 120, 40);
        assert_eq!(rows[3], "");
        assert_eq!(rows[4], "─".repeat(120));
        for h in [24, 29, 30, 40] {
            let a = areas(&state, Rect::new(0, 0, 80, h));
            assert_eq!((a.rule, a.body.y), (None, 3), "80x{h}");
        }
    }

    #[test]
    fn a_failed_read_is_crit_and_anything_else_warn() {
        // T2-14: `✕` crit for a failed read and a lane's death, `!` warn for the rest, one
        // column in, under the Now strip.
        let banner_row = |state: &State, theme: Theme| {
            let buf = draw_buffer(state, 80, 24, NOW_MS);
            (text_of(&buf)[3].clone(), roles(&buf, theme, 3))
        };
        for theme in TIERS {
            for (kind, mark, letter) in [
                (BannerKind::Accounts, "✕", 'c'),
                (BannerKind::History, "✕", 'c'),
                (BannerKind::Lane, "✕", 'c'),
                (BannerKind::Collect, "!", 'w'),
                (BannerKind::Refusal, "!", 'w'),
            ] {
                let mut state = on_accounts();
                state.theme = theme;
                let words = "usage was not collected: the store is locked";
                state.raise_banner(kind, words);
                let (text, letters) = banner_row(&state, theme);
                let want = format!(" {mark} {words}");
                assert_eq!(text, want, "{kind:?}");
                let spelled = spell(&[(&want, letter)]);
                assert_eq!(letters.trim_end(), canon(theme, &spelled), "{kind:?}");
            }
            let mut state = fresh(Screen::Accounts);
            state.theme = theme;
            let mut snap = snapshot(many(1));
            snap.in_run_shell = true;
            state.accounts.apply(snap);
            let (text, letters) = banner_row(&state, theme);
            assert!(
                text.starts_with(" ! inside a `tagteam run` session"),
                "{text}"
            );
            assert_eq!(
                &letters[..2],
                &canon(theme, " w"),
                "the accounts view's is warn"
            );
            state.raise_banner(BannerKind::Accounts, "the store is locked");
            assert_eq!(banner_row(&state, theme).0, " ✕ the store is locked");
            update(&mut state, Msg::KeysHeld { held: true }, NOW_MS);
            let (text, letters) = banner_row(&state, theme);
            assert!(
                text.starts_with(" ! the terminal sent an unfinished colour report"),
                "held keys first: {text}"
            );
            assert_eq!(&letters[..2], &canon(theme, " w"));
        }
    }

    #[test]
    fn an_error_banner_is_crit_in_every_tier() {
        // The old red-or-bold test, in §4.1's roles: red in tier 1, bold and underlined without
        // colour, the tier 2 hue on either background.
        for (theme, fg, modifier) in [
            (
                TIERS[0],
                Color::Reset,
                Modifier::BOLD | Modifier::UNDERLINED,
            ),
            (TIERS[1], Color::Red, Modifier::empty()),
            (TIERS[2], Color::Rgb(0xf0, 0x71, 0x78), Modifier::empty()),
            (TIERS[3], Color::Rgb(0xcf, 0x22, 0x2e), Modifier::empty()),
        ] {
            let mut state = fresh(Screen::Accounts);
            state.theme = theme;
            state.raise_banner(BannerKind::Accounts, "the store is locked");
            let buf = draw_buffer(&state, 80, 24, NOW_MS);
            assert_eq!(text_of(&buf)[3], " ✕ the store is locked");
            for x in [1, 3] {
                let cell = &buf[(x, 3)];
                assert_eq!(cell.fg, fg, "{theme:?}");
                assert_eq!(cell.modifier, modifier, "{theme:?}");
            }
        }
    }

    #[test]
    fn banners_rank_held_keys_crit_a_stale_report_warn_then_the_rest() {
        // T5-4: held keys, a crit state banner, on Tokens a stale report's words, a warn state
        // banner, on Tokens its other errors, the start-up warnings, then the accounts view's
        // own (here a run shell).
        let mut state = fresh(Screen::Accounts);
        let mut snap = snapshot(many(1));
        snap.in_run_shell = true;
        state.accounts.apply(snap);
        state.startup.warnings = vec![
            "/cfg/config.toml: unknown key x (ignored)".to_owned(),
            "/cfg/config.toml: unknown key y (ignored)".to_owned(),
        ];
        let startup = "/cfg/config.toml: unknown key x (ignored) · \
                       /cfg/config.toml: unknown key y (ignored)";
        let stale = "couldn't read the token index (the index is locked) · r retries";
        let collect = "usage was not collected";
        let on_tokens = update(&mut state, ch('2'), NOW_MS);
        let Some(Cmd::LoadReport { r#gen, .. }) = on_tokens.first() else {
            panic!("{on_tokens:?}");
        };
        update(
            &mut state,
            Msg::TokenReport {
                r#gen: *r#gen,
                result: Err(SourceError {
                    kind: "store".into(),
                    message: "the index is locked".into(),
                }),
            },
            NOW_MS,
        );
        update(&mut state, Msg::KeysHeld { held: true }, NOW_MS);
        state.raise_banner(BannerKind::Collect, collect);
        assert_eq!(banner(&state), Some((KEYS_HELD.to_owned(), Role::Warn)));
        state.keys_held = false;
        assert_eq!(
            banner(&state),
            Some((stale.to_owned(), Role::Warn)),
            "the stale report's words over a collection's warnings"
        );
        state.raise_banner(BannerKind::Accounts, "the store is locked");
        assert_eq!(
            banner(&state),
            Some(("the store is locked".to_owned(), Role::Crit)),
            "a crit banner over the stale report's words"
        );
        state.banner = None;
        assert_eq!(banner(&state), Some((stale.to_owned(), Role::Warn)));
        update(&mut state, ch('1'), NOW_MS);
        state.raise_banner(BannerKind::Collect, collect);
        assert_eq!(
            banner(&state),
            Some((collect.to_owned(), Role::Warn)),
            "off Tokens the stale report has no say"
        );
        state.banner = None;
        assert_eq!(
            banner(&state),
            Some((startup.to_owned(), Role::Warn)),
            "the warnings, joined, though the accounts view has a banner too"
        );
        state.startup.warnings.clear();
        assert!(
            banner(&state).is_some_and(|(text, _)| text.starts_with("inside a `tagteam run`")),
            "the accounts view's, once they are gone"
        );
        let back = update(&mut state, ch('2'), NOW_MS);
        let Some(Cmd::LoadReport { r#gen, query }) = back.first() else {
            panic!("{back:?}");
        };
        let report = token_report(query.clone(), vec![]);
        update(
            &mut state,
            Msg::TokenReport {
                r#gen: *r#gen,
                result: Ok(report),
            },
            NOW_MS,
        );
        let failed = Err(SourceError {
            kind: "store".into(),
            message: "the store is locked".into(),
        });
        update(&mut state, Msg::Refreshed { result: failed }, NOW_MS);
        state.raise_banner(BannerKind::Collect, collect);
        assert_eq!(
            banner(&state),
            Some((collect.to_owned(), Role::Warn)),
            "a warn banner over a failed refresh"
        );
        state.banner = None;
        assert_eq!(
            banner(&state),
            Some((
                "the token index was not refreshed: the store is locked".to_owned(),
                Role::Warn
            )),
            "the refresh's words"
        );
    }

    #[test]
    fn a_long_banner_wraps_onto_three_rows_at_most_and_the_body_gives_them_up() {
        // Spec §5.1: 80×24 is enough, so nothing a banner says may be lost at 80 columns. Its
        // rows sit under the Now strip, wrapped under the words.
        let mut state = fresh(Screen::Accounts);
        let words = "the store could not be opened because another tagteam holds its lock";
        state.raise_banner(BannerKind::Refusal, format!("{words}; {words}"));
        let lines = draw(&state, 80, 24);
        assert_eq!(
            lines[3..5],
            [
                " ! the store could not be opened because another tagteam holds its lock; the",
                "   store could not be opened because another tagteam holds its lock",
            ]
        );
        assert_eq!(
            areas(&state, Rect::new(0, 0, 80, 24)).body,
            Rect::new(0, 5, 80, 18),
            "two rows, then the body"
        );
        state.raise_banner(BannerKind::Refusal, [words; 5].join("; "));
        let lines = draw(&state, 80, 24);
        assert!(lines[5].starts_with("   "), "{lines:?}");
        assert!(lines[5].ends_with('…'), "{lines:?}");
        assert_eq!(width_of(&lines[5]), 80);
        assert_eq!(
            areas(&state, Rect::new(0, 0, 80, 24)).body,
            Rect::new(0, 6, 80, 17),
            "three rows at most"
        );
    }

    /// Accounts with `n` accounts read, on a terminal tall enough for `w` columns.
    fn accounts_of(n: u32) -> State {
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snapshot(many(n)));
        state
    }

    /// The footer's own row of a screen drawn at `w` columns: the last one.
    fn last(state: &State, w: u16) -> String {
        drawn(state, w).pop().unwrap()
    }

    const ACCOUNTS_80: &str = "s switch  b best  n next  ⏎ history  P pause auto  r refresh";

    #[test]
    fn the_footer_pins_keys_and_quit_on_every_screen_at_80_columns() {
        for theme in TIERS {
            for screen in [Screen::Accounts, Screen::Tokens, Screen::History] {
                let mut state = fresh(screen);
                state.theme = theme;
                state.accounts.apply(snapshot(many(2)));
                let buf = draw_buffer(&state, 80, 24, NOW_MS);
                let row = &text_of(&buf)[23];
                assert!(row.ends_with("? keys  q quit"), "{screen:?}: {row}");
                assert!(!row.starts_with(' '), "{screen:?}: from column 0 (T2-28)");
                let letters = roles(&buf, theme, 23);
                assert!(
                    letters
                        .trim_end()
                        .ends_with(&canon(theme, "a mmmm  a mmmm")),
                    "{theme:?} {screen:?}: {letters}"
                );
                if screen == Screen::Accounts {
                    assert!(
                        letters.starts_with(&canon(theme, "a mmmmmm  a mmmm")),
                        "{theme:?}: keys in accent, labels muted: {letters}"
                    );
                }
            }
        }
    }

    #[test]
    fn accounts_footer_order_at_80_and_120() {
        let state = accounts_of(2);
        assert_eq!(last(&state, 80), footer_row(ACCOUNTS_80, 80));
        assert_eq!(
            last(&state, 120),
            footer_row(&format!("{ACCOUNTS_80}  e disable  a alias  x remove"), 120)
        );
    }

    #[test]
    fn e_s_label_follows_the_selected_row() {
        let mut state = fresh(Screen::Accounts);
        let mut accounts = many(2);
        accounts[1].row.disabled = true;
        state.accounts.apply(snapshot(accounts));
        assert!(
            last(&state, 120).contains("e disable  a alias"),
            "{}",
            last(&state, 120)
        );
        update(&mut state, ch('j'), NOW_MS);
        let row = last(&state, 120);
        assert!(row.contains("e enable  a alias  x remove"), "{row}");
        assert!(!row.contains("e disable"), "{row}");
    }

    #[test]
    fn with_no_accounts_the_footer_hides_the_account_keys() {
        let state = accounts_of(0);
        for w in [80, 120] {
            assert_eq!(
                last(&state, w),
                footer_row("r refresh", usize::from(w)),
                "{w}"
            );
        }
    }

    #[test]
    fn every_screen_s_chrome_in_each_tier() {
        // The header, the auto line and (at 120×40) the rule, in their roles, on every screen:
        // in timer mode and in live mode, with a stored switch of the last 30 minutes (T4-30)
        // and, live, the next check.
        let mut job = daemon_job("claude-code", "auto", true);
        job.next_tick_at = Some("2026-09-21T14:16:00Z".into());
        let timer_state = [("○", 'm'), (" auto off", 't'), (" · no daemon", 'm')];
        let text = "auto switched 1 a1 → 2 a2 · a1's 5h reached 93%";
        let auto_line = |width: usize, next: bool| {
            let mut line = vec![(" ", ' '), ("⇄", 'a'), (" ", ' ')];
            line.extend([("14:11", 'm'), ("  ", ' '), (text, 't')]);
            if next {
                line.push((" · next check 14:16", 'm'));
            }
            let left = width_of(&line.iter().map(|(t, _)| *t).collect::<String>());
            let gap = " ".repeat(width - 1 - 5 - left);
            let mut letters = spell(&line);
            letters.push_str(&gap);
            letters.push_str("a mmm");
            letters
        };
        for theme in TIERS {
            for screen in [Screen::Accounts, Screen::Tokens, Screen::History] {
                for (w, h) in [(80, 24), (120, 40)] {
                    let mut state = fresh(screen);
                    state.theme = theme;
                    let mut snap = snapshot(many(2));
                    snap.last_auto = vec![(cc(), stored_switch())];
                    state.accounts.apply(snap);
                    let width = usize::from(w);
                    let buf = draw_buffer(&state, w, h, NOW_MS);
                    let want = header_letters(screen, width, &timer_state);
                    assert_eq!(roles(&buf, theme, 0), canon(theme, &want), "{screen:?}");
                    assert_eq!(
                        roles(&buf, theme, 1).trim_end(),
                        canon(theme, &auto_line(width, false)),
                        "{screen:?} {w}"
                    );
                    // The Now strip is on row 2 of every screen (no account is in use here).
                    assert!(
                        text_of(&buf)[2].starts_with(" no account in use · s "),
                        "{screen:?} {w}"
                    );
                    if h >= RULE_FROM {
                        // Under the Now strip on Tokens and History; on Accounts it is the
                        // body's own, under a blank row.
                        let y = if screen == Screen::Accounts { 4 } else { 3 };
                        let rule = "f".repeat(width);
                        assert_eq!(roles(&buf, theme, y), canon(theme, &rule), "{screen:?}");
                    }
                    go_live(&mut state, daemon_status(vec![job.clone()], vec![]));
                    let buf = draw_buffer(&state, w, h, NOW_MS);
                    let live_state: &[(&str, char)] = if w >= WIDE {
                        &[("●", 'a'), (" auto on", 't'), (" · switches at 90%", 'm')]
                    } else {
                        &[("●", 'a'), (" auto on", 't'), (" · at 90%", 'm')]
                    };
                    let want = header_letters(screen, width, live_state);
                    assert_eq!(roles(&buf, theme, 0), canon(theme, &want), "{screen:?}");
                    assert_eq!(
                        roles(&buf, theme, 1).trim_end(),
                        canon(theme, &auto_line(width, w >= WIDE)),
                        "{screen:?} {w}"
                    );
                }
            }
        }
    }

    /// Auto's switch 1 → 2 stored at 14:11:20, as a1's 5h reached 93%.
    fn stored_switch() -> tagteam_engine::timeline::EventView {
        use tagteam_engine::{EventKind, EventSource};
        let at = NOW_MS - 120_000;
        let mut e = crate::screens::history::event_at(at, EventKind::Switch, EventSource::Auto);
        e.trigger = Some("proactive".into());
        e.from = Some(crate::screens::history::account_ref(1));
        e.to = Some(crate::screens::history::account_ref(2));
        e.detail = Some(
            serde_json::json!({"window": "5h", "label": "5h", "pct": 93.0, "threshold": 90.0}),
        );
        e
    }

    /// Row `y` of `state` drawn at `w` columns, and its role letters under `theme`.
    fn footer_of(state: &State, w: u16, theme: Theme) -> (String, String) {
        let h = if w >= WIDE { 40 } else { 24 };
        let buf = draw_buffer(state, w, h, NOW_MS);
        let last = h - 1;
        (
            text_of(&buf)[usize::from(last)].clone(),
            roles(&buf, theme, last),
        )
    }

    #[test]
    fn a_toast_keeps_the_pinned_keys_and_the_spinner_on_its_right() {
        // §5.5: the toast replaces the left side; the spinner moves right of it, before the
        // pinned two, in accent.
        let busy = "still switching to 1 a1 · wait for it to finish";
        let right = "⠋ switching to 1 a1…  ? keys  q quit";
        for theme in TIERS {
            let mut state = accounts_of(2);
            state.theme = theme;
            state.running = Some(Action::Switch(AccountRef {
                id: id(1),
                provider: cc(),
                position: 1,
            }));
            state.toast(ToastKind::Info, busy, NOW_MS);
            let (text, letters) = footer_of(&state, 120, theme);
            let gap = 120 - 1 - width_of(busy) - width_of(right);
            assert_eq!(text, format!("{busy}{}{right}", " ".repeat(gap)));
            let spelled = format!(
                "{}{}{}",
                spell(&[(busy, 't')]),
                " ".repeat(gap),
                spell(&[("⠋ switching to 1 a1…", 'a'), ("  ", ' ')])
            );
            let pinned = "a mmmm  a mmmm";
            assert_eq!(
                letters.trim_end(),
                canon(theme, &format!("{spelled}{pinned}")),
                "{theme:?}"
            );
        }
        // At 80 columns the room beside the right part is 41: it wraps, and the right part sits
        // on the last row.
        let mut state = accounts_of(2);
        state.running = Some(Action::Switch(AccountRef {
            id: id(1),
            provider: cc(),
            position: 1,
        }));
        state.toast(ToastKind::Info, busy, NOW_MS);
        let lines = draw(&state, 80, 24);
        assert_eq!(lines[22], "still switching to 1 a1 · wait for it to");
        assert_eq!(
            lines[23],
            format!("finish{}{right}", " ".repeat(80 - 1 - 6 - width_of(right)))
        );
        // With under 40 columns beside a long label, the spinner shows its glyph alone and
        // keeps the pinned two.
        state.running = Some(Action::AddLive { provider: cc() });
        let lines = draw(&state, 80, 24);
        let glyph = "⠋  ? keys  q quit";
        assert_eq!(
            lines[23],
            format!(
                "{busy}{}{glyph}",
                " ".repeat(80 - 1 - width_of(busy) - width_of(glyph))
            )
        );
        assert_eq!(areas(&state, Rect::new(0, 0, 80, 24)).footer.height, 1);
        let adding = "still adding Claude Code's login · wait for it to finish";
        state.toast(ToastKind::Info, adding, NOW_MS);
        assert_eq!(
            draw(&state, 80, 24)[23],
            format!(
                "{adding}{}{glyph}",
                " ".repeat(80 - 1 - width_of(adding) - width_of(glyph))
            )
        );
        state.theme = TIERS[1];
        let buf = draw_buffer(&state, 80, 24, NOW_MS);
        assert_eq!(
            roles(&buf, TIERS[1], 23).trim_end(),
            canon(
                TIERS[1],
                &format!(
                    "{}{}a  a mmmm  a mmmm",
                    spell(&[(adding, 't')]),
                    " ".repeat(80 - 1 - width_of(adding) - width_of(glyph))
                )
            ),
            "the glyph is accent"
        );
        // Without a toast the spinner stands on the left and the pinned two stay.
        state.toast = None;
        assert_eq!(
            draw(&state, 80, 24)[23],
            footer_row("⠋ adding Claude Code's login…", 80)
        );
    }

    #[test]
    fn a_long_spinner_label_is_cut_and_never_costs_the_pinned_keys() {
        // §5.5, UX-4: the longest alias is 64 columns; at 80 the label gives way, not `? keys`.
        let mut accounts = many(2);
        accounts[0].row.alias = Some("x".repeat(64));
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snapshot(accounts));
        state.running = Some(Action::Switch(AccountRef {
            id: id(1),
            provider: cc(),
            position: 1,
        }));
        // 63 columns of label (the 80 less the pinned 14, its inset and a gap of 2), ending `…`.
        let label = format!("⠋ switching to 1 {}…", "x".repeat(45));
        assert_eq!(width_of(&label), 63);
        assert_eq!(draw(&state, 80, 24)[23], format!("{label}  ? keys  q quit"));
    }

    #[test]
    fn each_kind_has_its_mark_in_its_role() {
        let words = "Switched to a1@x.co";
        for theme in TIERS {
            for (kind, mark, letter) in [
                (ToastKind::Done, Some("✓"), 'o'),
                (ToastKind::Info, None, 't'),
                (ToastKind::Warn, Some("!"), 'w'),
                (ToastKind::Failed, Some("✕"), 'c'),
            ] {
                let mut state = accounts_of(2);
                state.theme = theme;
                state.toast(kind, words, NOW_MS);
                let (text, letters) = footer_of(&state, 80, theme);
                let said = match mark {
                    Some(mark) => format!("{mark} {words}"),
                    None => words.to_owned(),
                };
                assert_eq!(text, footer_row(&said, 80), "{kind:?}");
                let lead = match mark {
                    Some(_) => format!("{letter} "),
                    None => String::new(),
                };
                let body = spell(&[(words, 't')]);
                let gap = " ".repeat(80 - 1 - width_of(&said) - 14);
                let want = canon(theme, &format!("{lead}{body}{gap}a mmmm  a mmmm"));
                assert_eq!(letters.trim_end(), want.trim_end(), "{kind:?} in {theme:?}");
            }
        }
    }
}
