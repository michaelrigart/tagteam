//! Actions in the TUI (spec §5.5, §7): the action keys, the CLI's prompts, and what an action's
//! outcome shows.

use crossterm::event::{KeyCode, KeyEvent};
use tagteam_engine::switch::UsageStrategy;
use tagteam_engine::views::AccountView;
use tagteam_engine::words;

use crate::daemon;
use crate::host::{Action, ActionOutcome};
use crate::msg::{Answer, AnswerTo, Cmd, Prompt, PromptKind};
use crate::overlay::{ConfirmThen, Overlay, TextThen};
use crate::screens;
use crate::source::CollectSummary;
use crate::state::{Screen, State, ToastKind};

/// `P` without a daemon to pause (spec §10, T2-16).
pub const NO_DAEMON_RUNNING: &str = "no daemon runs · tagteam daemon install starts one";

/// Starts `action`: on the actions lane, or suspended when it needs the terminal (spec §7.3).
/// While another runs it says `still <label> · wait for it to finish` and drops it (redesign
/// §9.4).
pub(crate) fn start(state: &mut State, action: Action, now_ms: i64) -> Vec<Cmd> {
    if busy(state, now_ms) {
        return vec![];
    }
    state.running = Some(action.clone());
    state.dirty = true;
    if action.needs_terminal() {
        state.on_terminal = true;
        vec![Cmd::Suspend(action)]
    } else {
        vec![Cmd::Act(action)]
    }
}

/// `still <label> · wait for it to finish` when an action runs; the action keys that open a
/// dialog check it first.
fn busy(state: &mut State, now_ms: i64) -> bool {
    let Some(running) = &state.running else {
        return false;
    };
    let text = format!("still {} · wait for it to finish", state.label_of(running));
    state.toast(ToastKind::Info, text, now_ms);
    true
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
        state.checking = Some(state.gens.collect);
    }
    cmds
}

/// The accounts the last accounts view lists.
fn views(state: &State) -> impl Iterator<Item = &AccountView> {
    state
        .accounts
        .snapshot()
        .into_iter()
        .flat_map(|s| s.providers.iter().flat_map(|p| p.accounts.iter()))
}

/// `r`'s toast once its collection is over and the accounts are read (T2-9, redesign §9.7), in
/// this order: accounts rate-limited, the first warning, accounts updated, else that nothing
/// was due.
pub(crate) fn say_checked(state: &mut State, summary: &CollectSummary, now_ms: i64) {
    let now_s = now_ms.div_euclid(1000);
    let in_span = |at: i64| words::duration(at - now_s);
    let (kind, text) = if !summary.rate_limited.is_empty() {
        let retry = views(state)
            .filter(|v| summary.rate_limited.contains(&v.row.id))
            .filter_map(|v| v.usage.retry_at)
            .min();
        let mut text = format!(
            "{} rate-limited",
            words::plural(summary.rate_limited.len() as u64, "account")
        );
        if let Some(at) = retry {
            text.push_str(&format!(" · retry in {}", in_span(at)));
        }
        (ToastKind::Warn, text)
    } else if let Some(first) = summary.warnings.first() {
        (ToastKind::Warn, format!("warning: {first}"))
    } else if summary.updated > 0 {
        let accounts = words::plural(summary.updated as u64, "account");
        (ToastKind::Done, format!("updated {accounts}"))
    } else {
        let next = views(state).filter_map(|v| v.usage.next_poll_at).min();
        let text = match next {
            Some(at) => format!("already current · next update in {}", in_span(at)),
            None => "already current".to_owned(),
        };
        (ToastKind::Info, text)
    };
    state.toast(kind, text, now_ms);
}

/// `P` (spec §7.4): pause the daemon's auto-switching for the provider the header follows, or
/// resume it, as its last `status` says it is. A confirm dialog asks first, default no. Without a
/// daemon (timer mode) it says how to start one and changes nothing, and so for a provider the
/// daemon runs no job for: the TUI changes no setting whose effect it cannot show (T2-16).
fn pause(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    if busy(state, now_ms) {
        return vec![];
    }
    if !state.daemon.live() {
        state.toast(ToastKind::Info, NO_DAEMON_RUNNING, now_ms);
        return vec![];
    }
    let provider = daemon::focus(state);
    let Some(job) = state.daemon.job(&provider) else {
        let display = state.names_of(&provider).display_name;
        let text = format!("the daemon runs no job for {display}");
        state.toast(ToastKind::Info, text, now_ms);
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

/// An action's outcome (redesign §9.3, §9.6): a command that needs the terminal is run again
/// suspended (§7.3); then, in this order:
/// 1. a suspended action's return, worded by its exit code alone;
/// 2. doctor's report, in its panel whatever the exit code;
/// 3. `P`'s pause or resume with no stderr, in the provider's name;
/// 4. nothing printed, a `Done` or `Failed` toast from the exit code;
/// 5. exit 0 with no stderr, a `Done` toast of the output's lines joined by a space, when it
///    fits the toast's three rows, else the result panel;
/// 6. exit 0 with stderr, a `Warn` toast of one line, else the panel;
/// 7. a non-zero exit, a `Failed` toast of one line, else the panel.
///
/// Every view is re-read after every outcome (§7.3 step 4).
pub fn on_done(state: &mut State, action: Action, outcome: ActionOutcome, now_ms: i64) -> Vec<Cmd> {
    let (exit, out, err) = match outcome {
        ActionOutcome::NeedsTerminal if state.quitting.is_none() => {
            state.running = Some(action.clone());
            state.on_terminal = true;
            return vec![Cmd::Suspend(action)];
        }
        ActionOutcome::NeedsTerminal => {
            state.running = None;
            return vec![];
        }
        ActionOutcome::Done { exit, out, err } => (exit, out, err),
    };
    let on_terminal = std::mem::take(&mut state.on_terminal);
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
    announce(state, &action, (exit, &out, &err), on_terminal, now_ms);
    state.reload_views(now_ms)
}

/// What an outcome shows (`on_done`'s list). `output` is the exit code, stdout and stderr.
fn announce(
    state: &mut State,
    action: &Action,
    output: (i32, &str, &str),
    on_terminal: bool,
    now_ms: i64,
) {
    let (exit, out, err) = output;
    let name = action.name();
    if on_terminal {
        // R12: the output went to the terminal; there is nothing to show but how it ended.
        let (kind, text) = match action {
            Action::Run(account) => (
                ToastKind::Info,
                format!("session on {} ended", state.short_name(account)),
            ),
            _ if exit == 0 => (ToastKind::Done, format!("{name} finished")),
            _ => (
                ToastKind::Failed,
                format!("{name} failed · its output is above in your terminal's scrollback"),
            ),
        };
        state.toast(kind, text, now_ms);
        return;
    }
    let text = [out.trim_end(), err.trim_end()]
        .into_iter()
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let quiet = err.trim().is_empty();
    match action {
        Action::Doctor { online, .. } => {
            state.overlay = Some(Overlay::Doctor {
                text,
                scroll: 0,
                online: *online,
            });
            return;
        }
        Action::Autoswitch { provider, on } if exit == 0 && quiet => {
            let display = state.names_of(provider).display_name;
            let text = if *on {
                "Auto resumed.".to_owned()
            } else {
                format!("Auto paused for {display}. P resumes.")
            };
            state.toast(ToastKind::Done, text, now_ms);
            return;
        }
        _ => {}
    }
    if text.is_empty() {
        let (kind, text) = if exit == 0 {
            (ToastKind::Done, format!("{name} finished"))
        } else {
            (ToastKind::Failed, format!("{name} failed (exit {exit})"))
        };
        state.toast(kind, text, now_ms);
        return;
    }
    let (kind, words) = match (exit == 0, quiet) {
        (true, true) => {
            let lines: Vec<&str> = out
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .collect();
            (ToastKind::Done, lines.join(" "))
        }
        (true, false) => (ToastKind::Warn, text.clone()),
        (false, _) => (ToastKind::Failed, text.clone()),
    };
    let one_line = kind == ToastKind::Done || text.lines().count() == 1;
    if one_line && crate::chrome::toast_fits(state, kind, &words) {
        state.toast(kind, words, now_ms);
    } else {
        state.overlay = Some(Overlay::Result {
            title: name.to_owned(),
            text,
            scroll: 0,
        });
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyCode;

    use super::*;
    use tagteam_engine::views::UsageStatus;

    use crate::host::AccountRef;
    use crate::msg::Msg;
    use crate::source::CollectSummary;
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
            let toast = state.toast.as_ref().unwrap();
            assert_eq!(
                (toast.kind, toast.text.as_str()),
                (
                    ToastKind::Info,
                    "still switching to 1 a1 · wait for it to finish"
                ),
                "{c}"
            );
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
        assert!(matches!(state.overlay, Some(Overlay::Help { .. })));
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
        let toast = state.toast.as_ref().unwrap();
        assert_eq!(
            (toast.kind, toast.text.as_str()),
            (ToastKind::Done, "Enabled 1 a1@x.co.")
        );
        assert!(state.running.is_none() && state.overlay.is_none());
        assert_eq!(
            draw(&state, 80, 24)[23],
            footer_row("✓ Enabled 1 a1@x.co.", 80)
        );
    }

    #[test]
    fn warnings_several_lines_or_a_failure_open_the_result_panel() {
        let words = "word ".repeat(20).trim_end().to_owned();
        let two_long = format!("{words}\n{words}\n");
        for (exit, out, err, want) in [
            // Two lines that do not fit a toast's three rows.
            (0, two_long.as_str(), "", format!("{words}\n{words}")),
            (
                0,
                "Switched.\n",
                "warning: usage was not collected\n",
                "Switched.\nwarning: usage was not collected".to_owned(),
            ),
            (
                1,
                "",
                "tagteam: the selected account changed; nothing was done\nrun list and try again\n",
                "tagteam: the selected account changed; nothing was done\nrun list and try again"
                    .to_owned(),
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
            assert_eq!((title.as_str(), text.as_str()), ("switch", want.as_str()));
            assert!(state.toast.is_none(), "{exit} {out:?} {err:?}");
        }
        // One line is a toast: here a failure.
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
        let toast = state.toast.as_ref().unwrap();
        assert_eq!(
            (toast.kind, toast.text.as_str()),
            (
                ToastKind::Failed,
                "tagteam: the selected account changed; nothing was done"
            )
        );
        assert!(state.overlay.is_none());
        // A panel shows the CLI's words and closes on Esc.
        let mut state = loaded(1);
        on_done(
            &mut state,
            Action::Switch(sel(1)),
            done(
                1,
                "",
                "tagteam: the selected account changed; nothing was done\nrun list and try again\n",
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

    /// What an outcome left on screen.
    #[derive(Debug, PartialEq)]
    enum Shown {
        Toast(ToastKind, String),
        Panel(String, String),
        Nothing,
    }

    /// `action`'s outcome arriving at a state with one account read, taken on the terminal or
    /// not.
    fn outcome_of(
        action: Action,
        on_terminal: bool,
        exit: i32,
        out: &str,
        err: &str,
    ) -> (State, Shown) {
        let mut state = loaded(1);
        state.running = Some(action.clone());
        state.on_terminal = on_terminal;
        on_done(&mut state, action, done(exit, out, err), NOW_MS);
        assert!(state.running.is_none() && !state.on_terminal);
        let shown = match (&state.toast, &state.overlay) {
            (Some(t), None) => Shown::Toast(t.kind, t.text.clone()),
            (None, Some(Overlay::Result { title, text, .. })) => {
                Shown::Panel(title.clone(), text.clone())
            }
            (None, Some(Overlay::Doctor { text, .. })) => {
                Shown::Panel("doctor".to_owned(), text.clone())
            }
            (None, None) => Shown::Nothing,
            (toast, overlay) => panic!("{:?} {overlay:?}", toast.as_ref().map(|t| &t.text)),
        };
        (state, shown)
    }

    #[test]
    fn the_outcome_table_in_its_order() {
        use ToastKind::{Done, Failed, Info, Warn};
        let toast = |kind, text: &str| Shown::Toast(kind, text.to_owned());
        let panel = |title: &str, text: &str| Shown::Panel(title.to_owned(), text.to_owned());
        let switch = || Action::Switch(sel(1));
        let doctor = || Action::Doctor {
            provider: None,
            online: false,
        };
        let auto = |on| Action::Autoswitch { provider: cc(), on };
        let set = "Set provider.claude-code.daemon.autoswitch = false in ~/.config/tagteam/config.toml.\n";
        let scrollback = "switch failed · its output is above in your terminal's scrollback";
        let cases = [
            // 2. A suspended action is worded by its exit code alone, whatever it printed.
            (
                "run",
                Action::Run(sel(1)),
                true,
                0,
                "",
                "",
                toast(Info, "session on 1 a1 ended"),
            ),
            (
                "run, killed",
                Action::Run(sel(1)),
                true,
                130,
                "",
                "",
                toast(Info, "session on 1 a1 ended"),
            ),
            (
                "hand-over ok",
                switch(),
                true,
                0,
                "",
                "",
                toast(Done, "switch finished"),
            ),
            (
                "hand-over failed",
                switch(),
                true,
                1,
                "",
                "",
                toast(Failed, scrollback),
            ),
            (
                "hand-over, output",
                switch(),
                true,
                0,
                "Enabled.\n",
                "",
                toast(Done, "switch finished"),
            ),
            // 3. Doctor's panel, whatever the exit code, even with no text.
            (
                "doctor",
                doctor(),
                false,
                0,
                "claude-code\n  ok\n",
                "",
                panel("doctor", "claude-code\n  ok"),
            ),
            (
                "doctor, failed",
                doctor(),
                false,
                1,
                "",
                "",
                panel("doctor", ""),
            ),
            // 4. Auto's pause and resume, in the provider's name.
            (
                "pause",
                auto(false),
                false,
                0,
                set,
                "",
                toast(Done, "Auto paused for Claude Code. P resumes."),
            ),
            (
                "resume",
                auto(true),
                false,
                0,
                set,
                "",
                toast(Done, "Auto resumed."),
            ),
            // With stderr it falls through to 7: a warning is never hidden.
            (
                "pause, warning",
                auto(false),
                false,
                0,
                "",
                "warning: unknown key\n",
                toast(Warn, "warning: unknown key"),
            ),
            (
                "pause, failed",
                auto(false),
                false,
                1,
                "",
                "tagteam: no config\n",
                toast(Failed, "tagteam: no config"),
            ),
            // 5. Nothing printed.
            (
                "quiet",
                switch(),
                false,
                0,
                "",
                "",
                toast(Done, "switch finished"),
            ),
            (
                "quiet, failed",
                switch(),
                false,
                1,
                "",
                "",
                toast(Failed, "switch failed (exit 1)"),
            ),
            // 6. Exit 0 with no stderr: the lines joined by a space.
            (
                "one line",
                switch(),
                false,
                0,
                "Enabled 1 a1@x.co.\n",
                "",
                toast(Done, "Enabled 1 a1@x.co."),
            ),
            (
                "two lines",
                switch(),
                false,
                0,
                "Switched to b@x.co (position 2).\nRestart Claude Code to apply.\n",
                "",
                toast(
                    Done,
                    "Switched to b@x.co (position 2). Restart Claude Code to apply.",
                ),
            ),
            // 7. Exit 0 with stderr: a Warn when it is one line.
            (
                "warning",
                switch(),
                false,
                0,
                "",
                "warning: usage was not collected\n",
                toast(Warn, "warning: usage was not collected"),
            ),
            (
                "output and warning",
                switch(),
                false,
                0,
                "Switched.\n",
                "warning: usage was not collected\n",
                panel("switch", "Switched.\nwarning: usage was not collected"),
            ),
            // 8. A non-zero exit: a Failed when it is one line.
            (
                "failure",
                switch(),
                false,
                2,
                "",
                "tagteam: no such account\n",
                toast(Failed, "tagteam: no such account"),
            ),
            (
                "failure, two lines",
                switch(),
                false,
                2,
                "",
                "tagteam: no such account\ntry list\n",
                panel("switch", "tagteam: no such account\ntry list"),
            ),
            (
                "failure with output",
                switch(),
                false,
                1,
                "Switched.\n",
                "tagteam: x\n",
                panel("switch", "Switched.\ntagteam: x"),
            ),
        ];
        for (case, action, on_terminal, exit, out, err, want) in cases {
            let (_, shown) = outcome_of(action, on_terminal, exit, out, err);
            assert_eq!(shown, want, "{case}");
        }
        // 1. NeedsTerminal asks to run again suspended, and marks the outcome to come.
        let mut state = loaded(1);
        let cmds = on_done(&mut state, switch(), ActionOutcome::NeedsTerminal, NOW_MS);
        assert_eq!(cmds, [Cmd::Suspend(switch())]);
        assert!(state.on_terminal && state.toast.is_none() && state.overlay.is_none());
    }

    #[test]
    fn a_done_outcome_too_long_for_three_rows_opens_the_panel() {
        let words = |n: usize| "word ".repeat(n).trim_end().to_owned();
        // 300 characters on one line take five rows at 80 columns.
        let long = words(60);
        let (state, shown) = outcome_of(Action::Switch(sel(1)), false, 0, &format!("{long}\n"), "");
        assert_eq!(shown, Shown::Panel("switch".to_owned(), long));
        assert!(state.toast.is_none());
        // 120 characters take two, with the pinned two on the last.
        let fits = words(24);
        let (state, shown) = outcome_of(Action::Switch(sel(1)), false, 0, &format!("{fits}\n"), "");
        assert_eq!(shown, Shown::Toast(ToastKind::Done, fits.clone()));
        let lines = draw(&state, 80, 24);
        assert!(lines[22].starts_with("✓ word word"), "{lines:?}");
        assert!(!lines[22].contains("q quit"), "{lines:?}");
        assert!(lines[23].ends_with("? keys  q quit"), "{lines:?}");
        let second = lines[23].trim_end_matches("? keys  q quit").trim_end();
        assert_eq!(format!("{} {second}", lines[22]), format!("✓ {fits}"));
        assert_eq!(
            state.toast.as_ref().unwrap().until_ms,
            NOW_MS + 8_000,
            "a toast that wraps lasts 8 s"
        );
    }

    /// R12: a suspended action's output went to the terminal, so its outcome arrives empty and
    /// is worded by its exit code alone; every view is read again (spec §7.3 step 4).
    #[test]
    fn a_suspended_action_s_outcome_toasts_from_its_exit_code() {
        let toast = |state: &State| {
            let t = state.toast.as_ref().unwrap();
            (t.kind, t.text.clone())
        };
        let reread = |cmds: &[Cmd], what: &str| {
            assert!(
                matches!(
                    cmds,
                    [Cmd::LoadAccounts { .. }, Cmd::LoadHistory { account, .. }]
                        if *account == id(1)
                ),
                "{what}: {cmds:?}"
            );
        };
        for exit in [0, 1, 130] {
            let mut state = loaded(1);
            state.history.select(id(1));
            let started = update(&mut state, ch('R'), NOW_MS);
            state.screen = Screen::History;
            assert_eq!(started, [Cmd::Suspend(Action::Run(sel(1)))]);
            assert!(state.on_terminal);
            let cmds = on_done(&mut state, Action::Run(sel(1)), done(exit, "", ""), NOW_MS);
            reread(&cmds, "run");
            assert_eq!(
                toast(&state),
                (ToastKind::Info, "session on 1 a1 ended".to_owned()),
                "{exit}"
            );
            assert!(state.overlay.is_none() && state.running.is_none(), "{exit}");
            assert!(!state.on_terminal, "{exit}");
        }
        // A hand-over for another action: `s` finds the keychain locked and asks for the
        // terminal, then runs again suspended.
        for (exit, want) in [
            (0, (ToastKind::Done, "switch finished".to_owned())),
            (
                1,
                (
                    ToastKind::Failed,
                    "switch failed · its output is above in your terminal's scrollback".to_owned(),
                ),
            ),
        ] {
            let mut state = loaded(1);
            state.history.select(id(1));
            update(&mut state, ch('s'), NOW_MS);
            state.screen = Screen::History;
            on_done(
                &mut state,
                Action::Switch(sel(1)),
                ActionOutcome::NeedsTerminal,
                NOW_MS,
            );
            let cmds = on_done(
                &mut state,
                Action::Switch(sel(1)),
                done(exit, "", ""),
                NOW_MS,
            );
            reread(&cmds, "switch");
            assert_eq!(toast(&state), want, "{exit}");
            assert!(state.running.is_none() && !state.on_terminal, "{exit}");
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
        // A first line keeps the failure from being one line, which a toast would show.
        let line = format!(
            "tagteam: it failed\n{}{}",
            " ".repeat(60),
            "word ".repeat(20).trim_end()
        );
        on_done(
            &mut state,
            Action::Switch(sel(1)),
            done(1, &line, ""),
            NOW_MS,
        );
        let drawn = draw(&state, 80, 24);
        let rows: Vec<String> = drawn[3..6]
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
                "▌ 2. claude-code #2",
                "     averyveryverylongname.with.a.long.tail@example.com",
                "     [Example Org]",
            ]
        );
        // The gutter's `▌` is on the first row only; the text is bold on all three rows, and
        // nothing is reversed.
        let (bold, reversed) = (
            ratatui::style::Modifier::BOLD,
            ratatui::style::Modifier::REVERSED,
        );
        for y in at..at + 3 {
            let y = u16::try_from(y).unwrap();
            let bar = if y as usize == at { "▌" } else { " " };
            assert_eq!(buf[(10, y)].symbol(), bar, "row {y}");
            for x in 12..20 {
                assert!(buf[(x, y)].modifier.contains(bold), "{x},{y}");
            }
            for x in 10..70 {
                assert!(!buf[(x, y)].modifier.contains(reversed), "{x},{y}");
            }
        }
        // The first choice is not selected: no bar, no bold.
        let first = u16::try_from(at - 1).unwrap();
        assert_eq!(buf[(10, first)].symbol(), " ");
        assert!(!buf[(12, first)].modifier.contains(bold));
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
        // The box's content starts at column 10. A choice's two-column gutter (`▌ ` on the
        // selected one) comes first, so `choices` drops it to read the choices' own text.
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
        let choices = |state: &State| -> Vec<String> {
            text(state)
                .iter()
                .map(|l| l.chars().skip(2).collect())
                .collect()
        };
        let shown = choices(&state);
        let first = shown
            .iter()
            .position(|l| l.starts_with("1. Claude"))
            .unwrap();
        assert!(shown[first + 2].ends_with('…'), "{shown:?}");
        assert!(
            shown[first + 3].starts_with("2. claude-code #2"),
            "{shown:?}"
        );
        assert!(
            text(&state)
                .iter()
                .any(|l| l == "↑ ↓ choose · Enter · Esc cancel")
        );
        for _ in 0..19 {
            update(&mut state, key(KeyCode::Down), NOW_MS);
        }
        let shown = choices(&state);
        assert!(
            shown.iter().any(|l| l.starts_with("20. claude-code #20")),
            "{shown:?}"
        );
        assert!(
            text(&state)
                .iter()
                .any(|l| l == "↑ ↓ choose · Enter · Esc cancel")
        );
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
                .any(|l| l.contains("R") && l.contains("run a session on it here"))
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
        assert_eq!(state.checking, Some(1), "the collection a person asked for");
        assert_eq!(
            draw(&state, 80, 24)[23],
            footer_row("⠋ checking usage…", 80),
            "the spinner says so until it is over"
        );
    }

    /// The accounts read `r` asks for, with `edit` applied to the accounts it holds.
    fn read_back(state: &mut State, n: u32, edit: impl Fn(&mut Vec<AccountView>)) {
        let mut accounts = many(n);
        edit(&mut accounts);
        let r#gen = state.gens.accounts;
        let msg = Msg::Accounts {
            r#gen,
            result: Ok(snapshot(accounts)),
        };
        update(state, msg, NOW_MS);
    }

    fn collected(r#gen: u64, summary: CollectSummary) -> Msg {
        Msg::Collected {
            r#gen,
            result: Ok(summary),
        }
    }

    fn summary(fetched: usize, updated: usize) -> CollectSummary {
        CollectSummary {
            fetched,
            warnings: vec![],
            updated,
            rate_limited: vec![],
        }
    }

    #[test]
    fn r_says_what_it_did() {
        // T2-9, redesign §9.7: rate-limited, then warnings, then updated, then already current.
        let said = |state: &State| {
            state
                .toast
                .as_ref()
                .map(|t| (t.kind, t.text.clone()))
                .unwrap()
        };
        let pressed = |n: u32, edit: &dyn Fn(&mut Vec<AccountView>)| {
            let mut accounts = many(n);
            edit(&mut accounts);
            let mut state = fresh(Screen::Accounts);
            state.accounts.apply(snapshot(accounts));
            update(&mut state, ch('r'), NOW_MS);
            state
        };
        let limited = |retry_in: Vec<i64>| {
            move |a: &mut Vec<AccountView>| {
                for (i, v) in a.iter_mut().enumerate() {
                    if let Some(s) = retry_in.get(i) {
                        v.usage = unread(UsageStatus::Unavailable, Some("http-429"), Some(*s));
                    }
                }
            }
        };

        // Rate-limited: the soonest retry of those accounts, from the accounts read after it.
        let mut state = pressed(3, &|_| {});
        update(
            &mut state,
            collected(
                1,
                CollectSummary {
                    rate_limited: vec![id(1), id(2)],
                    ..summary(2, 0)
                },
            ),
            NOW_MS,
        );
        assert!(state.checking.is_none() && state.toast.is_none(), "not yet");
        assert!(
            !draw(&state, 80, 24)[23].contains("checking usage"),
            "the spinner is over"
        );
        read_back(&mut state, 3, limited(vec![600, 240]));
        assert_eq!(
            said(&state),
            (
                ToastKind::Warn,
                "2 accounts rate-limited · retry in 4m".to_owned()
            )
        );
        assert!(state.checked.is_none(), "said once");
        let mut state = pressed(1, &|_| {});
        update(
            &mut state,
            collected(
                1,
                CollectSummary {
                    rate_limited: vec![id(1)],
                    ..summary(1, 0)
                },
            ),
            NOW_MS,
        );
        read_back(&mut state, 1, limited(vec![240]));
        assert_eq!(
            said(&state),
            (
                ToastKind::Warn,
                "1 account rate-limited · retry in 4m".to_owned()
            )
        );

        // Warnings come next, at once when nothing was fetched.
        let mut state = pressed(1, &|_| {});
        let warned = CollectSummary {
            warnings: vec!["usage for a1@x.co (position 1) was not collected: x".into()],
            ..summary(0, 0)
        };
        update(&mut state, collected(1, warned), NOW_MS);
        assert_eq!(
            said(&state),
            (
                ToastKind::Warn,
                "warning: usage for a1@x.co (position 1) was not collected: x".to_owned()
            )
        );

        // Updated accounts: said once the accounts are read back.
        let mut state = pressed(2, &|_| {});
        let cmds = update(&mut state, collected(1, summary(2, 2)), NOW_MS);
        assert!(
            matches!(cmds.as_slice(), [Cmd::LoadAccounts { .. }]),
            "{cmds:?}"
        );
        assert!(state.toast.is_none());
        read_back(&mut state, 2, |_| {});
        assert_eq!(
            said(&state),
            (ToastKind::Done, "updated 2 accounts".to_owned())
        );
        let mut state = pressed(2, &|_| {});
        update(&mut state, collected(1, summary(1, 1)), NOW_MS);
        read_back(&mut state, 2, |_| {});
        assert_eq!(
            said(&state),
            (ToastKind::Done, "updated 1 account".to_owned())
        );

        // Already current: at once, with the soonest next poll of the accounts shown.
        let next = |polls: Vec<i64>| {
            move |a: &mut Vec<AccountView>| {
                for (v, at) in a.iter_mut().zip(&polls) {
                    v.usage.next_poll_at = Some(NOW_S + at);
                }
            }
        };
        let mut state = pressed(2, &next(vec![300, 60]));
        let cmds = update(&mut state, collected(1, summary(0, 0)), NOW_MS);
        assert!(cmds.is_empty(), "nothing was fetched: {cmds:?}");
        assert_eq!(
            said(&state),
            (
                ToastKind::Info,
                "already current · next update in 1m".to_owned()
            )
        );
        let mut state = pressed(2, &|_| {});
        update(&mut state, collected(1, summary(0, 0)), NOW_MS);
        assert_eq!(
            said(&state),
            (ToastKind::Info, "already current".to_owned())
        );
        assert!(state.checking.is_none());
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
        assert_eq!(
            draw(&state, 80, 24)[23],
            footer_row("⠋ pausing auto…", 80),
            "the spinner says what runs"
        );
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
        // Spec §7.4, T2-16: in timer mode `P` says how to start a daemon, also while the first
        // `status` is still to come; and so for a provider the daemon runs no job for, which it
        // names as the person knows it.
        let said = |state: &State| {
            let toast = state.toast.as_ref().unwrap();
            (toast.kind, toast.text.clone())
        };
        let no_daemon = (
            ToastKind::Info,
            "no daemon runs · tagteam daemon install starts one".to_owned(),
        );
        let mut state = loaded(1);
        assert!(update(&mut state, ch('P'), NOW_MS).is_empty());
        assert_eq!(said(&state), no_daemon);
        assert_eq!(NO_DAEMON_RUNNING, no_daemon.1);
        assert!(state.overlay.is_none());
        update(
            &mut state,
            Msg::Live(crate::live::Live::Up { link: 1 }),
            NOW_MS,
        );
        update(&mut state, ch('P'), NOW_MS);
        assert_eq!(said(&state), no_daemon);
        let mut state = loaded(1);
        let other = daemon_status(vec![daemon_job("fake-agent", "auto", true)], vec![]);
        go_live(&mut state, other);
        assert!(update(&mut state, ch('P'), NOW_MS).is_empty());
        assert_eq!(
            said(&state),
            (
                ToastKind::Info,
                "the daemon runs no job for Claude Code".to_owned()
            )
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
    fn p_toasts_the_pause_in_the_provider_s_name() {
        // Redesign §9.4: `Auto paused for <display name>. P resumes.`, never the provider's ID
        // nor `config set`'s line, which names a file path.
        let fake = tagteam_core::ProviderId::new("fake-agent");
        let outcome = |display: &'static str, on: bool, focus_second: bool| {
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
                        display_name: display,
                        launch_command: "fakeagent",
                    },
                });
            let mut state = fresh(Screen::Accounts);
            state.accounts.apply(snap);
            let status = daemon_status(
                vec![
                    daemon_job("claude-code", "auto", !on),
                    daemon_job("fake-agent", "auto", !on),
                ],
                vec![],
            );
            go_live(&mut state, status);
            if focus_second {
                update(&mut state, ch('j'), NOW_MS);
            }
            update(&mut state, ch('P'), NOW_MS);
            let cmds = update(&mut state, ch('y'), NOW_MS);
            let [Cmd::Act(action)] = cmds.as_slice() else {
                panic!("{cmds:?}");
            };
            let provider = if focus_second { fake.clone() } else { cc() };
            assert_eq!(*action, Action::Autoswitch { provider, on });
            let set = format!(
                "Set provider.x.daemon.autoswitch = {on} in ~/.config/tagteam/config.toml.\n"
            );
            on_done(&mut state, action.clone(), done(0, &set, ""), NOW_MS);
            let toast = state.toast.as_ref().unwrap();
            (toast.kind, toast.text.clone())
        };
        assert_eq!(
            outcome("FakeAgent", false, false),
            (
                ToastKind::Done,
                "Auto paused for Claude Code. P resumes.".into()
            )
        );
        assert_eq!(
            outcome("FakeAgent", false, true),
            (
                ToastKind::Done,
                "Auto paused for FakeAgent. P resumes.".into()
            )
        );
        assert_eq!(
            outcome("FakeAgent", true, true),
            (ToastKind::Done, "Auto resumed.".into())
        );
    }

    #[test]
    fn p_while_an_action_runs_is_busy() {
        let mut state = loaded(2);
        go_live(&mut state, job_in("auto", true));
        update(&mut state, ch('s'), NOW_MS);
        assert!(update(&mut state, ch('P'), NOW_MS).is_empty());
        assert_eq!(
            state.toast.as_ref().unwrap().text,
            "still switching to 1 a1 · wait for it to finish"
        );
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
        let toast = state.toast.as_ref().unwrap();
        assert_eq!(
            (toast.kind, toast.text.as_str()),
            (ToastKind::Done, "Auto paused for Claude Code. P resumes."),
            "in the provider's name, in place of the file path the CLI prints"
        );
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
