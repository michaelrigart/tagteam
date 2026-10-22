//! The key table (spec §9.2, §10): one list that the footer and the help are both drawn from,
//! and that a test holds against `update`.

#[cfg(test)]
use crossterm::event::{KeyCode, KeyModifiers};

use crate::chrome::WIDE;
use crate::daemon;
use crate::screens::history;
use crate::state::{Screen, State};
use crate::view::width_of;

/// One row of the key table (spec §9.2, §10): what help lists, and what the footer shows.
pub(crate) struct KeyRow {
    /// `None`: every screen.
    pub(crate) screen: Option<Screen>,
    /// As help shows them: `j k ↑ ↓`.
    pub(crate) keys: &'static str,
    pub(crate) does: &'static str,
    pub(crate) footer: Footer,
    /// The key events this row stands for, for `every_key_the_table_lists_is_handled`.
    #[cfg(test)]
    pub(crate) probes: &'static [(KeyCode, KeyModifiers)],
}

pub(crate) enum Footer {
    No,
    /// From `from` columns: `key` and the label `label` gives for the state, or nothing when it
    /// cannot apply.
    Show {
        key: &'static str,
        label: fn(&State) -> Option<&'static str>,
        from: u16,
    },
}

/// Shown on the right of every footer, never dropped (§5.5).
pub(crate) const PINNED: [(&str, &str); 2] = [("?", "keys"), ("q", "quit")];
/// The columns the pinned items keep from the right edge, as the header's `? keys` does.
pub(crate) const RIGHT_INSET: usize = 1;
/// The least gap between the footer's items and the pinned ones.
const MIN_GAP: usize = 2;

#[cfg(test)]
const fn plain(c: char) -> (KeyCode, KeyModifiers) {
    (KeyCode::Char(c), KeyModifiers::NONE)
}

/// An upper-case letter, as a terminal reports it.
#[cfg(test)]
const fn shifted(c: char) -> (KeyCode, KeyModifiers) {
    (KeyCode::Char(c), KeyModifiers::SHIFT)
}

#[cfg(test)]
const fn control(c: char) -> (KeyCode, KeyModifiers) {
    (KeyCode::Char(c), KeyModifiers::CONTROL)
}

#[cfg(test)]
const fn named(code: KeyCode) -> (KeyCode, KeyModifiers) {
    (code, KeyModifiers::NONE)
}

/// `label` while an account is selected: VIS-10, no account keys with no accounts.
fn on_account(state: &State, label: &'static str) -> Option<&'static str> {
    state.accounts.selected_view().map(|_| label)
}

/// `P`'s words on Accounts, while an account is selected: `resume auto` when the daemon's job for
/// the provider the header follows is paused, else `pause auto`.
fn pause_label(state: &State) -> Option<&'static str> {
    if state.screen != Screen::Accounts {
        return None;
    }
    let paused = state
        .daemon
        .job(&daemon::focus(state))
        .is_some_and(|job| !job.autoswitch);
    on_account(state, if paused { "resume auto" } else { "pause auto" })
}

/// The keys that move a list screen's selection by a row.
#[cfg(test)]
const STEP: &[(KeyCode, KeyModifiers)] = &[
    plain('j'),
    plain('k'),
    named(KeyCode::Down),
    named(KeyCode::Up),
];

/// The keys that jump: a page, or to either end.
#[cfg(test)]
const JUMP: &[(KeyCode, KeyModifiers)] = &[
    named(KeyCode::PageDown),
    named(KeyCode::PageUp),
    named(KeyCode::Home),
    named(KeyCode::End),
];

/// The table, in the footer's order: the footer shows a screen's items and the everywhere ones
/// that have a label, in this order, so `P` and `r` sit among the screens' own keys, after the
/// ones Accounts shows at 80 columns and before the ones it shows from 120. Help groups the rows
/// by screen and keeps this order within each.
pub(crate) const KEYS: &[KeyRow] = &[
    KeyRow {
        screen: None,
        keys: "1 2 3  Tab Shift-Tab",
        does: "Accounts, Tokens, History",
        footer: Footer::No,
        #[cfg(test)]
        probes: &[
            plain('1'),
            plain('2'),
            plain('3'),
            named(KeyCode::Tab),
            (KeyCode::BackTab, KeyModifiers::SHIFT),
        ],
    },
    KeyRow {
        screen: None,
        keys: "?",
        does: "keys and marks",
        footer: Footer::No,
        #[cfg(test)]
        probes: &[plain('?')],
    },
    KeyRow {
        screen: None,
        keys: "q",
        does: "close the open overlay, else quit",
        footer: Footer::No,
        #[cfg(test)]
        probes: &[plain('q')],
    },
    KeyRow {
        screen: None,
        keys: "Esc",
        does: "close the overlay, go back, or clear warnings",
        footer: Footer::No,
        #[cfg(test)]
        probes: &[named(KeyCode::Esc)],
    },
    KeyRow {
        screen: Some(Screen::Accounts),
        keys: "j k ↑ ↓",
        does: "move up or down",
        footer: Footer::No,
        #[cfg(test)]
        probes: STEP,
    },
    KeyRow {
        screen: Some(Screen::Accounts),
        keys: "PgUp PgDn Home End",
        does: "a page up or down, the first or last",
        footer: Footer::No,
        #[cfg(test)]
        probes: JUMP,
    },
    KeyRow {
        screen: Some(Screen::Accounts),
        keys: "s",
        does: "switch to the selected account",
        footer: Footer::Show {
            key: "s",
            label: |state| on_account(state, "switch"),
            from: 0,
        },
        #[cfg(test)]
        probes: &[plain('s')],
    },
    KeyRow {
        screen: Some(Screen::Accounts),
        keys: "b",
        does: "switch to the best account",
        footer: Footer::Show {
            key: "b",
            label: |state| on_account(state, "best"),
            from: 0,
        },
        #[cfg(test)]
        probes: &[plain('b')],
    },
    KeyRow {
        screen: Some(Screen::Accounts),
        keys: "n",
        does: "switch to the next account",
        footer: Footer::Show {
            key: "n",
            label: |state| on_account(state, "next"),
            from: 0,
        },
        #[cfg(test)]
        probes: &[plain('n')],
    },
    KeyRow {
        screen: Some(Screen::Accounts),
        keys: "⏎",
        does: "its history",
        footer: Footer::Show {
            key: "⏎",
            label: |state| on_account(state, "history"),
            from: 0,
        },
        #[cfg(test)]
        probes: &[named(KeyCode::Enter)],
    },
    KeyRow {
        screen: Some(Screen::Tokens),
        keys: "⏎",
        does: "drill down: sessions, then models",
        footer: Footer::Show {
            key: "⏎",
            label: |state| state.tokens.drill_to(),
            from: 0,
        },
        #[cfg(test)]
        probes: &[named(KeyCode::Enter)],
    },
    KeyRow {
        screen: Some(Screen::Tokens),
        keys: "p",
        does: "period: day, month, session or block",
        footer: Footer::Show {
            key: "p",
            label: |_| Some("period"),
            from: 0,
        },
        #[cfg(test)]
        probes: &[plain('p')],
    },
    KeyRow {
        screen: Some(Screen::Tokens),
        keys: "g",
        does: "group: none, account, model, or both",
        footer: Footer::Show {
            key: "g",
            label: |_| Some("group"),
            from: 0,
        },
        #[cfg(test)]
        probes: &[plain('g')],
    },
    KeyRow {
        screen: Some(Screen::Tokens),
        keys: "f",
        does: "filter accounts and models",
        footer: Footer::Show {
            key: "f",
            label: |_| Some("filter"),
            from: 0,
        },
        #[cfg(test)]
        probes: &[plain('f')],
    },
    KeyRow {
        screen: Some(Screen::Tokens),
        keys: "[ ]",
        does: "range: 7, 30 or 90 days, or all",
        footer: Footer::Show {
            key: "[ ]",
            label: |_| Some("range"),
            from: 0,
        },
        #[cfg(test)]
        probes: &[plain('['), plain(']')],
    },
    KeyRow {
        screen: Some(Screen::Tokens),
        keys: "Esc Backspace",
        does: "back up a level",
        footer: Footer::Show {
            key: "Esc",
            label: |state| (state.tokens.depth() > 0).then_some("back"),
            from: 0,
        },
        #[cfg(test)]
        probes: &[named(KeyCode::Esc), named(KeyCode::Backspace)],
    },
    KeyRow {
        screen: Some(Screen::Tokens),
        keys: "j k ↑ ↓",
        does: "move up or down",
        footer: Footer::No,
        #[cfg(test)]
        probes: STEP,
    },
    KeyRow {
        screen: Some(Screen::Tokens),
        keys: "PgUp PgDn Home End",
        does: "a page up or down, the first or last",
        footer: Footer::No,
        #[cfg(test)]
        probes: JUMP,
    },
    KeyRow {
        screen: Some(Screen::Tokens),
        keys: "J K",
        does: "scroll the detail and the notes",
        footer: Footer::No,
        #[cfg(test)]
        probes: &[shifted('J'), shifted('K')],
    },
    KeyRow {
        screen: Some(Screen::History),
        keys: "← → Home End",
        does: "the cursor by column; the oldest event, now",
        footer: Footer::Show {
            key: "←/→",
            label: |_| Some("cursor"),
            from: 0,
        },
        #[cfg(test)]
        probes: &[
            named(KeyCode::Left),
            named(KeyCode::Right),
            named(KeyCode::Home),
            named(KeyCode::End),
        ],
    },
    KeyRow {
        screen: Some(Screen::History),
        keys: "j k ↑ ↓",
        does: "previous or next event",
        footer: Footer::Show {
            key: "j/k",
            label: |state| history::has_events(state).then_some("event"),
            from: 0,
        },
        #[cfg(test)]
        probes: STEP,
    },
    KeyRow {
        screen: Some(Screen::History),
        keys: "[ ]",
        does: "range: 1, 7, 14 or 30 days",
        footer: Footer::Show {
            key: "[ ]",
            label: |_| Some("range"),
            from: 0,
        },
        #[cfg(test)]
        probes: &[plain('['), plain(']')],
    },
    KeyRow {
        screen: Some(Screen::History),
        keys: "f",
        does: "scope: all accounts or one",
        footer: Footer::Show {
            key: "f",
            label: |_| Some("scope"),
            from: 0,
        },
        #[cfg(test)]
        probes: &[plain('f')],
    },
    KeyRow {
        screen: Some(Screen::History),
        keys: "z",
        does: "the event list on the whole body",
        footer: Footer::Show {
            key: "z",
            label: |state| history::lists(state).then_some("list"),
            from: 0,
        },
        #[cfg(test)]
        probes: &[plain('z')],
    },
    KeyRow {
        screen: Some(Screen::History),
        keys: "J K",
        does: "scroll the charts",
        footer: Footer::No,
        #[cfg(test)]
        probes: &[shifted('J'), shifted('K')],
    },
    KeyRow {
        screen: None,
        keys: "P",
        does: "pause or resume auto (confirm)",
        footer: Footer::Show {
            key: "P",
            label: pause_label,
            from: 0,
        },
        #[cfg(test)]
        probes: &[shifted('P')],
    },
    KeyRow {
        screen: None,
        keys: "r",
        does: "update usage now (Tokens: the index)",
        footer: Footer::Show {
            key: "r",
            label: |_| Some("refresh"),
            from: 0,
        },
        #[cfg(test)]
        probes: &[plain('r')],
    },
    KeyRow {
        screen: Some(Screen::Accounts),
        keys: "J K",
        does: "scroll the selected card",
        footer: Footer::No,
        #[cfg(test)]
        probes: &[shifted('J'), shifted('K')],
    },
    KeyRow {
        screen: Some(Screen::Accounts),
        keys: "e",
        does: "disable or enable",
        footer: Footer::Show {
            key: "e",
            label: |state| {
                state
                    .accounts
                    .selected_view()
                    .map(|v| if v.row.disabled { "enable" } else { "disable" })
            },
            from: WIDE,
        },
        #[cfg(test)]
        probes: &[plain('e')],
    },
    KeyRow {
        screen: Some(Screen::Accounts),
        keys: "a",
        does: "set or clear the alias",
        footer: Footer::Show {
            key: "a",
            label: |state| on_account(state, "alias"),
            from: WIDE,
        },
        #[cfg(test)]
        probes: &[plain('a')],
    },
    KeyRow {
        screen: Some(Screen::Accounts),
        keys: "m",
        does: "move to another position",
        footer: Footer::No,
        #[cfg(test)]
        probes: &[plain('m')],
    },
    KeyRow {
        screen: Some(Screen::Accounts),
        keys: "x",
        does: "remove (confirm)",
        footer: Footer::Show {
            key: "x",
            label: |state| on_account(state, "remove"),
            from: WIDE,
        },
        #[cfg(test)]
        probes: &[plain('x')],
    },
    KeyRow {
        screen: Some(Screen::Accounts),
        keys: "R",
        does: "run a session on it here (confirm)",
        footer: Footer::No,
        #[cfg(test)]
        probes: &[shifted('R')],
    },
    KeyRow {
        screen: None,
        keys: "A",
        does: "add the current login",
        footer: Footer::No,
        #[cfg(test)]
        probes: &[shifted('A')],
    },
    KeyRow {
        screen: None,
        keys: "D",
        does: "doctor",
        footer: Footer::No,
        #[cfg(test)]
        probes: &[shifted('D')],
    },
    KeyRow {
        screen: None,
        keys: "Ctrl-C",
        does: "quit",
        footer: Footer::No,
        #[cfg(test)]
        probes: &[control('c')],
    },
    KeyRow {
        screen: None,
        keys: "Ctrl-Z",
        does: "suspend",
        footer: Footer::No,
        #[cfg(test)]
        probes: &[control('z')],
    },
];

/// The columns of `key` and `label` as the footer draws them: the two a space apart.
fn item_width(key: &str, label: &str) -> usize {
    width_of(key) + 1 + width_of(label)
}

/// The columns the pinned items take, two apart.
pub(crate) fn pinned_width() -> usize {
    let items: usize = PINNED
        .iter()
        .map(|(key, label)| item_width(key, label))
        .sum();
    items + 2 * (PINNED.len() - 1)
}

/// The current screen's footer items, in the table's order, that fit beside the pinned ones on a
/// terminal `width` wide, two columns apart: those the state gives a label and `width` allows,
/// dropped from the end.
pub(crate) fn footer_items(state: &State, width: u16) -> Vec<(&'static str, &'static str)> {
    let room = usize::from(width).saturating_sub(pinned_width() + RIGHT_INSET + MIN_GAP);
    let mut items = Vec::new();
    let mut used = 0;
    for row in KEYS
        .iter()
        .filter(|row| row.screen.is_none_or(|screen| screen == state.screen))
    {
        let Footer::Show { key, label, from } = &row.footer else {
            continue;
        };
        if width < *from {
            continue;
        }
        let Some(label) = label(state) else {
            continue;
        };
        let taken = item_width(key, label) + if items.is_empty() { 0 } else { 2 };
        if used + taken > room {
            break;
        }
        used += taken;
        items.push((*key, label));
    }
    items
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tagteam_core::{Pace, WindowKind};
    use tagteam_engine::timeline::{Scope, Series, TimelineView};
    use tagteam_engine::tokens::TokenPeriod;
    use tagteam_engine::views::UsageView;

    use super::{KEYS, footer_items};
    use crate::msg::{Cmd, Msg};
    use crate::state::{Screen, State};
    use crate::testutil::*;
    use crate::update::update;

    /// A good reading with `n` scoped windows more than the two it has, to overflow a card.
    fn crowded(n: u32) -> UsageView {
        let mut usage = read(120, 9.0, 77.0, false);
        for i in 1..=n {
            let (key, label) = (format!("scoped:M{i}"), format!("M{i}"));
            usage.windows.as_mut().unwrap().push((
                window(&key, &label, WindowKind::Scoped, 40.0, Some(291_630)),
                Pace::default(),
            ));
        }
        usage
    }

    /// Accounts with three accounts and row `selected` (1 to 3) selected.
    fn accounts(selected: u32) -> State {
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snapshot(many(3)));
        for _ in 1..selected {
            update(&mut state, ch('j'), NOW_MS);
        }
        state
    }

    /// The same, each account's card too tall for the pane.
    fn crowded_accounts() -> State {
        let mut state = fresh(Screen::Accounts);
        let all = (1..=3)
            .map(|p| account(p, &format!("a{p}@x.co"), crowded(30)))
            .collect();
        state.accounts.apply(snapshot(all));
        update(&mut state, ch('j'), NOW_MS);
        state
    }

    /// Tokens on a three-row day report with row 2 selected; `unpriced` models make its notes
    /// run past the screen.
    fn tokens(unpriced: u32) -> State {
        let mut state = fresh(Screen::Tokens);
        let rows = (0..3)
            .map(|d| {
                token_row(
                    day(d),
                    Some(row_account(1)),
                    None,
                    counts(1, 2, 3),
                    Some(10_000_000),
                )
            })
            .collect();
        let mut report = token_report(token_query(TokenPeriod::Day), rows);
        report.unpriced_models = (1..=unpriced)
            .map(|n| format!("claude-model-{n:02}"))
            .collect();
        state.tokens.apply(report);
        update(&mut state, ch('j'), NOW_MS);
        state
    }

    /// History over 3 accounts at 7 days, its timeline read.
    fn history() -> State {
        history_scoped(Scope::All, timeline_view(70, vec![], vec![]))
    }

    /// History of `scope` over 3 accounts at 7 days, `view` read.
    fn history_scoped(scope: Scope, view: TimelineView) -> State {
        let mut state = fresh(Screen::History);
        state.accounts.apply(snapshot(many(3)));
        state.history.scope = scope;
        let r#gen = state.gens.history;
        let result = Ok(view);
        update(&mut state, Msg::Timeline { r#gen, result }, NOW_MS);
        state
    }

    /// The same, the cursor a column before now: each cursor key moves it.
    fn cursor_moved() -> State {
        let mut state = history();
        update(&mut state, key(KeyCode::Left), NOW_MS);
        state
    }

    /// History with three events, the middle one selected: `j` and `k` each move from it, and
    /// `z` has a list to give the body.
    fn with_events() -> State {
        use tagteam_engine::{EventKind, EventSource};
        let mut view = timeline_view(70, vec![], vec![]);
        view.events = (1..=3)
            .rev()
            .map(|k| {
                let at = NOW_MS - k * HOUR * 1000;
                crate::screens::history::event_at(at, EventKind::Add, EventSource::Cli)
            })
            .collect();
        let mut state = history_scoped(Scope::All, view);
        update(&mut state, ch('j'), NOW_MS);
        state
    }

    /// Account 1's History with four windows' charts, two of which fit at 80×24.
    fn four_charts() -> State {
        let chart = |label: &str| Series {
            label: label.into(),
            peaks: vec![Some(10.0); 70],
        };
        let charts = ["5h", "7d", "Fable", "Sonnet"].map(chart).to_vec();
        let lane = lane_of(1, vec![Some(10.0); 70], vec![true; 70]);
        history_scoped(Scope::Account(id(1)), timeline_view(70, charts, vec![lane]))
    }

    /// `state` with a start-up warning showing.
    fn warned(mut state: State) -> State {
        state
            .startup
            .warnings
            .push("config.toml: unknown key x (ignored)".into());
        state
    }

    fn opened(mut state: State) -> State {
        update(&mut state, ch('?'), NOW_MS);
        state
    }

    /// `state` after one `J`: what a `K` has to scroll back.
    fn scrolled(mut state: State) -> State {
        update(&mut state, ch('J'), NOW_MS);
        state
    }

    /// The states in which `code` (with Ctrl when `ctrl`) must act, for a probe of a row on
    /// `screen`. A key with no arm has no state, and fails the test: a row added to the table
    /// needs its state here.
    fn states(screen: Option<Screen>, code: KeyCode, ctrl: bool) -> Vec<State> {
        use KeyCode::{BackTab, Backspace, Char, Down, End, Enter, Esc, Home, PageDown, PageUp};
        use KeyCode::{Left, Right, Tab, Up};
        use Screen::{Accounts, History, Tokens};
        match (screen, code, ctrl) {
            (_, Char('c' | 'z'), true) => vec![accounts(1)],
            (None, Char('1'), false) => vec![tokens(0)],
            (None, Char('2' | '3') | Tab | BackTab, false) => vec![accounts(1)],
            (None, Char('?'), false) => vec![accounts(1)],
            (None, Char('q'), false) => vec![opened(accounts(1)), accounts(1)],
            (None, Esc, false) => vec![opened(accounts(1)), warned(accounts(1))],
            (None, Char('P' | 'r' | 'A' | 'D'), false) => vec![accounts(2)],
            (
                Some(Accounts),
                Char('j' | 'k' | 's' | 'b' | 'n' | 'e' | 'a' | 'm' | 'x' | 'R')
                | Down
                | Up
                | PageDown
                | PageUp
                | Home
                | End
                | Enter,
                false,
            ) => vec![accounts(2)],
            (Some(Accounts), Char('J'), false) => vec![crowded_accounts()],
            (Some(Accounts), Char('K'), false) => vec![scrolled(crowded_accounts())],
            (
                Some(Tokens),
                Char('j' | 'k' | 'p' | 'g' | 'f' | '[' | ']')
                | Down
                | Up
                | PageDown
                | PageUp
                | Home
                | End
                | Enter,
                false,
            ) => vec![tokens(0)],
            (Some(Tokens), Esc | Backspace, false) => {
                let mut state = tokens(0);
                update(&mut state, key(Enter), NOW_MS);
                vec![state]
            }
            (Some(Tokens), Char('J'), false) => vec![tokens(60)],
            (Some(Tokens), Char('K'), false) => vec![scrolled(tokens(60))],
            (Some(History), Char('[' | ']' | 'f'), false) => vec![history()],
            (Some(History), Left | Right | Home | End, false) => vec![cursor_moved()],
            (Some(History), Char('j' | 'k' | 'z') | Down | Up, false) => vec![with_events()],
            (Some(History), Char('J'), false) => vec![four_charts()],
            (Some(History), Char('K'), false) => vec![scrolled(four_charts())],
            other => panic!("no state in which {other:?} acts: the table's row needs one"),
        }
    }

    #[test]
    fn every_key_the_table_lists_is_handled() {
        // T2-29: a key is handled when `update` returns a command, or the frame drawn after it
        // differs from the one before.
        for row in KEYS {
            assert!(!row.probes.is_empty(), "{} has no probes", row.keys);
            for &(code, modifiers) in row.probes {
                let ctrl = modifiers.contains(KeyModifiers::CONTROL);
                for mut state in states(row.screen, code, ctrl) {
                    let before = draw(&state, 80, 24);
                    let msg = Msg::Key(KeyEvent::new(code, modifiers));
                    let cmds = update(&mut state, msg, NOW_MS);
                    let at = format!("{:?} {code:?} {modifiers:?}", row.screen);
                    assert!(
                        !cmds.is_empty() || draw(&state, 80, 24) != before,
                        "{at} did nothing"
                    );
                }
            }
        }
    }

    #[test]
    fn q_quits_when_nothing_is_open_and_only_closes_help_when_it_is() {
        let mut state = accounts(1);
        assert_eq!(update(&mut state, ch('q'), NOW_MS), [Cmd::Quit { code: 0 }]);
        let mut state = opened(accounts(1));
        assert!(update(&mut state, ch('q'), NOW_MS).is_empty());
        assert!(state.overlay.is_none());
        assert_eq!(state.quitting, None);
    }

    #[test]
    fn items_drop_from_the_end_and_a_footer_item_has_a_key_and_a_label() {
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snapshot(many(1)));
        let all = footer_items(&state, 200);
        assert_eq!(all.len(), 9);
        assert!(all.iter().all(|(k, l)| !k.is_empty() && !l.is_empty()));
        for width in [80, 100, 119] {
            let shown = footer_items(&state, width);
            assert_eq!(
                shown,
                all[..6],
                "{width}: the 120-column items wait for 120"
            );
        }
        // 14 for the pinned items, a column in, and two before them.
        let room = |n: usize| 14 + 1 + 2 + n;
        let first_two = "s switch  b best".len();
        let cut = footer_items(&state, room(first_two) as u16 + 6);
        assert_eq!(
            cut,
            all[..2],
            "cut from the end, never a later item in an earlier one's place"
        );
    }
}
