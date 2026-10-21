//! A closed vocabulary of stored spellings: the enum, its variants in declaration order, and the
//! one place each spelling is written (review refactor §8.9).

/// Declares a fieldless enum whose variants each have one stored spelling, with
/// `ALL` (declaration order), `as_str` and `parse`. `parse` is `Some` only for a spelling
/// `as_str` writes. The enum derives `Debug, Clone, Copy, PartialEq, Eq` and no `Display`.
#[macro_export]
macro_rules! str_enum {
    (
        $(#[$m:meta])*
        $vis:vis enum $name:ident {
            $($(#[$vm:meta])* $variant:ident => $spelling:literal),+ $(,)?
        }
    ) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        $vis enum $name {
            $($(#[$vm])* $variant),+
        }

        impl $name {
            /// Every variant, in declaration order.
            pub const ALL: [Self; [$(Self::$variant),+].len()] = [$(Self::$variant),+];

            /// The stored spelling.
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $spelling),+
                }
            }

            /// The variant a stored spelling names; `None` for any other text.
            pub fn parse(s: &str) -> Option<Self> {
                match s {
                    $($spelling => Some(Self::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

#[cfg(test)]
mod tests {
    crate::str_enum! {
        /// A probe enum, to pin what the macro writes.
        pub enum Probe {
            /// The first.
            One => "one",
            TwoWords => "two-words",
        }
    }

    #[test]
    fn a_str_enum_spells_each_variant_and_parses_only_those_spellings() {
        assert_eq!(Probe::ALL, [Probe::One, Probe::TwoWords]);
        assert_eq!(Probe::One.as_str(), "one");
        assert_eq!(Probe::TwoWords.as_str(), "two-words");
        assert_eq!(Probe::parse("one"), Some(Probe::One));
        assert_eq!(Probe::parse("two-words"), Some(Probe::TwoWords));
        for v in Probe::ALL {
            assert_eq!(Probe::parse(v.as_str()), Some(v));
        }
        for s in ["One", "two_words", " one", ""] {
            assert_eq!(Probe::parse(s), None, "{s:?}");
        }
    }
}
