//! The reply filter (redesign spec §4.2, T2-3): keeps a colour reply detection did not finish, or
//! that came after it gave up, from reaching `update` as keys. It works on crossterm's decoded
//! key events, so there is no second decoder, and it has no clock: only a key releases a key.
//!
//! crossterm 0.29.0 surfaces `ESC ]` as Alt+`]` in one read, or as Esc then `]` when ESC ends a
//! read. Each body byte is a plain char key, BEL is Ctrl+G, and ST is Alt+`\` or Esc then `\`.
//! It keeps a whole DA1 answer to itself; a split one comes out as Esc, `[`, `?`, digits, `;`
//! and `c`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::detect::Handoff;

/// What follows `ESC ]` in the reply to the background query.
const HEAD: &[u8] = b"11;";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Open,
    /// An Esc is held.
    Esc,
    /// `ESC ]` and the first `n` bytes of `11;` are held.
    Head(usize),
    /// `ESC [` is held.
    Bracket,
    /// A DA1 answer that began after detection gave up: only digits and `;` follow.
    Late,
    /// An OSC reply is being dropped, up to its BEL or ST.
    Osc,
    /// The same, after its ESC.
    OscEsc,
    /// A DA1 answer is being dropped, up to its `c`.
    Da1,
    /// A CSI that detection cut off after `ESC [`: its next key is dropped, unless it opens a new
    /// sequence (Esc, Alt+`]`) or is the `?` of a DA1 answer.
    Cut,
}

/// Judges the input thread's keys (spec §4.2, T2-3). Calm, it only drops an unasked one-read
/// Alt+`]` reply. Wary, it holds every key that may begin a reply until a later key shows what
/// it is, and never releases one by time.
pub(crate) struct ReplyFilter {
    calm: bool,
    state: State,
    held: Vec<KeyEvent>,
}

/// A character key, with or without Shift.
fn is_char(key: &KeyEvent, c: char) -> bool {
    key.code == KeyCode::Char(c) && (key.modifiers - KeyModifiers::SHIFT).is_empty()
}

fn is_alt(key: &KeyEvent, c: char) -> bool {
    key.code == KeyCode::Char(c) && key.modifiers - KeyModifiers::SHIFT == KeyModifiers::ALT
}

fn is_ctrl(key: &KeyEvent, c: char) -> bool {
    key.code == KeyCode::Char(c) && key.modifiers == KeyModifiers::CONTROL
}

fn is_esc(key: &KeyEvent) -> bool {
    key.code == KeyCode::Esc && key.modifiers.is_empty()
}

/// BEL, or a whole ST in one key.
fn ends_osc(key: &KeyEvent) -> bool {
    is_ctrl(key, 'g') || is_alt(key, '\\')
}

impl ReplyFilter {
    /// No query was asked, or its DA1 answer was read: calm.
    pub(crate) fn calm() -> ReplyFilter {
        ReplyFilter {
            calm: true,
            state: State::Open,
            held: Vec::new(),
        }
    }

    /// Detection ended without the DA1 answer: wary, starting where `handoff` says.
    pub(crate) fn wary(handoff: Handoff) -> ReplyFilter {
        let state = match handoff {
            Handoff::None => State::Open,
            Handoff::Esc => State::Esc,
            Handoff::Csi => State::Cut,
            Handoff::Da1 => State::Da1,
            Handoff::Osc => State::Osc,
            Handoff::OscEsc => State::OscEsc,
        };
        ReplyFilter {
            calm: false,
            state,
            held: Vec::new(),
        }
    }

    /// Fail-closed: inside an OSC, or a DA1 cut off at the hand-off, until its terminator.
    pub(crate) fn holding(&self) -> bool {
        matches!(self.state, State::Osc | State::OscEsc | State::Da1)
    }

    /// Judges one key, and returns the keys to pass now, in order: released held keys first.
    /// Ctrl+C and Ctrl+Z pass at once in every state; they drop the held keys and leave the state
    /// as it is.
    pub(crate) fn key(&mut self, key: KeyEvent) -> Vec<KeyEvent> {
        if is_ctrl(&key, 'c') || is_ctrl(&key, 'z') {
            self.held.clear();
            return vec![key];
        }
        let mut pass = Vec::new();
        self.judge(key, &mut pass);
        pass
    }

    fn judge(&mut self, key: KeyEvent, pass: &mut Vec<KeyEvent>) {
        match self.state {
            State::Open => self.open(key, pass),
            State::Esc => {
                if is_char(&key, ']') {
                    self.hold(key, State::Head(0));
                } else if is_char(&key, '[') {
                    self.hold(key, State::Bracket);
                } else if is_esc(&key) {
                    pass.append(&mut self.held);
                    self.held.push(key);
                } else {
                    self.release(key, pass);
                }
            }
            State::Head(n) => {
                if HEAD.get(n).is_some_and(|&b| is_char(&key, char::from(b))) {
                    if n + 1 == HEAD.len() {
                        self.drop_to(State::Osc);
                    } else {
                        self.hold(key, State::Head(n + 1));
                    }
                } else {
                    self.release(key, pass);
                }
            }
            State::Bracket => {
                if is_char(&key, '?') {
                    self.drop_to(State::Late);
                } else {
                    self.release(key, pass);
                }
            }
            State::Late => {
                if is_char(&key, ';') || matches!(key.code, KeyCode::Char(c) if c.is_ascii_digit())
                {
                    // Dropped, and the state stays.
                } else if is_char(&key, 'c') {
                    self.calm = true;
                    self.state = State::Open;
                } else {
                    self.state = State::Open;
                    self.open(key, pass);
                }
            }
            State::Osc => {
                if ends_osc(&key) {
                    self.state = State::Open;
                } else if is_esc(&key) {
                    self.state = State::OscEsc;
                }
            }
            State::OscEsc => {
                if is_char(&key, '\\') || ends_osc(&key) {
                    self.state = State::Open;
                } else if !is_esc(&key) {
                    self.state = State::Osc;
                }
            }
            State::Da1 => {
                if is_char(&key, 'c') {
                    self.calm = true;
                    self.state = State::Open;
                }
            }
            State::Cut => {
                if is_char(&key, '?') {
                    self.state = State::Da1;
                } else if is_alt(&key, ']') || is_esc(&key) {
                    // The cut CSI ended, and this key opens a new sequence: held, not dropped.
                    self.state = State::Open;
                    self.open(key, pass);
                } else {
                    self.state = State::Open;
                }
            }
        }
    }

    /// A key in the open: Alt+`]` may begin an OSC reply, and while wary so may Esc.
    fn open(&mut self, key: KeyEvent, pass: &mut Vec<KeyEvent>) {
        if is_alt(&key, ']') {
            self.hold(key, State::Head(0));
        } else if !self.calm && is_esc(&key) {
            self.hold(key, State::Esc);
        } else {
            pass.push(key);
        }
    }

    /// Keeps `key` back until a later key shows what it is.
    fn hold(&mut self, key: KeyEvent, to: State) {
        self.held.push(key);
        self.state = to;
    }

    /// The held keys were the person's: passes them in order, then judges `key` as in the open.
    fn release(&mut self, key: KeyEvent, pass: &mut Vec<KeyEvent>) {
        pass.append(&mut self.held);
        self.state = State::Open;
        self.open(key, pass);
    }

    /// The held keys and this one were a reply: drops them all.
    fn drop_to(&mut self, to: State) {
        self.held.clear();
        self.state = to;
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::*;
    use crate::detect::Handoff;

    fn plain(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn alt(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn esc() -> KeyEvent {
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
    }

    /// `text` as plain keys.
    fn typed(text: &str) -> Vec<KeyEvent> {
        text.chars().map(plain).collect()
    }

    /// Everything `keys` let through, in order.
    fn through(
        filter: &mut ReplyFilter,
        keys: impl IntoIterator<Item = KeyEvent>,
    ) -> Vec<KeyEvent> {
        keys.into_iter().flat_map(|key| filter.key(key)).collect()
    }

    /// `3`: a key the filter must pass once it is in the open and the reply is over.
    fn passes(filter: &mut ReplyFilter) {
        assert_eq!(filter.key(plain('3')), vec![plain('3')]);
    }

    #[test]
    fn after_an_osc_hand_off_every_key_drops_until_its_terminator() {
        let ends: [Vec<KeyEvent>; 4] = [
            vec![ctrl('g')],
            vec![alt('\\')],
            vec![esc(), plain('\\')],
            vec![esc(), ctrl('g')],
        ];
        for end in ends {
            let mut filter = ReplyFilter::wary(Handoff::Osc);
            assert!(filter.holding());
            for key in [plain('j'), plain('b'), plain('2'), plain('\\'), alt('x')] {
                assert_eq!(filter.key(key), vec![], "{key:?}");
            }
            assert_eq!(filter.key(ctrl('c')), vec![ctrl('c')]);
            assert_eq!(filter.key(ctrl('z')), vec![ctrl('z')]);
            assert!(filter.holding());
            assert_eq!(through(&mut filter, end.clone()), vec![], "{end:?}");
            assert!(!filter.holding(), "{end:?}");
            passes(&mut filter);
        }
        // Esc then a key that is no terminator keeps it going.
        let mut filter = ReplyFilter::wary(Handoff::Osc);
        assert_eq!(through(&mut filter, [esc(), plain('j')]), vec![]);
        assert!(filter.holding());
        assert_eq!(
            through(&mut filter, [plain('\\')]),
            vec![],
            "a bare backslash"
        );
        assert!(filter.holding());
        // ESC handed off inside the body: `\` ends it.
        let mut filter = ReplyFilter::wary(Handoff::OscEsc);
        assert!(filter.holding());
        assert_eq!(filter.key(plain('\\')), vec![]);
        assert!(!filter.holding());
        passes(&mut filter);
    }

    #[test]
    fn after_a_da1_hand_off_keys_drop_until_its_c() {
        let mut filter = ReplyFilter::wary(Handoff::Da1);
        assert!(filter.holding());
        assert_eq!(through(&mut filter, typed("2;22")), vec![]);
        assert_eq!(through(&mut filter, [esc(), plain('j'), alt('x')]), vec![]);
        assert_eq!(filter.key(ctrl('c')), vec![ctrl('c')]);
        assert!(filter.holding());
        assert_eq!(filter.key(plain('c')), vec![]);
        assert!(!filter.holding());
        passes(&mut filter);
        assert_eq!(filter.key(esc()), vec![esc()], "calm: Esc passes at once");
    }

    #[test]
    fn after_an_esc_hand_off_the_next_key_decides() {
        let mut filter = ReplyFilter::wary(Handoff::Esc);
        assert!(!filter.holding());
        assert_eq!(through(&mut filter, [plain(']')]), vec![]);
        assert_eq!(through(&mut filter, typed("11;")), vec![]);
        assert!(filter.holding());
        assert_eq!(through(&mut filter, typed("rgb:2b3b")), vec![]);
        assert_eq!(filter.key(ctrl('g')), vec![]);
        assert!(!filter.holding());
        passes(&mut filter);

        let mut filter = ReplyFilter::wary(Handoff::Esc);
        assert_eq!(through(&mut filter, typed("[?62;22")), vec![]);
        assert_eq!(filter.key(plain('c')), vec![]);
        passes(&mut filter);

        let mut filter = ReplyFilter::wary(Handoff::Esc);
        assert_eq!(filter.key(plain('x')), vec![plain('x')]);
        assert_eq!(filter.key(plain('3')), vec![plain('3')]);
    }

    #[test]
    fn an_unasked_osc_answer_is_dropped_whole() {
        for mut filter in [ReplyFilter::calm(), ReplyFilter::wary(Handoff::None)] {
            let mut keys = vec![alt(']')];
            keys.extend(typed("11;rgb:2b"));
            keys.push(alt('\\'));
            assert_eq!(through(&mut filter, keys), vec![]);
            assert!(!filter.holding());
            passes(&mut filter);
        }
    }

    #[test]
    fn a_split_osc_answer_is_dropped_whole() {
        let mut filter = ReplyFilter::wary(Handoff::None);
        let mut keys = vec![esc(), plain(']')];
        keys.extend(typed("11;rgb:2b3b/2b3b/2b3b"));
        keys.push(ctrl('g'));
        assert_eq!(through(&mut filter, keys), vec![]);
        passes(&mut filter);
    }

    #[test]
    fn a_split_da1_answer_after_detection_is_dropped() {
        let mut filter = ReplyFilter::wary(Handoff::None);
        let mut keys = vec![esc()];
        keys.extend(typed("[?62;22c"));
        assert_eq!(through(&mut filter, keys), vec![]);
        passes(&mut filter);
        assert_eq!(
            filter.key(esc()),
            vec![esc()],
            "calm now: Esc passes at once"
        );
    }

    #[test]
    fn while_wary_a_person_s_keys_after_esc_pass_in_order() {
        let mut filter = ReplyFilter::wary(Handoff::None);
        assert_eq!(filter.key(esc()), vec![]);
        assert_eq!(filter.key(plain('j')), vec![esc(), plain('j')]);

        assert_eq!(filter.key(esc()), vec![]);
        assert_eq!(filter.key(plain(']')), vec![]);
        assert_eq!(filter.key(plain('j')), vec![esc(), plain(']'), plain('j')]);

        assert_eq!(filter.key(esc()), vec![]);
        assert_eq!(filter.key(plain('[')), vec![]);
        assert_eq!(filter.key(plain('j')), vec![esc(), plain('['), plain('j')]);

        // A second Esc passes the first and is held in its turn.
        assert_eq!(filter.key(esc()), vec![]);
        assert_eq!(filter.key(esc()), vec![esc()]);
        assert_eq!(filter.key(plain('j')), vec![esc(), plain('j')]);

        // A held `11` that is no reply.
        assert_eq!(filter.key(alt(']')), vec![]);
        assert_eq!(filter.key(plain('1')), vec![]);
        assert_eq!(
            filter.key(plain('x')),
            vec![alt(']'), plain('1'), plain('x')]
        );
    }

    #[test]
    fn while_calm_esc_and_brackets_pass_at_once() {
        let mut filter = ReplyFilter::calm();
        assert_eq!(filter.key(esc()), vec![esc()]);
        assert_eq!(filter.key(plain(']')), vec![plain(']')]);
        assert_eq!(filter.key(plain('[')), vec![plain('[')]);
        assert!(!filter.holding());
    }

    #[test]
    fn a_digit_run_after_a_late_da1_opener_ends_at_the_first_other_key() {
        let mut filter = ReplyFilter::wary(Handoff::None);
        assert_eq!(
            through(&mut filter, [esc(), plain('['), plain('?'), plain('6')]),
            vec![]
        );
        // `j` is no part of a DA1 answer: it is the person's, and judged afresh.
        assert_eq!(filter.key(plain('j')), vec![plain('j')]);
        assert_eq!(filter.key(esc()), vec![], "still wary");
    }

    #[test]
    fn a_csi_cut_off_after_its_bracket_drops_one_key() {
        let mut filter = ReplyFilter::wary(Handoff::Csi);
        assert!(!filter.holding());
        assert_eq!(through(&mut filter, typed("?62")), vec![]);
        assert_eq!(filter.key(plain('c')), vec![]);
        passes(&mut filter);
        let mut filter = ReplyFilter::wary(Handoff::Csi);
        assert_eq!(filter.key(plain('x')), vec![]);
        passes(&mut filter);
    }

    #[test]
    fn a_new_sequence_after_a_cut_csi_is_held_and_its_reply_dropped() {
        let reply = typed("11;rgb:2b3b");
        for opener in [vec![alt(']')], vec![esc(), plain(']')]] {
            let mut filter = ReplyFilter::wary(Handoff::Csi);
            assert_eq!(through(&mut filter, opener.clone()), vec![], "{opener:?}");
            assert_eq!(through(&mut filter, reply.clone()), vec![], "{opener:?}");
            assert!(filter.holding(), "{opener:?}");
            assert_eq!(filter.key(ctrl('g')), vec![], "{opener:?}");
            assert!(!filter.holding(), "{opener:?}");
            passes(&mut filter);
        }
        // An Esc that starts nothing is the person's, as in the open.
        let mut filter = ReplyFilter::wary(Handoff::Csi);
        assert_eq!(filter.key(esc()), vec![]);
        assert_eq!(filter.key(plain('j')), vec![esc(), plain('j')]);
    }

    #[test]
    fn ctrl_c_and_ctrl_z_pass_in_every_state() {
        let states = [
            ReplyFilter::calm(),
            ReplyFilter::wary(Handoff::None),
            ReplyFilter::wary(Handoff::Esc),
            ReplyFilter::wary(Handoff::Csi),
            ReplyFilter::wary(Handoff::Da1),
            ReplyFilter::wary(Handoff::Osc),
            ReplyFilter::wary(Handoff::OscEsc),
        ];
        for mut filter in states {
            for key in [ctrl('c'), ctrl('z')] {
                assert_eq!(filter.key(key), vec![key]);
            }
        }
        // Mid-way through the states a key holds, they drop the held keys and leave the state.
        let mut filter = ReplyFilter::wary(Handoff::None);
        assert_eq!(filter.key(esc()), vec![]);
        assert_eq!(filter.key(plain(']')), vec![]);
        assert_eq!(filter.key(ctrl('c')), vec![ctrl('c')]);
        assert_eq!(through(&mut filter, typed("11;")), vec![]);
        assert!(filter.holding(), "the state was left as it was");
    }
}
