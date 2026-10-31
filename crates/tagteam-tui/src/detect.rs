//! Asking the terminal its background (redesign spec §4.2): OSC 11, then DA1 as a sentinel, read
//! byte by byte from the terminal before the input thread's first crossterm read. Detection
//! owns every byte it reads: what is not part of a reply is dropped, never replayed.

use std::os::fd::RawFd;
use std::time::{Duration, Instant};

use tagteam_provider::term::{ByteRead, Readiness, read_byte, wait_readable};

use crate::theme::Background;

/// OSC 11 then DA1: `ESC ] 11 ; ? ESC \` `ESC [ c`.
pub(crate) const QUERY: &[u8] = b"\x1b]11;?\x1b\\\x1b[c";
/// How long the terminal may send no byte before detection gives up.
pub(crate) const QUIET: Duration = Duration::from_secs(1);
/// How long detection may take in all.
pub(crate) const LIMIT: Duration = Duration::from_secs(2);
/// How long one wait on the terminal lasts (T2-2): the longest a stop waits to be seen.
const SLICE: Duration = Duration::from_millis(50);
/// Tests: an ending that is not the quiet give-up (a stop, a control byte, the DA1 answer) is
/// told from it by coming before this, whatever the scheduler does to the events that cause it.
#[cfg(test)]
pub(crate) const BEFORE_QUIET: Duration = QUIET.saturating_sub(Duration::from_millis(300));
/// The longest OSC body that can be a colour.
const BODY: usize = 64;

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;
/// The 8-bit string terminator.
const ST: u8 = 0x9c;
const CTRL_C: u8 = 0x03;
const CTRL_Z: u8 = 0x1a;

/// A key detection read that the person typed, not the terminal: it ends detection at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Control {
    Interrupt,
    Suspend,
}

/// Where a reply stood when detection stopped (spec §4.2): the filter's start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Handoff {
    /// Between replies: no byte of one is in hand.
    None,
    /// `ESC`, nothing after it.
    Esc,
    /// `ESC [`, nothing after it.
    Csi,
    /// `ESC [ ?` and perhaps parameters: a DA1 answer without its `c`.
    Da1,
    /// `ESC ]` and perhaps a body: an OSC without its end.
    Osc,
    /// An OSC body and its `ESC`.
    OscEsc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    Esc,
    /// `ESC [`: `private` once its first byte was `?`, `first` until it has had a byte.
    Csi {
        private: bool,
        first: bool,
    },
    Osc,
    /// `ESC` inside an OSC body.
    OscEsc,
}

/// The byte parser: ground, ESC, CSI (`ESC [`, its `?` and parameters), OSC (`ESC ]`, its body
/// up to 64 bytes, then ESC of a two-byte ST).
pub(crate) struct Replies {
    state: State,
    body: Vec<u8>,
    /// The OSC body in hand outgrew `BODY`: it is consumed to its end, but it is no colour.
    overlong: bool,
    background: Option<Background>,
    control: Option<Control>,
    /// The DA1 answer has ended.
    answered: bool,
}

impl Replies {
    pub(crate) fn new() -> Replies {
        Replies {
            state: State::Ground,
            body: Vec::new(),
            overlong: false,
            background: None,
            control: None,
            answered: false,
        }
    }

    /// Takes one byte. True once the DA1 answer (`ESC [ ? … c`) has ended.
    pub(crate) fn push(&mut self, byte: u8) -> bool {
        // No reply contains either, and in raw mode they are Ctrl-C and Ctrl-Z, which always
        // work (§4.2): wherever the parser is, they end the read and leave its state alone.
        match byte {
            CTRL_C => {
                self.control = Some(Control::Interrupt);
                return false;
            }
            CTRL_Z => {
                self.control = Some(Control::Suspend);
                return false;
            }
            _ => {}
        }
        match self.state {
            State::Ground => {
                if byte == ESC {
                    self.state = State::Esc;
                }
            }
            State::Esc => self.after_esc(byte),
            State::Csi { private, first } => match byte {
                // An ESC inside a CSI starts a new sequence: no reply byte may reach the keys.
                ESC => self.state = State::Esc,
                0x40..=0x7e => {
                    self.state = State::Ground;
                    let end = private && byte == b'c';
                    self.answered |= end;
                    return end;
                }
                b'?' if first => {
                    self.state = State::Csi {
                        private: true,
                        first: false,
                    };
                }
                _ => {
                    self.state = State::Csi {
                        private,
                        first: false,
                    };
                }
            },
            State::Osc => match byte {
                BEL | ST => self.end_osc(),
                ESC => self.state = State::OscEsc,
                _ if self.body.len() < BODY => self.body.push(byte),
                _ => self.overlong = true,
            },
            State::OscEsc => {
                if byte == b'\\' {
                    self.end_osc();
                } else {
                    // Not an ST: the ESC starts a new sequence, and this byte is its second.
                    self.after_esc(byte);
                }
            }
        }
        false
    }

    /// The second byte of a sequence that began with ESC.
    fn after_esc(&mut self, byte: u8) {
        self.state = match byte {
            b']' => {
                self.body.clear();
                self.overlong = false;
                State::Osc
            }
            b'[' => State::Csi {
                private: false,
                first: true,
            },
            // A typed-ahead ESC, or a second one: it starts a new sequence (§4.2), so a reply
            // that follows is still read whole.
            ESC => State::Esc,
            _ => State::Ground,
        };
    }

    fn end_osc(&mut self) {
        if !self.overlong
            && let Some(background) = background_of(&self.body)
        {
            self.background = Some(background);
        }
        self.state = State::Ground;
    }

    /// The background an OSC 11 answer gave, if one parsed.
    pub(crate) fn background(&self) -> Option<Background> {
        self.background
    }

    /// Ctrl-C (0x03) or Ctrl-Z (0x1a), if one was read: it ends detection at once.
    pub(crate) fn control(&self) -> Option<Control> {
        self.control
    }

    /// Whether the read ended at the DA1 answer: every reply asked for has then ended.
    pub(crate) fn answered(&self) -> bool {
        self.answered
    }

    /// Where the parser stands, however the read ended (the DA1 answer, the quiet or the limit,
    /// a stop, Ctrl-C or Ctrl-Z, which leave it as it was): the filter's start.
    pub(crate) fn handoff(&self) -> Handoff {
        match self.state {
            State::Ground => Handoff::None,
            State::Esc => Handoff::Esc,
            State::Csi { private: true, .. } => Handoff::Da1,
            State::Csi {
                private: false,
                first: true,
            } => Handoff::Csi,
            // A CSI that is no DA1 answer (a cursor position, say) has no `?`, and none was
            // asked for.
            State::Csi {
                private: false,
                first: false,
            } => Handoff::None,
            State::Osc => Handoff::Osc,
            State::OscEsc => Handoff::OscEsc,
        }
    }
}

/// A colour component of 1 to 4 hex digits, scaled to 0 to 1.
fn component(digits: &[u8]) -> Option<f64> {
    if digits.is_empty() || digits.len() > 4 || !digits.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    let n = u32::from_str_radix(std::str::from_utf8(digits).ok()?, 16).ok()?;
    let full = 16u32.pow(digits.len() as u32) - 1;
    Some(f64::from(n) / f64::from(full))
}

fn linear(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// `11;rgb:R/G/B` (or `rgba:R/G/B/A`), each 1–4 hex digits, either case: relative luminance
/// ≥ 0.5 is light (spec §4.2).
pub(crate) fn background_of(body: &[u8]) -> Option<Background> {
    if body.len() > BODY {
        return None;
    }
    let rest = body.strip_prefix(b"11;")?;
    let (count, colour) = if let Some(colour) = rest.strip_prefix(b"rgb:") {
        (3, colour)
    } else {
        (4, rest.strip_prefix(b"rgba:")?)
    };
    let parts: Vec<&[u8]> = colour.split(|&b| b == b'/').collect();
    if parts.len() != count {
        return None;
    }
    let mut rgb = [0.0; 3];
    for (i, part) in parts.iter().enumerate() {
        let value = component(part)?;
        if let Some(slot) = rgb.get_mut(i) {
            *slot = linear(value);
        }
    }
    let luminance = 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2];
    Some(if luminance >= 0.5 {
        Background::Light
    } else {
        Background::Dark
    })
}

/// `d` in whole milliseconds, rounded up, for `poll(2)`.
fn millis_up(d: Duration) -> i32 {
    i32::try_from(d.as_micros().div_ceil(1000)).unwrap_or(i32::MAX)
}

/// Reads `fd` byte by byte until the first of: the DA1 answer; Ctrl-C or Ctrl-Z; `QUIET` with
/// no byte; `LIMIT` after `started`; `stop()`. Waits in 50 ms slices (T2-2), never past those
/// bounds, and reads nothing past the DA1 answer.
pub(crate) fn read(fd: RawFd, started: Instant, stop: &dyn Fn() -> bool) -> Replies {
    let mut replies = Replies::new();
    let mut last_byte = started;
    loop {
        if stop() {
            break;
        }
        let now = Instant::now();
        let left = (last_byte + QUIET)
            .saturating_duration_since(now)
            .min((started + LIMIT).saturating_duration_since(now));
        if left.is_zero() {
            break;
        }
        match wait_readable(fd, millis_up(left.min(SLICE))) {
            Readiness::Ready => match read_byte(fd) {
                ByteRead::Byte(byte) => {
                    last_byte = Instant::now();
                    if replies.push(byte) || replies.control().is_some() {
                        break;
                    }
                }
                ByteRead::Interrupted => {}
                ByteRead::End | ByteRead::Failed => break,
            },
            Readiness::TimedOut | Readiness::Interrupted => {}
            Readiness::Invalid | Readiness::Failed => break,
        }
    }
    replies
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;

    use super::*;

    const DA1: &[u8] = b"\x1b[?62;22c";

    /// What `read` gave on `ours`, and how long it took.
    fn read_from(ours: &UnixStream) -> (Replies, Duration) {
        let started = Instant::now();
        let replies = read(ours.as_raw_fd(), started, &|| false);
        (replies, started.elapsed())
    }

    /// `bytes` already written by the peer, then `read`.
    fn answered(bytes: &[u8]) -> (Replies, Duration) {
        let (ours, mut theirs) = UnixStream::pair().unwrap();
        theirs.write_all(bytes).unwrap();
        read_from(&ours)
    }

    /// `script` run on the peer's end while `read` runs, with `stop`. The peer stays open until
    /// `read` has returned: a hang-up would end it too.
    fn trickled(
        stop: &dyn Fn() -> bool,
        script: impl FnOnce(&mut UnixStream) + Send + 'static,
    ) -> (Replies, Duration) {
        let (ours, theirs) = UnixStream::pair().unwrap();
        // Taken first: the writer's events come at least their delay after it.
        let started = Instant::now();
        let writer = thread::spawn(move || {
            let mut theirs = theirs;
            script(&mut theirs);
            theirs
        });
        let replies = read(ours.as_raw_fd(), started, stop);
        let took = started.elapsed();
        drop(writer.join().unwrap());
        (replies, took)
    }

    /// An ending that came from something other than `QUIET`'s give-up.
    fn assert_before_quiet(took: Duration) {
        assert!(
            took < BEFORE_QUIET,
            "{took:?} is not before {BEFORE_QUIET:?}"
        );
    }

    fn assert_within(took: Duration, low: Duration, high: Duration) {
        assert!(
            took >= low && took < high,
            "{took:?} not in {low:?}..{high:?}"
        );
    }

    #[test]
    fn an_osc_11_answer_then_da1_gives_the_background() {
        let (dark, took) = answered(b"\x1b]11;rgb:2828/2c2c/3434\x1b\\\x1b[?62;22c");
        assert_eq!(dark.background(), Some(Background::Dark));
        assert_before_quiet(took);
        let (light, took) = answered(b"\x1b]11;rgb:ffff/ffff/ffff\x07\x1b[?62;22c");
        assert_eq!(light.background(), Some(Background::Light));
        assert_before_quiet(took);
    }

    #[test]
    fn da1_alone_gives_no_background() {
        let (replies, took) = answered(DA1);
        assert_eq!(replies.background(), None);
        assert_eq!(replies.control(), None);
        assert_before_quiet(took);
    }

    #[test]
    fn a_silent_terminal_is_given_up_after_a_second() {
        let (ours, _theirs) = UnixStream::pair().unwrap();
        let (replies, took) = read_from(&ours);
        assert_eq!(replies.background(), None);
        assert_within(took, QUIET, LIMIT);
    }

    #[test]
    fn a_trickle_is_given_up_after_two_seconds() {
        let (replies, took) = trickled(&|| false, |theirs| {
            for _ in 0..8 {
                if theirs.write_all(b"j").is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(300));
            }
        });
        assert_eq!(replies.background(), None);
        assert_within(took, LIMIT, LIMIT + QUIET);
    }

    #[test]
    fn a_stop_ends_detection_within_a_slice() {
        let stopped = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stopped);
        let (ours, _theirs) = UnixStream::pair().unwrap();
        let started = Instant::now();
        let raise = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            flag.store(true, Ordering::SeqCst);
        });
        let stop = || stopped.load(Ordering::SeqCst);
        read(ours.as_raw_fd(), started, &stop);
        let took = started.elapsed();
        raise.join().unwrap();
        assert_within(took, Duration::from_millis(100), BEFORE_QUIET);
    }

    #[test]
    fn ctrl_c_or_ctrl_z_ends_detection_at_once_and_is_kept() {
        for (typed, control) in [(b"\x03", Control::Interrupt), (b"\x1a", Control::Suspend)] {
            for before in [&b""[..], b"\x1b]11;rgb:28"] {
                let (replies, took) = trickled(&|| false, move |theirs| {
                    theirs.write_all(before).unwrap();
                    thread::sleep(Duration::from_millis(100));
                    theirs.write_all(typed).unwrap();
                });
                assert_eq!(replies.control(), Some(control), "after {before:?}");
                assert_eq!(replies.background(), None);
                assert_within(took, Duration::from_millis(100), BEFORE_QUIET);
            }
        }
    }

    #[test]
    fn a_key_before_and_between_the_replies_is_dropped_and_the_replies_still_read() {
        let (replies, _) = answered(b"j\x1b]11;rgb:2828/2c2c/3434\x1b\\k\x1b[?62;22c");
        assert_eq!(replies.background(), Some(Background::Dark));
        assert_eq!(replies.control(), None);
    }

    #[test]
    fn a_reply_split_across_writes_is_read_whole() {
        let (replies, took) = trickled(&|| false, |theirs| {
            theirs.write_all(b"\x1b]11;rgb:ffff/ff").unwrap();
            thread::sleep(Duration::from_millis(50));
            theirs.write_all(b"ff/ffff\x1b\\").unwrap();
            theirs.write_all(DA1).unwrap();
        });
        assert_eq!(replies.background(), Some(Background::Light));
        assert_before_quiet(took);
    }

    #[test]
    fn nothing_past_the_da1_answer_is_read() {
        let (ours, mut theirs) = UnixStream::pair().unwrap();
        theirs.write_all(&[DA1, b"q"].concat()).unwrap();
        read_from(&ours);
        let fd = ours.as_raw_fd();
        assert_eq!(wait_readable(fd, 100), Readiness::Ready);
        assert_eq!(read_byte(fd), ByteRead::Byte(b'q'));
    }

    #[test]
    fn odd_colour_replies_parse_or_leave_tier_1() {
        let long = format!("11;{}", "a".repeat(200));
        for (body, expected) in [
            (&b"11;rgb:28/2c/34"[..], Some(Background::Dark)),
            (b"11;rgb:FFFF/FFFF/FFFF", Some(Background::Light)),
            (b"11;rgba:ffff/ffff/ffff/ffff", Some(Background::Light)),
            (b"11;rgb:f/f/f", Some(Background::Light)),
            (b"11;rgb:zz/zz/zz", None),
            (b"11;rgb:1/2", None),
            (b"11;rgb:1/2/3/4", None),
            (b"11;rgb:12345/1/1", None),
            (b"11;rgb:+1/+1/+1", None),
            (b"11;?", None),
            (b"10;rgb:ffff/ffff/ffff", None),
            (long.as_bytes(), None),
        ] {
            assert_eq!(
                background_of(body),
                expected,
                "{}",
                String::from_utf8_lossy(body)
            );
            // Through the parser, each way an OSC ends, then DA1 ends the read.
            for end in [&b"\x07"[..], b"\x1b\\", b"\x9c"] {
                let bytes = [b"\x1b]".as_slice(), body, end, DA1].concat();
                let (replies, took) = answered(&bytes);
                assert_eq!(
                    replies.background(),
                    expected,
                    "{} then {end:?}",
                    String::from_utf8_lossy(body)
                );
                assert_before_quiet(took);
            }
        }
    }

    #[test]
    fn an_esc_inside_an_osc_body_starts_a_new_sequence() {
        let (replies, _) = answered(b"\x1b]11;rgb:00\x1b]11;rgb:ffff/ffff/ffff\x07\x1b[?62c");
        assert_eq!(replies.background(), Some(Background::Light));
        // ESC and a byte that is no ST end the body without a colour.
        let (replies, _) = answered(b"\x1b]11;rgb:ffff/ffff/ffff\x1bx\x1b[?62c");
        assert_eq!(replies.background(), None);
    }

    #[test]
    fn a_csi_other_than_the_da1_answer_does_not_end_detection() {
        let mut replies = Replies::new();
        for byte in b"\x1b[1;1R\x1b[?1u\x1b[c" {
            assert!(!replies.push(*byte), "{byte:#x}");
        }
        for byte in &b"\x1b[?62;22c"[..8] {
            assert!(!replies.push(*byte));
        }
        assert!(replies.push(b'c'));
    }

    #[test]
    fn a_typed_ahead_esc_does_not_hide_the_replies_that_follow() {
        for typed in [&b"\x1b"[..], b"\x1b[", b"\x1b\x1b"] {
            let bytes = [typed, b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\\x1b[?62;22c"].concat();
            let (replies, took) = answered(&bytes);
            assert!(replies.answered(), "{typed:?}");
            assert_eq!(replies.background(), Some(Background::Light), "{typed:?}");
            assert_eq!(replies.handoff(), Handoff::None, "{typed:?}");
            assert_before_quiet(took);
        }
    }

    /// Bytes a read can end after, and where the parser then stands.
    const CUTS: [(&[u8], Handoff); 11] = [
        (b"", Handoff::None),
        (b"j\x1b]11;rgb:ffff/ffff/ffff\x07k", Handoff::None),
        (b"\x1b[1;1", Handoff::None),
        (b"j\x1b", Handoff::Esc),
        (b"\x1b[", Handoff::Csi),
        (b"\x1b[?62;2", Handoff::Da1),
        (b"\x1b]11;rgb:28", Handoff::Osc),
        (b"\x1b]11;rgb:28\x1b", Handoff::OscEsc),
        (b"\x1b\x1b]11;rgb:28", Handoff::Osc),
        (b"\x1b[\x1b]11;rgb:28", Handoff::Osc),
        (b"\x1b\x1b[?62;2", Handoff::Da1),
    ];

    #[test]
    fn the_hand_off_is_where_the_parser_stands() {
        for (bytes, handoff) in CUTS {
            let mut replies = Replies::new();
            for byte in bytes {
                assert!(!replies.push(*byte));
            }
            assert_eq!(replies.handoff(), handoff, "{bytes:?}");
            assert!(!replies.answered(), "{bytes:?}");
        }
    }

    #[test]
    fn the_hand_off_survives_a_control_byte() {
        for (bytes, handoff) in CUTS {
            for (typed, control) in [(CTRL_C, Control::Interrupt), (CTRL_Z, Control::Suspend)] {
                let (replies, _) = answered(&[bytes, &[typed][..]].concat());
                assert_eq!(replies.control(), Some(control), "{bytes:?}");
                assert_eq!(replies.handoff(), handoff, "{bytes:?} then {typed:#x}");
                assert!(!replies.answered());
            }
        }
    }

    #[test]
    fn the_hand_off_after_a_quiet_second_is_where_the_parser_stood() {
        thread::scope(|scope| {
            let reads: Vec<_> = CUTS
                .iter()
                .map(|(bytes, handoff)| {
                    scope.spawn(move || {
                        let (ours, mut theirs) = UnixStream::pair().unwrap();
                        theirs.write_all(bytes).unwrap();
                        let (replies, took) = read_from(&ours);
                        assert_within(took, QUIET, LIMIT);
                        assert_eq!(replies.handoff(), *handoff, "{bytes:?}");
                        assert!(!replies.answered(), "{bytes:?}");
                    })
                })
                .collect();
            for read in reads {
                read.join().unwrap();
            }
        });
    }

    #[test]
    fn the_hand_off_after_the_limit_is_where_the_parser_stood() {
        // An OSC body that never ends, a byte every 300 ms: the limit ends the read, not the
        // quiet second.
        let (replies, took) = trickled(&|| false, |theirs| {
            if theirs.write_all(b"\x1b]11;").is_err() {
                return;
            }
            for _ in 0..8 {
                thread::sleep(Duration::from_millis(300));
                if theirs.write_all(b"0").is_err() {
                    break;
                }
            }
        });
        assert_within(took, LIMIT, LIMIT + QUIET);
        assert_eq!(replies.handoff(), Handoff::Osc);
        assert!(!replies.answered());
    }

    #[test]
    fn the_hand_off_after_a_stop_is_where_the_parser_stood() {
        for (bytes, handoff) in CUTS {
            let stopped = Arc::new(AtomicBool::new(false));
            let flag = Arc::clone(&stopped);
            let stop = move || stopped.load(Ordering::SeqCst);
            let (replies, took) = trickled(&stop, move |theirs| {
                theirs.write_all(bytes).unwrap();
                thread::sleep(Duration::from_millis(100));
                flag.store(true, Ordering::SeqCst);
            });
            assert_within(took, Duration::from_millis(100), BEFORE_QUIET);
            assert_eq!(replies.handoff(), handoff, "{bytes:?}");
            assert!(!replies.answered(), "{bytes:?}");
        }
    }

    #[test]
    fn only_the_da1_answer_is_answered() {
        for bytes in [
            &b"\x1b[?62;22c"[..],
            b"\x1b]11;rgb:28\x1b\\\x1b[?c",
            b"j\x1b[?1u\x1b[?6c",
        ] {
            let (replies, took) = answered(bytes);
            assert!(replies.answered(), "{bytes:?}");
            assert_eq!(replies.handoff(), Handoff::None, "{bytes:?}");
            assert_before_quiet(took);
        }
        for bytes in [&b"\x1b[1;1R"[..], b"\x1b[?1u", b"\x1b]11;?\x07"] {
            let mut replies = Replies::new();
            for byte in bytes {
                replies.push(*byte);
            }
            assert!(!replies.answered(), "{bytes:?}");
        }
    }
}
