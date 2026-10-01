#![forbid(unsafe_code)]

pub mod backoff;
pub mod classify;
pub mod fingerprint;
pub mod ids;
pub mod poll;
pub mod rotation;
pub mod trust;
pub mod usage;
pub mod validate;

pub use classify::{OracleVerdict, OutgoingAction, OutgoingClass, OutgoingFacts, decide_outgoing};
pub use fingerprint::Fingerprint;
pub use ids::{AccountId, CLAUDE_CODE, IdentityKey, ProviderId};
pub use poll::{PollBudget, PollInputs, PollPlan};
pub use rotation::rotation_order;
pub use trust::TrustInputs;
pub use usage::{Window, WindowKind};
