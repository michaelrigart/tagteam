//! The overlays (spec §5.5): the dialogs, the panels and the help over the screens. This
//! module routes a key and the drawing to the overlay's own module; what more than one of
//! them uses lives here.

mod choose;
mod confirm;
mod help;
mod panel;
mod text;

use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, Paragraph};

use crate::host::{AccountRef, Action};
use crate::msg::{Answer, AnswerTo, Cmd};
use crate::screens::tokens::filter::{self, FilterDialog};
use crate::state::State;
use crate::view::{centered, wrap_rows};

#[derive(Debug)]
pub enum Overlay {
    Help,
    Confirm {
        question: String,
        choice: bool,
        then: ConfirmThen,
    },
    Choose {
        question: String,
        options: Vec<String>,
        selected: usize,
        reply: AnswerTo,
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
    /// Tokens' filter (spec §5.3).
    Filter(FilterDialog),
}

impl Overlay {
    /// The answer that declines the CLI prompt this overlay shows, when it shows one. Every kind
    /// is named, so that a new one is classified before it compiles.
    pub(crate) fn decline(&self) -> Option<Cmd> {
        match self {
            Overlay::Confirm {
                then: ConfirmThen::Reply(to),
                ..
            } => Some(Cmd::Answer {
                to: *to,
                answer: Answer::CONFIRM_DECLINED,
            }),
            Overlay::Choose { reply, .. } => Some(Cmd::Answer {
                to: *reply,
                answer: Answer::CHOOSE_DECLINED,
            }),
            Overlay::Confirm {
                then: ConfirmThen::Run(_),
                ..
            }
            | Overlay::Help
            | Overlay::Text { .. }
            | Overlay::Result { .. }
            | Overlay::Doctor { .. }
            | Overlay::Filter(_) => None,
        }
    }
}

/// What a confirm dialog's answer goes to: a waiting CLI prompt, or the TUI's own question
/// before an action (`Remove`'s).
#[derive(Debug)]
pub enum ConfirmThen {
    Reply(AnswerTo),
    Run(Action),
}

/// What a text dialog's input is for.
#[derive(Debug)]
pub enum TextThen {
    Alias(AccountRef),
    Move(AccountRef),
}

/// The rows a choice takes, at most (R21): a CLI's `<provider> #<n> <name> [<org>]` fits.
pub(crate) const CHOICE_ROWS: usize = 3;

/// The rows a dialog's question takes on a terminal `height` rows tall: a third of it at most,
/// so its buttons, choices or input always show (R21).
fn question_rows(question: &str, width: usize, height: u16) -> Vec<String> {
    wrap_rows(question, width, usize::from(height / 3))
}

/// What a key did to the overlay it went to.
pub(crate) enum KeyOutcome {
    /// Nothing more than what it changed in place: the overlay stays open.
    Keep,
    /// The overlay stays open and the key started `Cmd`s.
    KeepWith(Vec<Cmd>),
    /// The overlay is closed.
    Close,
    /// The overlay is closed and the key started `Cmd`s.
    CloseWith(Vec<Cmd>),
}

/// A key while an overlay is open. `Esc` declines a dialog and closes a panel (spec §5.5).
pub fn handle_key(state: &mut State, key: KeyEvent, now_ms: i64) -> Vec<Cmd> {
    // Out of `state`: the handlers start actions, which take `state` whole.
    let Some(mut overlay) = state.overlay.take() else {
        return vec![];
    };
    let outcome = match &mut overlay {
        Overlay::Help => help::key(key),
        Overlay::Confirm { choice, then, .. } => confirm::key(state, choice, then, key, now_ms),
        Overlay::Choose {
            options,
            selected,
            reply,
            ..
        } => choose::key(options, selected, *reply, key),
        Overlay::Text { input, then, .. } => text::key(state, input, then, key, now_ms),
        Overlay::Result { text, scroll, .. } => panel::result_key(state, text, scroll, key),
        Overlay::Doctor { text, scroll, .. } => panel::doctor_key(state, text, scroll, key, now_ms),
        Overlay::Filter(dialog) => filter::handle_key(state, dialog, key, now_ms),
    };
    match outcome {
        KeyOutcome::Keep => {
            state.overlay = Some(overlay);
            vec![]
        }
        KeyOutcome::KeepWith(cmds) => {
            state.overlay = Some(overlay);
            cmds
        }
        KeyOutcome::Close => {
            state.overlay = None;
            vec![]
        }
        KeyOutcome::CloseWith(cmds) => {
            state.overlay = None;
            cmds
        }
    }
}

pub fn render(state: &State, frame: &mut Frame) {
    let Some(overlay) = &state.overlay else {
        return;
    };
    let area = frame.area();
    match overlay {
        Overlay::Help => help::render(state, frame, area),
        Overlay::Confirm {
            question, choice, ..
        } => confirm::render(state, frame, area, question, choice),
        Overlay::Choose {
            question,
            options,
            selected: at,
            ..
        } => choose::render(state, frame, area, question, options, at),
        Overlay::Text { prompt, input, .. } => text::render(state, frame, area, prompt, input),
        Overlay::Result {
            title,
            text,
            scroll,
        } => panel::render_result(frame, area, title, text, scroll),
        Overlay::Doctor {
            text,
            scroll,
            online,
        } => panel::render_doctor(frame, area, text, scroll, online),
        Overlay::Filter(dialog) => filter::render(state, dialog, frame),
    }
}

/// `lines` in a bordered box `w` wide, as tall as they need, in the middle of `area`.
pub(crate) fn boxed(frame: &mut Frame, area: Rect, title: &str, lines: Vec<Line<'static>>, w: u16) {
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

#[cfg(test)]
mod tests {
    use crossterm::event::KeyCode;

    use super::*;
    use crate::state::Screen;
    use crate::testutil::*;

    #[test]
    fn a_kept_overlay_is_the_one_its_key_changed() {
        let mut state = fresh(Screen::Accounts);
        state.overlay = Some(Overlay::Text {
            prompt: "Alias".to_owned(),
            input: "ab".to_owned(),
            then: TextThen::Alias(AccountRef {
                id: id(1),
                provider: cc(),
                position: 1,
            }),
        });
        assert!(handle_key(&mut state, press(KeyCode::Char('c')), NOW_MS).is_empty());
        assert!(matches!(
            &state.overlay,
            Some(Overlay::Text { input, .. }) if input == "abc"
        ));
        assert!(handle_key(&mut state, press(KeyCode::Esc), NOW_MS).is_empty());
        assert!(state.overlay.is_none());
    }
}
