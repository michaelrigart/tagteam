//! Actions in the TUI (spec §5.5, §7): the action keys, the dialogs and panels, the CLI's
//! prompts, and what an action's outcome shows.

use std::sync::mpsc::Sender;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use tagteam_engine::words;

use crate::host::{AccountRef, Action, ActionOutcome, Strategy};
use crate::msg::{Answer, Cmd, Prompt, PromptKind};
use crate::screens;
use crate::state::{Screen, State};
use crate::view::{bold, centered, pad, scrolled_windows, selected, width_of, wrap, wrap_rows};

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
    ("1 2 3  Tab  Shift-Tab", "screens"),
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
const HELP_TOKENS: [(&str, &str); 5] = [
    ("p", "period: day, month, session or block"),
    ("g", "group: none, account, model, or both"),
    ("[ ]", "range: 7, 30 or 90 days, or all"),
    ("Enter", "drill down: sessions, then models"),
    ("Esc  Backspace", "back up a level"),
];
const HELP_HISTORY: [(&str, &str); 3] = [
    ("[ ]", "range: 1, 7, 14 or 30 days"),
    ("r", "refresh"),
    ("J K", "scroll the windows"),
];

/// The rows a choice takes, at most (R21): a CLI's `<provider> #<n> <name> [<org>]` fits.
const CHOICE_ROWS: usize = 3;

/// The rows a dialog's question takes on a terminal `height` rows tall: a third of it at most,
/// so its buttons, choices or input always show (R21).
fn question_rows(question: &str, width: usize, height: u16) -> Vec<String> {
    wrap_rows(question, width, usize::from(height / 3))
}

/// The longest alias and position the text dialogs take.
const ALIAS_MAX: usize = 64;
const POSITION_MAX: usize = 9;

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
        's' | 'b' | 'n' | 'e' | 'a' | 'm' | 'x' | 'R' if state.screen == Screen::Accounts => {
            Some(account_key(state, c, now_ms))
        }
        _ => None,
    }
}

/// `r` (spec §7.1): the views re-read, and the shown accounts offered to the collector at once.
fn refresh(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    if busy(state, now_ms) {
        return vec![];
    }
    let mut cmds = vec![state.load_accounts()];
    if state.screen == Screen::History {
        cmds.extend(screens::history::reload(state, now_ms));
    }
    let shown = state.accounts.shown_ids();
    if !shown.is_empty() {
        cmds.push(state.collect_due(shown));
    }
    cmds
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
                Strategy::Best
            } else {
                Strategy::NextAvailable
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
                default_yes: false,
                choice: false,
                then: ConfirmThen::Run(Action::Remove(account)),
            });
            vec![]
        }
        'R' => start(state, Action::Run(account), now_ms),
        _ => vec![],
    }
}

/// A key while an overlay is open. `Esc` declines a dialog and closes a panel (spec §5.5).
pub fn handle_overlay_key(state: &mut State, key: KeyEvent, now_ms: i64) -> Vec<Cmd> {
    let Some(overlay) = state.overlay.take() else {
        return vec![];
    };
    let (kept, cmds) = overlay_key(state, overlay, key, now_ms);
    state.overlay = kept;
    cmds
}

/// The overlay after `key`, or `None` once it is closed, and what it started.
fn overlay_key(
    state: &mut State,
    overlay: Overlay,
    key: KeyEvent,
    now_ms: i64,
) -> (Option<Overlay>, Vec<Cmd>) {
    match overlay {
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
                Some(yes) => (None, confirmed(state, then, yes, now_ms)),
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
        Overlay::Choose {
            question,
            options,
            selected,
            reply,
        } => {
            let last = options.len().saturating_sub(1);
            let digit = match key.code {
                KeyCode::Char(c @ '1'..='9') => Some(c as usize - '1' as usize),
                _ => None,
            };
            let answer = match key.code {
                KeyCode::Enter if !options.is_empty() => Some(Some(selected)),
                KeyCode::Esc => Some(None),
                _ => digit.filter(|d| *d <= last).map(Some),
            };
            if let Some(answer) = answer {
                let _ = reply.send(Answer::Choose(answer));
                return (None, vec![]);
            }
            let selected = match key.code {
                KeyCode::Up | KeyCode::Char('k') => selected.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => (selected + 1).min(last),
                _ => selected,
            };
            let kept = Overlay::Choose {
                question,
                options,
                selected,
                reply,
            };
            (Some(kept), vec![])
        }
        Overlay::Text {
            prompt,
            mut input,
            then,
        } => match key.code {
            KeyCode::Esc => (None, vec![]),
            KeyCode::Enter => match submitted(&then, &input) {
                Some(action) => (None, start(state, action, now_ms)),
                None => (
                    Some(Overlay::Text {
                        prompt,
                        input,
                        then,
                    }),
                    vec![],
                ),
            },
            KeyCode::Backspace => {
                input.pop();
                (
                    Some(Overlay::Text {
                        prompt,
                        input,
                        then,
                    }),
                    vec![],
                )
            }
            KeyCode::Char(c) if !c.is_control() => {
                let fits = match then {
                    TextThen::Alias(_) => input.chars().count() < ALIAS_MAX,
                    TextThen::Move(_) => c.is_ascii_digit() && input.len() < POSITION_MAX,
                };
                if fits {
                    input.push(c);
                }
                (
                    Some(Overlay::Text {
                        prompt,
                        input,
                        then,
                    }),
                    vec![],
                )
            }
            _ => (
                Some(Overlay::Text {
                    prompt,
                    input,
                    then,
                }),
                vec![],
            ),
        },
        Overlay::Result {
            title,
            text,
            scroll,
        } => match key.code {
            KeyCode::Esc | KeyCode::Enter => (None, vec![]),
            code => {
                let scroll = scrolled(scroll, code, &text, state.size);
                (
                    Some(Overlay::Result {
                        title,
                        text,
                        scroll,
                    }),
                    vec![],
                )
            }
        },
        Overlay::Doctor {
            text,
            scroll,
            online,
        } => match key.code {
            KeyCode::Esc | KeyCode::Enter => (None, vec![]),
            KeyCode::Char('o') => {
                let provider = state.provider.clone();
                let action = Action::Doctor {
                    provider,
                    online: true,
                };
                let cmds = start(state, action, now_ms);
                let kept = Overlay::Doctor {
                    text,
                    scroll,
                    online,
                };
                (Some(kept), cmds)
            }
            code => {
                let scroll = scrolled(scroll, code, &text, state.size);
                (
                    Some(Overlay::Doctor {
                        text,
                        scroll,
                        online,
                    }),
                    vec![],
                )
            }
        },
    }
}

/// A confirm dialog's answer: sent to the prompt waiting for it, or a yes starts the action.
fn confirmed(state: &mut State, then: ConfirmThen, yes: bool, now_ms: i64) -> Vec<Cmd> {
    match then {
        ConfirmThen::Reply(reply) => {
            let _ = reply.send(Answer::Confirm(yes));
            vec![]
        }
        ConfirmThen::Run(action) if yes => start(state, action, now_ms),
        ConfirmThen::Run(_) => vec![],
    }
}

/// The action a text dialog's input asks for: an empty alias clears it (spec §7.1), and a
/// position must be a number.
fn submitted(then: &TextThen, input: &str) -> Option<Action> {
    match then {
        TextThen::Alias(account) if input.trim().is_empty() => {
            Some(Action::ClearAlias(account.clone()))
        }
        TextThen::Alias(account) => Some(Action::SetAlias {
            account: account.clone(),
            alias: input.to_owned(),
        }),
        TextThen::Move(account) => input.parse::<u32>().ok().map(|position| Action::Move {
            account: account.clone(),
            position,
        }),
    }
}

/// A panel's scroll after `code`, never past its last full page on a `size` terminal: rows,
/// not lines, as the panel wraps them. It moves from the row that shows, which a resize may
/// have brought nearer than the stored scroll.
fn scrolled(scroll: u16, code: KeyCode, text: &str, (width, height): (u16, u16)) -> u16 {
    let last = furthest(text, width, height);
    let scroll = scroll.min(last);
    let to = match code {
        KeyCode::Down | KeyCode::Char('j') => scroll.saturating_add(1),
        KeyCode::Up | KeyCode::Char('k') => scroll.saturating_sub(1),
        KeyCode::PageDown | KeyCode::Char(' ') => scroll.saturating_add(10),
        KeyCode::PageUp => scroll.saturating_sub(10),
        KeyCode::Home | KeyCode::Char('g') => 0,
        KeyCode::End | KeyCode::Char('G') => last,
        _ => scroll,
    };
    to.min(last)
}

/// A CLI prompt from the actions lane (spec §7.2) opens a dialog bound to its reply. During a
/// shutdown it is dropped, which declines it.
pub fn on_prompt(state: &mut State, prompt: Prompt) {
    if state.quitting.is_some() {
        return;
    }
    let Prompt {
        question,
        kind,
        reply,
    } = prompt;
    state.overlay = Some(match kind {
        PromptKind::Confirm { default_yes } => Overlay::Confirm {
            question,
            default_yes,
            choice: default_yes,
            then: ConfirmThen::Reply(reply),
        },
        PromptKind::Choose { options } => Overlay::Choose {
            question,
            options,
            selected: 0,
            reply,
        },
    });
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
    let mut cmds = vec![state.load_accounts()];
    if state.screen == Screen::History {
        cmds.extend(screens::history::reload(state, now_ms));
    }
    cmds
}

pub fn render_overlay(state: &State, frame: &mut Frame, now_ms: i64) {
    let _ = now_ms;
    let Some(overlay) = &state.overlay else {
        return;
    };
    let area = frame.area();
    match overlay {
        Overlay::Help => {
            let keys: &[(&str, &str)] = match state.screen {
                Screen::Accounts => &HELP_ACCOUNTS,
                Screen::Tokens => &HELP_TOKENS,
                Screen::History => &HELP_HISTORY,
            };
            let mut lines = vec![Line::from(Span::styled("Everywhere", bold()))];
            lines.extend(HELP_GLOBAL.iter().map(|(k, what)| help_line(k, what)));
            let title = match state.screen {
                Screen::Accounts => "Accounts",
                Screen::Tokens => "Tokens",
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
            let mut lines: Vec<Line<'static>> =
                question_rows(question, usize::from(w) - 4, area.height)
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
        Overlay::Choose {
            question,
            options,
            selected: at,
            ..
        } => {
            let w = 64u16.min(area.width.saturating_sub(4));
            let inner = usize::from(w) - 4;
            let asked = question_rows(question, inner, area.height);
            // The box: the question, a blank row, the choices, the keys, the border, and a
            // row's margin above and below.
            let room = usize::from(area.height)
                .saturating_sub(asked.len() + 7)
                .max(CHOICE_ROWS);
            let mut lines: Vec<Line<'static>> = asked.into_iter().map(Line::from).collect();
            lines.push(Line::default());
            let blocks: Vec<Vec<String>> = options
                .iter()
                .enumerate()
                .map(|(i, option)| choice_rows(i, option, inner))
                .collect();
            let heights: Vec<usize> = blocks.iter().map(Vec::len).collect();
            // From the first choice that keeps the selected one in view, whole.
            let mut first = *at;
            let mut used = heights.get(*at).copied().unwrap_or(0);
            while first > 0 && used + heights[first - 1] <= room {
                first -= 1;
                used += heights[first];
            }
            let shown = scrolled_windows(&heights, first, room);
            for (i, block) in blocks.into_iter().enumerate() {
                if !shown.contains(&i) {
                    continue;
                }
                for row in block {
                    let line = Line::from(row);
                    lines.push(if i == *at {
                        line.style(selected())
                    } else {
                        line
                    });
                }
            }
            lines.push(Line::from("↑ ↓ choose · Enter · Esc cancel"));
            boxed(frame, area, " Choose ", lines, w);
        }
        Overlay::Text { prompt, input, .. } => {
            let w = 64u16.min(area.width.saturating_sub(4));
            let inner = usize::from(w) - 4;
            let mut lines: Vec<Line<'static>> = question_rows(prompt, inner, area.height)
                .into_iter()
                .map(Line::from)
                .collect();
            let shown = tail(input, inner.saturating_sub(1));
            lines.push(Line::from(vec![
                Span::raw(shown),
                Span::styled(" ", selected()),
            ]));
            lines.push(Line::from("Enter · Esc cancel"));
            boxed(frame, area, " Input ", lines, w);
        }
        Overlay::Result {
            title,
            text,
            scroll,
        } => panel(
            frame,
            area,
            &format!(" {title} "),
            text,
            *scroll,
            "Esc close · ↑ ↓ scroll",
        ),
        Overlay::Doctor {
            text,
            scroll,
            online,
        } => {
            let title = if *online {
                " doctor --online "
            } else {
                " doctor "
            };
            panel(
                frame,
                area,
                title,
                text,
                *scroll,
                "o online · Esc close · ↑ ↓ scroll",
            );
        }
    }
}

/// Choice `i`'s rows, `w` columns each: `N. ` and its text, which wraps at word boundaries under
/// itself, onto `CHOICE_ROWS` at most; the rest is cut (`wrap_rows`).
fn choice_rows(i: usize, option: &str, w: usize) -> Vec<String> {
    let number = format!("{}. ", i + 1);
    let hang = width_of(&number);
    wrap_rows(option, w.saturating_sub(hang), CHOICE_ROWS)
        .into_iter()
        .enumerate()
        .map(|(row, text)| {
            let lead = if row == 0 {
                number.clone()
            } else {
                " ".repeat(hang)
            };
            pad(&format!("{lead}{text}"), w)
        })
        .collect()
}

/// The end of `s` that fits in `w` columns: what was typed last stays in view.
fn tail(s: &str, w: usize) -> String {
    let mut start = s.len();
    for (i, _) in s.char_indices().rev() {
        if width_of(&s[i..]) > w {
            break;
        }
        start = i;
    }
    s[start..].to_owned()
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
                .padding(ratatui::widgets::Padding::horizontal(1)),
        ),
        rect,
    );
}

/// The rows of text a panel shows at once on a terminal `height` rows tall: the panel leaves a
/// row above and below it, and its borders take two more.
fn panel_height(height: u16) -> usize {
    usize::from(height.saturating_sub(4)).max(1)
}

/// The furthest a panel's text can be scrolled on a `width` by `height` terminal: to the top of
/// its last full page, so the last row ends the panel and never sits alone at its top. Text
/// shorter than the panel does not scroll.
fn furthest(text: &str, width: u16, height: u16) -> u16 {
    let rows = panel_rows(text, panel_width(width)).len();
    u16::try_from(rows.saturating_sub(panel_height(height))).unwrap_or(u16::MAX)
}

/// The width of a panel's text on a terminal `width` wide: two columns of margin, a border and
/// a column of padding on either side.
fn panel_width(width: u16) -> usize {
    usize::from(width.saturating_sub(8))
}

/// `text` in rows of at most `width` columns: each line broken at word boundaries (`wrap`; a
/// word wider than a row breaks by width), its continuation rows indented as the line is. An
/// indent is a third of the width at most, so the words keep room.
fn panel_rows(text: &str, width: usize) -> Vec<String> {
    let mut rows = Vec::new();
    for line in text.lines() {
        let body = line.trim_start_matches(' ');
        let indent = " ".repeat((line.len() - body.len()).min(width / 3));
        let room = width - indent.len();
        if body.is_empty() {
            rows.push(String::new());
            continue;
        }
        rows.extend(
            wrap(body, room)
                .into_iter()
                .map(|row| format!("{indent}{row}")),
        );
    }
    rows
}

/// A scrollable panel over most of `area`, its keys on its last line. Its text wraps at the
/// panel's width (`panel_rows`), and its scroll stops at its last full page at that width and height.
fn panel(frame: &mut Frame, area: Rect, title: &str, text: &str, scroll: u16, keys: &str) {
    let scroll = scroll.min(furthest(text, area.width, area.height));
    let rect = Rect::new(
        area.x + 2,
        area.y + 1,
        area.width.saturating_sub(4),
        area.height.saturating_sub(2),
    );
    frame.render_widget(Clear, rect);
    let block = Block::bordered()
        .title(title.to_owned())
        .title_bottom(Line::from(format!(" {keys} ")))
        .padding(ratatui::widgets::Padding::horizontal(1));
    let lines: Vec<Line<'static>> = panel_rows(text, panel_width(area.width))
        .into_iter()
        .map(Line::from)
        .collect();
    frame.render_widget(Paragraph::new(lines).block(block).scroll((scroll, 0)), rect);
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use crossterm::event::KeyCode;

    use super::*;
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
                    strategy: Strategy::Best,
                },
            ),
            (
                'n',
                Action::SwitchBy {
                    provider: cc(),
                    strategy: Strategy::NextAvailable,
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
            question,
            default_yes,
            choice,
            ..
        }) = &state.overlay
        else {
            panic!("{:?}", state.overlay);
        };
        assert_eq!(
            question,
            "Remove 2 a2@x.co? Its stored login is deleted; Claude Code is not changed."
        );
        assert!(!default_yes && !choice);
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

    #[test]
    fn a_cli_prompt_opens_a_dialog_bound_to_its_reply() {
        let mut state = loaded(1);
        let (reply, answer) = mpsc::channel();
        update(
            &mut state,
            Msg::Prompt(Prompt {
                question: "Add it?".into(),
                kind: PromptKind::Confirm { default_yes: true },
                reply,
            }),
            NOW_MS,
        );
        assert!(matches!(
            state.overlay,
            Some(Overlay::Confirm { choice: true, .. })
        ));
        update(&mut state, key(KeyCode::Right), NOW_MS);
        update(&mut state, key(KeyCode::Enter), NOW_MS);
        assert_eq!(answer.try_recv(), Ok(Answer::Confirm(false)));

        let (reply, answer) = mpsc::channel();
        update(
            &mut state,
            Msg::Prompt(Prompt {
                question: "Which account?".into(),
                kind: PromptKind::Choose {
                    options: vec!["one".into(), "two".into(), "three".into()],
                },
                reply,
            }),
            NOW_MS,
        );
        let drawn = draw(&state, 80, 24);
        assert!(drawn.iter().any(|l| l.contains("1. one")), "{drawn:?}");
        update(&mut state, key(KeyCode::Down), NOW_MS);
        update(&mut state, key(KeyCode::Down), NOW_MS);
        update(&mut state, key(KeyCode::Down), NOW_MS);
        update(&mut state, key(KeyCode::Enter), NOW_MS);
        assert_eq!(answer.try_recv(), Ok(Answer::Choose(Some(2))));

        for (code, want) in [
            (KeyCode::Esc, Answer::Choose(None)),
            (KeyCode::Char('2'), Answer::Choose(Some(1))),
        ] {
            let (reply, answer) = mpsc::channel();
            actions_prompt_choose(&mut state, reply);
            update(&mut state, key(code), NOW_MS);
            assert_eq!(answer.try_recv(), Ok(want));
        }
        let (reply, answer) = mpsc::channel();
        on_prompt(
            &mut state,
            Prompt {
                question: "Sure?".into(),
                kind: PromptKind::Confirm { default_yes: true },
                reply,
            },
        );
        update(&mut state, key(KeyCode::Esc), NOW_MS);
        assert_eq!(
            answer.try_recv(),
            Ok(Answer::Confirm(false)),
            "Esc declines"
        );
    }

    fn actions_prompt_choose(state: &mut State, reply: Sender<Answer>) {
        on_prompt(
            state,
            Prompt {
                question: "Which?".into(),
                kind: PromptKind::Choose {
                    options: vec!["a".into(), "b".into()],
                },
                reply,
            },
        );
    }

    #[test]
    fn a_prompt_during_a_shutdown_is_declined_unshown() {
        let mut state = loaded(1);
        state.quitting = Some(0);
        let (reply, answer) = mpsc::channel();
        update(
            &mut state,
            Msg::Prompt(Prompt {
                question: "Add it?".into(),
                kind: PromptKind::Confirm { default_yes: true },
                reply,
            }),
            NOW_MS,
        );
        assert!(state.overlay.is_none());
        assert_eq!(answer.try_recv(), Err(mpsc::TryRecvError::Disconnected));
    }

    #[test]
    fn quitting_with_a_dialog_open_declines_it() {
        let mut state = loaded(1);
        let (reply, answer) = mpsc::channel();
        on_prompt(
            &mut state,
            Prompt {
                question: "Add it?".into(),
                kind: PromptKind::Confirm { default_yes: true },
                reply,
            },
        );
        assert_eq!(
            update(&mut state, ctrl('c'), NOW_MS),
            [Cmd::Quit { code: 0 }]
        );
        assert_eq!(answer.try_recv(), Err(mpsc::TryRecvError::Disconnected));
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
        let (reply, _answer) = mpsc::channel();
        let long =
            "claude-code #2 averyveryverylongname.with.a.long.tail@example.com [Example Org]";
        update(
            &mut state,
            Msg::Prompt(Prompt {
                question: "Which account?".into(),
                kind: PromptKind::Choose {
                    options: vec!["claude-code #1 a@x.co".into(), long.into()],
                },
                reply,
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
        let (reply, _answer) = mpsc::channel();
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
                reply,
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
        let (reply, _answer) = mpsc::channel();
        let question = format!("{}?", "Claude ".repeat(286).trim_end());
        update(
            &mut state,
            Msg::Prompt(Prompt {
                question,
                kind: PromptKind::Confirm { default_yes: false },
                reply,
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
    fn a_long_input_shows_its_end() {
        assert_eq!(tail("abcdef", 3), "def");
        assert_eq!(tail("山田太郎", 5), "太郎");
        assert_eq!(tail("ab", 5), "ab");
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
}
