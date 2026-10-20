//! The state machine (spec §4.2): `update` applies one `Msg` and returns the `Cmd`s it needs. It
//! does no I/O and reads no clock: `now_ms` is the caller's.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::actions::{self, Overlay};
use crate::daemon;
use crate::msg::{Cmd, Msg};
use crate::screens;
use crate::state::{BannerKind, REFRESH_MS, Screen, State};

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
                    state.accounts.apply(snapshot);
                    state.clear_banner(BannerKind::Accounts);
                    if state.quitting.is_some() {
                        return vec![];
                    }
                    let mut cmds = screens::history::on_accounts(state, now_ms);
                    cmds.extend(screens::tokens::index::on_accounts(state, now_ms));
                    cmds
                }
                Err(e) => {
                    state.raise_banner(BannerKind::Accounts, e.message);
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
        Msg::Collected { r#gen, result } => {
            // A collection slower than the timer is overtaken by the next. What it recorded is
            // in the store all the same, so it is read back; its banner and toast are dropped.
            let stale = r#gen != state.gens.collect;
            match result {
                Ok(summary) => {
                    if !stale {
                        state.dirty = true;
                        state.clear_banner(BannerKind::Collect);
                        if let Some(first) = summary.warnings.first() {
                            state.toast(format!("warning: {first}"), now_ms);
                        }
                    }
                    if summary.fetched == 0 || state.quitting.is_some() {
                        return vec![];
                    }
                    // Spec §6.2: a collection that fetched is read back at once.
                    let mut cmds = vec![state.load_accounts()];
                    if state.screen == Screen::History {
                        cmds.extend(screens::history::reload(state, now_ms));
                    }
                    cmds
                }
                Err(_) if stale => vec![],
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
            actions::on_prompt(state, prompt);
            vec![]
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
                if let Some(Overlay::Text { input, .. }) = &mut state.overlay {
                    input.clear();
                }
                vec![]
            }
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
        KeyCode::Char('2') => show(state, Screen::Tokens, now_ms),
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
    // Countdowns change at most once a second; the spinner turns while anything runs.
    let now_s = now_ms.div_euclid(1000);
    if now_s != state.shown_s || state.running.is_some() || state.quitting.is_some() {
        state.shown_s = now_s;
        state.dirty = true;
    }
    // Spec §6.4: the token index's own timers, while Tokens is open.
    let mut cmds = screens::tokens::index::on_tick(state, now_ms);
    // Spec §6.3, §8: the daemon's link.
    cmds.extend(daemon::on_tick(state, now_ms));
    if state.quitting.is_some() || now_ms < state.refresh.next_ms {
        return cmds;
    }
    // Spec §6.2: every 30 s the views are re-read, and the shown accounts offered to the
    // collector, unless an action holds the actions lane (spec §7.2). In live mode the views
    // are read on the daemon's events and every 60 s instead, and the collector is offered
    // only the accounts nothing else collects for (§6.3).
    state.refresh.next_ms = now_ms + REFRESH_MS;
    if !state.daemon.live() {
        cmds.push(state.load_accounts());
        if state.screen == Screen::History {
            cmds.extend(screens::history::reload(state, now_ms));
        }
    }
    let due = daemon::to_collect(state);
    if state.running.is_none() && !due.is_empty() {
        cmds.push(state.collect_due(due));
    }
    cmds
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

    use super::*;
    use crate::source::{CollectSummary, SourceError};
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
            state.banner_text(),
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
            draw(&state, 80, 24)[1].ends_with("loading…"),
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
                result: Ok(CollectSummary { fetched, warnings }),
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

    #[test]
    fn every_30_seconds_the_views_are_read_and_the_shown_accounts_offered_to_the_collector() {
        let mut state = fresh(Screen::Accounts);
        state.start();
        update(&mut state, accounts_msg(1, 2), NOW_MS);
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
        assert_eq!(drawn[1], " the store is locked");
        assert!(drawn[3].contains("a1@x.co"), "{drawn:?}");
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
                }),
            },
            NOW_MS,
        );
        assert_eq!(
            state.toast.as_ref().unwrap().text,
            "warning: usage for a@x.co (position 1) was not collected: x"
        );
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
        let shown: String = lines[1..3]
            .iter()
            .map(|l| l.trim())
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(shown, format!("usage was not collected: {message}"));
        assert_eq!(lines[3], " loading…");
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
}
