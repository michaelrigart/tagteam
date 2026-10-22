//! The events lane's marks (T4-13, §7.4's table): each event's glyph, its role, and the rank
//! that decides which of a column's events the lane draws.

use tagteam_engine::timeline::EventView;
use tagteam_engine::{EventKind, EventSource};

use crate::theme::Role;

/// How an event shows on the lane and in the list: its glyph in its role, and its rank, the
/// highest of a column's events winning (§7.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Mark {
    pub(crate) glyph: &'static str,
    pub(crate) role: Role,
    pub(crate) rank: u8,
}

const fn of(glyph: &'static str, role: Role, rank: u8) -> Mark {
    Mark { glyph, role, rank }
}

/// §7.4's table: what failed or needs a login `✕` crit, a pause `‖` warn, auto's or the daemon's
/// switch `⇄` accent, any other switch `⇄` text, a resume `‖` muted, then the rest at rank 1.
pub(crate) fn mark(e: &EventView) -> Mark {
    match e.kind {
        EventKind::SwitchFailed | EventKind::AllExhausted | EventKind::Quarantine => {
            of("✕", Role::Crit, 6)
        }
        EventKind::AutoswitchPaused => of("‖", Role::Warn, 5),
        EventKind::Switch => match e.source {
            EventSource::Auto | EventSource::Daemon => of("⇄", Role::Accent, 4),
            EventSource::Cli => of("⇄", Role::Text, 3),
        },
        EventKind::SwitchRecovered => of("⇄", Role::Text, 3),
        EventKind::AutoswitchResumed => of("‖", Role::Muted, 2),
        EventKind::Unquarantine => of("✓", Role::Ok, 1),
        EventKind::Add | EventKind::Import => of("+", Role::Muted, 1),
        EventKind::Remove => of("-", Role::Muted, 1),
    }
}

#[cfg(test)]
mod tests {
    use tagteam_engine::{EventKind, EventSource};

    use super::*;
    use crate::screens::history::event_at;
    use crate::testutil::NOW_MS;
    use crate::theme::Role;

    #[test]
    fn each_event_kind_has_its_glyph_role_and_rank() {
        use EventKind::*;
        use EventSource::{Auto, Cli, Daemon};
        // §7.4's table, row by row; a kind with no source of its own is any source's.
        let table: [(EventKind, Option<EventSource>, &str, Role, u8); 13] = [
            (SwitchFailed, None, "✕", Role::Crit, 6),
            (AllExhausted, None, "✕", Role::Crit, 6),
            (Quarantine, None, "✕", Role::Crit, 6),
            (AutoswitchPaused, None, "‖", Role::Warn, 5),
            (Switch, Some(Auto), "⇄", Role::Accent, 4),
            (Switch, Some(Daemon), "⇄", Role::Accent, 4),
            (Switch, Some(Cli), "⇄", Role::Text, 3),
            (SwitchRecovered, None, "⇄", Role::Text, 3),
            (AutoswitchResumed, None, "‖", Role::Muted, 2),
            (Unquarantine, None, "✓", Role::Ok, 1),
            (Add, None, "+", Role::Muted, 1),
            (Import, None, "+", Role::Muted, 1),
            (Remove, None, "-", Role::Muted, 1),
        ];
        for kind in EventKind::ALL {
            for source in EventSource::ALL {
                let (.., glyph, role, rank) = table
                    .iter()
                    .find(|(k, s, ..)| *k == kind && s.is_none_or(|s| s == source))
                    .unwrap_or_else(|| panic!("{kind:?} by {source:?} is not in the table"));
                let e = event_at(NOW_MS, kind, source);
                assert_eq!(
                    mark(&e),
                    Mark {
                        glyph,
                        role: *role,
                        rank: *rank
                    },
                    "{kind:?} by {source:?}"
                );
            }
        }
    }
}
