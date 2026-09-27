use std::fmt;

pub const CLAUDE_CODE: &str = "claude-code";

macro_rules! string_id {
    ($name:ident, $ctor:ident) => {
        #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(String);

        impl $name {
            pub fn $ctor(s: impl Into<String>) -> Self {
                Self(s.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

string_id!(ProviderId, new);
string_id!(AccountId, from_string);
string_id!(IdentityKey, new);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_their_strings() {
        assert_eq!(ProviderId::new(CLAUDE_CODE).as_str(), "claude-code");
        assert_eq!(AccountId::from_string("0192").to_string(), "0192");
        assert_eq!(IdentityKey::new("a@b.co\n").as_str(), "a@b.co\n");
    }

    #[test]
    fn account_ids_order_as_strings() {
        let mut v = vec![AccountId::from_string("b"), AccountId::from_string("a")];
        v.sort();
        assert_eq!(
            v,
            vec![AccountId::from_string("a"), AccountId::from_string("b")]
        );
    }
}
