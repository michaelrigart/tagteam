//! crossterm 0.29.0's Unix input decoder, vendored (input-reader spec §3.1).
//!
//! The TUI reads fd 0 itself and decodes the bytes here, so crossterm's event reader, a second
//! poll on the terminal that loses a key typed with a resize, is never created. The reasons are
//! in [`docs/superpowers/specs/2026-10-31-tagteam-tui-input-reader-design.md`][spec] §1.3.
//!
//! # Source
//!
//! Both files are from crossterm 0.29.0 (<https://github.com/crossterm-rs/crossterm>):
//!
//! - `src/event/sys/unix/parse.rs` is [`parse`], with its tests;
//! - `src/event/source/unix/mio.rs` lines 161-229 are [`Parser`] below, and `src/event.rs` lines
//!   1470-1487 are [`InternalEvent`].
//!
//! # Edits
//!
//! None changes logic.
//!
//! - `parse.rs`: `crate::event::` is `crossterm::event::`; `super::super::super::InternalEvent`
//!   is `super::InternalEvent`; the four `#[cfg(feature = "bracketed-paste")]` lines are deleted,
//!   with the code under them kept, because tagteam's crossterm build has the feature on by
//!   default; both calls to the private `crate::terminal::sys::is_raw_mode_enabled()` are
//!   `matches!(crossterm::terminal::is_raw_mode_enabled(), Ok(true))`, the public function over
//!   the same static; lint `allow`s; and a first line naming this module.
//! - `Parser` and `InternalEvent` are `pub(crate)`, and `InternalEvent` has no `cfg(unix)`.
//! - `TTY_BUFFER_SIZE` is `mio.rs`'s constant, here for the reader's read buffer.
//!
//! # Licence
//!
//! This crate is published on its own, so crossterm's licence travels in the source.
//!
//! Copyright (c) 2019 Timon
//!
//! Permission is hereby granted, free of charge, to any person obtaining a copy
//! of this software and associated documentation files (the "Software"), to deal
//! in the Software without restriction, including without limitation the rights
//! to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
//! copies of the Software, and to permit persons to whom the Software is
//! furnished to do so, subject to the following conditions:
//!
//! The above copyright notice and this permission notice shall be included in all
//! copies or substantial portions of the Software.
//!
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//! IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//! FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//! AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//! LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//! OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
//! SOFTWARE.
//!
//! [spec]: https://github.com/michaelrigart/tagteam/blob/main/docs/superpowers/specs/2026-10-31-tagteam-tui-input-reader-design.md

use std::collections::VecDeque;

use crossterm::event::{Event, KeyboardEnhancementFlags};

mod parse;

use parse::parse_event;

/// The size of the buffer one read of the terminal fills (crossterm's `TTY_BUFFER_SIZE`).
pub(crate) const TTY_BUFFER_SIZE: usize = 1_024;

/// An internal event.
///
/// Encapsulates publicly available `Event` with additional internal
/// events that shouldn't be publicly available to the crate users.
#[derive(Debug, PartialOrd, PartialEq, Hash, Clone, Eq)]
pub(crate) enum InternalEvent {
    /// An event.
    Event(Event),
    /// A cursor position (`col`, `row`).
    CursorPosition(u16, u16),
    /// The progressive keyboard enhancement flags enabled by the terminal.
    KeyboardEnhancementFlags(KeyboardEnhancementFlags),
    /// Attributes and architectural class of the terminal.
    PrimaryDeviceAttributes,
}

//
// Following `Parser` structure exists for two reasons:
//
//  * mimic anes Parser interface
//  * move the advancing, parsing, ... stuff out of the `try_read` method
//
#[derive(Debug)]
pub(crate) struct Parser {
    buffer: Vec<u8>,
    internal_events: VecDeque<InternalEvent>,
}

impl Default for Parser {
    fn default() -> Self {
        Parser {
            // This buffer is used for -> 1 <- ANSI escape sequence. Are we
            // aware of any ANSI escape sequence that is bigger? Can we make
            // it smaller?
            //
            // Probably not worth spending more time on this as "there's a plan"
            // to use the anes crate parser.
            buffer: Vec::with_capacity(256),
            // TTY_BUFFER_SIZE is 1_024 bytes. How many ANSI escape sequences can
            // fit? What is an average sequence length? Let's guess here
            // and say that the average ANSI escape sequence length is 8 bytes. Thus
            // the buffer size should be 1024/8=128 to avoid additional allocations
            // when processing large amounts of data.
            //
            // There's no need to make it bigger, because when you look at the `try_read`
            // method implementation, all events are consumed before the next TTY_BUFFER
            // is processed -> events pushed.
            internal_events: VecDeque::with_capacity(128),
        }
    }
}

impl Parser {
    pub(crate) fn advance(&mut self, buffer: &[u8], more: bool) {
        for (idx, byte) in buffer.iter().enumerate() {
            let more = idx + 1 < buffer.len() || more;

            self.buffer.push(*byte);

            match parse_event(&self.buffer, more) {
                Ok(Some(ie)) => {
                    self.internal_events.push_back(ie);
                    self.buffer.clear();
                }
                Ok(None) => {
                    // Event can't be parsed, because we don't have enough bytes for
                    // the current sequence. Keep the buffer and process next bytes.
                }
                Err(_) => {
                    // Event can't be parsed (not enough parameters, parameter is not a number, ...).
                    // Clear the buffer and continue with another sequence.
                    self.buffer.clear();
                }
            }
        }
    }
}

impl Iterator for Parser {
    type Item = InternalEvent;

    fn next(&mut self) -> Option<Self::Item> {
        self.internal_events.pop_front()
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> InternalEvent {
        InternalEvent::Event(Event::Key(KeyEvent::new(code, modifiers)))
    }

    fn plain(c: char) -> InternalEvent {
        key(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn named(code: KeyCode) -> InternalEvent {
        key(code, KeyModifiers::NONE)
    }

    /// The keys `keys::KEYS` binds, then the other byte strings the reader meets. The expected
    /// events are written from `parse.rs`'s rules, never computed by the decoder, so a later
    /// crossterm's decoder can be compared against this table.
    #[test]
    fn the_decoder_reads_these_bytes_as_crossterm_0_29_0_does() {
        let mut table: Vec<(&[u8], Vec<InternalEvent>)> = vec![
            // Every key the table binds: digits, Tab, Shift-Tab, `?`, `q`, Esc, `w`.
            (b"1", vec![plain('1')]),
            (b"2", vec![plain('2')]),
            (b"3", vec![plain('3')]),
            (b"\t", vec![named(KeyCode::Tab)]),
            (b"\x1b[Z", vec![key(KeyCode::BackTab, KeyModifiers::SHIFT)]),
            (b"?", vec![plain('?')]),
            (b"q", vec![plain('q')]),
            (b"\x1b", vec![named(KeyCode::Esc)]),
            (b"w", vec![plain('w')]),
            // Enter, Backspace and the bracket keys.
            (b"\r", vec![named(KeyCode::Enter)]),
            (b"\x7f", vec![named(KeyCode::Backspace)]),
            (b"[", vec![plain('[')]),
            (b"]", vec![plain(']')]),
            // Arrows, Home and End, in CSI and SS3 forms.
            (b"\x1b[A", vec![named(KeyCode::Up)]),
            (b"\x1b[B", vec![named(KeyCode::Down)]),
            (b"\x1bOA", vec![named(KeyCode::Up)]),
            (b"\x1bOB", vec![named(KeyCode::Down)]),
            (b"\x1b[D", vec![named(KeyCode::Left)]),
            (b"\x1b[C", vec![named(KeyCode::Right)]),
            (b"\x1bOD", vec![named(KeyCode::Left)]),
            (b"\x1bOC", vec![named(KeyCode::Right)]),
            (b"\x1b[H", vec![named(KeyCode::Home)]),
            (b"\x1b[F", vec![named(KeyCode::End)]),
            (b"\x1bOH", vec![named(KeyCode::Home)]),
            (b"\x1bOF", vec![named(KeyCode::End)]),
            (b"\x1b[5~", vec![named(KeyCode::PageUp)]),
            (b"\x1b[6~", vec![named(KeyCode::PageDown)]),
            // Control keys.
            (
                b"\x03",
                vec![key(KeyCode::Char('c'), KeyModifiers::CONTROL)],
            ),
            (
                b"\x1a",
                vec![key(KeyCode::Char('z'), KeyModifiers::CONTROL)],
            ),
            // Alt+`]`, which is how `ESC ]` reads.
            (b"\x1b]", vec![key(KeyCode::Char(']'), KeyModifiers::ALT)]),
            // A bracketed paste.
            (
                b"\x1b[200~hi\x1b[201~",
                vec![InternalEvent::Event(Event::Paste("hi".to_string()))],
            ),
            // An OSC 11 reply is not recognised: Alt+`]`, one plain key per byte, Alt+`\`.
            (
                b"\x1b]11;rgb:2828/2c2c/3434\x1b\\",
                std::iter::once(key(KeyCode::Char(']'), KeyModifiers::ALT))
                    .chain("11;rgb:2828/2c2c/3434".chars().map(plain))
                    .chain(std::iter::once(key(KeyCode::Char('\\'), KeyModifiers::ALT)))
                    .collect(),
            ),
            // A DA1 answer, a cursor position report and the keyboard enhancement flags.
            (
                b"\x1b[?62;22c",
                vec![InternalEvent::PrimaryDeviceAttributes],
            ),
            // Row 2, column 5, both 1-based: `CursorPosition(column, row)`, 0-based.
            (b"\x1b[2;5R", vec![InternalEvent::CursorPosition(4, 1)]),
            (
                b"\x1b[?1u",
                vec![InternalEvent::KeyboardEnhancementFlags(
                    KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES,
                )],
            ),
        ];
        // Lower-case letters the table binds, and the upper-case ones, which a terminal reports
        // with Shift.
        let lower: &[(&[u8], char)] = &[
            (b"a", 'a'),
            (b"b", 'b'),
            (b"e", 'e'),
            (b"f", 'f'),
            (b"g", 'g'),
            (b"j", 'j'),
            (b"k", 'k'),
            (b"m", 'm'),
            (b"n", 'n'),
            (b"p", 'p'),
            (b"r", 'r'),
            (b"s", 's'),
            (b"v", 'v'),
            (b"x", 'x'),
            (b"z", 'z'),
        ];
        let upper: &[(&[u8], char)] = &[
            (b"A", 'A'),
            (b"D", 'D'),
            (b"J", 'J'),
            (b"K", 'K'),
            (b"P", 'P'),
            (b"R", 'R'),
        ];
        for &(bytes, c) in lower {
            table.push((bytes, vec![plain(c)]));
        }
        for &(bytes, c) in upper {
            table.push((bytes, vec![key(KeyCode::Char(c), KeyModifiers::SHIFT)]));
        }

        for (bytes, expected) in table {
            let mut parser = Parser::default();
            parser.advance(bytes, false);
            let got: Vec<InternalEvent> = parser.by_ref().collect();
            assert_eq!(got, expected, "bytes {bytes:?}");
        }
    }

    /// A character the terminal's bytes split across two reads is one key, not two errors.
    #[test]
    fn a_character_split_across_two_reads_is_one_key() {
        let mut parser = Parser::default();
        parser.advance(&[0xC3], false);
        assert_eq!(parser.next(), None);
        parser.advance(&[0xA9], false);
        assert_eq!(parser.next(), Some(plain('é')));
        assert_eq!(parser.next(), None);
    }
}
