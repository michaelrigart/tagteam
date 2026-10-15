//! `refresh_tokens` (spec §5): the incremental index of a provider's transcripts.

/// How a refresh takes the provider's tokens lock (spec §5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wait {
    /// Waits for another refresh to finish, cancellably: `tagteam tokens`.
    Wait,
    /// Returns `Busy` at once while another refresh holds it: the TUI's timer and the daemon.
    Try,
}
