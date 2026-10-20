//! Text that tagteam did not write, made safe to print for a person.

use std::borrow::Cow;

/// `s` with every control character replaced by one U+FFFD: C0 except tab (newline and ESC
/// included), DEL and C1. Borrowed when nothing changes. Everything else is kept.
pub fn printable(s: &str) -> Cow<'_, str> {
    let is_control = |c: char| c.is_control() && c != '\t';
    if !s.chars().any(is_control) {
        return Cow::Borrowed(s);
    }
    Cow::Owned(
        s.chars()
            .map(|c| if is_control(c) { '\u{fffd}' } else { c })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_text_and_tabs_come_back_borrowed() {
        for s in ["", "work@x.co", "a\tb", "é — 日本語 😀", "\u{a0}"] {
            assert!(matches!(printable(s), Cow::Borrowed(b) if b == s), "{s:?}");
        }
    }

    #[test]
    fn c0_del_and_c1_become_one_replacement_character_each() {
        assert_eq!(printable("a\u{1b}[31mb"), "a\u{fffd}[31mb");
        assert_eq!(
            printable("x\u{0}\n\r\u{7}\u{7f}y"),
            "x\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}y"
        );
        assert_eq!(printable("\u{80}\u{9b}\u{9f}"), "\u{fffd}\u{fffd}\u{fffd}"); // CSI is two bytes, one character
    }
}
