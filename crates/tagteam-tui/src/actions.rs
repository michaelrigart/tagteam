//! Actions in the TUI (spec §5.5, §7): the overlays, the CLI's prompts, and what an action's
//! outcome shows. Task 9 adds the action keys, the text dialogs and the panels.

use std::sync::mpsc::Sender;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Padding, Paragraph};

use crate::host::{AccountRef, Action, ActionOutcome};
use crate::msg::{Answer, Cmd, Prompt, PromptKind};
use crate::state::{Screen, State};
use crate::view::{bold, centered, pad, selected, wrap};

#[derive(Debug)]
pub enum Overlay {
    Help,
    Confirm {
        question: String,
        default_yes: bool,
        choice: bool,
        then: ConfirmThen,
    },
    Choose {
        question: String,
        options: Vec<String>,
        selected: usize,
        reply: Sender<Answer>,
    },
    Text {
        prompt: String,
        input: String,
        then: TextThen,
    },
    Result {
        title: String,
        text: String,
        scroll: u16,
    },
    Doctor {
        text: String,
        scroll: u16,
        online: bool,
    },
}

/// What a confirm dialog's answer goes to: a waiting CLI prompt, or the TUI's own question
/// before an action (`Remove`'s).
#[derive(Debug)]
pub enum ConfirmThen {
    Reply(Sender<Answer>),
    Run(Action),
}

/// What a text dialog's input is for.
#[derive(Debug)]
pub enum TextThen {
    Alias(AccountRef),
    Move(AccountRef),
}

/// The help panel's keys (spec §5.6): everywhere, then the current screen's.
const HELP_GLOBAL: [(&str, &str); 7] = [
    ("1 3  Tab  Shift-Tab", "screens"),
    ("j k  ↑ ↓", "move"),
    ("A", "add the current login"),
    ("D", "doctor"),
    ("?", "help"),
    ("q  Ctrl-C", "quit"),
    ("Ctrl-Z", "suspend"),
];
const HELP_ACCOUNTS: [(&str, &str); 11] = [
    ("s", "switch to the selected account"),
    ("b", "switch to the best account"),
    ("n", "switch to the next available account"),
    ("e", "enable or disable"),
    ("a", "set or clear the alias"),
    ("m", "move to another position"),
    ("x", "remove"),
    ("R", "run a session on the account"),
    ("r", "refresh"),
    ("Enter", "history of the account"),
    ("J K", "scroll the detail's windows"),
];
const HELP_HISTORY: [(&str, &str); 2] = [("[ ]", "range: 1, 7, 14 or 30 days"), ("r", "refresh")];

/// The action keys; Task 9 adds them. `None`: not an action key.
pub fn handle_action_key(_state: &mut State, _key: KeyEvent, _now_ms: i64) -> Option<Vec<Cmd>> {
    None
}

/// A key while an overlay is open. `Esc` declines a dialog and closes a panel (spec §5.5).
pub fn handle_overlay_key(state: &mut State, key: KeyEvent, _now_ms: i64) -> Vec<Cmd> {
    let Some(overlay) = state.overlay.take() else {
        return vec![];
    };
    let (kept, cmds) = match overlay {
        Overlay::Help => match key.code {
            KeyCode::Char('?') | KeyCode::Esc | KeyCode::Enter => (None, vec![]),
            _ => (Some(Overlay::Help), vec![]),
        },
        Overlay::Confirm {
            question,
            default_yes,
            choice,
            then,
        } => {
            let answer = match key.code {
                KeyCode::Char('y' | 'Y') => Some(true),
                KeyCode::Char('n' | 'N') | KeyCode::Esc => Some(false),
                KeyCode::Enter => Some(choice),
                _ => None,
            };
            match answer {
                Some(yes) => (None, confirmed(then, yes)),
                None => {
                    let choice = match key.code {
                        KeyCode::Left
                        | KeyCode::Right
                        | KeyCode::Tab
                        | KeyCode::BackTab
                        | KeyCode::Char('h' | 'l') => !choice,
                        _ => choice,
                    };
                    let kept = Overlay::Confirm {
                        question,
                        default_yes,
                        choice,
                        then,
                    };
                    (Some(kept), vec![])
                }
            }
        }
        Overlay::Choose { reply, .. } if key.code == KeyCode::Esc => {
            let _ = reply.send(Answer::Choose(None));
            (None, vec![])
        }
        Overlay::Text { .. } | Overlay::Result { .. } | Overlay::Doctor { .. }
            if key.code == KeyCode::Esc =>
        {
            (None, vec![])
        }
        other => (Some(other), vec![]),
    };
    state.overlay = kept;
    cmds
}

/// A confirm dialog's answer: sent to the prompt waiting for it, or a yes runs the action.
fn confirmed(then: ConfirmThen, yes: bool) -> Vec<Cmd> {
    match then {
        ConfirmThen::Reply(reply) => {
            let _ = reply.send(Answer::Confirm(yes));
            vec![]
        }
        ConfirmThen::Run(action) if yes => vec![Cmd::Act(action)],
        ConfirmThen::Run(_) => vec![],
    }
}

/// A CLI prompt from the actions lane. Until Task 9's dialogs, every prompt is declined.
pub fn on_prompt(_state: &mut State, prompt: Prompt) {
    let answer = match prompt.kind {
        PromptKind::Confirm { .. } => Answer::Confirm(false),
        PromptKind::Choose { .. } => Answer::Choose(None),
    };
    let _ = prompt.reply.send(answer);
}

/// An action's outcome: the action is no longer running, and the first line it printed is a
/// toast. Task 9 adds the panels and the re-read.
pub fn on_done(state: &mut State, action: Action, outcome: ActionOutcome, now_ms: i64) -> Vec<Cmd> {
    state.running = None;
    let text = match outcome {
        ActionOutcome::Done { out, err, .. } => out
            .lines()
            .chain(err.lines())
            .find(|l| !l.trim().is_empty())
            .map_or_else(|| format!("{} done", action.name()), str::to_owned),
        ActionOutcome::NeedsTerminal => format!("{} needs the terminal", action.name()),
    };
    state.toast(text, now_ms);
    vec![]
}

pub fn render_overlay(state: &State, frame: &mut Frame, _now_ms: i64) {
    let Some(overlay) = &state.overlay else {
        return;
    };
    let area = frame.area();
    match overlay {
        Overlay::Help => {
            let keys: &[(&str, &str)] = match state.screen {
                Screen::Accounts => &HELP_ACCOUNTS,
                Screen::History => &HELP_HISTORY,
            };
            let mut lines = vec![Line::from(Span::styled("Everywhere", bold()))];
            lines.extend(HELP_GLOBAL.iter().map(|(k, what)| help_line(k, what)));
            let title = match state.screen {
                Screen::Accounts => "Accounts",
                Screen::History => "History",
            };
            lines.push(Line::from(Span::styled(title, bold())));
            lines.extend(keys.iter().map(|(k, what)| help_line(k, what)));
            boxed(frame, area, " Help ", lines, 64);
        }
        Overlay::Confirm {
            question, choice, ..
        } => {
            let w = 64u16.min(area.width.saturating_sub(4));
            let mut lines: Vec<Line<'static>> = wrap(question, usize::from(w) - 4)
                .into_iter()
                .map(Line::from)
                .collect();
            lines.push(Line::default());
            let button = |label: &str, on: bool| {
                let text = format!("[ {label} ]");
                if on {
                    Span::styled(text, selected())
                } else {
                    Span::raw(text)
                }
            };
            lines.push(Line::from(vec![
                button("Yes", *choice),
                Span::raw("   "),
                button("No", !*choice),
            ]));
            lines.push(Line::from("y yes · n no · Enter choose · Esc no"));
            boxed(frame, area, " Confirm ", lines, w);
        }
        Overlay::Choose { .. }
        | Overlay::Text { .. }
        | Overlay::Result { .. }
        | Overlay::Doctor { .. } => {}
    }
}

fn help_line(key: &str, what: &str) -> Line<'static> {
    Line::from(format!("  {}  {what}", pad(key, 20)))
}

/// `lines` in a bordered box `w` wide, as tall as they need, in the middle of `area`.
fn boxed(frame: &mut Frame, area: Rect, title: &str, lines: Vec<Line<'static>>, w: u16) {
    let h = u16::try_from(lines.len())
        .unwrap_or(u16::MAX)
        .saturating_add(2);
    let rect = centered(area, w.min(area.width), h);
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .title(title.to_owned())
                .padding(Padding::horizontal(1)),
        ),
        rect,
    );
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;
    use crate::msg::Msg;
    use crate::testutil::*;
    use crate::update::update;

    #[test]
    fn until_the_dialogs_exist_every_prompt_is_declined() {
        let mut state = fresh(Screen::Accounts);
        let (reply, answer) = mpsc::channel();
        let confirm = Prompt {
            question: "Add it?".into(),
            kind: PromptKind::Confirm { default_yes: true },
            reply,
        };
        update(&mut state, Msg::Prompt(confirm), NOW_MS);
        assert_eq!(answer.try_recv(), Ok(Answer::Confirm(false)));
        let (reply, answer) = mpsc::channel();
        let choose = Prompt {
            question: "Which?".into(),
            kind: PromptKind::Choose {
                options: vec!["a".into()],
            },
            reply,
        };
        update(&mut state, Msg::Prompt(choose), NOW_MS);
        assert_eq!(answer.try_recv(), Ok(Answer::Choose(None)));
        assert!(state.overlay.is_none());
    }

    #[test]
    fn an_outcome_ends_the_action_with_a_toast_of_its_first_line() {
        let mut state = fresh(Screen::Accounts);
        let action = Action::AddLive { provider: cc() };
        state.running = Some(action.clone());
        let outcome = ActionOutcome::Done {
            exit: 0,
            out: "\nAdded 1 a@x.co.\nmore\n".into(),
            err: String::new(),
        };
        let cmds = update(&mut state, Msg::ActionDone { action, outcome }, NOW_MS);
        assert!(cmds.is_empty());
        assert!(state.running.is_none());
        assert_eq!(state.toast.as_ref().unwrap().text, "Added 1 a@x.co.");
        assert_eq!(draw(&state, 80, 24)[23], "Added 1 a@x.co.");
    }

    #[test]
    fn help_lists_the_keys_and_closes() {
        let mut state = fresh(Screen::Accounts);
        update(&mut state, ch('?'), NOW_MS);
        let drawn = draw(&state, 80, 24);
        assert!(drawn.iter().any(|l| l.contains("Everywhere")), "{drawn:?}");
        assert!(
            drawn
                .iter()
                .any(|l| l.contains("run a session on the account"))
        );
        update(&mut state, ch('?'), NOW_MS);
        assert!(state.overlay.is_none());
        update(&mut state, ch('?'), NOW_MS);
        update(&mut state, key(KeyCode::Esc), NOW_MS);
        assert!(state.overlay.is_none());
    }
}
