//! Who raised an `events` row and what it records (core §6.1).

use tagteam_core::str_enum;

str_enum! {
    /// Who raised an `events` row: the command that built the engine (review refactor §8.7).
    pub enum EventSource {
        Cli => "cli",
        Auto => "auto",
        Daemon => "daemon",
    }
}

str_enum! {
    /// What an `events` row records: every kind any build has written, so older stores read back
    /// whole.
    pub enum EventKind {
        Switch => "switch",
        SwitchRecovered => "switch-recovered",
        Quarantine => "quarantine",
        Unquarantine => "unquarantine",
        Add => "add",
        Import => "import",
        Remove => "remove",
        SwitchFailed => "switch-failed",
        AllExhausted => "all-exhausted",
        AutoswitchPaused => "autoswitch-paused",
        AutoswitchResumed => "autoswitch-resumed",
    }
}

/// §5.3's auto-line kinds, in its order; `add`, `import` and `remove` are not.
pub const AUTO_LINE_KINDS: [EventKind; 8] = [
    EventKind::Switch,
    EventKind::SwitchRecovered,
    EventKind::SwitchFailed,
    EventKind::AllExhausted,
    EventKind::AutoswitchPaused,
    EventKind::AutoswitchResumed,
    EventKind::Quarantine,
    EventKind::Unquarantine,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_event_kind_and_source_keeps_its_stored_spelling() {
        for (s, v) in [
            ("cli", EventSource::Cli),
            ("auto", EventSource::Auto),
            ("daemon", EventSource::Daemon),
        ] {
            assert_eq!(v.as_str(), s);
            assert_eq!(EventSource::parse(s), Some(v));
        }
        for (s, v) in [
            ("switch", EventKind::Switch),
            ("switch-recovered", EventKind::SwitchRecovered),
            ("quarantine", EventKind::Quarantine),
            ("unquarantine", EventKind::Unquarantine),
            ("add", EventKind::Add),
            ("import", EventKind::Import),
            ("remove", EventKind::Remove),
            ("switch-failed", EventKind::SwitchFailed),
            ("all-exhausted", EventKind::AllExhausted),
            ("autoswitch-paused", EventKind::AutoswitchPaused),
            ("autoswitch-resumed", EventKind::AutoswitchResumed),
        ] {
            assert_eq!(v.as_str(), s);
            assert_eq!(EventKind::parse(s), Some(v));
        }
        for v in EventSource::ALL {
            assert_eq!(EventSource::parse(v.as_str()), Some(v));
        }
        for v in EventKind::ALL {
            assert_eq!(EventKind::parse(v.as_str()), Some(v));
        }
        for s in ["menubar", "CLI", ""] {
            assert_eq!(EventSource::parse(s), None, "{s:?}");
        }
        for s in ["Switch", "switch ", "limit"] {
            assert_eq!(EventKind::parse(s), None, "{s:?}");
        }
    }
}
