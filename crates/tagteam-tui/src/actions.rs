//! Actions in the TUI (spec §5.5, §7): the action keys, the CLI's prompts, and what an action's
//! outcome shows.

use crossterm::event::{KeyCode, KeyEvent};
use tagteam_engine::switch::UsageStrategy;
use tagteam_engine::words;

use crate::daemon;
use crate::host::{Action, ActionOutcome};
use crate::msg::{Answer, AnswerTo, Cmd, Prompt, PromptKind};
use crate::overlay::{ConfirmThen, Overlay, TextThen};
use crate::screens;
use crate::state::{Screen, State};

/// `P` without a daemon to pause (spec §7.4).
pub const NO_DAEMON_RUNNING: &str = "no daemon running";

/// Starts `action`: on the actions lane, or suspended when it needs the terminal (spec §7.3).
/// While another runs it shows `busy: <action>` and drops it (spec §7.2).
pub(crate) fn start(state: &mut State, action: Action, now_ms: i64) -> Vec<Cmd> {
    if let Some(running) = &state.running {
        let text = format!("busy: {}", running.name());
        state.toast(text, now_ms);
        return vec![];
    }
    state.running = Some(action.clone());
    state.dirty = true;
    if action.needs_terminal() {
        vec![Cmd::Suspend(action)]
    } else {
        vec![Cmd::Act(action)]
    }
}

/// `busy: <action>` when one runs; the action keys that open a dialog check it first.
fn busy(state: &mut State, now_ms: i64) -> bool {
    match &state.running {
        Some(running) => {
            let text = format!("busy: {}", running.name());
            state.toast(text, now_ms);
            true
        }
        None => false,
    }
}

/// The provider `A` adds to: `--provider`'s, else the default, which the accounts view lists
/// first (`Engine::accounts`).
fn add_provider(state: &State) -> Option<tagteam_core::ProviderId> {
    state.provider.clone().or_else(|| {
        state
            .accounts
            .snapshot()
            .and_then(|s| s.providers.first())
            .map(|p| p.provider.clone())
    })
}

/// The action keys (spec §5.6, §7.1). `None` for any other key. An account key with no
/// account selected does nothing (Review Focus 3).
pub fn handle_action_key(state: &mut State, key: KeyEvent, now_ms: i64) -> Option<Vec<Cmd>> {
    let KeyCode::Char(c) = key.code else {
        return None;
    };
    match c {
        'A' => Some(match add_provider(state) {
            Some(provider) => start(state, Action::AddLive { provider }, now_ms),
            None => vec![],
        }),
        'D' => {
            let provider = state.provider.clone();
            Some(start(
                state,
                Action::Doctor {
                    provider,
                    online: false,
                },
                now_ms,
            ))
        }
        'r' => Some(refresh(state, now_ms)),
        'P' => Some(pause(state, now_ms)),
        's' | 'b' | 'n' | 'e' | 'a' | 'm' | 'x' | 'R' if state.screen == Screen::Accounts => {
            Some(account_key(state, c, now_ms))
        }
        _ => None,
    }
}

/// `r` (spec §7.1): the views re-read, and the shown accounts offered to the collector at once;
/// on Tokens, a refresh of the token index, on its own lane.
fn refresh(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    if state.screen == Screen::Tokens {
        return screens::tokens::index::refresh_now(state, now_ms);
    }
    if busy(state, now_ms) {
        return vec![];
    }
    let mut cmds = state.reload_views(now_ms);
    let shown = state.accounts.shown_ids();
    if !shown.is_empty() {
        cmds.push(state.collect_due(shown));
    }
    cmds
}

/// `P` (spec §7.4): pause the daemon's auto-switching for the provider the header follows, or
/// resume it, as its last `status` says it is. A confirm dialog asks first, default no. Without a
/// daemon (timer mode) it says so and changes nothing, and so for a provider the daemon runs no
/// job for: the TUI changes no setting whose effect it cannot show.
fn pause(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    if busy(state, now_ms) {
        return vec![];
    }
    if !state.daemon.live() {
        state.toast(NO_DAEMON_RUNNING, now_ms);
        return vec![];
    }
    let provider = daemon::focus(state);
    let Some(job) = state.daemon.job(&provider) else {
        state.toast(format!("the daemon runs no job for {provider}"), now_ms);
        return vec![];
    };
    let (verb, on) = if job.autoswitch {
        ("Pause", false)
    } else {
        ("Resume", true)
    };
    state.overlay = Some(Overlay::Confirm {
        question: format!("{verb} auto-switch for {provider}?"),
        choice: false,
        then: ConfirmThen::Run(Action::Autoswitch { provider, on }),
    });
    vec![]
}

fn account_key(state: &mut State, c: char, now_ms: i64) -> Vec<Cmd> {
    let (Some(account), Some(view)) = (state.accounts.selected(), state.accounts.selected_view())
    else {
        return vec![];
    };
    let (disabled, alias) = (view.row.disabled, view.row.alias.clone());
    let named = format!("{} {}", account.position, words::email(&view.row));
    if busy(state, now_ms) {
        return vec![];
    }
    let provider = account.provider.clone();
    match c {
        's' => start(state, Action::Switch(account), now_ms),
        'b' | 'n' => {
            let strategy = if c == 'b' {
                UsageStrategy::Best
            } else {
                UsageStrategy::NextAvailable
            };
            start(state, Action::SwitchBy { provider, strategy }, now_ms)
        }
        'e' if disabled => start(state, Action::Enable(account), now_ms),
        'e' => start(state, Action::Disable(account), now_ms),
        'a' => {
            state.overlay = Some(Overlay::Text {
                prompt: format!("Alias for {named} (empty clears):"),
                input: alias.unwrap_or_default(),
                then: TextThen::Alias(account),
            });
            vec![]
        }
        'm' => {
            state.overlay = Some(Overlay::Text {
                prompt: format!("Move {named} to position:"),
                input: String::new(),
                then: TextThen::Move(account),
            });
            vec![]
        }
        'x' => {
            state.overlay = Some(Overlay::Confirm {
                question: format!(
                    "Remove {named}? Its stored login is deleted; Claude Code is not changed."
                ),
                choice: false,
                then: ConfirmThen::Run(Action::Remove(account)),
            });
            vec![]
        }
        'R' => start(state, Action::Run(account), now_ms),
        _ => vec![],
    }
}

/// A CLI prompt from the actions lane (spec §7.2) opens a dialog bound to its id. During a
/// shutdown it opens nothing and is declined; a dialog it replaces is declined first.
pub fn on_prompt(state: &mut State, prompt: Prompt) -> Vec<Cmd> {
    let Prompt { question, kind, to } = prompt;
    if state.quitting.is_some() {
        return vec![declined(to, &kind)];
    }
    let replaced = state.overlay.as_ref().and_then(Overlay::decline);
    state.overlay = Some(match kind {
        PromptKind::Confirm { default_yes } => Overlay::Confirm {
            question,
            choice: default_yes,
            then: ConfirmThen::Reply(to),
        },
        PromptKind::Choose { options } => Overlay::Choose {
            question,
            options,
            selected: 0,
            reply: to,
        },
    });
    replaced.into_iter().collect()
}

/// The decline of a prompt of `kind` that was never shown.
fn declined(to: AnswerTo, kind: &PromptKind) -> Cmd {
    let answer = match kind {
        PromptKind::Confirm { .. } => Answer::CONFIRM_DECLINED,
        PromptKind::Choose { .. } => Answer::CHOOSE_DECLINED,
    };
    Cmd::Answer { to, answer }
}

/// An action's outcome (spec §7.2): a command that needs the terminal is run again suspended
/// (§7.3); a suspended action's outcome, whose output went to the terminal, shows nothing (R12);
/// doctor's report opens its panel; one plain line is a toast; warnings, several lines or a
/// failure open the result panel. Every view is re-read after every outcome (§7.3 step 4).
pub fn on_done(state: &mut State, action: Action, outcome: ActionOutcome, now_ms: i64) -> Vec<Cmd> {
    let (exit, out, err) = match outcome {
        ActionOutcome::NeedsTerminal if state.quitting.is_none() => {
            state.running = Some(action.clone());
            return vec![Cmd::Suspend(action)];
        }
        ActionOutcome::NeedsTerminal => {
            state.running = None;
            return vec![];
        }
        ActionOutcome::Done { exit, out, err } => (exit, out, err),
    };
    state.running = None;
    state.dirty = true;
    if state.quitting.is_some() {
        return vec![];
    }
    // Spec §7.4: a pause need not change the daemon's mode, so no event may follow; its
    // `status` is fetched again once it has applied the setting.
    if matches!(action, Action::Autoswitch { .. }) {
        daemon::recheck(state, now_ms);
    }
    let text = [out.trim_end(), err.trim_end()]
        .into_iter()
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    match action {
        // R12: a suspended action's output went to the terminal; there is nothing to show.
        _ if text.is_empty() => {}
        Action::Doctor { online, .. } => {
            state.overlay = Some(Overlay::Doctor {
                text,
                scroll: 0,
                online,
            });
        }
        _ if exit == 0 && err.trim().is_empty() && text.lines().count() == 1 => {
            state.toast(text, now_ms);
        }
        _ => {
            state.overlay = Some(Overlay::Result {
                title: action.name().to_owned(),
                text,
                scroll: 0,
            });
        }
    }
    state.reload_views(now_ms)
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyCode;

    use super::*;
    use crate::host::AccountRef;
    use crate::msg::Msg;
    use crate::testutil::*;
    use crate::update::update;

    fn loaded(n: u32) -> State {
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snapshot(many(n)));
        state
    }

    fn sel(position: u32) -> AccountRef {
        AccountRef {
            id: id(position),
            provider: cc(),
            position,
        }
    }

    fn done(exit: i32, out: &str, err: &str) -> ActionOutcome {
        ActionOutcome::Done {
            exit,
            out: out.into(),
            err: err.into(),
        }
    }

    #[test]
    fn the_account_keys_run_the_cli_s_commands_on_the_selected_account() {
        let mut disabled = many(2);
        disabled[1].row.disabled = true;
        for (c, action) in [
            ('s', Action::Switch(sel(1))),
            (
                'b',
                Action::SwitchBy {
                    provider: cc(),
                    strategy: UsageStrategy::Best,
                },
            ),
            (
                'n',
                Action::SwitchBy {
                    provider: cc(),
                    strategy: UsageStrategy::NextAvailable,
                },
            ),
            ('e', Action::Disable(sel(1))),
            ('A', Action::AddLive { provider: cc() }),
            (
                'D',
                Action::Doctor {
                    provider: None,
                    online: false,
                },
            ),
        ] {
            let mut state = loaded(2);
            let cmds = update(&mut state, ch(c), NOW_MS);
            assert_eq!(cmds, [Cmd::Act(action.clone())], "{c}");
            assert_eq!(state.running, Some(action), "{c}");
        }
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snapshot(disabled));
        update(&mut state, ch('j'), NOW_MS);
        assert_eq!(
            update(&mut state, ch('e'), NOW_MS),
            [Cmd::Act(Action::Enable(sel(2)))]
        );
    }

    #[test]
    fn run_always_suspends() {
        let mut state = loaded(1);
        assert_eq!(
            update(&mut state, ch('R'), NOW_MS),
            [Cmd::Suspend(Action::Run(sel(1)))]
        );
        assert_eq!(state.running, Some(Action::Run(sel(1))));
    }

    #[test]
    fn a_second_action_while_one_runs_is_busy_and_dropped() {
        let mut state = loaded(2);
        update(&mut state, ch('s'), NOW_MS);
        for c in ['s', 'b', 'x', 'a', 'm', 'A', 'D', 'R', 'r'] {
            let cmds = update(&mut state, ch(c), NOW_MS);
            assert!(cmds.is_empty(), "{c}: {cmds:?}");
            assert_eq!(state.toast.as_ref().unwrap().text, "busy: switch", "{c}");
            assert!(state.overlay.is_none(), "{c}");
        }
        assert_eq!(state.running, Some(Action::Switch(sel(1))));
    }

    /// Review Focus 3: with no accounts, every account key does nothing; `A`, `D` and `?` work.
    #[test]
    fn with_no_accounts_only_add_doctor_and_help_do_anything() {
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snapshot(vec![]));
        for c in ['s', 'b', 'n', 'e', 'a', 'm', 'x', 'R'] {
            let cmds = update(&mut state, ch(c), NOW_MS);
            assert!(cmds.is_empty(), "{c}: {cmds:?}");
            assert!(state.overlay.is_none() && state.toast.is_none() && state.running.is_none());
        }
        let cmds = update(&mut state, ch('r'), NOW_MS);
        assert!(
            matches!(cmds.as_slice(), [Cmd::LoadAccounts { .. }]),
            "{cmds:?}"
        );
        assert_eq!(
            update(&mut state, ch('A'), NOW_MS),
            [Cmd::Act(Action::AddLive { provider: cc() })]
        );
        state.running = None;
        assert!(matches!(
            update(&mut state, ch('D'), NOW_MS).as_slice(),
            [Cmd::Act(Action::Doctor { .. })]
        ));
        update(&mut state, ch('?'), NOW_MS);
        assert!(matches!(state.overlay, Some(Overlay::Help)));
    }

    #[test]
    fn add_uses_the_provider_flag_over_the_default() {
        let mut state = loaded(1);
        state.provider = Some(tagteam_core::ProviderId::new("fake-agent"));
        assert_eq!(
            update(&mut state, ch('A'), NOW_MS),
            [Cmd::Act(Action::AddLive {
                provider: tagteam_core::ProviderId::new("fake-agent")
            })]
        );
        let mut unloaded = fresh(Screen::Accounts);
        assert!(update(&mut unloaded, ch('A'), NOW_MS).is_empty());
    }

    #[test]
    fn remove_asks_its_own_question_and_defaults_to_no() {
        let mut state = loaded(2);
        update(&mut state, ch('j'), NOW_MS);
        assert!(update(&mut state, ch('x'), NOW_MS).is_empty());
        let Some(Overlay::Confirm {
            question, choice, ..
        }) = &state.overlay
        else {
            panic!("{:?}", state.overlay);
        };
        assert_eq!(
            question,
            "Remove 2 a2@x.co? Its stored login is deleted; Claude Code is not changed."
        );
        assert!(!choice);
        let drawn = draw(&state, 80, 24);
        assert!(
            drawn
                .iter()
                .any(|l| l.contains("Remove 2 a2@x.co? Its stored login is")),
            "{drawn:?}"
        );
        // Enter takes the default: no.
        assert!(update(&mut state, key(KeyCode::Enter), NOW_MS).is_empty());
        assert!(state.overlay.is_none() && state.running.is_none());
        update(&mut state, ch('x'), NOW_MS);
        assert!(update(&mut state, key(KeyCode::Esc), NOW_MS).is_empty());
        update(&mut state, ch('x'), NOW_MS);
        assert_eq!(
            update(&mut state, ch('y'), NOW_MS),
            [Cmd::Act(Action::Remove(sel(2)))]
        );
    }

    #[test]
    fn the_alias_dialog_sets_or_clears_and_takes_any_key() {
        let mut state = loaded(1);
        update(&mut state, ch('a'), NOW_MS);
        for c in "-q1?x".chars() {
            update(&mut state, ch(c), NOW_MS);
        }
        let Some(Overlay::Text { input, prompt, .. }) = &state.overlay else {
            panic!("{:?}", state.overlay);
        };
        assert_eq!(input, "-q1?x");
        assert_eq!(prompt, "Alias for 1 a1@x.co (empty clears):");
        assert_eq!(state.quitting, None, "q was typed, not quit");
        assert_eq!(
            update(&mut state, key(KeyCode::Enter), NOW_MS),
            [Cmd::Act(Action::SetAlias {
                account: sel(1),
                alias: "-q1?x".into()
            })]
        );
        state.running = None;
        let mut aliased = many(1);
        aliased[0].row.alias = Some("w".into());
        state.accounts.apply(snapshot(aliased));
        update(&mut state, ch('a'), NOW_MS);
        update(&mut state, key(KeyCode::Backspace), NOW_MS);
        assert_eq!(
            update(&mut state, key(KeyCode::Enter), NOW_MS),
            [Cmd::Act(Action::ClearAlias(sel(1)))]
        );
        state.running = None;
        update(&mut state, ch('a'), NOW_MS);
        assert!(update(&mut state, key(KeyCode::Esc), NOW_MS).is_empty());
        assert!(state.overlay.is_none() && state.running.is_none());
    }

    fn alias_dialog_with(alias: &str) -> State {
        let mut state = loaded(1);
        let mut aliased = many(1);
        aliased[0].row.alias = Some(alias.into());
        state.accounts.apply(snapshot(aliased));
        update(&mut state, ch('a'), NOW_MS);
        state
    }

    fn typed(state: &State) -> &str {
        let Some(Overlay::Text { input, .. }) = &state.overlay else {
            panic!("{:?}", state.overlay);
        };
        input
    }

    #[test]
    fn ctrl_h_is_backspace_in_a_text_dialog() {
        // crossterm reports a terminal's 0x08 Backspace as Ctrl+h.
        let mut state = alias_dialog_with("work");
        assert_eq!(typed(&state), "work");
        update(&mut state, ctrl('h'), NOW_MS);
        assert_eq!(typed(&state), "wor");
        assert_eq!(state.quitting, None);
        update(&mut state, ctrl('h'), NOW_MS);
        update(&mut state, ctrl('h'), NOW_MS);
        update(&mut state, ctrl('h'), NOW_MS);
        assert_eq!(typed(&state), "");
        update(&mut state, ctrl('h'), NOW_MS);
        assert_eq!(typed(&state), "", "an empty input stays empty");
    }

    #[test]
    fn ctrl_u_clears_a_text_dialog_s_input() {
        let mut state = alias_dialog_with("work");
        update(&mut state, ctrl('u'), NOW_MS);
        assert_eq!(typed(&state), "");
        assert!(state.overlay.is_some());
        assert_eq!(
            update(&mut state, key(KeyCode::Enter), NOW_MS),
            [Cmd::Act(Action::ClearAlias(sel(1)))]
        );
    }

    #[test]
    fn another_ctrl_key_does_nothing_in_a_dialog() {
        let mut state = loaded(2);
        update(&mut state, ch('x'), NOW_MS);
        let before = format!("{:?}", state.overlay);
        for c in ['n', 'y', 'l', 'u', 'h'] {
            assert!(update(&mut state, ctrl(c), NOW_MS).is_empty(), "{c}");
            assert_eq!(format!("{:?}", state.overlay), before, "{c}");
        }
        assert!(state.running.is_none());
        let mut state = alias_dialog_with("work");
        update(&mut state, ctrl('n'), NOW_MS);
        update(&mut state, ctrl('w'), NOW_MS);
        assert_eq!(typed(&state), "work");
    }

    #[test]
    fn the_move_dialog_takes_digits_only() {
        let mut state = loaded(3);
        update(&mut state, ch('m'), NOW_MS);
        assert!(
            update(&mut state, key(KeyCode::Enter), NOW_MS).is_empty(),
            "nothing typed"
        );
        assert!(state.overlay.is_some());
        for c in "a-3x".chars() {
            update(&mut state, ch(c), NOW_MS);
        }
        let Some(Overlay::Text { input, .. }) = &state.overlay else {
            panic!();
        };
        assert_eq!(input, "3");
        assert_eq!(
            update(&mut state, key(KeyCode::Enter), NOW_MS),
            [Cmd::Act(Action::Move {
                account: sel(1),
                position: 3
            })]
        );
    }

    fn confirm_prompt(to: u64) -> Msg {
        Msg::Prompt(Prompt {
            question: "Add it?".into(),
            kind: PromptKind::Confirm { default_yes: true },
            to: AnswerTo(to),
        })
    }

    fn choose_prompt(to: u64, options: &[&str]) -> Prompt {
        Prompt {
            question: "Which account?".into(),
            kind: PromptKind::Choose {
                options: options.iter().map(|o| (*o).to_owned()).collect(),
            },
            to: AnswerTo(to),
        }
    }

    fn answer(to: u64, answer: Answer) -> [Cmd; 1] {
        [Cmd::Answer {
            to: AnswerTo(to),
            answer,
        }]
    }

    #[test]
    fn a_cli_prompt_opens_a_dialog_bound_to_its_reply() {
        let mut state = loaded(1);
        assert_eq!(update(&mut state, confirm_prompt(1), NOW_MS), []);
        assert!(matches!(
            state.overlay,
            Some(Overlay::Confirm { choice: true, .. })
        ));
        update(&mut state, key(KeyCode::Right), NOW_MS);
        assert_eq!(
            update(&mut state, key(KeyCode::Enter), NOW_MS),
            answer(1, Answer::Confirm(false))
        );

        let prompt = choose_prompt(2, &["one", "two", "three"]);
        assert_eq!(update(&mut state, Msg::Prompt(prompt), NOW_MS), []);
        let drawn = draw(&state, 80, 24);
        assert!(drawn.iter().any(|l| l.contains("1. one")), "{drawn:?}");
        update(&mut state, key(KeyCode::Down), NOW_MS);
        update(&mut state, key(KeyCode::Down), NOW_MS);
        update(&mut state, key(KeyCode::Down), NOW_MS);
        assert_eq!(
            update(&mut state, key(KeyCode::Enter), NOW_MS),
            answer(2, Answer::Choose(Some(2)))
        );

        for (code, want) in [
            (KeyCode::Esc, Answer::Choose(None)),
            (KeyCode::Char('2'), Answer::Choose(Some(1))),
        ] {
            assert_eq!(on_prompt(&mut state, choose_prompt(3, &["a", "b"])), []);
            assert_eq!(update(&mut state, key(code), NOW_MS), answer(3, want));
        }
        let prompt = Prompt {
            question: "Sure?".into(),
            kind: PromptKind::Confirm { default_yes: true },
            to: AnswerTo(4),
        };
        assert_eq!(on_prompt(&mut state, prompt), []);
        assert_eq!(
            update(&mut state, key(KeyCode::Esc), NOW_MS),
            answer(4, Answer::Confirm(false)),
            "Esc declines"
        );
    }

    #[test]
    fn a_second_prompt_declines_the_dialog_it_replaces() {
        let mut state = loaded(1);
        assert_eq!(update(&mut state, confirm_prompt(1), NOW_MS), []);
        let second = choose_prompt(2, &["one", "two"]);
        assert_eq!(
            update(&mut state, Msg::Prompt(second), NOW_MS),
            answer(1, Answer::Confirm(false))
        );
        assert!(matches!(
            state.overlay,
            Some(Overlay::Choose {
                reply: AnswerTo(2),
                ..
            })
        ));
        let drawn = draw(&state, 80, 24);
        assert!(drawn.iter().any(|l| l.contains("1. one")), "{drawn:?}");
    }

    #[test]
    fn a_prompt_during_a_shutdown_is_declined_unshown() {
        let mut state = loaded(1);
        state.quitting = Some(0);
        assert_eq!(
            update(&mut state, confirm_prompt(1), NOW_MS),
            answer(1, Answer::Confirm(false))
        );
        assert!(state.overlay.is_none());
        let choice = choose_prompt(2, &["one"]);
        assert_eq!(
            update(&mut state, Msg::Prompt(choice), NOW_MS),
            answer(2, Answer::Choose(None))
        );
        assert!(state.overlay.is_none());
    }

    #[test]
    fn quitting_with_a_dialog_open_declines_it() {
        let mut state = loaded(1);
        assert_eq!(update(&mut state, confirm_prompt(1), NOW_MS), []);
        assert_eq!(
            update(&mut state, ctrl('c'), NOW_MS),
            [Cmd::Quit { code: 0 }]
        );
        assert!(state.overlay.is_none(), "the runtime declines its prompt");
    }

    #[test]
    fn one_plain_line_is_a_toast_and_the_views_are_read_again() {
        let mut state = loaded(1);
        update(&mut state, ch('s'), NOW_MS);
        let cmds = update(
            &mut state,
            Msg::ActionDone {
                action: Action::Switch(sel(1)),
                outcome: done(0, "Enabled 1 a1@x.co.\n", ""),
            },
            NOW_MS,
        );
        assert!(
            matches!(cmds.as_slice(), [Cmd::LoadAccounts { .. }]),
            "{cmds:?}"
        );
        assert_eq!(state.toast.as_ref().unwrap().text, "Enabled 1 a1@x.co.");
        assert!(state.running.is_none() && state.overlay.is_none());
        assert_eq!(draw(&state, 80, 24)[23], "Enabled 1 a1@x.co.");
    }

    #[test]
    fn warnings_several_lines_or_a_failure_open_the_result_panel() {
        for (exit, out, err, want) in [
            (
                0,
                "Switched to b@x.co (position 2).\nRestart Claude Code to apply.\n",
                "",
                "Switched to b@x.co (position 2).\nRestart Claude Code to apply.",
            ),
            (
                0,
                "Switched.\n",
                "warning: usage was not collected\n",
                "Switched.\nwarning: usage was not collected",
            ),
            (
                1,
                "",
                "tagteam: the selected account changed; nothing was done\n",
                "tagteam: the selected account changed; nothing was done",
            ),
        ] {
            let mut state = loaded(1);
            update(&mut state, ch('s'), NOW_MS);
            let cmds = on_done(
                &mut state,
                Action::Switch(sel(1)),
                done(exit, out, err),
                NOW_MS,
            );
            assert!(matches!(cmds.as_slice(), [Cmd::LoadAccounts { .. }]));
            let Some(Overlay::Result { title, text, .. }) = &state.overlay else {
                panic!("{exit} {out:?} {err:?}: {:?}", state.overlay);
            };
            assert_eq!((title.as_str(), text.as_str()), ("switch", want));
        }
        let mut state = loaded(1);
        on_done(
            &mut state,
            Action::Switch(sel(1)),
            done(
                1,
                "",
                "tagteam: the selected account changed; nothing was done\n",
            ),
            NOW_MS,
        );
        let drawn = draw(&state, 80, 24);
        assert!(
            drawn
                .iter()
                .any(|l| l.contains("tagteam: the selected account changed; nothing was done")),
            "{drawn:?}"
        );
        update(&mut state, key(KeyCode::Esc), NOW_MS);
        assert!(state.overlay.is_none());
    }

    /// R12: a suspended action's output went to the terminal, so its outcome arrives empty and
    /// shows nothing, whatever its exit; every view is read again (spec §7.3 step 4).
    #[test]
    fn a_suspended_actions_outcome_shows_nothing_and_rereads_every_view() {
        for exit in [0, 1, 130] {
            let mut state = loaded(1);
            state.history.select(id(1));
            state.screen = Screen::History;
            state.running = Some(Action::Run(sel(1)));
            let cmds = on_done(&mut state, Action::Run(sel(1)), done(exit, "", ""), NOW_MS);
            assert!(
                matches!(
                    cmds.as_slice(),
                    [Cmd::LoadAccounts { .. }, Cmd::LoadHistory { account, .. }]
                        if *account == id(1)
                ),
                "{exit}: {cmds:?}"
            );
            assert!(state.toast.is_none() && state.overlay.is_none(), "{exit}");
            assert!(state.running.is_none(), "{exit}");
        }
    }

    #[test]
    fn needs_terminal_runs_the_action_again_suspended() {
        let mut state = loaded(1);
        update(&mut state, ch('s'), NOW_MS);
        let cmds = on_done(
            &mut state,
            Action::Switch(sel(1)),
            ActionOutcome::NeedsTerminal,
            NOW_MS,
        );
        assert_eq!(cmds, [Cmd::Suspend(Action::Switch(sel(1)))]);
        assert_eq!(
            state.running,
            Some(Action::Switch(sel(1))),
            "still running, suspended"
        );
    }

    #[test]
    fn doctor_opens_its_panel_and_o_runs_it_online() {
        let mut state = loaded(1);
        update(&mut state, ch('D'), NOW_MS);
        let report: String = (1..=40).map(|i| format!("check {i}: ok\n")).collect();
        on_done(
            &mut state,
            Action::Doctor {
                provider: None,
                online: false,
            },
            done(0, &report, ""),
            NOW_MS,
        );
        let drawn = draw(&state, 80, 24);
        assert!(drawn.iter().any(|l| l.contains(" doctor ")), "{drawn:?}");
        assert!(drawn.iter().any(|l| l.contains("check 1: ok")));
        for _ in 0..5 {
            update(&mut state, key(KeyCode::Down), NOW_MS);
        }
        let drawn = draw(&state, 80, 24);
        assert!(drawn.iter().any(|l| l.contains("check 6: ok")));
        assert!(!drawn.iter().any(|l| l.contains("check 5: ok")));
        for _ in 0..100 {
            update(&mut state, key(KeyCode::Down), NOW_MS);
        }
        assert!(
            matches!(state.overlay, Some(Overlay::Doctor { scroll: 20, .. })),
            "40 rows in a 20-row panel: the last page, not the last row on top"
        );
        assert_eq!(
            update(&mut state, ch('o'), NOW_MS),
            [Cmd::Act(Action::Doctor {
                provider: None,
                online: true
            })]
        );
        update(&mut state, key(KeyCode::Esc), NOW_MS);
        assert!(state.overlay.is_none());
    }

    #[test]
    fn a_long_output_line_wraps_in_its_panel_and_scrolls_to_its_end() {
        // Spec §5.5: the panel shows the action's output; at 80 columns it is 72 wide.
        let mut state = loaded(1);
        let long = "  warning: usage for averyveryverylongname@example.com (position 2) was not collected: the endpoint answered http-429, retry in 5m";
        let text: String = (1..=30)
            .map(|i| format!("line {i}\n"))
            .chain([format!("{long}\n")])
            .collect();
        on_done(
            &mut state,
            Action::Switch(sel(1)),
            done(1, &text, ""),
            NOW_MS,
        );
        update(&mut state, key(KeyCode::End), NOW_MS);
        let Some(Overlay::Result { scroll, .. }) = &state.overlay else {
            panic!("{:?}", state.overlay);
        };
        assert_eq!(*scroll, 12, "32 rows, the last page of 20");
        let drawn = draw(&state, 80, 24);
        let inside = |l: &String| l.chars().skip(4).take(72).collect::<String>();
        assert_eq!(
            inside(&drawn[21]).trim_end(),
            "  not collected: the endpoint answered http-429, retry in 5m",
            "the last row, indented as its line is: {drawn:?}"
        );
        assert_eq!(
            drawn[2].chars().skip(4).take(7).collect::<String>(),
            "line 13"
        );
        update(&mut state, key(KeyCode::Up), NOW_MS);
        let drawn = draw(&state, 80, 24);
        assert_eq!(
            inside(&drawn[21]).trim_end(),
            "  warning: usage for averyveryverylongname@example.com (position 2) was"
        );
    }

    #[test]
    fn a_resize_clamps_a_panels_scroll_and_the_next_key_moves_at_once() {
        // Thirty 100-column lines: two rows each at 80 columns, one at 120.
        let text: String = (1..=30)
            .map(|i| format!("{i:02} {}\n", "word ".repeat(19).trim_end()))
            .collect();
        let first_row = |state: &State, w: u16| -> String {
            let drawn = draw(state, w, 24);
            drawn[2]
                .chars()
                .skip(4)
                .take(usize::from(w) - 8)
                .collect::<String>()
        };
        for action in [
            Action::Switch(sel(1)),
            Action::Doctor {
                provider: None,
                online: false,
            },
        ] {
            let mut state = loaded(1);
            on_done(&mut state, action.clone(), done(1, &text, ""), NOW_MS);
            update(&mut state, key(KeyCode::End), NOW_MS);
            assert!(
                first_row(&state, 80).starts_with("21 word"),
                "{action:?}: the last page of 60 rows starts at row 40"
            );
            update(
                &mut state,
                Msg::Resize {
                    width: 120,
                    height: 24,
                },
                NOW_MS,
            );
            assert!(
                first_row(&state, 120).starts_with("11 word"),
                "{action:?}: the last page of 30 rows, not a blank panel"
            );
            update(&mut state, key(KeyCode::Up), NOW_MS);
            assert!(
                first_row(&state, 120).starts_with("10 word"),
                "{action:?}: Up moves from what shows"
            );
        }
    }

    #[test]
    fn a_panel_scrolls_no_further_than_its_last_page() {
        // A 24-row terminal: 20 rows of text between the panel's borders.
        for (lines, furthest) in [(5, 0), (20, 0), (21, 1), (45, 25)] {
            for action in [
                Action::Switch(sel(1)),
                Action::Doctor {
                    provider: None,
                    online: false,
                },
            ] {
                let text: String = (1..=lines).map(|i| format!("line {i}\n")).collect();
                let mut state = loaded(1);
                on_done(&mut state, action.clone(), done(1, &text, ""), NOW_MS);
                for code in [KeyCode::End, KeyCode::PageDown, KeyCode::Down] {
                    update(&mut state, key(code), NOW_MS);
                    update(&mut state, key(KeyCode::Down), NOW_MS);
                    let Some(Overlay::Result { scroll, .. } | Overlay::Doctor { scroll, .. }) =
                        &state.overlay
                    else {
                        panic!("{:?}", state.overlay);
                    };
                    assert!(*scroll <= furthest, "{lines} lines, {code:?}: {scroll}");
                }
                update(&mut state, key(KeyCode::End), NOW_MS);
                let Some(Overlay::Result { scroll, .. } | Overlay::Doctor { scroll, .. }) =
                    &state.overlay
                else {
                    panic!("{:?}", state.overlay);
                };
                assert_eq!(*scroll, furthest, "{lines} lines");
                let drawn = draw(&state, 80, 24);
                assert!(
                    drawn.iter().any(|l| l.contains(&format!("line {lines}"))),
                    "the last line shows: {drawn:?}"
                );
                assert!(
                    drawn
                        .iter()
                        .any(|l| l.contains(&format!("line {}", lines - 19.min(lines - 1)))),
                    "and a full page with it: {drawn:?}"
                );
            }
        }
    }

    #[test]
    fn blank_output_lines_are_blank_rows_in_the_panels() {
        let text = "claude-code\n  ok one\n\nfake-agent\n  ok two\n   \n2 ok";
        for action in [
            Action::Switch(sel(1)),
            Action::Doctor {
                provider: None,
                online: false,
            },
        ] {
            let mut state = loaded(1);
            on_done(&mut state, action.clone(), done(1, text, ""), NOW_MS);
            let drawn = draw(&state, 80, 24);
            let inside = |l: &String| l.chars().skip(4).take(72).collect::<String>();
            let rows: Vec<String> = drawn[2..9]
                .iter()
                .map(|l| inside(l).trim_end().to_owned())
                .collect();
            assert_eq!(
                rows,
                [
                    "claude-code",
                    "  ok one",
                    "",
                    "fake-agent",
                    "  ok two",
                    "",
                    "2 ok"
                ],
                "{action:?}"
            );
            update(&mut state, key(KeyCode::End), NOW_MS);
            let Some(Overlay::Result { scroll, .. } | Overlay::Doctor { scroll, .. }) =
                &state.overlay
            else {
                panic!("{:?}", state.overlay);
            };
            assert_eq!(*scroll, 0, "{action:?}: seven rows fit the panel");
        }
    }

    #[test]
    fn an_indent_wider_than_a_third_of_a_panel_is_capped() {
        let mut state = loaded(1);
        let line = format!("{}{}", " ".repeat(60), "word ".repeat(20).trim_end());
        on_done(
            &mut state,
            Action::Switch(sel(1)),
            done(1, &line, ""),
            NOW_MS,
        );
        let drawn = draw(&state, 80, 24);
        let rows: Vec<String> = drawn[2..5]
            .iter()
            .map(|l| {
                l.chars()
                    .skip(4)
                    .take(72)
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect();
        let indent = " ".repeat(24);
        assert!(
            rows.iter().all(|r| r.starts_with(&indent) && r.len() > 24),
            "every row keeps a third of the panel for its words: {rows:?}"
        );
        assert_eq!(
            rows.iter()
                .map(|r| r.matches("word").count())
                .sum::<usize>(),
            20
        );
    }

    #[test]
    fn a_long_choice_wraps_under_its_text_and_is_selected_whole() {
        let mut state = loaded(1);
        let long =
            "claude-code #2 averyveryverylongname.with.a.long.tail@example.com [Example Org]";
        update(
            &mut state,
            Msg::Prompt(Prompt {
                question: "Which account?".into(),
                kind: PromptKind::Choose {
                    options: vec!["claude-code #1 a@x.co".into(), long.into()],
                },
                to: AnswerTo(1),
            }),
            NOW_MS,
        );
        update(&mut state, key(KeyCode::Down), NOW_MS);
        let buf = draw_buffer(&state, 80, 24, NOW_MS);
        let lines = text_of(&buf);
        let at = lines
            .iter()
            .position(|l| l.contains("2. claude-code #2"))
            .unwrap();
        let text = |y: usize| -> String {
            lines[y]
                .chars()
                .skip(10)
                .take(60)
                .collect::<String>()
                .trim_end()
                .to_owned()
        };
        assert_eq!(
            [text(at), text(at + 1), text(at + 2)],
            [
                "2. claude-code #2",
                "   averyveryverylongname.with.a.long.tail@example.com",
                "   [Example Org]",
            ]
        );
        for y in at..at + 3 {
            let cell = &buf[(10, u16::try_from(y).unwrap())];
            let reversed = ratatui::style::Modifier::REVERSED;
            assert!(cell.modifier.contains(reversed), "row {y}");
        }
    }

    #[test]
    fn a_1000_character_choice_takes_three_rows_and_every_choice_stays_reachable() {
        let mut state = loaded(1);
        let long = "Claude ".repeat(143).trim_end().to_owned();
        let options: Vec<String> = [long]
            .into_iter()
            .chain((2..=20).map(|i| format!("claude-code #{i} a{i}@x.co")))
            .collect();
        update(
            &mut state,
            Msg::Prompt(Prompt {
                question: "Which account?".into(),
                kind: PromptKind::Choose { options },
                to: AnswerTo(1),
            }),
            NOW_MS,
        );
        let text = |state: &State| -> Vec<String> {
            draw(state, 80, 24)
                .iter()
                .map(|l| {
                    l.chars()
                        .skip(10)
                        .take(60)
                        .collect::<String>()
                        .trim_end()
                        .to_owned()
                })
                .collect()
        };
        let shown = text(&state);
        let first = shown
            .iter()
            .position(|l| l.starts_with("1. Claude"))
            .unwrap();
        assert!(shown[first + 2].ends_with('…'), "{shown:?}");
        assert!(
            shown[first + 3].starts_with("2. claude-code #2"),
            "{shown:?}"
        );
        assert!(shown.iter().any(|l| l == "↑ ↓ choose · Enter · Esc cancel"));
        for _ in 0..19 {
            update(&mut state, key(KeyCode::Down), NOW_MS);
        }
        let shown = text(&state);
        assert!(
            shown.iter().any(|l| l.starts_with("20. claude-code #20")),
            "{shown:?}"
        );
        assert!(shown.iter().any(|l| l == "↑ ↓ choose · Enter · Esc cancel"));
    }

    #[test]
    fn a_long_question_is_cut_so_the_dialog_keeps_its_buttons() {
        let mut state = loaded(1);
        let question = format!("{}?", "Claude ".repeat(286).trim_end());
        update(
            &mut state,
            Msg::Prompt(Prompt {
                question,
                kind: PromptKind::Confirm { default_yes: false },
                to: AnswerTo(1),
            }),
            NOW_MS,
        );
        let drawn = draw(&state, 80, 24);
        assert!(drawn.iter().any(|l| l.contains("[ Yes ]")), "{drawn:?}");
        assert!(
            drawn.iter().any(|l| l.contains("y yes · n no")),
            "{drawn:?}"
        );
        assert!(
            drawn.iter().any(|l| l.contains('…')),
            "the question is cut: {drawn:?}"
        );
    }

    #[test]
    fn help_lists_the_keys_and_closes() {
        let mut state = loaded(1);
        update(&mut state, ch('?'), NOW_MS);
        let drawn = draw(&state, 80, 24);
        assert!(drawn.iter().any(|l| l.contains("Everywhere")), "{drawn:?}");
        assert!(
            drawn
                .iter()
                .any(|l| l.contains("R") && l.contains("run a session"))
        );
        update(&mut state, ch('?'), NOW_MS);
        assert!(state.overlay.is_none());
        update(&mut state, ch('?'), NOW_MS);
        update(&mut state, key(KeyCode::Esc), NOW_MS);
        assert!(state.overlay.is_none());
    }

    #[test]
    fn refresh_rereads_and_collects_the_shown_accounts() {
        let mut state = loaded(2);
        assert_eq!(
            update(&mut state, ch('r'), NOW_MS),
            [
                Cmd::LoadAccounts { r#gen: 1 },
                Cmd::CollectDue {
                    r#gen: 1,
                    accounts: vec![id(1), id(2)]
                }
            ]
        );
    }

    /// Claude Code's job in `mode`, with `autoswitch`.
    fn job_in(mode: &str, autoswitch: bool) -> tagteam_daemon::protocol::Status {
        daemon_status(vec![daemon_job("claude-code", mode, autoswitch)], vec![])
    }

    /// The confirm dialog's question, its default, and the action a yes runs.
    fn asked(state: &State) -> (&str, bool, &Action) {
        match &state.overlay {
            Some(Overlay::Confirm {
                question,
                choice,
                then: ConfirmThen::Run(action),
                ..
            }) => (question.as_str(), *choice, action),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn p_asks_to_pause_the_daemon_s_auto_switch_and_a_yes_sets_it_through_the_cli() {
        // Spec §7.4: from the effective `autoswitch` in `status`; default no.
        let mut state = loaded(2);
        go_live(&mut state, job_in("auto", true));
        assert!(update(&mut state, ch('P'), NOW_MS).is_empty());
        let pause = Action::Autoswitch {
            provider: cc(),
            on: false,
        };
        assert_eq!(
            asked(&state),
            ("Pause auto-switch for claude-code?", false, &pause)
        );
        let drawn = draw(&state, 80, 24);
        assert!(
            drawn
                .iter()
                .any(|l| l.contains("Pause auto-switch for claude-code?")),
            "{drawn:?}"
        );
        assert!(
            update(&mut state, key(KeyCode::Enter), NOW_MS).is_empty(),
            "no"
        );
        assert!(state.overlay.is_none() && state.running.is_none());
        update(&mut state, ch('P'), NOW_MS);
        assert_eq!(
            update(&mut state, ch('y'), NOW_MS),
            [Cmd::Act(pause.clone())]
        );
        assert_eq!(state.running, Some(pause));
        assert_eq!(draw(&state, 80, 24)[23], "⠋ config…");
    }

    #[test]
    fn p_asks_to_resume_auto_switch_that_is_paused() {
        let mut state = loaded(2);
        go_live(&mut state, job_in("collect", false));
        update(&mut state, ch('P'), NOW_MS);
        let resume = Action::Autoswitch {
            provider: cc(),
            on: true,
        };
        assert_eq!(
            asked(&state),
            ("Resume auto-switch for claude-code?", false, &resume)
        );
    }

    #[test]
    fn p_without_a_daemon_says_so_and_changes_nothing() {
        // Spec §7.4: in timer mode `P` shows `no daemon running`, also while the first
        // `status` is still to come; and so for a provider the daemon runs no job for.
        let mut state = loaded(1);
        assert!(update(&mut state, ch('P'), NOW_MS).is_empty());
        assert_eq!(state.toast.as_ref().unwrap().text, NO_DAEMON_RUNNING);
        assert!(state.overlay.is_none());
        update(
            &mut state,
            Msg::Live(crate::live::Live::Up { link: 1 }),
            NOW_MS,
        );
        update(&mut state, ch('P'), NOW_MS);
        assert_eq!(state.toast.as_ref().unwrap().text, NO_DAEMON_RUNNING);
        let mut state = loaded(1);
        let other = daemon_status(vec![daemon_job("fake-agent", "auto", true)], vec![]);
        go_live(&mut state, other);
        assert!(update(&mut state, ch('P'), NOW_MS).is_empty());
        assert_eq!(
            state.toast.as_ref().unwrap().text,
            "the daemon runs no job for claude-code"
        );
        assert!(state.overlay.is_none());
    }

    #[test]
    fn p_acts_on_the_selected_account_s_provider() {
        // Spec §7.4: with several providers.
        let fake = tagteam_core::ProviderId::new("fake-agent");
        let mut third = account(3, "c@x.co", read(60, 10.0, 20.0, false));
        third.row.provider = fake.clone();
        let mut snap = snapshot(many(1));
        snap.providers
            .push(tagteam_engine::views::ProviderAccounts {
                provider: fake.clone(),
                active_position: None,
                accounts: vec![third],
                threshold: 90.0,
                strategy: tagteam_core::autoswitch::Strategy::Best,
                names: tagteam_engine::views::ProviderNames {
                    display_name: "FakeAgent",
                    launch_command: "fakeagent",
                },
            });
        let mut state = fresh(Screen::Accounts);
        state.accounts.apply(snap);
        let status = daemon_status(
            vec![
                daemon_job("claude-code", "auto", true),
                daemon_job("fake-agent", "collect", false),
            ],
            vec![],
        );
        go_live(&mut state, status);
        update(&mut state, ch('j'), NOW_MS);
        update(&mut state, ch('P'), NOW_MS);
        let resume = Action::Autoswitch {
            provider: fake,
            on: true,
        };
        assert_eq!(
            asked(&state),
            ("Resume auto-switch for fake-agent?", false, &resume)
        );
    }

    #[test]
    fn p_while_an_action_runs_is_busy() {
        let mut state = loaded(2);
        go_live(&mut state, job_in("auto", true));
        update(&mut state, ch('s'), NOW_MS);
        assert!(update(&mut state, ch('P'), NOW_MS).is_empty());
        assert_eq!(state.toast.as_ref().unwrap().text, "busy: switch");
        assert!(state.overlay.is_none());
    }

    #[test]
    fn two_seconds_after_p_the_status_is_read_and_the_header_says_paused_with_no_event() {
        // Spec §7.4, §11: with one switchable account the job is `collect` before and after,
        // so no `mode` event follows; the fetch 2 s later shows the pause.
        let mut state = loaded(1);
        let link = go_live(&mut state, job_in("collect", true));
        update(&mut state, Msg::Tick, NOW_MS + 250);
        assert!(draw(&state, 80, 24)[0].ends_with("○ needs 2 usable accounts   ? keys"));
        update(&mut state, ch('P'), NOW_MS);
        let cmds = update(&mut state, ch('y'), NOW_MS);
        let [Cmd::Act(pause)] = cmds.as_slice() else {
            panic!("{cmds:?}");
        };
        let set = "Set provider.claude-code.daemon.autoswitch = false in ~/.config/tagteam/config.toml.\n";
        let cmds = update(
            &mut state,
            Msg::ActionDone {
                action: pause.clone(),
                outcome: done(0, set, ""),
            },
            NOW_MS + 500,
        );
        assert!(
            matches!(cmds.as_slice(), [Cmd::LoadAccounts { .. }]),
            "{cmds:?}"
        );
        assert_eq!(state.toast.as_ref().unwrap().text, set.trim_end());
        let fetch = |cmds: Vec<Cmd>| cmds.contains(&Cmd::FetchStatus { link });
        assert!(!fetch(update(&mut state, Msg::Tick, NOW_MS + 2_499)));
        assert!(fetch(update(&mut state, Msg::Tick, NOW_MS + 2_500)));
        assert!(
            !fetch(update(&mut state, Msg::Tick, NOW_MS + 2_750)),
            "once"
        );
        let reply = Msg::DaemonStatus {
            link,
            result: Ok(job_in("collect", false)),
        };
        update(&mut state, reply, NOW_MS + 2_800);
        assert!(draw(&state, 80, 24)[0].ends_with("‖ paused · P resumes   ? keys"));
    }

    #[test]
    fn p_s_fetch_of_the_status_outlives_a_link_change() {
        // C-R21: the link changes after the setting is written. The new link's own fetch can
        // come before the daemon applies the setting (D4.2), so the fetch 2 s after `P` is
        // still made, on the new link.
        let mut state = loaded(1);
        go_live(&mut state, job_in("collect", true));
        update(&mut state, ch('P'), NOW_MS);
        let cmds = update(&mut state, ch('y'), NOW_MS);
        let [Cmd::Act(pause)] = cmds.as_slice() else {
            panic!("{cmds:?}");
        };
        let set = "Set provider.claude-code.daemon.autoswitch = false in ~/.config/tagteam/config.toml.\n";
        let written = Msg::ActionDone {
            action: pause.clone(),
            outcome: done(0, set, ""),
        };
        update(&mut state, written, NOW_MS + 500);
        update(&mut state, Msg::Live(crate::live::Live::Down), NOW_MS + 600);
        let up = update(
            &mut state,
            Msg::Live(crate::live::Live::Up { link: 2 }),
            NOW_MS + 700,
        );
        assert_eq!(up, [Cmd::FetchStatus { link: 2 }]);
        let early = Msg::DaemonStatus {
            link: 2,
            result: Ok(job_in("collect", true)),
        };
        update(&mut state, early, NOW_MS + 800);
        assert!(
            draw(&state, 80, 24)[0].ends_with("○ needs 2 usable accounts   ? keys"),
            "read before the daemon applied it"
        );
        let fetch = |cmds: Vec<Cmd>| cmds.contains(&Cmd::FetchStatus { link: 2 });
        assert!(!fetch(update(&mut state, Msg::Tick, NOW_MS + 2_499)));
        assert!(fetch(update(&mut state, Msg::Tick, NOW_MS + 2_500)));
        let applied = Msg::DaemonStatus {
            link: 2,
            result: Ok(job_in("collect", false)),
        };
        update(&mut state, applied, NOW_MS + 2_600);
        assert!(draw(&state, 80, 24)[0].ends_with("‖ paused · P resumes   ? keys"));
    }

    #[test]
    fn p_s_fetch_of_the_status_is_kept_when_its_outcome_comes_with_no_link_up() {
        // C-R21: the link drops before the setting's outcome arrives. The fetch 2 s after the
        // outcome is still owed. A link up before then fetches at the 2 s; one up only after
        // them fetches once, at once, which is after the daemon applied the setting.
        let set = "Set provider.claude-code.daemon.autoswitch = false in ~/.config/tagteam/config.toml.\n";
        let fetches = |cmds: &[Cmd]| {
            cmds.iter()
                .filter(|c| matches!(c, Cmd::FetchStatus { .. }))
                .count()
        };
        for up_at in [NOW_MS + 1_500, NOW_MS + 4_000] {
            let mut state = loaded(1);
            go_live(&mut state, job_in("collect", true));
            update(&mut state, ch('P'), NOW_MS);
            let cmds = update(&mut state, ch('y'), NOW_MS);
            let [Cmd::Act(pause)] = cmds.as_slice() else {
                panic!("{cmds:?}");
            };
            update(&mut state, Msg::Live(crate::live::Live::Down), NOW_MS + 100);
            let written = Msg::ActionDone {
                action: pause.clone(),
                outcome: done(0, set, ""),
            };
            update(&mut state, written, NOW_MS + 500);
            let down = update(&mut state, Msg::Tick, NOW_MS + 1_000);
            assert_eq!(fetches(&down), 0, "no link to fetch on");
            let up = update(
                &mut state,
                Msg::Live(crate::live::Live::Up { link: 2 }),
                up_at,
            );
            assert_eq!(up, [Cmd::FetchStatus { link: 2 }], "{up_at}");
            let applied = up_at >= NOW_MS + 2_500;
            let first = Msg::DaemonStatus {
                link: 2,
                result: Ok(job_in("collect", !applied)),
            };
            update(&mut state, first, up_at + 100);
            if !applied {
                let early = update(&mut state, Msg::Tick, NOW_MS + 2_499);
                assert_eq!(fetches(&early), 0, "{up_at}");
                let due = update(&mut state, Msg::Tick, NOW_MS + 2_500);
                assert_eq!(fetches(&due), 1, "{up_at}");
                let paused = Msg::DaemonStatus {
                    link: 2,
                    result: Ok(job_in("collect", false)),
                };
                update(&mut state, paused, NOW_MS + 2_600);
            }
            assert!(
                draw(&state, 80, 24)[0].ends_with("‖ paused · P resumes   ? keys"),
                "{up_at}"
            );
            let later = update(&mut state, Msg::Tick, up_at + 250);
            assert_eq!(fetches(&later), 0, "owed once: {up_at}");
            let later = update(&mut state, Msg::Tick, NOW_MS + 6_000);
            assert_eq!(fetches(&later), 0, "owed once: {up_at}");
        }
    }
}
