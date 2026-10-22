//! History's event list (T4-20, §7.6): the range's events newest first, each its time, its
//! glyph and its sentence, the selection kept in view and the rest counted on a more line.

use jiff::tz::TimeZone;
use ratatui::text::{Line, Span};
use serde_json::Value;
use tagteam_engine::timeline::{EventView, TimelineView};
use tagteam_engine::words;

use super::GAP;
use super::marks::mark;
use crate::autoline;
use crate::chrome::fit_spans;
use crate::daemon::words::clock;
use crate::keys::RIGHT_INSET;
use crate::state::State;
use crate::theme::{Role, Theme};
use crate::view::{keep_visible, pad, select, width_of};

/// Before a row's time: the marker column and two spaces.
const LEAD: &str = "   ";
/// The head's words (T4-20).
const WHEN: &str = "WHEN";
const WHAT: &str = "WHAT";
/// A row's spans before its sentence, which `view::select` bolds.
const SENTENCE: usize = 5;

/// The list's rows past what fits: the first shown, how many show, and the count the more line
/// gives, `None` when they all fit. The more line takes the last row, and the shown rows follow
/// the selection (`keep_visible`) from `from`.
pub(super) fn window(
    len: usize,
    selected: Option<usize>,
    rows: usize,
    from: usize,
) -> (usize, usize, Option<usize>) {
    if len <= rows {
        return (0, len, None);
    }
    let shown = rows.saturating_sub(1);
    if shown == 0 {
        return (0, 0, Some(len));
    }
    let first = match selected {
        Some(at) => keep_visible(from, at, at, shown, len),
        None => from.min(len - shown),
    };
    (first, shown, Some(len - shown))
}

/// `e` in plain words (§11.4), `all-exhausted`'s earliest reset in the TUI's clock form (T4-29).
fn sentence(e: &EventView, tz: &TimeZone) -> String {
    let frees_at = e
        .detail
        .as_ref()
        .and_then(|d| d.get("earliest_reset_at"))
        .and_then(Value::as_i64)
        .map(|s| clock(s.saturating_mul(1000), tz));
    words::event(e, frees_at.as_deref())
}

/// Each event's time as the list shows it (§5.3's form), newest first, and the widest, which
/// the rows and the head pad to.
pub(crate) struct Times {
    at: Vec<String>,
    wide: usize,
}

impl Times {
    pub(crate) fn of(view: &TimelineView, now_ms: i64, tz: &TimeZone) -> Times {
        let at: Vec<String> = view
            .events
            .iter()
            .rev()
            .map(|e| autoline::when(e.at_ms, now_ms, tz))
            .collect();
        let wide = at.iter().map(|t| width_of(t)).max().unwrap_or(0);
        Times { at, wide }
    }
}

/// The list's rows in `rows` rows, newest first, the selected one kept in view (T4-20).
pub(crate) fn list_rows(
    state: &State,
    view: &TimelineView,
    times: &Times,
    rows: u16,
    width: u16,
) -> Vec<Line<'static>> {
    let (theme, h, tz) = (state.theme, &state.history, state.tokens.tz());
    let muted = theme.role(Role::Muted);
    let rows = usize::from(rows);
    let room = usize::from(width).saturating_sub(RIGHT_INSET);
    if rows == 0 {
        return vec![];
    }
    if view.events.is_empty() {
        let none = vec![Span::raw(LEAD), Span::styled(h.range.no_events(), muted)];
        return vec![Line::from(fit_spans(none, room))];
    }
    let len = view.events.len();
    let (first, shown, more) = window(len, h.selected, rows, h.list_from);
    let mut lines: Vec<Line<'static>> = (first..first + shown)
        .map(|i| {
            let e = &view.events[len - 1 - i];
            let m = mark(e);
            let spans = vec![
                Span::raw(LEAD),
                Span::styled(pad(&times.at[i], times.wide), muted),
                Span::raw(GAP),
                Span::styled(m.glyph, theme.role(m.role)),
                Span::raw(" "),
                Span::raw(sentence(e, tz)),
            ];
            let line = Line::from(fit_spans(spans, room));
            if h.selected == Some(i) {
                select(line, Some(SENTENCE), usize::from(width), theme)
            } else {
                line
            }
        })
        .collect();
    if let Some(n) = more {
        let to = if h.full_list {
            "z charts"
        } else {
            "z full list"
        };
        let text = format!("{LEAD}⋮ {n} more · {to}");
        lines.push(Line::from(fit_spans(vec![Span::styled(text, muted)], room)));
    }
    lines
}

/// The list's head, `WHEN` over the times and `WHAT` over the glyphs, muted (T4-20).
pub(crate) fn head_row(times: &Times, theme: Theme) -> Line<'static> {
    let text = format!(
        "{LEAD}{}{GAP}{WHAT}",
        pad(WHEN, times.wide.max(width_of(WHEN)))
    );
    Line::from(Span::styled(text, theme.role(Role::Muted)))
}

#[cfg(test)]
mod tests {
    use ratatui::style::Modifier;
    use tagteam_engine::timeline::TimelineView;
    use tagteam_engine::{EventKind, EventSource};

    use crate::msg::Msg;
    use crate::screens::history::{account_ref, event_at, three_events};
    use crate::state::{Screen, State};
    use crate::testutil::*;
    use crate::theme::Theme;
    use crate::update::update;

    /// History over three accounts, `view` read and placed.
    fn shown(view: TimelineView) -> State {
        let mut state = fresh(Screen::History);
        state.accounts.apply(snapshot(many(3)));
        let r#gen = state.gens.history;
        update(
            &mut state,
            Msg::Timeline {
                r#gen,
                result: Ok(view),
            },
            NOW_MS,
        );
        state
    }

    fn with_events(events: Vec<tagteam_engine::timeline::EventView>) -> TimelineView {
        let lanes = (1..=3)
            .map(|p| lane_of(p, vec![Some(10.0); 70], vec![true; 70]))
            .collect();
        let mut view = timeline_view(70, vec![], lanes);
        view.events = events;
        view
    }

    /// `n` events, an account added each hour before now, oldest first.
    fn hourly(n: i64) -> Vec<tagteam_engine::timeline::EventView> {
        (1..=n)
            .rev()
            .map(|k| {
                let mut e = event_at(NOW_MS - k * HOUR * 1000, EventKind::Add, EventSource::Cli);
                e.to = Some(account_ref(1));
                e
            })
            .collect()
    }

    /// Each part's characters in its role letter, spaces kept.
    fn letters(parts: &[(&str, char)]) -> String {
        parts
            .iter()
            .flat_map(|(text, role)| {
                text.chars()
                    .map(move |c| if c == ' ' { ' ' } else { *role })
            })
            .collect()
    }

    #[test]
    fn the_list_is_newest_first_with_its_time_glyph_and_sentence() {
        let mut state = shown(with_events(three_events()));
        let rows = [
            "▌  11:28       ⇄ auto switched 1 a1 → 2 a2 · a1's 5h reached 93%",
            "   Sat 11:33   ✓ 3 a3 works again · new login stored",
            "   Sat 08:58   ✕ 3 a3 needs a new login · its stored login was refused (invali…",
        ];
        let body = body_of(&state, 80, 24, NOW_MS);
        assert_eq!(body[16..19], rows, "{body:?}");
        assert_eq!(body[19], "", "three fit: no more line");

        // Times muted, glyphs in their roles, the sentences text.
        let theme = Theme::new(true);
        state.theme = theme;
        let buf = draw_buffer(&state, 80, 24, NOW_MS);
        let y = crate::view::areas(&state, ratatui::layout::Rect::new(0, 0, 80, 24))
            .body
            .y
            + 16;
        let want = [
            letters(&[
                ("▌", 'a'),
                ("  ", ' '),
                ("11:28", 'm'),
                ("       ", ' '),
                ("⇄", 'a'),
                (" ", ' '),
                ("auto switched 1 a1 → 2 a2 · a1's 5h reached 93%", 't'),
            ]),
            letters(&[
                ("   ", ' '),
                ("Sat 11:33", 'm'),
                ("   ", ' '),
                ("✓", 'o'),
                (" ", ' '),
                ("3 a3 works again · new login stored", 't'),
            ]),
            letters(&[
                ("   ", ' '),
                ("Sat 08:58", 'm'),
                ("   ", ' '),
                ("✕", 'c'),
                (" ", ' '),
                (
                    "3 a3 needs a new login · its stored login was refused (invali…",
                    't',
                ),
            ]),
        ];
        for (i, want) in want.iter().enumerate() {
            let y = y + i as u16;
            assert_eq!(
                roles(&buf, theme, y).trim_end(),
                canon(theme, want),
                "row {i}"
            );
        }
        // The selected row's sentence is bold; the others' are not.
        let bold = |x: u16, y: u16| buf[(x, y)].modifier.contains(Modifier::BOLD);
        assert!((17..60).all(|x| bold(x, y)), "the selected sentence");
        assert!(!(17..40).any(|x| bold(x, y + 1)), "another row's");
        assert!(!bold(3, y), "the time is not bold");

        // Tier 2: the selected row on sel-bg, the others not.
        let theme = TIERS[2];
        state.theme = theme;
        let buf = draw_buffer(&state, 80, 24, NOW_MS);
        let sel = theme.sel_bg().bg.expect("tier 2 has sel-bg");
        assert_eq!(buf[(5, y)].bg, sel);
        assert_eq!(buf[(70, y)].bg, sel, "across the row");
        assert_ne!(buf[(5, y + 1)].bg, sel);

        // From 120 columns the head aligns with the rows.
        let body = body_of(&state, 120, 40, NOW_MS);
        let head = body
            .iter()
            .position(|r| r.starts_with("   WHEN"))
            .unwrap_or_else(|| panic!("no head: {body:?}"));
        assert_eq!(body[head], "   WHEN        WHAT");
        assert!(
            body[head + 1].starts_with("▌  11:28       ⇄ auto"),
            "{body:?}"
        );
    }

    #[test]
    fn the_list_overflows_into_a_more_line_and_z_gives_it_the_body() {
        let mut state = shown(with_events(hourly(12)));
        let body = body_of(&state, 80, 24, NOW_MS);
        assert_eq!(
            body[16..20],
            [
                "▌  13:13   + 1 a1 added",
                "   12:13   + 1 a1 added",
                "   11:13   + 1 a1 added",
                "   ⋮ 9 more · z full list",
            ],
            "{body:?}"
        );
        let theme = Theme::new(true);
        state.theme = theme;
        let buf = draw_buffer(&state, 80, 24, NOW_MS);
        let y = crate::view::areas(&state, ratatui::layout::Rect::new(0, 0, 80, 24))
            .body
            .y;
        assert_eq!(
            roles(&buf, theme, y + 19).trim(),
            canon(theme, &letters(&[("⋮ 9 more · z full list", 'm')]))
        );

        // `z`: every row under the scope row is the list's; all twelve fit.
        update(&mut state, ch('z'), NOW_MS);
        let body = body_of(&state, 80, 24, NOW_MS);
        assert!(body[0].starts_with(" f all accounts"), "{body:?}");
        assert_eq!(body[1], "▌  13:13   + 1 a1 added", "{body:?}");
        assert_eq!(body[12], "   02:13   + 1 a1 added", "{body:?}");
        assert!(body[13..].iter().all(String::is_empty), "{body:?}");
        // `z` again gives the charts back.
        update(&mut state, ch('z'), NOW_MS);
        let again = body_of(&state, 80, 24, NOW_MS);
        assert_eq!(
            again[16..20],
            body_of(&shown(with_events(hourly(12))), 80, 24, NOW_MS)[16..20]
        );

        // `j` to the fourth row scrolls the list.
        for _ in 0..3 {
            update(&mut state, ch('j'), NOW_MS);
        }
        let body = body_of(&state, 80, 24, NOW_MS);
        assert_eq!(
            body[16..20],
            [
                "   12:13   + 1 a1 added",
                "   11:13   + 1 a1 added",
                "▌  10:13   + 1 a1 added",
                "   ⋮ 9 more · z full list",
            ],
            "{body:?}"
        );

        // More than the body holds: the whole-body list says `z charts` on its last row.
        let mut state = shown(with_events(hourly(30)));
        update(&mut state, ch('z'), NOW_MS);
        let body = body_of(&state, 80, 24, NOW_MS);
        assert_eq!(body[1], "▌  13:13       + 1 a1 added", "{body:?}");
        assert_eq!(body[18], "   Sun 20:13   + 1 a1 added", "{body:?}");
        assert_eq!(body[19], "   ⋮ 12 more · z charts", "{body:?}");
    }

    /// `view` delivered again under the same read: a refresh, keeping the cursor.
    fn refreshed(state: &mut State, view: TimelineView) {
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

    /// Every row `state` draws at `w`×`h` is within `w` columns.
    fn within(state: &State, w: u16, h: u16) {
        for (y, row) in draw(state, w, h).iter().enumerate() {
            assert!(
                crate::view::width_of(row) <= usize::from(w),
                "{w}×{h} row {y} is {} wide: {row:?}",
                crate::view::width_of(row)
            );
        }
    }

    #[test]
    fn no_row_is_wider_than_the_screen() {
        // A stored message with control characters reads as one row.
        let mut failed = event_at(
            NOW_MS - 3 * HOUR * 1000,
            EventKind::SwitchFailed,
            EventSource::Auto,
        );
        failed.to = Some(account_ref(2));
        failed.detail = Some(serde_json::json!({"message": "a\nb\tc", "transient": false}));
        // A CJK alias, and a removed account's 200-character label.
        let mut wide = event_at(
            NOW_MS - 2 * HOUR * 1000,
            EventKind::Switch,
            EventSource::Cli,
        );
        wide.from = Some(account_ref(1));
        wide.to = Some(tagteam_engine::timeline::AccountRef {
            alias: Some("工作用のアカウント".repeat(8)),
            ..account_ref(2)
        });
        let mut long = event_at(NOW_MS - HOUR * 1000, EventKind::Switch, EventSource::Auto);
        long.from = Some(account_ref(1));
        long.to = Some(tagteam_engine::timeline::AccountRef {
            id: tagteam_core::AccountId::from_string("gone"),
            position: None,
            alias: None,
            email: Some(format!("{}@example.com", "x".repeat(188))),
            removed: true,
        });
        let state = shown(with_events(vec![failed, wide, long]));
        let body = body_of(&state, 80, 24, NOW_MS);
        assert!(
            body[18].starts_with("   11:13   ✕ auto couldn't switch to 2 a2 · a b c"),
            "{body:?}"
        );
        for (w, h) in [(80, 24), (120, 40), (300, 60)] {
            within(&state, w, h);
        }
        let mut state = state;
        update(&mut state, ch('j'), NOW_MS);
        for (w, h) in [(80, 24), (120, 40), (300, 60)] {
            within(&state, w, h);
        }
    }

    #[test]
    fn a_refresh_with_fewer_events_keeps_the_selection_and_the_window_valid() {
        let twelve = hourly(12);
        let mut state = shown(with_events(twelve.clone()));
        update(&mut state, key(crossterm::event::KeyCode::Home), NOW_MS);
        assert_eq!(state.history.selected, Some(11), "the last row");
        // The oldest five: the cursor's event is now the last of five rows.
        refreshed(&mut state, with_events(twelve[..5].to_vec()));
        assert_eq!(state.history.selected, Some(4));
        let body = body_of(&state, 80, 24, NOW_MS);
        assert_eq!(
            body[16..20],
            [
                "   04:13   + 1 a1 added",
                "   03:13   + 1 a1 added",
                "▌  02:13   + 1 a1 added",
                "   ⋮ 2 more · z full list",
            ],
            "{body:?}"
        );
        // The newest two, both after the cursor: no selection, and the list from its top.
        refreshed(&mut state, with_events(twelve[10..].to_vec()));
        assert_eq!(state.history.selected, None);
        let body = body_of(&state, 80, 24, NOW_MS);
        assert_eq!(
            body[16..19],
            ["   13:13   + 1 a1 added", "   12:13   + 1 a1 added", ""],
            "{body:?}"
        );
        update(&mut state, ch('k'), NOW_MS);
        assert_eq!(state.history.selected, Some(1));
    }

    #[test]
    fn all_exhausted_says_when_the_first_account_frees_up_in_the_tui_s_clock() {
        // `earliest_reset_at` is stored in seconds: 2h10m after now is 16:23.
        let mut exhausted = event_at(
            NOW_MS - HOUR * 1000,
            EventKind::AllExhausted,
            EventSource::Auto,
        );
        exhausted.from = Some(account_ref(1));
        exhausted.detail = Some(serde_json::json!({"earliest_reset_at": NOW_S + 2 * HOUR + 600}));
        let state = shown(with_events(vec![exhausted]));
        let body = body_of(&state, 120, 40, NOW_MS);
        let row = body
            .iter()
            .find(|r| r.contains("near its limit"))
            .unwrap_or_else(|| panic!("{body:?}"));
        assert!(
            row.ends_with("✕ auto couldn't switch · every account is near its limit; the first frees up at 16:23"),
            "{row:?}"
        );
    }
}
