#![forbid(unsafe_code)]

pub mod classify;
pub mod fingerprint;
pub mod ids;
pub mod rotation;
pub mod validate;

pub use classify::{OracleVerdict, OutgoingAction, OutgoingClass, OutgoingFacts, decide_outgoing};
pub use fingerprint::Fingerprint;
pub use ids::{AccountId, CLAUDE_CODE, IdentityKey, ProviderId};
pub use rotation::next_in_rotation;
