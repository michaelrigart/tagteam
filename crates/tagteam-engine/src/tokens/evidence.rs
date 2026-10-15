//! Session evidence (spec §6): sightings of sessions in homes, and `run`'s launches.

use tagteam_core::{AccountId, ProviderId};

/// One launch `run` recorded (spec §6.1), as `open_launch` returns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchId {
    pub provider: ProviderId,
    pub account: AccountId,
    pub started_at_ms: i64,
}
