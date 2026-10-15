//! The state machine (spec §4.2): `update` applies one `Msg` and returns the `Cmd`s it needs. It
//! does no I/O and reads no clock: `now_ms` is the caller's.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::actions::{self, Overlay};
use crate::msg::{Cmd, Msg};
use crate::screens;
use crate::state::{Screen, State};

pub fn update(state: &mut State, msg: Msg, now_ms: i64) -> Vec<Cmd> {
    match msg {
        Msg::Key(key) => {
            if key.kind == KeyEventKind::Release {
                return vec![];
            }
            state.dirty = true;
            on_key(state, key, now_ms)
        }
        Msg::Resize { width, height } => {
            state.size = (width, height);
            state.dirty = true;
            vec![]
        }
        Msg::Tick => on_tick(state, now_ms),
        Msg::Accounts { .. } | Msg::History { .. } | Msg::Collected { .. } => vec![],
        Msg::Prompt(prompt) => {
            state.dirty = true;
            actions::on_prompt(state, prompt);
            vec![]
        }
        Msg::ActionDone { action, outcome } => {
            state.dirty = true;
            actions::on_done(state, action, outcome, now_ms)
        }
        Msg::LaneDied { lane } => {
            state.dirty = true;
            state.banner = Some(format!("the {lane} lane stopped unexpectedly"));
            if state.quitting.is_some() {
                return vec![];
            }
            quit(state, 1)
        }
    }
}

/// Starts the shutdown (spec §10.2) with `code`. An open dialog is dropped with it, so the
/// action waiting on its answer is declined.
pub(crate) fn quit(state: &mut State, code: i32) -> Vec<Cmd> {
    state.quitting = Some(code);
    state.overlay = None;
    state.dirty = true;
    vec![Cmd::Quit { code }]
}

fn on_key(state: &mut State, key: KeyEvent, now_ms: i64) -> Vec<Cmd> {
    if state.quitting.is_some() {
        return vec![];
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('c') => quit(state, 0),
            KeyCode::Char('z') => vec![Cmd::Stop],
            _ => vec![],
        };
    }
    // A dialog or panel takes every other key: a text dialog must be able to type `q`.
    if state.overlay.is_some() {
        return actions::handle_overlay_key(state, key, now_ms);
    }
    match key.code {
        KeyCode::Char('q') => quit(state, 0),
        KeyCode::Char('1') => show(state, Screen::Accounts, now_ms),
        KeyCode::Char('3') => show(state, Screen::History, now_ms),
        KeyCode::Tab => show(state, state.screen.next(), now_ms),
        KeyCode::BackTab => show(state, state.screen.prev(), now_ms),
        KeyCode::Char('?') => {
            state.overlay = Some(Overlay::Help);
            vec![]
        }
        _ => match actions::handle_action_key(state, key, now_ms) {
            Some(cmds) => cmds,
            None => match state.screen {
                Screen::Accounts => screens::accounts::handle_key(state, key, now_ms),
                Screen::History => screens::history::handle_key(state, key, now_ms),
            },
        },
    }
}

/// Switches to `screen`; History reads its account's history as it opens.
pub(crate) fn show(state: &mut State, screen: Screen, now_ms: i64) -> Vec<Cmd> {
    if state.screen == screen {
        return vec![];
    }
    state.screen = screen;
    match screen {
        Screen::Accounts => vec![],
        Screen::History => screens::history::on_enter(state, now_ms),
    }
}

fn on_tick(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    if state.toast.as_ref().is_some_and(|t| t.until_ms <= now_ms) {
        state.toast = None;
        state.dirty = true;
    }
    // Countdowns change at most once a second; the spinner turns while anything runs.
    let now_s = now_ms.div_euclid(1000);
    if now_s != state.shown_s || state.running.is_some() || state.quitting.is_some() {
        state.shown_s = now_s;
        state.dirty = true;
    }
    vec![]
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

    use super::*;
    use crate::testutil::*;

    fn quit_code(cmds: &[Cmd]) -> Option<i32> {
        match cmds {
            [Cmd::Quit { code }] => Some(*code),
            _ => None,
        }
    }

    #[test]
    fn the_screen_keys_switch_screens_and_two_waits_for_part_b() {
        let mut state = fresh(Screen::Accounts);
        update(&mut state, ch('3'), NOW_MS);
        assert_eq!(state.screen, Screen::History);
        update(&mut state, ch('2'), NOW_MS);
        assert_eq!(
            state.screen,
            Screen::History,
            "2 is the Tokens screen, part B's"
        );
        update(&mut state, ch('1'), NOW_MS);
        assert_eq!(state.screen, Screen::Accounts);
        update(&mut state, key(KeyCode::Tab), NOW_MS);
        assert_eq!(state.screen, Screen::History);
        update(
            &mut state,
            Msg::Key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)),
            NOW_MS,
        );
        assert_eq!(state.screen, Screen::Accounts);
    }

    #[test]
    fn q_and_ctrl_c_quit_with_0_and_ctrl_z_stops() {
        for msg in [ch('q'), ctrl('c')] {
            let mut state = fresh(Screen::Accounts);
            assert_eq!(quit_code(&update(&mut state, msg, NOW_MS)), Some(0));
            assert_eq!(state.quitting, Some(0));
            // A second `q` does not skip the wait (spec §10.2 step 4): keys are ignored.
            assert!(update(&mut state, ch('q'), NOW_MS).is_empty());
            assert!(update(&mut state, ch('3'), NOW_MS).is_empty());
            assert_eq!(state.screen, Screen::Accounts);
        }
        let mut state = fresh(Screen::Accounts);
        assert_eq!(update(&mut state, ctrl('z'), NOW_MS), [Cmd::Stop]);
        assert_eq!(state.quitting, None);
    }

    #[test]
    fn help_toggles_and_takes_the_keys_while_open() {
        let mut state = fresh(Screen::Accounts);
        update(&mut state, ch('?'), NOW_MS);
        assert!(matches!(state.overlay, Some(Overlay::Help)));
        update(&mut state, ch('3'), NOW_MS);
        assert_eq!(state.screen, Screen::Accounts, "the overlay has the keys");
        update(&mut state, ch('?'), NOW_MS);
        assert!(state.overlay.is_none());
        update(&mut state, ch('?'), NOW_MS);
        assert_eq!(quit_code(&update(&mut state, ctrl('c'), NOW_MS)), Some(0));
        assert!(state.overlay.is_none());
    }

    #[test]
    fn a_resize_is_recorded_and_a_release_is_ignored() {
        let mut state = fresh(Screen::Accounts);
        state.dirty = false;
        update(
            &mut state,
            Msg::Resize {
                width: 120,
                height: 40,
            },
            NOW_MS,
        );
        assert_eq!((state.size, state.dirty), ((120, 40), true));
        let release = KeyEvent::new_with_kind(
            KeyCode::Char('q'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        );
        assert!(update(&mut state, Msg::Key(release), NOW_MS).is_empty());
        assert_eq!(state.quitting, None);
    }

    #[test]
    fn a_tick_redraws_once_a_second_and_while_an_action_runs() {
        let mut state = fresh(Screen::Accounts);
        state.dirty = false;
        update(&mut state, Msg::Tick, NOW_MS + 250);
        assert!(!state.dirty, "the same second");
        update(&mut state, Msg::Tick, NOW_MS + 1_000);
        assert!(state.dirty, "a countdown may have changed");
        state.dirty = false;
        state.running = Some(crate::host::Action::AddLive { provider: cc() });
        update(&mut state, Msg::Tick, NOW_MS + 1_250);
        assert!(state.dirty, "the spinner turns");
    }

    #[test]
    fn a_lane_that_died_ends_the_tui_with_1() {
        let mut state = fresh(Screen::Accounts);
        let cmds = update(&mut state, Msg::LaneDied { lane: "views" }, NOW_MS);
        assert_eq!(quit_code(&cmds), Some(1));
        assert_eq!(
            state.banner.as_deref(),
            Some("the views lane stopped unexpectedly")
        );
        assert!(update(&mut state, Msg::LaneDied { lane: "actions" }, NOW_MS).is_empty());
        assert_eq!(state.quitting, Some(1));
    }

    #[test]
    fn start_reads_the_accounts_first() {
        let mut state = fresh(Screen::History);
        assert_eq!(state.start(), [Cmd::LoadAccounts { r#gen: 1 }]);
        assert_eq!(state.gens.accounts, 1);
    }
}
