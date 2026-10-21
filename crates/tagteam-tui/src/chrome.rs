//! The rows around the body (redesign §5.1): the header (§5.2), the auto line (§5.3), the faint
//! rule under them from 30 rows (§4.3), the banner (§5.6), and the footer's keys (§5.5).

use ratatui::text::{Line, Span};

use crate::autoline;
use crate::daemon::words::auto_state;
use crate::keys;
use crate::screens;
use crate::state::{BannerKind, Screen, State, Toast, ToastKind};
use crate::theme::{Role, Theme};
use crate::view::{WRAP_ROWS, fit, width_of, wrap, wrap_rows};

/// From this many rows a faint rule sits under the chrome (§4.5).
pub(crate) const RULE_FROM: u16 = 30;
/// From this many columns the header and the auto line take their long forms (§4.5).
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
/// suffix muted, cut to `width`. Empty when there is nothing to say (M-7): the row stays.
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
    let text = if said.muted { Role::Muted } else { Role::Text };
    spans.push(Span::styled(said.text, theme.role(text)));
    if let Some(next) = said.next {
        spans.push(Span::styled(format!(" · {next}"), theme.role(Role::Muted)));
    }
    Line::from(fit_spans(spans, usize::from(width)))
}

/// `─` across `width`, faint: under the chrome from `RULE_FROM` rows (§4.3).
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

/// The banner's words and role (T2-14), its rows as `banner_rows` wraps them. One shows at a
/// time, the first of: held keys, the keyboard being unusable meanwhile; the state's banner (a
/// failed read or a lane's death crit, a collection's warnings or a refused hand-over warn); on
/// Tokens a failed token read; the start-up warnings, joined by ` · ` (T2-21); what the accounts
/// view has to say (a run shell, an unmanaged live login).
pub(crate) fn banner(state: &State) -> Option<(String, Role)> {
    if state.keys_held {
        return Some((KEYS_HELD.to_owned(), Role::Warn));
    }
    if let Some(banner) = &state.banner {
        let role = match banner.kind {
            BannerKind::Accounts | BannerKind::History | BannerKind::Lane => Role::Crit,
            BannerKind::Collect | BannerKind::Refusal => Role::Warn,
        };
        return Some((banner.text.clone(), role));
    }
    let tokens = (state.screen == Screen::Tokens)
        .then(|| state.tokens.error())
        .flatten();
    if let Some(error) = tokens {
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
    use tagteam_daemon::protocol::{Event, Status, StatusBackoff, StatusSwitch, Wire};

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
        assert_eq!(areas(&state, Rect::new(0, 0, 80, 24)).body.y, 2);
        // The body: the lead card's two rows, a blank one, then the table.
        assert!(
            body_of(&state, 80, 24, NOW_MS)[3].contains("ACCOUNT"),
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
            ((80, 24), Rect::new(0, 2, 80, 21)),
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

    #[test]
    fn from_30_rows_a_faint_rule_sits_under_the_chrome() {
        for theme in TIERS {
            let mut state = on_accounts();
            state.theme = theme;
            let buf = draw_buffer(&state, 120, 40, NOW_MS);
            assert_eq!(text_of(&buf)[2], "─".repeat(120));
            assert_eq!(roles(&buf, theme, 2), canon(theme, &"f".repeat(120)));
        }
        let state = on_accounts();
        for (h, rule, body) in [(24, None, 2), (29, None, 2), (30, Some(2), 3)] {
            let a = areas(&state, Rect::new(0, 0, 80, h));
            assert_eq!(a.rule.map(|r| r.y), rule, "80x{h}");
            assert_eq!(a.body.y, body, "80x{h}");
        }
    }

    #[test]
    fn a_failed_read_is_crit_and_anything_else_warn() {
        // T2-14: `✕` crit for a failed read and a lane's death, `!` warn for the rest, one
        // column in, under the auto line.
        let banner_row = |state: &State, theme: Theme| {
            let buf = draw_buffer(state, 80, 24, NOW_MS);
            (text_of(&buf)[2].clone(), roles(&buf, theme, 2))
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
            assert_eq!(text_of(&buf)[2], " ✕ the store is locked");
            for x in [1, 3] {
                let cell = &buf[(x, 2)];
                assert_eq!(cell.fg, fg, "{theme:?}");
                assert_eq!(cell.modifier, modifier, "{theme:?}");
            }
        }
    }

    #[test]
    fn the_start_up_warnings_come_after_tokens_error_and_before_the_accounts_views() {
        // T2-14: held keys, the state's banner, Tokens' error, the start-up warnings, then the
        // accounts view's own (here a run shell).
        let mut state = fresh(Screen::Accounts);
        let mut snap = snapshot(many(1));
        snap.in_run_shell = true;
        state.accounts.apply(snap);
        state.startup.warnings = vec![
            "/cfg/config.toml: unknown key x (ignored)".to_owned(),
            "/cfg/config.toml: unknown key y (ignored)".to_owned(),
        ];
        let warn = |state: &State| banner(state).map(|(text, role)| (text, role == Role::Warn));
        let startup = "/cfg/config.toml: unknown key x (ignored) · \
                       /cfg/config.toml: unknown key y (ignored)";
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
        state.raise_banner(BannerKind::Collect, "usage was not collected");
        assert_eq!(warn(&state), Some((KEYS_HELD.to_owned(), true)));
        state.keys_held = false;
        assert_eq!(
            warn(&state),
            Some(("usage was not collected".to_owned(), true))
        );
        state.banner = None;
        assert_eq!(
            warn(&state),
            Some((
                "couldn't read the token index (the index is locked) · r retries".to_owned(),
                true
            )),
            "Tokens' error"
        );
        update(&mut state, ch('1'), NOW_MS);
        assert_eq!(
            warn(&state),
            Some((startup.to_owned(), true)),
            "the warnings, joined, though the accounts view has a banner too"
        );
        state.startup.warnings.clear();
        assert!(
            warn(&state).is_some_and(|(text, _)| text.starts_with("inside a `tagteam run`")),
            "the accounts view's, once they are gone"
        );
    }

    #[test]
    fn a_long_banner_wraps_onto_three_rows_at_most_and_the_body_gives_them_up() {
        // Spec §5.1: 80×24 is enough, so nothing a banner says may be lost at 80 columns. Its
        // rows sit under the auto line, wrapped under the words.
        let mut state = fresh(Screen::Accounts);
        let words = "the store could not be opened because another tagteam holds its lock";
        state.raise_banner(BannerKind::Refusal, format!("{words}; {words}"));
        let lines = draw(&state, 80, 24);
        assert_eq!(
            lines[2..4],
            [
                " ! the store could not be opened because another tagteam holds its lock; the",
                "   store could not be opened because another tagteam holds its lock",
            ]
        );
        assert_eq!(
            areas(&state, Rect::new(0, 0, 80, 24)).body,
            Rect::new(0, 4, 80, 19),
            "two rows, then the body"
        );
        state.raise_banner(BannerKind::Refusal, [words; 5].join("; "));
        let lines = draw(&state, 80, 24);
        assert!(lines[4].starts_with("   "), "{lines:?}");
        assert!(lines[4].ends_with('…'), "{lines:?}");
        assert_eq!(width_of(&lines[4]), 80);
        assert_eq!(
            areas(&state, Rect::new(0, 0, 80, 24)).body,
            Rect::new(0, 5, 80, 18),
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
        // in timer mode, and in live mode with a recent switch and the next check.
        let mut job = daemon_job("claude-code", "auto", true);
        job.last_switch = Some(StatusSwitch {
            at: "2026-09-21T14:11:20Z".into(),
            from: 1,
            to: 2,
            trigger: Wire::from_wire("proactive"),
        });
        job.next_tick_at = Some("2026-09-21T14:16:00Z".into());
        let timer_state = [("○", 'm'), (" auto off", 't'), (" · no daemon", 'm')];
        // M-7: timer mode's auto line is empty until T4 reads the store.
        let timer_line = String::new();
        for theme in TIERS {
            for screen in [Screen::Accounts, Screen::Tokens, Screen::History] {
                for (w, h) in [(80, 24), (120, 40)] {
                    let mut state = fresh(screen);
                    state.theme = theme;
                    state.accounts.apply(snapshot(many(2)));
                    let width = usize::from(w);
                    let buf = draw_buffer(&state, w, h, NOW_MS);
                    let want = header_letters(screen, width, &timer_state);
                    assert_eq!(roles(&buf, theme, 0), canon(theme, &want), "{screen:?}");
                    assert_eq!(
                        roles(&buf, theme, 1).trim_end(),
                        canon(theme, &timer_line),
                        "{screen:?} {w}"
                    );
                    if h >= RULE_FROM {
                        let rule = "f".repeat(width);
                        assert_eq!(roles(&buf, theme, 2), canon(theme, &rule));
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
                    let text = "auto switched 1 a1 → 2 a2 (before it reached the limit)";
                    let mut line = vec![(" ", ' '), ("⇄", 'a'), (" ", ' ')];
                    line.extend([("14:11", 'm'), ("  ", ' '), (text, 't')]);
                    if w >= WIDE {
                        line.push((" · next check 14:16", 'm'));
                    }
                    assert_eq!(
                        roles(&buf, theme, 1).trim_end(),
                        canon(theme, &spell(&line)),
                        "{screen:?} {w}"
                    );
                }
            }
        }
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
