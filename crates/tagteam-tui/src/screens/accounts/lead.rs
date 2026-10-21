//! The lead card (redesign §6.2, T3-19, T3-32): the account in use of focus's provider, its
//! windows as lead bars, the forecast's best pick, and its problem under them.

use ratatui::text::{Line, Span};
use tagteam_core::{Pace, Window, WindowKind};
use tagteam_engine::switch::{BestForecast, NoPick};
use tagteam_engine::views::{AccountView, ProviderAccounts};
use tagteam_engine::words;

use super::card;
use super::table::IN_USE;
use crate::chrome::fit_spans;
use crate::daemon;
use crate::gauge::lead_bar;
use crate::state::State;
use crate::theme::{Role, Theme};
use crate::view::{fit, pad, width_of};

/// The width from which the bars take `BAR_WIDE` cells, the title its org, and the card every
/// window (§6.2).
const WIDE_FROM: u16 = 120;
const BAR_WIDE: usize = 52;
const BAR: usize = 30;
/// The title starts this far in, the bars and the fold line further.
const TITLE_INDENT: &str = "  ";
const INDENT: &str = "    ";
/// Between a row's parts (§4.3).
const GAP: &str = "  ";
/// The least room between the title's left part and the best pick (§4.3's group gap).
pub(crate) const PICK_GAP: usize = 3;
/// The left part keeps this much room at least, or the best pick is dropped (T3-19).
pub(crate) const LEFT_LEAST: usize = 16;
const FACT_SEP: &str = " · ";
/// A login valid for less than this is warn, and says to log in again (§6.2, T3-10).
const SOON_S: i64 = 7 * 86_400;
const LOG_IN_AGAIN: &str = "log in again before then";
/// The ahead-of-pace mark.
const AHEAD: &str = "▲";
/// A percentage is bold from here (T3-18).
const BOLD_FROM: i64 = 70;
/// What the card says with no account in use (§6.2).
const NO_ACCOUNT: &str = "No account in use";
const NO_ACCOUNT_HINT: &str = "s switches to the selected account";

/// The lead card as drawn, and the windows whose bars it drew, by key (T3-32).
pub(crate) struct Lead {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) shown: Vec<String>,
}

/// Focus's provider and its account in use, when it has one.
pub(crate) fn in_use(state: &State) -> Option<(&ProviderAccounts, &AccountView)> {
    let view = state.view_of(&daemon::focus(state))?;
    let v = view.accounts.iter().find(|v| v.active)?;
    Some((view, v))
}

/// The lead card at `width` in at most `rows` rows (§6.2, T3-19, T3-32), for focus's
/// provider. The title and the problem rows are counted first; the bars of
/// `card::lead_keys` take the rows left, the last of them becoming the fold line when one is
/// left out. Drawn in §6.2's order: the title, the bars and the fold line, then the problem
/// rows under them.
pub(crate) fn lead(state: &State, width: u16, rows: usize, now_ms: i64) -> Lead {
    let Some((view, v)) = in_use(state) else {
        let mut lines = no_account(state, width);
        lines.truncate(rows);
        return Lead {
            lines,
            shown: vec![],
        };
    };
    let problems = card::problem_rows(state, v, width, now_ms);
    let keys = card::lead_keys(v, width);
    let room = rows.saturating_sub(1 + problems.len());
    let fold = keys.len() > room && room > 0;
    let bars = if fold { room - 1 } else { keys.len().min(room) };
    let shown = keys[..bars].to_vec();
    let mut lines = vec![title(state, view, v, width, now_ms)];
    lines.extend(bar_rows(v, &shown, width, now_ms, state.theme));
    if fold {
        let selected = state.accounts.selected_view().map(|s| &s.row.id) == Some(&v.row.id);
        lines.push(fold_line(keys.len() - bars, selected, width, state.theme));
    }
    lines.extend(problems);
    // Only a body too short for the title and the problem rows cuts them (T3-22).
    lines.truncate(rows);
    Lead { lines, shown }
}

/// Its rows with no bound: what T3-22 gives it when that fits.
pub(crate) fn height(state: &State, width: u16, now_ms: i64) -> usize {
    match in_use(state) {
        Some((_, v)) => {
            1 + card::lead_keys(v, width).len() + card::problem_rows(state, v, width, now_ms).len()
        }
        None => no_account(state, width).len(),
    }
}

/// The forecast's words: the lead card's when `with_key` (§6.2's table), the Now strip's
/// otherwise (§5.4: no key, and nothing for any `None`).
pub(crate) fn pick_words(
    state: &State,
    view: &ProviderAccounts,
    with_key: bool,
) -> Option<Vec<Span<'static>>> {
    let theme = state.theme;
    let muted = theme.role(Role::Muted);
    let accent = theme.role(Role::Accent);
    match &view.forecast {
        BestForecast::Pick { order } => {
            let position = *order.first()?;
            let name = view
                .accounts
                .iter()
                .find(|a| a.row.position == position)
                .map(|a| a.row.alias.clone().unwrap_or_else(|| words::email(&a.row)));
            let pick = match name {
                Some(name) => format!("{position} {name}"),
                None => position.to_string(),
            };
            let mut spans = vec![Span::styled("best now", muted)];
            if with_key {
                spans.extend([
                    Span::raw(GAP),
                    Span::raw(pick),
                    Span::raw("   "),
                    Span::styled("b", accent),
                    Span::raw(" "),
                    Span::styled("switch", muted),
                ]);
            } else {
                spans.extend([Span::raw(" "), Span::raw(pick)]);
            }
            Some(spans)
        }
        BestForecast::AlreadyBest => Some(vec![Span::styled("on the best account", muted)]),
        BestForecast::None(NoPick::UsageUnknown) if with_key => Some(vec![
            Span::styled("b", accent),
            Span::styled(" checks usage first, then picks", muted),
        ]),
        BestForecast::None(_) => None,
    }
}

/// `  No account in use` in bold, with focus's best pick on the right as the title has it, and
/// what `s` does, muted (§6.2).
fn no_account(state: &State, width: u16) -> Vec<Line<'static>> {
    let theme = state.theme;
    let pick = state
        .view_of(&daemon::focus(state))
        .and_then(|view| pick_words(state, view, true))
        .unwrap_or_default();
    let head = with_pick(pick, usize::from(width), |room| {
        fit_spans(
            vec![
                Span::raw(TITLE_INDENT),
                Span::styled(NO_ACCOUNT, theme.strong()),
            ],
            room,
        )
    });
    let hint = Line::from(fit_spans(
        vec![
            Span::raw(TITLE_INDENT),
            Span::styled(NO_ACCOUNT_HINT, theme.role(Role::Muted)),
        ],
        usize::from(width),
    ));
    vec![head, hint]
}

/// `left`, laid out in the room it is given, and `pick` on the right edge of `width`, never cut
/// (T3-19): the pick is dropped when it would leave the left part less than `LEFT_LEAST`
/// columns, and `left` then takes the whole width.
fn with_pick(
    pick: Vec<Span<'static>>,
    width: usize,
    left: impl FnOnce(usize) -> Vec<Span<'static>>,
) -> Line<'static> {
    let pick_w: usize = pick.iter().map(|s| width_of(&s.content)).sum();
    let Some(room) = width
        .checked_sub(pick_w + PICK_GAP)
        .filter(|&room| pick_w > 0 && room >= LEFT_LEAST)
    else {
        return Line::from(left(width));
    };
    let mut spans = left(room);
    let drawn: usize = spans.iter().map(|s| width_of(&s.content)).sum();
    spans.push(Span::raw(" ".repeat(width - pick_w - drawn)));
    spans.extend(pick);
    Line::from(spans)
}

/// The title (T3-19): `  ● {pos} {name}[ · {org}]    {facts}` on the left, laid out by
/// `card::title_spans`, and the forecast's words on the right edge, by `with_pick`.
fn title(
    state: &State,
    view: &ProviderAccounts,
    v: &AccountView,
    width: u16,
    now_ms: i64,
) -> Line<'static> {
    let theme = state.theme;
    let name = card::name_spans(&v.row, width, theme);
    // With several providers' sections, the card says whose account is in use.
    let several = state
        .accounts
        .snapshot()
        .is_some_and(|snap| snap.providers.len() > 1);
    let provider = several.then_some(view.names.display_name);
    let facts = card::fact_spans(facts_of(v, provider, now_ms), theme);
    let head = vec![
        Span::raw(TITLE_INDENT),
        Span::styled(IN_USE, theme.role(Role::Accent)),
        Span::raw(format!(" {} ", v.row.position)),
    ];
    let pick = pick_words(state, view, true).unwrap_or_default();
    with_pick(pick, usize::from(width), |room| {
        card::title_spans(head, name, facts, room)
    })
}

/// The title's facts and their roles (T3-19): `in use`, the provider's display name when
/// `provider` names it, the kind's display name, `in a session` and `login valid …`, muted. While
/// the login has less than `SOON_S` left it is warn, followed by `log in again before then`
/// (T3-10); once passed it is `login expired`, warn, alone.
fn facts_of(v: &AccountView, provider: Option<&str>, now_ms: i64) -> Vec<(String, Role)> {
    let now_s = now_ms.div_euclid(1000);
    let mut facts = vec![("in use".to_owned(), Role::Muted)];
    facts.extend(provider.map(|name| (name.to_owned(), Role::Muted)));
    facts.extend(v.kind.display.map(|d| (d.to_owned(), Role::Muted)));
    if v.in_session {
        facts.push(("in a session".to_owned(), Role::Muted));
    }
    if let Some(at_ms) = v.row.login_expires_at {
        let left = at_ms.div_euclid(1000).saturating_sub(now_s);
        let login = words::login_valid(at_ms, now_s);
        if left <= 0 {
            facts.push((login, Role::Warn));
        } else if left < SOON_S {
            facts.push((login, Role::Warn));
            facts.push((LOG_IN_AGAIN.to_owned(), Role::Warn));
        } else {
            facts.push((login, Role::Muted));
        }
    }
    facts
}

/// `v`'s windows named in `keys` as lead bars, in window order (T3-19):
/// `    {label}  {bar}  {pct:>4}[ ▲]  resets in {span}[ · {projection}]`, cut to the width.
fn bar_rows(
    v: &AccountView,
    keys: &[String],
    width: u16,
    now_ms: i64,
    theme: Theme,
) -> Vec<Line<'static>> {
    let windows: Vec<&(Window, Pace)> = v
        .usage
        .windows
        .iter()
        .flatten()
        .filter(|(w, _)| keys.contains(&w.key))
        .collect();
    let label_w = card::label_width(windows.iter().map(|(w, _)| w), width);
    let wide = width >= WIDE_FROM;
    let cells = if wide { BAR_WIDE } else { BAR };
    let now_s = now_ms.div_euclid(1000);
    windows
        .into_iter()
        .map(|(w, p)| {
            // C-8: only long and scoped windows have a pace mark.
            let expected = match w.kind {
                WindowKind::Long | WindowKind::Scoped => p.expected_pct,
                WindowKind::Short | WindowKind::Spend => None,
            };
            let pct = w.pct.round() as i64;
            let value = words::spend_text(w).unwrap_or_else(|| format!("{:>4}", format!("{pct}%")));
            let mut style = theme.severity(words::severity(pct));
            if pct >= BOLD_FROM {
                style = style.patch(theme.strong());
            }
            let mut spans = vec![Span::raw(format!(
                "{INDENT}{}{GAP}",
                pad(&w.label, label_w)
            ))];
            spans.extend(lead_bar(w.pct, expected, cells, theme));
            spans.push(Span::raw(GAP));
            spans.push(Span::styled(value, style));
            if p.ahead == Some(true) {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(AHEAD, theme.role(Role::Warn)));
            }
            spans.extend(card::reset_tail(w, p, wide, now_s, theme));
            Line::from(fit_spans(spans, usize::from(width)))
        })
        .collect()
}

/// The fold line (T3-32), muted, in place of the last bar that fits: `+N more windows · in the
/// card below` when the account in use is the selected one, else `· select it for its card`.
fn fold_line(left_out: usize, selected: bool, width: u16, theme: Theme) -> Line<'static> {
    let where_ = if selected {
        "in the card below"
    } else {
        "select it for its card"
    };
    let text = format!("+{left_out} more windows{FACT_SEP}{where_}");
    Line::from(vec![
        Span::raw(INDENT),
        Span::styled(
            fit(&text, usize::from(width).saturating_sub(INDENT.len())),
            theme.role(Role::Muted),
        ),
    ])
}
