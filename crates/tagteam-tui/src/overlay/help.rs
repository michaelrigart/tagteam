//! The help panel (spec §9.2): the current screen's keys, then every screen's, then what the
//! marks mean, as a scrolling panel. The keys come from the key table, the same one the footer
//! is drawn from.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use super::panel::{panel, panel_width, scrolled};
use super::{BoxSpec, KeyOutcome};
use crate::keys::KEYS;
use crate::screens::history::{chart, lanes};
use crate::screens::tokens::table::BEFORE_TRACKING;
use crate::state::{Screen, State};
use crate::theme::{Role, Theme};
use crate::view::{body_at, width_of, wrap};

/// From this many columns wide the help runs in two columns (§9.2).
const TWO_COLUMNS: u16 = 100;
/// The columns between the two.
const COLUMN_GAP: usize = 3;

/// §4.4's marks, each with the role it is drawn in and its words (T2-25). Every glyph is one
/// column wide.
const MARKS: &[(&str, Role, &str)] = &[
    ("●", Role::Accent, "in use"),
    ("▶", Role::Accent, "in a session"),
    ("○", Role::Muted, "disabled"),
    ("▲", Role::Warn, "faster than an even pace"),
    ("✕", Role::Crit, "needs you"),
    ("!", Role::Warn, "tagteam retries"),
    ("✓", Role::Ok, "done"),
    (
        "≥",
        Role::Warn,
        "at least: still being counted, or a cost missing some prices",
    ),
    (
        "~",
        Role::Text,
        "estimated: a 5h block placed by rule, not read from a reset",
    ),
    ("›", Role::Muted, "breadcrumb"),
    ("·", Role::Muted, "separator"),
    ("⏎", Role::Accent, "Enter"),
    ("⇄", Role::Accent, "switch"),
    ("‖", Role::Warn, "pause"),
    ("⠋", Role::Accent, "spinner"),
];

/// Every mark help lists, in `theme`'s tier: `MARKS`, then History's lane and chart glyphs
/// (T4-31), the pause's span and the threshold line as that tier draws them, so the legend
/// decodes the screen.
pub(crate) fn marks(theme: Theme) -> Vec<(&'static str, Role, &'static str)> {
    let history = [
        (lanes::pause_span(theme), Role::Warn, "paused meanwhile"),
        ("+", Role::Muted, "added"),
        ("-", Role::Muted, "removed"),
        (chart::threshold_line(theme), Role::Faint, "the threshold"),
    ];
    MARKS.iter().copied().chain(history).collect()
}

/// How a rate is measured (T3-9): the projections no longer name their method, so the help does.
const RATE: (&str, &str) = (
    "%/h",
    "how fast a window fills: a fit through its last two days of readings, or its average so \
     far when there are too few",
);

/// What Tokens' cache-write halves are (T5-25): `CACHE W 5M` and `1H` are columns, not marks, so
/// the help names them beside the rate.
const CACHE: (&str, &str) = (
    "5M 1H",
    "CACHE W 5M and 1H, from 160 columns: cache writes kept 5 minutes and 1 hour; CACHE W is \
     both",
);

/// What Tokens' `before tracking` account is, under Tokens' keys: it is a word in the table, not
/// a mark.
const BEFORE_TRACKING_HELP: &str = "usage from before tagteam recorded which account was in use; \
                                    the filter's unknown account includes it";

pub(super) fn key(state: &State, scroll: &mut u16, key: KeyEvent) -> KeyOutcome {
    match key.code {
        KeyCode::Char('?') | KeyCode::Esc | KeyCode::Enter => KeyOutcome::Close,
        code => {
            let rows = rows(state, state.size.0).len();
            *scroll = scrolled(*scroll, code, rows, body_at(state).height);
            KeyOutcome::Keep
        }
    }
}

pub(super) fn render(state: &State, frame: &mut Frame, area: Rect, scroll: u16) {
    let lines = rows(state, area.width);
    let spec = BoxSpec {
        title: "Help".to_owned(),
        keys: "Esc close · ↑ ↓ scroll",
        destructive: false,
        width: area.width,
    };
    panel(state, frame, spec, lines, scroll);
}

fn screen_name(screen: Screen) -> &'static str {
    match screen {
        Screen::Accounts => "Accounts",
        Screen::Tokens => "Tokens",
        Screen::History => "History",
    }
}

/// The help's rows on a terminal `width` wide: three sections, in one column, or from
/// `TWO_COLUMNS` in two, split between sections where that balances them best, the left one
/// first. A row of the panel is a row of the left column and one of the right.
pub(super) fn rows(state: &State, width: u16) -> Vec<Line<'static>> {
    let two = width >= TWO_COLUMNS;
    let inner = panel_width(width);
    let column = if two {
        inner.saturating_sub(COLUMN_GAP) / 2
    } else {
        inner
    };
    let theme = state.theme;
    let title = |name: &str| Line::from(Span::styled(name.to_owned(), theme.strong()));
    // A key list never wraps: its column is as wide as the widest list in its own section.
    let keys_of = |screen: Option<Screen>| {
        let rows = || KEYS.iter().filter(|row| row.screen == screen);
        let widest = rows().map(|row| width_of(row.keys)).max().unwrap_or(1);
        let keys_width = widest.min(column.saturating_sub(5)).max(1);
        let mut lines = vec![title(screen.map_or("Everywhere", screen_name))];
        for row in rows() {
            let accent = theme.role(Role::Accent);
            lines.extend(entry(row.keys, accent, row.does, keys_width, column));
        }
        if screen == Some(Screen::Tokens) {
            lines.extend(entry(
                BEFORE_TRACKING,
                theme.role(Role::Muted),
                BEFORE_TRACKING_HELP,
                width_of(BEFORE_TRACKING),
                column,
            ));
        }
        lines
    };
    // One lead column for the rate, the cache halves and every mark, as wide as the widest, so
    // their words line up.
    let marks_listed = marks(theme);
    let lead_width = marks_listed
        .iter()
        .map(|(glyph, _, _)| width_of(glyph))
        .chain([RATE, CACHE].map(|(lead, _)| width_of(lead)))
        .fold(1, usize::max);
    let mut marks = vec![title("Marks")];
    for (lead, words) in [RATE, CACHE] {
        marks.extend(entry(
            lead,
            theme.role(Role::Muted),
            words,
            lead_width,
            column,
        ));
    }
    for (glyph, role, words) in &marks_listed {
        marks.extend(entry(glyph, theme.role(*role), words, lead_width, column));
    }
    let sections = [keys_of(Some(state.screen)), keys_of(None), marks];
    if !two {
        return join(&sections);
    }
    // Rows count a blank one between sections.
    let at = (1..sections.len())
        .min_by_key(|at| {
            let (left, right) = sections.split_at(*at);
            join(left).len().abs_diff(join(right).len())
        })
        .unwrap_or(1);
    let (left, right) = sections.split_at(at);
    let (left, right) = (join(left), join(right));
    (0..left.len().max(right.len()))
        .map(|i| {
            let mut spans = left.get(i).map(|l| l.spans.clone()).unwrap_or_default();
            let used: usize = spans.iter().map(|s| width_of(&s.content)).sum();
            spans.push(Span::raw(
                " ".repeat(column.saturating_sub(used) + COLUMN_GAP),
            ));
            spans.extend(right.get(i).map(|l| l.spans.clone()).unwrap_or_default());
            Line::from(spans)
        })
        .collect()
}

/// `sections` one under another, a blank row between.
fn join(sections: &[Vec<Line<'static>>]) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (n, section) in sections.iter().enumerate() {
        if n > 0 {
            lines.push(Line::default());
        }
        lines.extend(section.iter().cloned());
    }
    lines
}

/// One entry, indented two columns: `lead` in `style` in a column `lead_width` wide, two
/// columns, then `words`, both wrapped to what a `column`-wide column leaves them. A wrapped
/// entry's continuation rows stay under their own part. Only the lead itself takes `style`; its
/// padding goes with the gap.
fn entry(
    lead: &str,
    style: Style,
    words: &str,
    lead_width: usize,
    column: usize,
) -> Vec<Line<'static>> {
    let words_width = column.saturating_sub(2 + lead_width + 2).max(1);
    let leads = wrap(lead, lead_width);
    let words = wrap(words, words_width);
    (0..leads.len().max(words.len()))
        .map(|i| {
            let lead = leads.get(i).map_or("", String::as_str);
            let words = words.get(i).map_or("", String::as_str);
            Line::from(vec![
                Span::raw("  "),
                Span::styled(lead.to_owned(), style),
                Span::raw(" ".repeat(lead_width.saturating_sub(width_of(lead)) + 2)),
                Span::raw(words.to_owned()),
            ])
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyCode;

    use ratatui::text::{Line, Span};

    use super::{
        BEFORE_TRACKING, BEFORE_TRACKING_HELP, KEYS, Role, Screen, State, marks, rows, width_of,
    };
    use crate::overlay::Overlay;
    use crate::testutil::*;
    use crate::update::update;

    /// A row of the drawn panel between its borders, trimmed.
    fn inner(line: &str) -> &str {
        line.split('│').nth(1).unwrap_or(line).trim()
    }

    fn open(screen: Screen) -> State {
        let mut state = fresh(screen);
        update(&mut state, ch('?'), NOW_MS);
        state
    }

    /// Every key list of `screen`'s help (its own rows and Everywhere's) is one span of one
    /// row at `width` columns: none wraps onto a second row.
    fn assert_key_lists_whole(screen: Screen, width: u16) {
        let mut state = open(screen);
        state.size = (width, 40);
        let rows = rows(&state, width);
        let spans: Vec<&str> = rows
            .iter()
            .flat_map(|line| &line.spans)
            .map(|span| span.content.trim_end())
            .collect();
        for row in KEYS.iter().filter(|r| r.screen.is_none_or(|s| s == screen)) {
            assert!(
                spans.contains(&row.keys),
                "{screen:?} at {width}: {} wraps",
                row.keys
            );
        }
    }

    #[test]
    fn help_keys_stay_on_one_line_at_100_and_120_columns() {
        // §9.2 brings in two columns at 100: the key lists must not break there (R21).
        for screen in [Screen::Accounts, Screen::Tokens, Screen::History] {
            for width in [80, 99, 100, 110, 120, 160] {
                assert_key_lists_whole(screen, width);
            }
            for (width, height) in [(100, 30), (120, 40)] {
                let lines = draw(&open(screen), width, height);
                assert!(lines.iter().all(|l| width_of(l) <= usize::from(width)));
                assert!(
                    screen == Screen::History
                        || lines.iter().any(|l| l.contains("PgUp PgDn Home End")),
                    "{screen:?} {width}: the jump keys on one row: {lines:?}"
                );
            }
        }
    }

    #[test]
    fn every_help_line_shows_whole_on_every_screen_at_80_columns() {
        // R21: the key column and the description both fit the help box, whichever the screen.
        // The panel is tall enough to show every row; scrolling is the next test's.
        for screen in [Screen::Accounts, Screen::Tokens, Screen::History] {
            let lines = draw(&open(screen), 80, 70);
            for row in KEYS.iter().filter(|r| r.screen.is_none_or(|s| s == screen)) {
                assert!(
                    lines
                        .iter()
                        .any(|l| l.contains(row.keys) && l.contains(row.does)),
                    "{screen:?}: {} {}: {lines:?}",
                    row.keys,
                    row.does
                );
            }
            assert!(lines.iter().all(|l| width_of(l) <= 80), "{lines:?}");
            assert_key_lists_whole(screen, 80);
        }
    }

    #[test]
    fn help_lists_this_screen_then_every_screen_then_the_marks() {
        let state = open(Screen::Accounts);
        let lines = draw(&state, 80, 70);
        let at = |name: &str| {
            lines
                .iter()
                .position(|l| inner(l) == name)
                .unwrap_or_else(|| panic!("{name}: {lines:?}"))
        };
        let (screen, everywhere, marks_at) = (at("Accounts"), at("Everywhere"), at("Marks"));
        assert!(screen < everywhere && everywhere < marks_at, "{lines:?}");
        for row in KEYS {
            let section = match row.screen {
                Some(Screen::Accounts) => screen..everywhere,
                None => everywhere..marks_at,
                Some(_) => continue,
            };
            assert!(
                lines[section]
                    .iter()
                    .any(|l| l.contains(row.keys) && l.contains(row.does)),
                "{} {}: {lines:?}",
                row.keys,
                row.does
            );
        }
        assert!(
            !lines.iter().any(|l| l.contains("drill down")),
            "another screen's keys are not listed: {lines:?}"
        );
        for (glyph, _, words) in marks(state.theme) {
            assert!(
                lines[marks_at..]
                    .iter()
                    .any(|l| l.contains(glyph) && l.contains(words)),
                "{glyph} {words}: {lines:?}"
            );
        }
    }

    #[test]
    fn accounts_help_lists_the_full_list_key_and_keeps_the_pace_mark() {
        let lines = draw(&open(Screen::Accounts), 80, 70);
        for (keys, does) in [
            ("v", "full list, or back to compact"),
            ("s", "switch to the selected account"),
            ("r", "update usage now"),
            ("J K", "scroll the selected card or block"),
            ("▲", "faster than an even pace"),
        ] {
            assert!(
                lines.iter().any(|l| {
                    let row = inner(l);
                    row.starts_with(keys) && row.contains(does)
                }),
                "{keys} {does}: {lines:?}"
            );
        }
    }

    #[test]
    fn help_has_two_columns_from_100_columns_and_scrolls_to_its_end() {
        let state = open(Screen::Accounts);
        let buf = draw_buffer(&state, 120, 40, NOW_MS);
        let (marks_x, _) = locate(&buf, "Marks").unwrap();
        let (screen_x, _) = locate(&buf, "Accounts").unwrap();
        assert!(screen_x < 60 && marks_x > 60, "{screen_x} {marks_x}");
        assert!(
            draw(&state, 99, 40).iter().all(|l| width_of(l) <= 99),
            "one column below 100"
        );
        let one = draw_buffer(&state, 99, 40, NOW_MS);
        let (marks_x, marks_y) = locate(&one, "Marks").unwrap();
        let (_, everywhere_y) = locate(&one, "Everywhere").unwrap();
        assert!(
            marks_x < 60 && marks_y > everywhere_y,
            "{marks_x} {marks_y}"
        );

        let mut state = open(Screen::Accounts);
        let all = marks(state.theme);
        let last = all.last().unwrap();
        assert!(
            !draw(&state, 80, 24).iter().any(|l| l.contains(last.2)),
            "the last mark is below the first page"
        );
        update(&mut state, key(KeyCode::End), NOW_MS);
        let end = draw(&state, 80, 24);
        let at = end.iter().position(|l| l.contains(last.2)).unwrap();
        assert!(inner(&end[at]).starts_with(last.0), "{end:?}");
        assert!(
            inner(&end[at + 1]).is_empty() && end[at + 2].trim_start().starts_with('╰'),
            "the last row ends the panel, its padding row and border below: {end:?}"
        );
        // The panel's text starts two rows (border, padding) under the body's top.
        let first = |state: &State| panel_text(state, 80, 24).remove(0);
        update(&mut state, key(KeyCode::Home), NOW_MS);
        assert!(first(&state).starts_with("Accounts"));
        update(&mut state, key(KeyCode::PageDown), NOW_MS);
        let after = panel_text(&state, 80, 24);
        assert!(!after[0].starts_with("Accounts"), "{after:?}");
        update(&mut state, key(KeyCode::PageUp), NOW_MS);
        assert!(first(&state).starts_with("Accounts"));
    }

    #[test]
    fn help_scrolls_with_the_move_keys_and_four_keys_close_it() {
        let mut state = open(Screen::Accounts);
        for c in ['j', 'j'] {
            update(&mut state, ch(c), NOW_MS);
        }
        assert!(matches!(state.overlay, Some(Overlay::Help { scroll: 2 })));
        update(&mut state, ch('k'), NOW_MS);
        assert!(matches!(state.overlay, Some(Overlay::Help { scroll: 1 })));
        for close in [ch('?'), ch('q'), key(KeyCode::Esc), key(KeyCode::Enter)] {
            let mut state = open(Screen::Tokens);
            assert!(update(&mut state, close, NOW_MS).is_empty());
            assert!(state.overlay.is_none());
            assert_eq!(state.quitting, None);
        }
    }

    #[test]
    fn help_names_how_the_rate_is_measured() {
        // T3-9: the projection no longer names its method; the help says it once, under Marks.
        let state = open(Screen::Accounts);
        let rows = rows(&state, 80);
        let text = |line: &Line<'_>| -> Vec<String> {
            line.spans.iter().map(|s| s.content.to_string()).collect()
        };
        let marks = rows
            .iter()
            .position(|l| text(l).concat() == "Marks")
            .unwrap();
        let at = marks
            + rows[marks..]
                .iter()
                .position(|l| text(l).get(1).is_some_and(|lead| lead == "%/h"))
                .unwrap_or_else(|| panic!("{rows:?}"));
        assert_eq!(rows[at].spans[1].style, state.theme.role(Role::Muted));
        // The entry's continuation rows have a blank lead.
        let said: Vec<String> = std::iter::once(&rows[at])
            .chain(
                rows[at + 1..]
                    .iter()
                    .take_while(|l| text(l).get(1).is_some_and(|lead| lead.trim().is_empty())),
            )
            .map(|l| text(l)[3].clone())
            .collect();
        assert_eq!(
            said.join(" "),
            "how fast a window fills: a fit through its last two days of readings, or its \
             average so far when there are too few"
        );
    }

    #[test]
    fn every_mark_s_words_start_in_one_column() {
        // `5M 1H` is five columns wide: the glyphs' words line up with its words and `%/h`'s.
        let state = open(Screen::Accounts);
        for width in [80, 99] {
            let rows = rows(&state, width);
            let text = |line: &Line<'_>| -> Vec<String> {
                line.spans.iter().map(|s| s.content.to_string()).collect()
            };
            let marks_at = rows
                .iter()
                .position(|l| text(l).concat() == "Marks")
                .unwrap();
            let starts: Vec<(String, usize)> = rows[marks_at + 1..]
                .iter()
                .map(|l| text(l))
                .filter(|t| t.len() == 4 && !t[1].trim().is_empty())
                .map(|t| (t[1].trim().to_owned(), width_of(&t[..3].concat())))
                .collect();
            assert_eq!(
                starts.len(),
                marks(state.theme).len() + 2,
                "{width}: {starts:?}"
            );
            assert!(
                starts.iter().all(|(_, at)| *at == starts[0].1),
                "{width}: {starts:?}"
            );
        }
    }

    #[test]
    fn every_mark_is_one_column_wide() {
        // §4.4: all glyphs are one column wide under `unicode-width`.
        for theme in TIERS {
            for (glyph, _, words) in marks(theme) {
                assert_eq!(width_of(glyph), 1, "{glyph} {words}");
            }
        }
    }

    /// The lead span of the entry for `words`, in either column: its first row, which may hold
    /// only their start.
    fn lead_of<'a>(rows: &'a [Line<'static>], words: &str) -> Option<&'a Span<'static>> {
        rows.iter().flat_map(|l| l.spans.windows(4)).find_map(|w| {
            let first = w[3].content.trim();
            let shown = !w[1].content.trim().is_empty() && w[0].content == "  ";
            (shown && (words == first || words.starts_with(&format!("{first} ")))).then(|| &w[1])
        })
    }

    #[test]
    fn every_mark_is_drawn_in_its_role() {
        // Found by its words: in tier 0 a pause's span, the threshold and the separator are
        // all `·`.
        for theme in TIERS {
            let mut state = open(Screen::Accounts);
            state.theme = theme;
            for width in [80, 120] {
                let rows = rows(&state, width);
                for (glyph, role, words) in marks(theme) {
                    let lead = lead_of(&rows, words)
                        .unwrap_or_else(|| panic!("{glyph} {words} at {width}"));
                    assert_eq!(lead.content, glyph, "{words} {theme:?}");
                    assert_eq!(lead.style, theme.role(role), "{glyph} {words} {theme:?}");
                }
            }
        }
    }

    /// The entry led by `lead` in either column: its lead span and its words, its continuation
    /// rows' (those under it at the same column, with a blank lead) joined on.
    fn entry_led_by(rows: &[Line<'static>], lead: &str) -> Option<(Span<'static>, String)> {
        // Each row's entries as (column, lead span, words).
        let entries = |line: &Line<'static>| -> Vec<(usize, Span<'static>, String)> {
            let mut x = 0;
            let mut found = Vec::new();
            for (i, span) in line.spans.iter().enumerate() {
                if span.content == "  " && i + 3 < line.spans.len() {
                    let words = line.spans[i + 3].content.trim().to_owned();
                    found.push((x, line.spans[i + 1].clone(), words));
                }
                x += width_of(&span.content);
            }
            found
        };
        rows.iter().enumerate().find_map(|(at, line)| {
            let (x, span, words) = entries(line)
                .into_iter()
                .find(|(_, span, _)| span.content.trim() == lead)?;
            let mut said = vec![words];
            for next in &rows[at + 1..] {
                let below = entries(next).into_iter().find(|(nx, _, _)| *nx == x);
                match below {
                    Some((_, lead, words)) if lead.content.trim().is_empty() => said.push(words),
                    _ => break,
                }
            }
            Some((span, said.join(" ")))
        })
    }

    #[test]
    fn the_marks_explain_at_least_estimated_and_the_cache_write_halves() {
        // T5-25: §9.2's legend decodes Tokens' `≥`, `~` and `CACHE W 5M`/`1H`.
        for theme in TIERS {
            let mut state = open(Screen::Tokens);
            state.theme = theme;
            for width in [80, 120] {
                let rows = rows(&state, width);
                for (lead, role, words) in [
                    (
                        "≥",
                        Role::Warn,
                        "at least: still being counted, or a cost missing some prices",
                    ),
                    (
                        "~",
                        Role::Text,
                        "estimated: a 5h block placed by rule, not read from a reset",
                    ),
                    (
                        "5M 1H",
                        Role::Muted,
                        "CACHE W 5M and 1H, from 160 columns: cache writes kept 5 minutes and \
                         1 hour; CACHE W is both",
                    ),
                ] {
                    let (span, said) = entry_led_by(&rows, lead)
                        .unwrap_or_else(|| panic!("{lead} at {width}: {rows:?}"));
                    assert_eq!(span.content.trim(), lead, "{theme:?} {width}");
                    assert_eq!(span.style, theme.role(role), "{lead} {theme:?} {width}");
                    assert_eq!(said, words, "{lead} {theme:?} {width}");
                }
            }
        }
    }

    #[test]
    fn help_explains_before_tracking_on_tokens_only() {
        let state = open(Screen::Tokens);
        let tokens = rows(&state, 80);
        let (span, said) = entry_led_by(&tokens, BEFORE_TRACKING).unwrap();
        assert_eq!(span.content.trim(), BEFORE_TRACKING);
        assert_eq!(span.style, state.theme.role(Role::Muted));
        assert_eq!(said, BEFORE_TRACKING_HELP);
        assert!(said.contains("unknown account"));
        // It is the section's last entry: only its own wrapped words, then a blank row, lie
        // between it and `Everywhere`.
        let at = |f: &dyn Fn(&Line<'_>) -> bool| tokens.iter().position(f).unwrap();
        let led = at(&|l| l.spans.iter().any(|s| s.content.trim() == BEFORE_TRACKING));
        let everywhere = at(&|l| l.spans.iter().any(|s| s.content == "Everywhere"));
        assert!(
            tokens[led + 1..everywhere - 1]
                .iter()
                .all(|l| l.spans[1].content.trim().is_empty())
        );
        assert!(tokens[everywhere - 1].spans.is_empty());
        for screen in [Screen::Accounts, Screen::History] {
            let other = rows(&open(screen), 80);
            assert!(
                !other
                    .iter()
                    .any(|l| l.spans.iter().any(|s| s.content.contains(BEFORE_TRACKING))),
                "{screen:?}"
            );
        }
    }

    #[test]
    fn the_marks_list_history_s_lane_glyphs() {
        // T4-31: §7.4's lane glyphs, each in its role, as the tier draws them: in tier 0 the
        // pause's span and the threshold line are `·`, as on the screen (T4-9, T4-13).
        for theme in TIERS {
            let mut state = open(Screen::History);
            state.theme = theme;
            let rows = rows(&state, 80);
            let plain = theme.colourless();
            for (glyph, role, words) in [
                (
                    if plain { "·" } else { "┄" },
                    Role::Warn,
                    "paused meanwhile",
                ),
                ("+", Role::Muted, "added"),
                ("-", Role::Muted, "removed"),
                (if plain { "·" } else { "┈" }, Role::Faint, "the threshold"),
            ] {
                let lead = lead_of(&rows, words).unwrap_or_else(|| panic!("{words}: {rows:?}"));
                assert_eq!(lead.content, glyph, "{words} {theme:?}");
                assert_eq!(lead.style, theme.role(role), "{glyph} {theme:?}");
            }
        }
        assert!(TIERS.iter().any(|t| t.colourless()) && TIERS.iter().any(|t| !t.colourless()));
    }
}
