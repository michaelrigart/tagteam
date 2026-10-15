//! The token report view (spec §10).

/// What a report row spans (spec §10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenPeriod {
    Day,
    Month,
    Session,
    Block,
}
