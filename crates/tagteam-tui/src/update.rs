//! The state machine (spec §4.2): `update` applies one `Msg` and returns the `Cmd`s it needs. It
//! does no I/O and reads no clock: `now_ms` is the caller's.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::actions;
use crate::daemon;
use crate::msg::{Cmd, Msg};
use crate::overlay::{self, Overlay, ProceedThen};
use crate::screens;
use crate::state::{BannerKind, REFRESH_MS, Screen, State, ToastKind};

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
        Msg::Accounts { r#gen, result } => {
            if r#gen != state.gens.accounts {
                return vec![];
            }
            state.dirty = true;
            match result {
                Ok(snapshot) => {
                    // The config file changed since start-up read it: its warnings are old.
                    if snapshot.config_mtime != state.startup.mtime {
                        state.startup.warnings.clear();
                    }
                    state.accounts.apply(snapshot);
                    // T3-21: a new selection, or a new reading of it, is read for its card.
                    state.accounts.schedule_card(now_ms);
                    state.clear_banner(BannerKind::Accounts);
                    // The read after `r`'s collection: the toast needs what it shows.
                    if let Some(summary) = state.checked.take() {
                        actions::say_checked(state, &summary, now_ms);
                    }
                    if state.quitting.is_some() {
                        return vec![];
                    }
                    let mut cmds = screens::history::on_accounts(state, now_ms);
                    cmds.extend(screens::tokens::index::on_accounts(state, now_ms));
                    cmds
                }
                Err(e) => {
                    state.raise_banner(BannerKind::Accounts, e.message);
                    if let Some(summary) = state.checked.take() {
                        actions::say_checked(state, &summary, now_ms);
                    }
                    vec![]
                }
            }
        }
        Msg::History { r#gen, result } => {
            if r#gen != state.gens.history {
                return vec![];
            }
            state.dirty = true;
            match result {
                Ok(view) => {
                    state.history.apply(view);
                    state.clear_banner(BannerKind::History);
                }
                Err(e) => state.raise_banner(BannerKind::History, e.message),
            }
            vec![]
        }
        Msg::Card {
            r#gen,
            account,
            result,
        } => {
            screens::accounts::on_card(state, r#gen, account, result);
            vec![]
        }
        Msg::Collected { r#gen, result } => {
            // A collection slower than the timer is overtaken by the next. What it recorded is
            // in the store all the same, so it is read back; its banner and toast are dropped,
            // unless a person's `r` asked for it: that one is answered whatever came since.
            // The lane replaces a queued collection with the next, so the one `r` asked for may
            // never complete: any at or after it (generations only grow) answers the person.
            let asked = state.checking.is_some_and(|asked_gen| r#gen >= asked_gen);
            if asked {
                state.checking = None;
                state.dirty = true;
            }
            let stale = r#gen != state.gens.collect;
            match result {
                Ok(summary) => {
                    if !stale {
                        state.dirty = true;
                        state.clear_banner(BannerKind::Collect);
                    }
                    if asked && state.quitting.is_none() {
                        // Spec §9.7: said at once when nothing was fetched, else once the
                        // accounts read back show what it did.
                        if summary.fetched == 0 {
                            actions::say_checked(state, &summary, now_ms);
                        } else {
                            state.checked = Some(summary.clone());
                        }
                    } else if !stale && let Some(first) = summary.warnings.first() {
                        state.toast(ToastKind::Warn, format!("warning: {first}"), now_ms);
                    }
                    if summary.fetched == 0 || state.quitting.is_some() {
                        return vec![];
                    }
                    // Spec §6.2: a collection that fetched is read back at once.
                    state.reload_views(now_ms)
                }
                Err(_) if stale && !asked => vec![],
                Err(e) => {
                    state.dirty = true;
                    let words = format!("usage was not collected: {}", e.message);
                    state.raise_banner(BannerKind::Collect, words);
                    vec![]
                }
            }
        }
        Msg::TokenReport { r#gen, result } => {
            screens::tokens::on_report(state, r#gen, result, now_ms)
        }
        Msg::IndexState { r#gen, result } => {
            screens::tokens::index::on_index_state(state, r#gen, result, now_ms)
        }
        Msg::IndexProgress(progress) => {
            screens::tokens::index::on_progress(state, progress);
            vec![]
        }
        Msg::Refreshed { result } => screens::tokens::index::on_refreshed(state, result, now_ms),
        Msg::Inventory { r#gen, result } => {
            screens::tokens::filter::on_inventory(state, r#gen, result);
            vec![]
        }
        Msg::Prompt(prompt) => {
            state.dirty = true;
            actions::on_prompt(state, prompt)
        }
        Msg::ActionDone { action, outcome } => {
            state.dirty = true;
            actions::on_done(state, action, outcome, now_ms)
        }
        Msg::LaneDied { lane } => {
            state.dirty = true;
            if state.banner.as_ref().map(|b| b.kind) != Some(BannerKind::Lane) {
                let words = format!("the {lane} lane stopped unexpectedly");
                state.raise_banner(BannerKind::Lane, words);
            }
            if state.quitting.is_some() {
                return vec![];
            }
            quit(state, 1)
        }
        Msg::Live(live) => daemon::on_live(state, live, now_ms),
        // The runtime takes the live queue at each wake, and applies what it holds as `Live`.
        Msg::LiveReady => vec![],
        Msg::DaemonStatus { link, result } => daemon::on_status(state, link, result, now_ms),
        Msg::ShutdownBegan { code } => {
            state.quitting = Some(code);
            state.dirty = true;
            vec![]
        }
        Msg::SuspendRefused { action, why } => {
            let words = match &action {
                Some(action) => format!("{} did not start: {why}", action.name()),
                None => format!("Ctrl-Z did not suspend: {why}"),
            };
            state.raise_banner(BannerKind::Refusal, words);
            if action.is_some() {
                state.running = None;
                state.on_terminal = false;
            }
            state.dirty = true;
            vec![]
        }
        Msg::Detected { background } => {
            if let Some(background) = background {
                state.theme = state.theme.truecolor(background);
                state.dirty = true;
            }
            vec![]
        }
        Msg::KeysHeld { held } => {
            state.keys_held = held;
            state.dirty = true;
            vec![]
        }
    }
}

/// Spec §6.1: the TUI collects after drawing. The runtime calls this after each frame it
/// draws. The first frame asks for the token index's first refresh (B5). The first frame that
/// shows accounts nothing else collects for (§6.3) asks for the first collection of them, once
/// no action holds the actions lane (spec §7.2), and once the TUI knows whether a daemon
/// collects for them (C-R20); until a collection has been asked for, `gens.collect` is 0. The
/// timer collects from then on (§6.2).
pub fn drawn(state: &mut State) -> Vec<Cmd> {
    let mut cmds = screens::tokens::index::first_refresh(state);
    let waits = state.running.is_some() || !state.daemon.settled();
    if state.gens.collect > 0 || state.quitting.is_some() || waits {
        return cmds;
    }
    let shown = daemon::to_collect(state);
    if !shown.is_empty() {
        cmds.push(state.collect_due(shown));
    }
    cmds
}

/// Starts the shutdown (spec §10.2) with `code`. An open dialog is dropped with it, and the
/// runtime declines the prompt waiting on it when the shutdown begins. A hand-over dialog dropped
/// with it leaves nothing running.
pub(crate) fn quit(state: &mut State, code: i32) -> Vec<Cmd> {
    state.quitting = Some(code);
    if matches!(
        state.overlay,
        Some(Overlay::Proceed {
            then: ProceedThen::Suspend(_),
            ..
        })
    ) {
        state.running = None;
    }
    state.overlay = None;
    state.dirty = true;
    vec![Cmd::Quit { code }]
}

fn on_key(state: &mut State, key: KeyEvent, now_ms: i64) -> Vec<Cmd> {
    if state.quitting.is_some() {
        return vec![];
    }
    // A terminal whose Backspace sends ^H reaches crossterm as Ctrl+h: it is a Backspace.
    let key = match key {
        KeyEvent {
            code: KeyCode::Char('h'),
            modifiers: KeyModifiers::CONTROL,
            kind,
            ..
        } => KeyEvent::new_with_kind(KeyCode::Backspace, KeyModifiers::NONE, kind),
        key => key,
    };
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('c') => quit(state, 0),
            KeyCode::Char('z') => vec![Cmd::Stop],
            // Ctrl-U clears a text dialog's input; no other dialog has one.
            KeyCode::Char('u') => {
                if let Some(Overlay::Text { input, error, .. }) = &mut state.overlay {
                    input.clear();
                    *error = None;
                }
                vec![]
            }
            _ => vec![],
        };
    }
    // A dialog or panel takes every other key: a text dialog must be able to type `q`.
    if state.overlay.is_some() {
        return overlay::handle_key(state, key, now_ms);
    }
    match key.code {
        KeyCode::Char('q') => quit(state, 0),
        KeyCode::Char('1') => show(state, Screen::Accounts, now_ms),
        KeyCode::Char('2') => show(state, Screen::Tokens, now_ms),
        KeyCode::Char('3') => show(state, Screen::History, now_ms),
        KeyCode::Tab => show(state, state.screen.next(), now_ms),
        KeyCode::BackTab => show(state, state.screen.prev(), now_ms),
        KeyCode::Char('?') => {
            state.overlay = Some(Overlay::Help { scroll: 0 });
            vec![]
        }
        // Right after an overlay closed, a repeated action key must not act (§9.5, T2-18).
        KeyCode::Char('s' | 'b' | 'n' | 'e' | 'a' | 'm' | 'x' | 'R' | 'P' | 'A' | 'D' | 'r')
            if now_ms < state.guard_until_ms =>
        {
            vec![]
        }
        _ => match actions::handle_action_key(state, key, now_ms) {
            Some(cmds) => cmds,
            None => match state.screen {
                Screen::Accounts => screens::accounts::handle_key(state, key, now_ms),
                Screen::Tokens => screens::tokens::handle_key(state, key, now_ms),
                Screen::History => screens::history::handle_key(state, key, now_ms),
            },
        },
    }
}

/// Switches to `screen`; Tokens reads its report and History its account's history as they
/// open.
pub(crate) fn show(state: &mut State, screen: Screen, now_ms: i64) -> Vec<Cmd> {
    if state.screen == screen {
        return vec![];
    }
    state.screen = screen;
    match screen {
        Screen::Accounts => vec![],
        Screen::Tokens => screens::tokens::on_enter(state, now_ms),
        Screen::History => screens::history::on_enter(state, now_ms),
    }
}

fn on_tick(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    if state.toast.as_ref().is_some_and(|t| t.until_ms <= now_ms) {
        state.toast = None;
        state.dirty = true;
    }
    // Countdowns change at most once a second; the spinner turns while anything runs or `r`
    // checks usage.
    let now_s = now_ms.div_euclid(1000);
    let spinning = state.running.is_some() || state.checking.is_some();
    if now_s != state.shown_s || spinning || state.quitting.is_some() {
        state.shown_s = now_s;
        state.dirty = true;
    }
    // Spec §6.4: the token index's own timers, while Tokens is open.
    let mut cmds = screens::tokens::index::on_tick(state, now_ms);
    // Spec §6.3, §8: the daemon's link.
    cmds.extend(daemon::on_tick(state, now_ms));
    // Redesign §6.4: the selected account's card, read once its selection has settled.
    cmds.extend(screens::accounts::on_tick(state, now_ms));
    if state.quitting.is_some() || now_ms < state.refresh.next_ms {
        return cmds;
    }
    // Spec §6.2: every 30 s the views are re-read, and the shown accounts offered to the
    // collector, unless an action holds the actions lane (spec §7.2). In live mode the views
    // are read on the daemon's events and every 60 s instead, and the collector is offered
    // only the accounts nothing else collects for (§6.3).
    state.refresh.next_ms = now_ms + REFRESH_MS;
    if !state.daemon.live() {
        cmds.extend(state.reload_views(now_ms));
    }
    let due = daemon::to_collect(state);
    if state.running.is_none() && !due.is_empty() {
        cmds.push(state.collect_due(due));
    }
    cmds
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

    use super::*;
    use crate::host::ActionOutcome;
    use crate::source::{CollectSummary, SourceError};
    use crate::state::{GUARD_MS, Startup};
    use crate::testutil::*;

    fn quit_code(cmds: &[Cmd]) -> Option<i32> {
        match cmds {
            [Cmd::Quit { code }] => Some(*code),
            _ => None,
        }
    }

    #[test]
    fn the_screen_keys_switch_screens_in_their_numbers_order() {
        let mut state = fresh(Screen::Accounts);
        update(&mut state, ch('3'), NOW_MS);
        assert_eq!(state.screen, Screen::History);
        update(&mut state, ch('2'), NOW_MS);
        assert_eq!(state.screen, Screen::Tokens);
        update(&mut state, ch('1'), NOW_MS);
        assert_eq!(state.screen, Screen::Accounts);
        let back_tab = || Msg::Key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        for screen in [Screen::Tokens, Screen::History, Screen::Accounts] {
            update(&mut state, key(KeyCode::Tab), NOW_MS);
            assert_eq!(state.screen, screen);
        }
        for screen in [Screen::History, Screen::Tokens, Screen::Accounts] {
            update(&mut state, back_tab(), NOW_MS);
            assert_eq!(state.screen, screen);
        }
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
        assert!(matches!(state.overlay, Some(Overlay::Help { .. })));
        update(&mut state, ch('3'), NOW_MS);
        assert_eq!(state.screen, Screen::Accounts, "the overlay has the keys");
        update(&mut state, ch('?'), NOW_MS);
        assert!(state.overlay.is_none());
        update(&mut state, ch('?'), NOW_MS);
        assert_eq!(quit_code(&update(&mut state, ctrl('c'), NOW_MS)), Some(0));
        assert!(state.overlay.is_none());
    }

    /// Review Focus 5 (§9.5, T2-18): for 300 ms after a key closes an overlay only the action
    /// keys are dropped.
    #[test]
    fn the_guard_drops_only_action_keys_and_only_for_300_ms() {
        // Two accounts, Help opened and closed at `NOW_MS`.
        let closed = || {
            let mut state = fresh(Screen::Accounts);
            state.accounts.apply(snapshot(many(2)));
            update(&mut state, ch('?'), NOW_MS);
            update(&mut state, ch('?'), NOW_MS);
            assert!(state.overlay.is_none());
            state
        };
        let selected = |state: &State| state.accounts.selected().map(|a| a.position);
        // Every action key at 299 ms does nothing.
        for c in "sbneamxRPADr".chars() {
            let mut state = closed();
            let cmds = update(&mut state, ch(c), NOW_MS + GUARD_MS - 1);
            assert!(cmds.is_empty(), "{c}: {cmds:?}");
            assert!(
                state.overlay.is_none() && state.running.is_none() && state.toast.is_none(),
                "{c}"
            );
        }
        // The keys a person uses right after a dialog still act at once.
        let mut state = closed();
        update(&mut state, ch('j'), NOW_MS + 1);
        assert_eq!(selected(&state), Some(2));
        update(&mut state, ch('k'), NOW_MS + 1);
        assert_eq!(selected(&state), Some(1));
        update(&mut state, ch('?'), NOW_MS + 1);
        assert!(matches!(state.overlay, Some(Overlay::Help { .. })));
        update(&mut state, key(KeyCode::Esc), NOW_MS + 1);
        assert!(state.overlay.is_none(), "Esc closes the next overlay");
        for (c, screen) in [
            ('1', Screen::Accounts),
            ('2', Screen::Tokens),
            ('3', Screen::History),
        ] {
            let mut state = closed();
            update(&mut state, ch(c), NOW_MS + 1);
            assert_eq!(state.screen, screen, "{c}");
        }
        let mut state = closed();
        assert_eq!(quit_code(&update(&mut state, ch('q'), NOW_MS + 1)), Some(0));
        // At 300 ms the guard is over.
        let mut state = closed();
        let cmds = update(&mut state, ch('n'), NOW_MS + GUARD_MS);
        assert!(matches!(cmds.as_slice(), [Cmd::Act(_)]), "{cmds:?}");
        // Any key that closes an overlay starts it: Esc on a dialog, too.
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snapshot(many(2)));
        update(&mut state, ch('x'), NOW_MS);
        update(&mut state, key(KeyCode::Esc), NOW_MS);
        assert!(update(&mut state, ch('n'), NOW_MS + GUARD_MS - 1).is_empty());
        assert!(!update(&mut state, ch('n'), NOW_MS + GUARD_MS).is_empty());
    }

    /// Every close starts the guard, not only `Esc`: a confirming `y` and a `q` as well.
    #[test]
    fn a_confirming_key_and_q_start_the_guard_too() {
        let loaded = || {
            let mut state = fresh(Screen::Accounts);
            state.accounts.apply(snapshot(many(2)));
            state
        };
        // `x`, then `y` (a `CloseWith`) at T: the removal starts, and `n` at T + 299 does not.
        let mut state = loaded();
        update(&mut state, ch('x'), NOW_MS);
        let started = update(&mut state, ch('y'), NOW_MS);
        assert!(matches!(started.as_slice(), [Cmd::Act(_)]), "{started:?}");
        state.running = None;
        assert!(update(&mut state, ch('n'), NOW_MS + GUARD_MS - 1).is_empty());
        assert!(state.running.is_none());
        // `q` closing Help at T: the same.
        let mut state = loaded();
        update(&mut state, ch('?'), NOW_MS);
        update(&mut state, ch('q'), NOW_MS);
        assert!(state.overlay.is_none() && state.quitting.is_none());
        assert!(update(&mut state, ch('n'), NOW_MS + GUARD_MS - 1).is_empty());
        assert!(state.running.is_none());
        assert!(!update(&mut state, ch('n'), NOW_MS + GUARD_MS).is_empty());
    }

    #[test]
    fn quitting_with_the_hand_over_open_leaves_nothing_running() {
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snapshot(many(1)));
        update(&mut state, ch('s'), NOW_MS);
        let switch = state.running.clone().unwrap();
        crate::actions::on_done(&mut state, switch, ActionOutcome::NeedsTerminal, NOW_MS);
        assert!(matches!(state.overlay, Some(Overlay::Proceed { .. })));
        assert!(state.running.is_some());
        assert_eq!(quit_code(&update(&mut state, ctrl('c'), NOW_MS)), Some(0));
        assert!(state.overlay.is_none() && state.running.is_none());
        assert!(!state.on_terminal);
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
            state.banner_text(),
            Some("the views lane stopped unexpectedly")
        );
        assert!(update(&mut state, Msg::LaneDied { lane: "actions" }, NOW_MS).is_empty());
        assert_eq!(state.quitting, Some(1));
    }

    #[test]
    fn the_runtime_s_shutdown_sets_the_exit_code_and_leaves_the_dialog() {
        let mut state = fresh(Screen::Accounts);
        update(&mut state, ch('?'), NOW_MS);
        state.dirty = false;
        assert!(update(&mut state, Msg::ShutdownBegan { code: 143 }, NOW_MS).is_empty());
        assert_eq!(state.quitting, Some(143));
        assert!(state.dirty);
        assert!(matches!(state.overlay, Some(Overlay::Help { .. })));
    }

    #[test]
    fn a_refused_hand_over_raises_a_refusal_and_frees_only_its_own_action() {
        let account = || crate::host::AccountRef {
            id: id(1),
            provider: cc(),
            position: 1,
        };
        let mut state = fresh(Screen::Accounts);
        let run = crate::host::Action::Run(account());
        state.running = Some(run.clone());
        state.dirty = false;
        let cmds = update(
            &mut state,
            Msg::SuspendRefused {
                action: Some(run.clone()),
                why: "w",
            },
            NOW_MS,
        );
        assert!(cmds.is_empty() && state.dirty);
        assert_eq!(
            state.banner_text(),
            Some(format!("{} did not start: w", run.name()).as_str())
        );
        assert_eq!(
            state.banner.as_ref().map(|b| b.kind),
            Some(BannerKind::Refusal)
        );
        assert_eq!(state.running, None);

        let switch = crate::host::Action::Switch(account());
        state.running = Some(switch.clone());
        state.dirty = false;
        let cmds = update(
            &mut state,
            Msg::SuspendRefused {
                action: None,
                why: "w",
            },
            NOW_MS,
        );
        assert!(cmds.is_empty() && state.dirty);
        assert_eq!(state.banner_text(), Some("Ctrl-Z did not suspend: w"));
        assert_eq!(
            state.banner.as_ref().map(|b| b.kind),
            Some(BannerKind::Refusal)
        );
        assert_eq!(state.running, Some(switch));
    }

    #[test]
    fn start_reads_the_accounts_first() {
        let mut state = fresh(Screen::History);
        assert_eq!(state.start(), [Cmd::LoadAccounts { r#gen: 1 }]);
        assert_eq!(state.gens.accounts, 1);
    }

    fn accounts_msg(r#gen: u64, n: u32) -> Msg {
        Msg::Accounts {
            r#gen,
            result: Ok(snapshot(many(n))),
        }
    }

    fn failed(message: &str) -> SourceError {
        SourceError {
            kind: "store".into(),
            message: message.into(),
        }
    }

    #[test]
    fn a_result_from_an_older_generation_is_dropped() {
        let mut state = fresh(Screen::Accounts);
        state.start();
        let newer = state.load_accounts();
        assert_eq!(newer, Cmd::LoadAccounts { r#gen: 2 });
        update(&mut state, accounts_msg(1, 3), NOW_MS);
        assert!(state.accounts.snapshot().is_none(), "gen 1 is stale");
        update(&mut state, accounts_msg(2, 2), NOW_MS);
        assert_eq!(state.accounts.shown_ids(), [id(1), id(2)]);

        state.history.select(id(1));
        let Cmd::LoadHistory { r#gen, .. } = state.load_history(id(1), 0) else {
            unreachable!()
        };
        state.load_history(id(2), 0);
        update(
            &mut state,
            Msg::History {
                r#gen,
                result: Ok(history_of(
                    account(1, "a1@x.co", read(60, 1.0, 1.0, false)),
                    vec![],
                )),
            },
            NOW_MS,
        );
        update(&mut state, ch('3'), NOW_MS);
        assert!(
            body_of(&state, 80, 24, NOW_MS)[0].ends_with("loading…"),
            "the stale history was dropped"
        );

        let Cmd::CollectDue { r#gen, .. } = state.collect_due(vec![id(1)]) else {
            unreachable!()
        };
        state.collect_due(vec![id(1)]);
        let stale = Msg::Collected {
            r#gen,
            result: Ok(CollectSummary {
                fetched: 0,
                warnings: vec!["usage for a@x.co (position 1) was not collected: x".into()],
                updated: 0,
                rate_limited: vec![],
            }),
        };
        assert!(update(&mut state, stale, NOW_MS).is_empty());
        assert!(
            state.toast.is_none(),
            "a stale collection's toast is dropped"
        );
    }

    #[test]
    fn a_collection_overtaken_by_a_newer_one_that_fetched_still_reads_the_views() {
        // A collection slower than the 30 s timer is overtaken by the next: what it recorded
        // is in the store all the same, and its banner and toast are the older run's.
        for screen in [Screen::Accounts, Screen::History] {
            let mut state = fresh(screen);
            state.start();
            update(&mut state, accounts_msg(1, 1), NOW_MS);
            let Cmd::CollectDue { r#gen, .. } = state.collect_due(vec![id(1)]) else {
                unreachable!()
            };
            state.collect_due(vec![id(1)]);
            let stale = |fetched, warnings: Vec<String>| Msg::Collected {
                r#gen,
                result: Ok(CollectSummary {
                    fetched,
                    warnings,
                    updated: 0,
                    rate_limited: vec![],
                }),
            };
            assert!(update(&mut state, stale(0, vec![]), NOW_MS).is_empty());
            let cmds = update(
                &mut state,
                stale(
                    2,
                    vec!["usage for a@x.co (position 1) was not collected: x".into()],
                ),
                NOW_MS,
            );
            match screen {
                Screen::Accounts => assert!(
                    matches!(cmds.as_slice(), [Cmd::LoadAccounts { .. }]),
                    "{cmds:?}"
                ),
                Screen::History => assert!(
                    matches!(
                        cmds.as_slice(),
                        [Cmd::LoadAccounts { .. }, Cmd::LoadHistory { .. }]
                    ),
                    "{cmds:?}"
                ),
                Screen::Tokens => unreachable!("not among the screens this test opens"),
            }
            assert!(state.toast.is_none() && state.banner.is_none());
            // A failure of the older run says nothing and reads nothing.
            let failure = Msg::Collected {
                r#gen,
                result: Err(failed("locked")),
            };
            assert!(update(&mut state, failure, NOW_MS).is_empty());
            assert!(state.banner.is_none());
            // Nothing is read once the shutdown has begun.
            state.quitting = Some(0);
            assert!(update(&mut state, stale(2, vec![]), NOW_MS).is_empty());
        }
    }

    /// The card's read of the first account, as the first accounts read selects it at `NOW_MS`.
    fn first_card() -> Cmd {
        Cmd::LoadCard {
            r#gen: 1,
            account: id(1),
            since_s: NOW_S - 7 * 86_400,
        }
    }

    #[test]
    fn every_30_seconds_the_views_are_read_and_the_shown_accounts_offered_to_the_collector() {
        let mut state = fresh(Screen::Accounts);
        state.start();
        update(&mut state, accounts_msg(1, 2), NOW_MS);
        // The first selection's card is read 250 ms after it (T3-21).
        assert_eq!(update(&mut state, Msg::Tick, NOW_MS + 250), [first_card()]);
        assert!(update(&mut state, Msg::Tick, NOW_MS + 29_999).is_empty());
        assert_eq!(
            update(&mut state, Msg::Tick, NOW_MS + 30_000),
            [
                Cmd::LoadAccounts { r#gen: 2 },
                Cmd::CollectDue {
                    r#gen: 1,
                    accounts: vec![id(1), id(2)]
                }
            ]
        );
        assert!(update(&mut state, Msg::Tick, NOW_MS + 59_999).is_empty());
        update(&mut state, ch('3'), NOW_MS + 59_999);
        let cmds = update(&mut state, Msg::Tick, NOW_MS + 60_000);
        assert!(
            matches!(
                cmds.as_slice(),
                [
                    Cmd::LoadAccounts { .. },
                    Cmd::LoadHistory { account, .. },
                    Cmd::CollectDue { .. }
                ] if *account == id(1)
            ),
            "{cmds:?}"
        );
    }

    #[test]
    fn the_timer_collects_nothing_while_an_action_runs_or_the_tui_quits() {
        let mut state = fresh(Screen::Accounts);
        state.start();
        update(&mut state, accounts_msg(1, 2), NOW_MS);
        assert_eq!(update(&mut state, Msg::Tick, NOW_MS + 250), [first_card()]);
        state.running = Some(crate::host::Action::AddLive { provider: cc() });
        assert_eq!(
            update(&mut state, Msg::Tick, NOW_MS + 30_000),
            [Cmd::LoadAccounts { r#gen: 2 }]
        );
        state.running = None;
        state.quitting = Some(0);
        assert!(update(&mut state, Msg::Tick, NOW_MS + 60_000).is_empty());
    }

    #[test]
    fn a_collection_that_fetched_reads_the_views_once_more() {
        let mut state = fresh(Screen::History);
        state.start();
        update(&mut state, accounts_msg(1, 1), NOW_MS);
        let Cmd::CollectDue { r#gen, .. } = state.collect_due(vec![id(1)]) else {
            unreachable!()
        };
        let collected = |fetched| Msg::Collected {
            r#gen,
            result: Ok(CollectSummary {
                fetched,
                warnings: vec![],
                updated: 0,
                rate_limited: vec![],
            }),
        };
        assert!(update(&mut state, collected(0), NOW_MS).is_empty());
        let cmds = update(&mut state, collected(1), NOW_MS);
        assert!(
            matches!(
                cmds.as_slice(),
                [Cmd::LoadAccounts { r#gen: 2 }, Cmd::LoadHistory { .. }]
            ),
            "{cmds:?}"
        );
    }

    #[test]
    fn a_failed_read_keeps_the_last_good_data_under_a_banner() {
        let mut state = fresh(Screen::Accounts);
        state.start();
        update(&mut state, accounts_msg(1, 2), NOW_MS);
        let Cmd::LoadAccounts { r#gen } = state.load_accounts() else {
            unreachable!()
        };
        update(
            &mut state,
            Msg::Accounts {
                r#gen,
                result: Err(failed("the store is locked")),
            },
            NOW_MS,
        );
        assert_eq!(state.accounts.shown_ids(), [id(1), id(2)]);
        let drawn = draw(&state, 80, 24);
        assert_eq!(drawn[2], " ✕ the store is locked");
        let body = body_of(&state, 80, 24, NOW_MS);
        assert!(body[1].contains("a1@x.co"), "{body:?}");
        // The next good read clears it.
        update(&mut state, accounts_msg(r#gen, 2), NOW_MS);
        assert!(state.banner.is_none());
    }

    #[test]
    fn a_failed_collection_says_so_and_a_warning_is_a_toast() {
        let mut state = fresh(Screen::Accounts);
        let Cmd::CollectDue { r#gen, .. } = state.collect_due(vec![id(1)]) else {
            unreachable!()
        };
        update(
            &mut state,
            Msg::Collected {
                r#gen,
                result: Err(failed("the store is locked")),
            },
            NOW_MS,
        );
        assert_eq!(
            state.banner_text(),
            Some("usage was not collected: the store is locked")
        );
        update(
            &mut state,
            Msg::Collected {
                r#gen,
                result: Ok(CollectSummary {
                    fetched: 0,
                    warnings: vec!["usage for a@x.co (position 1) was not collected: x".into()],
                    updated: 0,
                    rate_limited: vec![],
                }),
            },
            NOW_MS,
        );
        let toast = state.toast.as_ref().unwrap();
        assert_eq!(
            (toast.kind, toast.text.as_str()),
            (
                ToastKind::Warn,
                "warning: usage for a@x.co (position 1) was not collected: x"
            ),
            "a collection the timer asked for keeps today's words, as a warning"
        );
        assert_eq!(
            draw(&state, 80, 24)[23],
            footer_row(
                "! warning: usage for a@x.co (position 1) was not collected: x",
                80
            )
        );
    }

    #[test]
    fn an_r_overtaken_by_the_timer_and_failed_still_says_so() {
        // `r` does not push back the timer, so its collection can be overtaken. Whatever came
        // since, the person who pressed `r` hears how it went: the spinner stops with a banner.
        let mut state = fresh(Screen::Accounts);
        state.start();
        update(&mut state, accounts_msg(1, 1), NOW_MS);
        update(&mut state, ch('r'), NOW_MS);
        let r#gen = state.checking.expect("r asked for a collection");
        state.collect_due(vec![id(1)]);
        let failure = Msg::Collected {
            r#gen,
            result: Err(failed("locked")),
        };
        assert!(update(&mut state, failure, NOW_MS).is_empty());
        assert_eq!(state.checking, None);
        assert_eq!(state.banner_text(), Some("usage was not collected: locked"));
        // A stale failure nobody asked for stays silent.
        state.banner = None;
        let older = Msg::Collected {
            r#gen,
            result: Err(failed("locked")),
        };
        update(&mut state, older, NOW_MS);
        assert!(state.banner.is_none());
    }

    #[test]
    fn a_later_collection_answers_an_r_whose_own_the_lane_replaced() {
        // The lane replaces a queued collection with the next, so the completion of the one
        // `r` asked for never comes.
        let summary = |fetched, updated| CollectSummary {
            fetched,
            warnings: vec![],
            updated,
            rate_limited: vec![],
        };
        let collected = |r#gen, summary| Msg::Collected {
            r#gen,
            result: Ok(summary),
        };
        let pressed = || {
            let mut state = fresh(Screen::Accounts);
            state.start();
            update(&mut state, accounts_msg(1, 1), NOW_MS);
            update(&mut state, ch('r'), NOW_MS);
            state
        };
        let said = |state: &State| state.toast.as_ref().map(|t| (t.kind, t.text.clone()));

        // Nothing fetched: said at once.
        let mut state = pressed();
        let asked = state.checking.expect("r asked for a collection");
        let Cmd::CollectDue { r#gen: later, .. } = state.collect_due(vec![id(1)]) else {
            unreachable!()
        };
        assert_eq!(later, asked + 1);
        assert!(update(&mut state, collected(later, summary(0, 0)), NOW_MS).is_empty());
        assert_eq!(state.checking, None, "the spinner stops");
        assert_eq!(
            said(&state),
            Some((ToastKind::Info, "already current".to_owned()))
        );

        // Something fetched: deferred to the accounts read after it.
        let mut state = pressed();
        state.collect_due(vec![id(1)]);
        let cmds = update(&mut state, collected(2, summary(1, 1)), NOW_MS);
        assert!(
            matches!(cmds.as_slice(), [Cmd::LoadAccounts { .. }]),
            "{cmds:?}"
        );
        assert_eq!(state.checking, None);
        assert!(state.checked.is_some() && state.toast.is_none());
        let Cmd::LoadAccounts { r#gen } = state.load_accounts() else {
            unreachable!()
        };
        update(&mut state, accounts_msg(r#gen, 1), NOW_MS);
        assert_eq!(
            said(&state),
            Some((ToastKind::Done, "updated 1 account".to_owned()))
        );

        // A collection from before `r` does not answer it.
        let mut state = pressed();
        update(&mut state, ch('r'), NOW_MS);
        let asked = state.checking.expect("the second r");
        assert_eq!(asked, 2);
        update(&mut state, collected(asked - 1, summary(0, 0)), NOW_MS);
        assert_eq!(state.checking, Some(asked));
        assert_eq!(said(&state), None);
        update(&mut state, collected(asked, summary(0, 0)), NOW_MS);
        assert_eq!(state.checking, None);
        assert!(said(&state).is_some());
    }

    #[test]
    fn the_first_frame_that_shows_accounts_asks_for_one_collection() {
        // Spec §6.1: the TUI collects after drawing, not a timer's 30 s later. The first frame
        // starts the token index's catch-up whatever it shows (B5).
        let mut state = fresh(Screen::Accounts);
        state.start();
        assert_eq!(
            drawn(&mut state),
            [Cmd::RefreshIndex { provider: cc() }],
            "`loading…` shows no accounts"
        );
        update(&mut state, accounts_msg(1, 0), NOW_MS);
        assert!(drawn(&mut state).is_empty(), "nor does the empty state");
        let Cmd::LoadAccounts { r#gen } = state.load_accounts() else {
            unreachable!()
        };
        update(&mut state, accounts_msg(r#gen, 2), NOW_MS);
        state.running = Some(crate::host::Action::AddLive { provider: cc() });
        assert!(
            drawn(&mut state).is_empty(),
            "an action holds the actions lane (spec §7.2)"
        );
        state.running = None;
        assert_eq!(
            drawn(&mut state),
            [Cmd::CollectDue {
                r#gen: 1,
                accounts: vec![id(1), id(2)]
            }]
        );
        assert!(drawn(&mut state).is_empty(), "once");
        // A collection asked for before any frame showed accounts (`r`) is the first one.
        let mut state = fresh(Screen::Accounts);
        state.collect_due(vec![id(1)]);
        update(&mut state, accounts_msg(0, 1), NOW_MS);
        assert_eq!(drawn(&mut state), [Cmd::RefreshIndex { provider: cc() }]);
        assert!(drawn(&mut state).is_empty());
    }

    #[test]
    fn a_long_collection_error_shows_in_full_at_80_columns() {
        let mut state = fresh(Screen::Accounts);
        let Cmd::CollectDue { r#gen, .. } = state.collect_due(vec![id(1)]) else {
            unreachable!()
        };
        let message = "the usage endpoint could not be reached for a@x.co (position 1): connection refused by 127.0.0.1:9";
        update(
            &mut state,
            Msg::Collected {
                r#gen,
                result: Err(failed(message)),
            },
            NOW_MS,
        );
        let lines = draw(&state, 80, 24);
        let shown: String = lines[2..4]
            .iter()
            .map(|l| l.trim())
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(shown, format!("! usage was not collected: {message}"));
        assert!(
            draw(&state, 80, 24)
                .iter()
                .any(|l| l.trim() == "Loading accounts…")
        );
    }

    #[test]
    fn the_first_accounts_read_on_history_reads_the_first_accounts_history() {
        let mut state = fresh(Screen::History);
        state.start();
        assert_eq!(
            update(&mut state, accounts_msg(1, 2), NOW_MS),
            [Cmd::LoadHistory {
                r#gen: 1,
                account: id(1),
                since_s: NOW_S - 7 * 86_400,
            }]
        );
    }

    fn history_msg(r#gen: u64) -> Msg {
        Msg::History {
            r#gen,
            result: Ok(history_of(
                account(1, "a1@x.co", read(60, 1.0, 1.0, false)),
                vec![],
            )),
        }
    }

    #[test]
    fn a_read_clears_only_the_banner_its_own_kind_raised() {
        let mut state = fresh(Screen::History);
        state.start();
        update(&mut state, accounts_msg(1, 1), NOW_MS);
        let Cmd::LoadAccounts { r#gen: ag } = state.load_accounts() else {
            unreachable!()
        };
        let Cmd::LoadHistory { r#gen: hg, .. } = state.load_history(id(1), 0) else {
            unreachable!()
        };
        update(
            &mut state,
            Msg::Accounts {
                r#gen: ag,
                result: Err(failed("the run shell marker is unreadable")),
            },
            NOW_MS,
        );
        update(&mut state, history_msg(hg), NOW_MS);
        assert_eq!(
            state.banner_text(),
            Some("the run shell marker is unreadable"),
            "a history read does not hide a failed accounts read"
        );
        let Cmd::LoadAccounts { r#gen: ag } = state.load_accounts() else {
            unreachable!()
        };
        update(&mut state, accounts_msg(ag, 1), NOW_MS);
        assert!(state.banner.is_none());

        // A collection's banner survives both reads and goes with the next good collection.
        let Cmd::CollectDue { r#gen: cg, .. } = state.collect_due(vec![id(1)]) else {
            unreachable!()
        };
        update(
            &mut state,
            Msg::Collected {
                r#gen: cg,
                result: Err(failed("locked")),
            },
            NOW_MS,
        );
        let Cmd::LoadAccounts { r#gen: ag } = state.load_accounts() else {
            unreachable!()
        };
        update(&mut state, accounts_msg(ag, 1), NOW_MS);
        let Cmd::LoadHistory { r#gen: hg, .. } = state.load_history(id(1), 0) else {
            unreachable!()
        };
        update(&mut state, history_msg(hg), NOW_MS);
        assert_eq!(state.banner_text(), Some("usage was not collected: locked"));
        let Cmd::CollectDue { r#gen: cg, .. } = state.collect_due(vec![id(1)]) else {
            unreachable!()
        };
        update(
            &mut state,
            Msg::Collected {
                r#gen: cg,
                result: Ok(CollectSummary {
                    fetched: 0,
                    warnings: vec![],
                    updated: 0,
                    rate_limited: vec![],
                }),
            },
            NOW_MS,
        );
        assert!(state.banner.is_none());
    }

    #[test]
    fn nothing_is_read_once_the_shutdown_has_begun() {
        let mut state = fresh(Screen::History);
        state.start();
        update(&mut state, accounts_msg(1, 1), NOW_MS);
        let Cmd::CollectDue { r#gen, .. } = state.collect_due(vec![id(1)]) else {
            unreachable!()
        };
        state.quitting = Some(0);
        let fetched = Msg::Collected {
            r#gen,
            result: Ok(CollectSummary {
                fetched: 1,
                warnings: vec![],
                updated: 0,
                rate_limited: vec![],
            }),
        };
        assert!(update(&mut state, fetched, NOW_MS).is_empty());
        let Cmd::LoadAccounts { r#gen } = state.load_accounts() else {
            unreachable!()
        };
        assert!(update(&mut state, accounts_msg(r#gen, 1), NOW_MS).is_empty());
    }

    #[test]
    fn a_second_lane_death_keeps_the_first_ones_words() {
        let mut state = fresh(Screen::Accounts);
        update(&mut state, Msg::LaneDied { lane: "views" }, NOW_MS);
        update(&mut state, Msg::LaneDied { lane: "actions" }, NOW_MS);
        assert_eq!(
            state.banner_text(),
            Some("the views lane stopped unexpectedly")
        );
    }

    const WARNING: &str = "/cfg/config.toml: unknown key x (ignored)";

    /// Accounts with `WARNING` as the one start-up warning, the config file last changed at
    /// `mtime_s` seconds after the epoch.
    fn warned(mtime_s: u64) -> State {
        let mut state = fresh(Screen::Accounts);
        state.startup = Startup {
            warnings: vec![WARNING.to_owned()],
            mtime: Some(UNIX_EPOCH + Duration::from_secs(mtime_s)),
        };
        state.accounts.apply(snapshot(many(2)));
        state
    }

    #[test]
    fn the_start_up_warnings_show_until_esc_on_accounts() {
        // Redesign §5.6, T2-21: a warn banner as the CLI printed it, on every screen.
        let shown = format!(" ! {WARNING}");
        let mut state = warned(10);
        assert_eq!(draw(&state, 80, 24)[2], shown);
        update(&mut state, ch('2'), NOW_MS);
        assert_eq!(draw(&state, 80, 24)[2], shown, "on Tokens too");
        update(&mut state, key(KeyCode::Esc), NOW_MS);
        assert_eq!(
            draw(&state, 80, 24)[2],
            shown,
            "Esc on Tokens goes back a level, and it is at the top"
        );
        update(&mut state, ch('1'), NOW_MS);
        assert_eq!(draw(&state, 80, 24)[2], shown);
        update(&mut state, key(KeyCode::Esc), NOW_MS);
        assert!(state.startup.warnings.is_empty());
        assert!(
            !draw(&state, 80, 24)
                .iter()
                .any(|l| l.contains("unknown key")),
            "Esc on Accounts clears them"
        );
    }

    #[test]
    fn esc_closes_an_overlay_before_it_clears_the_start_up_warnings() {
        let mut state = warned(10);
        update(&mut state, ch('?'), NOW_MS);
        update(&mut state, key(KeyCode::Esc), NOW_MS);
        assert!(state.overlay.is_none());
        assert_eq!(state.startup.warnings, [WARNING]);
    }

    #[test]
    fn a_config_change_clears_the_start_up_warnings() {
        // T2-21: a snapshot whose config mtime is the one `tui_cmd` read keeps them; a later one
        // clears them.
        let mut state = warned(10);
        let mut same = snapshot(many(2));
        same.config_mtime = Some(UNIX_EPOCH + Duration::from_secs(10));
        let Cmd::LoadAccounts { r#gen } = state.load_accounts() else {
            unreachable!()
        };
        update(
            &mut state,
            Msg::Accounts {
                r#gen,
                result: Ok(same),
            },
            NOW_MS,
        );
        assert_eq!(state.startup.warnings, [WARNING], "the same mtime");
        let mut later = snapshot(many(2));
        later.config_mtime = Some(UNIX_EPOCH + Duration::from_secs(11));
        let Cmd::LoadAccounts { r#gen } = state.load_accounts() else {
            unreachable!()
        };
        update(
            &mut state,
            Msg::Accounts {
                r#gen,
                result: Ok(later),
            },
            NOW_MS,
        );
        assert!(state.startup.warnings.is_empty(), "a later mtime");
    }
}
