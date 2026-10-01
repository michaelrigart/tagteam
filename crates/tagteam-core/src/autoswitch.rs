//! §11: auto-switch decisions. Pure: no clock, no I/O.

/// `autoswitch.strategy` (§6.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    Best,
    ConsumeFirst,
}

impl Strategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Strategy::Best => "best",
            Strategy::ConsumeFirst => "consume-first",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "best" => Some(Strategy::Best),
            "consume-first" => Some(Strategy::ConsumeFirst),
            _ => None,
        }
    }
}
