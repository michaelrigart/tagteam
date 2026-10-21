//! The overlays (spec §5.5): the dialogs, the panels and the help over the screens. This
//! module routes a key and the drawing to the overlay's own module; what more than one of
//! them uses lives here.

mod choose;
mod confirm;
mod help;
mod panel;
mod text;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph};

use crate::host::{AccountRef, Action};
use crate::msg::{Answer, AnswerTo, Cmd};
use crate::screens::tokens::filter::{self, FilterDialog};
use crate::state::State;
use crate::theme::{Role, Theme};
use crate::view::{centered, select, tint, wrap_rows};

#[derive(Debug)]
pub enum Overlay {
    /// The key table and the marks (§9.2), scrolled `scroll` rows.
    Help { scroll: u16 },
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
            | Overlay::Help { .. }
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

/// The two columns a dialog list keeps before its text (spec §4.3): the selected choice's `▌`
/// goes in the first.
pub(crate) const GUTTER: usize = 2;

/// One choice's `rows` (its text, `width - GUTTER` columns each) behind the gutter. The selected
/// choice has `▌` on its first row, its text bold and `sel-bg` on every row.
pub(crate) fn gutter_rows(
    rows: Vec<String>,
    selected: bool,
    width: usize,
    theme: Theme,
) -> Vec<Line<'static>> {
    rows.into_iter()
        .enumerate()
        .map(|(n, text)| {
            let line = Line::from(vec![Span::raw(" ".repeat(GUTTER)), Span::raw(text)]);
            match (selected, n) {
                (false, _) => line,
                (true, 0) => select(line, Some(1), width, theme),
                (true, _) => {
                    let mut line = line;
                    line.spans[1].style = line.spans[1].style.patch(theme.strong());
                    tint(line, width, theme)
                }
            }
        })
        .collect()
}

/// What a box takes of its width around its text: a border and two columns of padding a side.
pub(crate) const BOX_COLS: usize = 6;
/// What a box takes of its height around its text: a border and a row of padding a side.
pub(crate) const BOX_ROWS: usize = 4;

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

/// A key while an overlay is open. `Esc` declines a dialog and closes a panel (spec §5.5), and
/// `q` is an `Esc` in every overlay but a text dialog, which types it (§9.1).
pub fn handle_key(state: &mut State, key: KeyEvent, now_ms: i64) -> Vec<Cmd> {
    // Out of `state`: the handlers start actions, which take `state` whole.
    let Some(mut overlay) = state.overlay.take() else {
        return vec![];
    };
    let key = match (&overlay, key.code) {
        (Overlay::Text { .. }, _) => key,
        (_, KeyCode::Char('q')) => {
            KeyEvent::new_with_kind(KeyCode::Esc, KeyModifiers::NONE, key.kind)
        }
        _ => key,
    };
    let outcome = match &mut overlay {
        Overlay::Help { scroll } => help::key(state, scroll, key),
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
    // The backdrop (§9.1): the whole screen recedes before the overlay draws.
    frame.buffer_mut().set_style(area, state.theme.dim());
    match overlay {
        Overlay::Help { scroll } => help::render(state, frame, area, *scroll),
        Overlay::Confirm {
            question,
            choice,
            then,
        } => confirm::render(state, frame, area, question, choice, then),
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
        } => panel::render_result(state, frame, title, text, scroll),
        Overlay::Doctor {
            text,
            scroll,
            online,
        } => panel::render_doctor(state, frame, text, scroll, online),
        Overlay::Filter(dialog) => filter::render(state, dialog, frame),
    }
}

/// What a box says of itself (§9.1).
pub(crate) struct BoxSpec {
    pub(crate) title: String,
    /// The keys, right-aligned in the bottom border.
    pub(crate) keys: &'static str,
    /// The title is in crit, not accent.
    pub(crate) destructive: bool,
    pub(crate) width: u16,
}

/// `lines` in a box `spec.width` wide, as tall as they need, in the middle of the screen.
pub(crate) fn boxed(frame: &mut Frame, theme: Theme, spec: BoxSpec, lines: Vec<Line<'static>>) {
    let area = frame.area();
    let h = u16::try_from(lines.len() + BOX_ROWS).unwrap_or(u16::MAX);
    let rect = centered(area, spec.width.min(area.width), h);
    framed(frame, theme, rect, &spec, lines, 0);
}

/// `lines` in `spec`'s frame filling `rect`, scrolled `scroll` rows: rounded and faint, the
/// title in accent and bold (crit when destructive), the keys right-aligned in the bottom
/// border, and `surface` under it all (§9.1).
pub(crate) fn framed(
    frame: &mut Frame,
    theme: Theme,
    rect: Rect,
    spec: &BoxSpec,
    lines: Vec<Line<'static>>,
    scroll: u16,
) {
    let role = if spec.destructive {
        Role::Crit
    } else {
        Role::Accent
    };
    let title = Span::styled(
        format!(" {} ", spec.title),
        theme.role(role).patch(theme.strong()),
    );
    let keys = Line::from(vec![
        Span::styled(format!(" {} ", spec.keys), theme.role(Role::Muted)),
        Span::styled("─", theme.role(Role::Faint)),
    ]);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(theme.role(Role::Faint))
        .style(theme.surface())
        .title(title)
        .title_bottom(keys.right_aligned())
        .padding(Padding::new(2, 2, 1, 1));
    frame.render_widget(Clear, rect);
    frame.render_widget(Paragraph::new(lines).block(block).scroll((scroll, 0)), rect);
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyCode;
    use ratatui::buffer::{Buffer, Cell};
    use ratatui::style::{Color, Modifier, Style};

    use super::*;
    use crate::state::Screen;
    use crate::testutil::*;
    use crate::update::update;
    use crate::view::areas;

    /// Whether `cell` is drawn in `style`: its foreground where the style has one, and its modifiers.
    fn shows(cell: &Cell, style: Style) -> bool {
        style.fg.is_none_or(|fg| cell.fg == fg) && cell.modifier.contains(style.add_modifier)
    }

    fn confirm(then: ConfirmThen, choice: bool) -> Overlay {
        Overlay::Confirm {
            question: "Remove 1 a1@x.co?".into(),
            choice,
            then,
        }
    }

    /// The box's corners in `buf`: the `╭` and the `╯`.
    fn corners(buf: &Buffer) -> ((u16, u16), (u16, u16)) {
        (locate(buf, "╭").unwrap(), locate(buf, "╯").unwrap())
    }

    #[test]
    fn an_overlay_dims_everything_behind_it() {
        for theme in TIERS {
            let mut state = fresh(Screen::Accounts);
            state.theme = theme;
            state.overlay = Some(Overlay::Help { scroll: 0 });
            let buf = draw_buffer(&state, 80, 24, NOW_MS);
            // A panel covers the body: everything else is the backdrop.
            let body = areas(&state, Rect::new(0, 0, 80, 24)).body;
            for y in (0..24).filter(|y| !(body.y..body.y + body.height).contains(y)) {
                for x in 0..80 {
                    assert!(
                        buf[(x, y)].modifier.contains(Modifier::DIM),
                        "{theme:?} {x},{y}"
                    );
                }
            }
            // A dialog leaves the screen around it dim, the body's rows included.
            state.overlay = Some(confirm(ConfirmThen::Reply(AnswerTo(1)), true));
            let buf = draw_buffer(&state, 80, 24, NOW_MS);
            let ((x0, y0), (x1, y1)) = corners(&buf);
            // Inside the box the screen is cleared: the question is not dimmed.
            let (qx, qy) = locate(&buf, "Remove").unwrap();
            for x in qx..qx + 6 {
                assert!(
                    !buf[(x, qy)].modifier.contains(Modifier::DIM),
                    "{theme:?} question {x}"
                );
            }
            for y in 0..24 {
                for x in (0..80).filter(|x| !(x0..=x1).contains(x) || !(y0..=y1).contains(&y)) {
                    assert!(
                        buf[(x, y)].modifier.contains(Modifier::DIM),
                        "{theme:?} {x},{y}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_dialog_has_a_rounded_faint_frame_its_title_in_accent_and_its_keys_in_the_border() {
        for theme in TIERS {
            let mut state = fresh(Screen::Accounts);
            state.theme = theme;
            state.overlay = Some(confirm(ConfirmThen::Reply(AnswerTo(1)), true));
            let buf = draw_buffer(&state, 80, 24, NOW_MS);
            let ((x0, y0), (x1, y1)) = corners(&buf);
            assert_eq!((buf[(x1, y0)].symbol(), buf[(x0, y1)].symbol()), ("╮", "╰"));
            let faint = theme.role(Role::Faint);
            for x in x0..=x1 {
                for y in [y0, y1] {
                    let cell = &buf[(x, y)];
                    // The title and the keys are the border's text, in their own styles.
                    if cell.symbol() == "─" || (x == x0 || x == x1) {
                        assert!(shows(cell, faint), "{theme:?} {x},{y}");
                    }
                }
            }
            for y in y0 + 1..y1 {
                assert_eq!((buf[(x0, y)].symbol(), buf[(x1, y)].symbol()), ("│", "│"));
                assert!(shows(&buf[(x0, y)], faint) && shows(&buf[(x1, y)], faint));
            }
            let (tx, ty) = locate(&buf, "Confirm").unwrap();
            assert_eq!(ty, y0);
            let title = theme.role(Role::Accent).patch(theme.strong());
            for x in tx..tx + 7 {
                assert!(shows(&buf[(x, ty)], title), "{theme:?} title {x}");
            }
            // Two columns and a row of padding inside the border.
            assert_eq!(locate(&buf, "Remove"), Some((x0 + 3, y0 + 2)));
            let lines = text_of(&buf);
            let keys = "y yes · n no · Enter choose · Esc no";
            assert!(
                lines[usize::from(y1)].ends_with(&format!("{keys} ─╯")),
                "{lines:?}"
            );
            let (kx, ky) = locate(&buf, keys).unwrap();
            assert_eq!(ky, y1);
            assert!(
                shows(&buf[(kx, ky)], theme.role(Role::Muted)),
                "{theme:?} keys"
            );
        }
    }

    #[test]
    fn in_truecolor_a_dialog_draws_on_its_surface() {
        let rgb = |r, g, b| Color::Rgb(r, g, b);
        for (theme, surface) in [
            (TIERS[0], None),
            (TIERS[1], None),
            (TIERS[2], Some(rgb(0x1f, 0x23, 0x29))),
            (TIERS[3], Some(rgb(0xf6, 0xf8, 0xfa))),
        ] {
            assert_eq!(theme.surface().bg, surface, "{theme:?}");
            let mut state = fresh(Screen::Accounts);
            state.theme = theme;
            state.overlay = Some(confirm(ConfirmThen::Reply(AnswerTo(1)), true));
            let buf = draw_buffer(&state, 80, 24, NOW_MS);
            let ((x0, y0), (x1, y1)) = corners(&buf);
            let want = surface.unwrap_or(Color::Reset);
            // The border, the padding rows and the question's row.
            for x in x0..=x1 {
                for y in [y0, y0 + 1, y0 + 2, y1 - 1, y1] {
                    assert_eq!(buf[(x, y)].bg, want, "{theme:?} {x},{y}");
                }
            }
            // What is around the dialog stays as it was.
            assert_ne!(buf[(x0 - 1, y0 + 2)].bg, rgb(0x1f, 0x23, 0x29));
            assert_ne!(buf[(x0 - 1, y0 + 2)].bg, rgb(0xf6, 0xf8, 0xfa));
        }
    }

    #[test]
    fn q_closes_an_overlay_before_it_quits() {
        let with = |overlay: Overlay| {
            let mut state = fresh(Screen::Accounts);
            state.overlay = Some(overlay);
            state
        };
        let closes = [
            Overlay::Help { scroll: 0 },
            Overlay::Result {
                title: "switch".into(),
                text: "it failed".into(),
                scroll: 0,
            },
            Overlay::Doctor {
                text: "✓ ok".into(),
                scroll: 0,
                online: false,
            },
            confirm(
                ConfirmThen::Run(Action::Autoswitch {
                    provider: cc(),
                    on: true,
                }),
                false,
            ),
        ];
        for overlay in closes {
            let mut state = with(overlay);
            assert!(
                update(&mut state, ch('q'), NOW_MS).is_empty(),
                "{:?}",
                state.overlay
            );
            assert!(state.overlay.is_none());
            assert_eq!(state.quitting, None);
        }
        // A prompt from the CLI is answered as `Esc` answers it.
        let mut state = with(confirm(ConfirmThen::Reply(AnswerTo(7)), true));
        assert_eq!(
            update(&mut state, ch('q'), NOW_MS),
            [Cmd::Answer {
                to: AnswerTo(7),
                answer: Answer::Confirm(false)
            }]
        );
        assert!(state.overlay.is_none());
        let mut state = with(Overlay::Choose {
            question: "Which?".into(),
            options: vec!["a".into(), "b".into()],
            selected: 1,
            reply: AnswerTo(8),
        });
        assert_eq!(
            update(&mut state, ch('q'), NOW_MS),
            [Cmd::Answer {
                to: AnswerTo(8),
                answer: Answer::Choose(None)
            }]
        );
        assert!(state.overlay.is_none());
        // A text dialog types it.
        let mut state = with(Overlay::Text {
            prompt: "Alias".into(),
            input: "a".into(),
            then: TextThen::Alias(AccountRef {
                id: id(1),
                provider: cc(),
                position: 1,
            }),
        });
        assert!(update(&mut state, ch('q'), NOW_MS).is_empty());
        assert!(matches!(&state.overlay, Some(Overlay::Text { input, .. }) if input == "aq"));
        assert_eq!(state.quitting, None);
        // With nothing open, it quits.
        let mut state = fresh(Screen::Accounts);
        assert_eq!(update(&mut state, ch('q'), NOW_MS), [Cmd::Quit { code: 0 }]);
    }

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
