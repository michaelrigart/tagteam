//! The `transcripts` capability's neutral types (token report spec §3.2): what a provider reads
//! from one transcript line, whatever its own format.

use tagteam_core::tokens::TokenCounts;

/// One transcript line that carries usage (spec §3.2). Several lines may describe one response;
/// the index merges them by `(message_id, request_id)` (spec §2.2).
#[derive(Debug, Clone, PartialEq)]
pub struct UsageLine {
    pub message_id: String,
    /// `""` when the line has none (ruling R5).
    pub request_id: String,
    pub session_id: String,
    /// Epoch milliseconds.
    pub at_ms: i64,
    pub model: String,
    /// The line's working directory.
    pub project: Option<String>,
    pub sidechain: bool,
    pub counts: TokenCounts,
    pub speed: Option<String>,
    /// The line carries a non-null stop reason, so its output count is final (spec §2.2).
    pub final_line: bool,
}

/// A line that should carry usage and cannot be read (spec §5.4). The text never quotes the
/// line.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct LineError(pub String);
