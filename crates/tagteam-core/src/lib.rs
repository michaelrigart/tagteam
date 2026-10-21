#![forbid(unsafe_code)]

pub mod autoswitch;
pub mod backoff;
pub mod classify;
pub mod fingerprint;
pub mod fmt;
pub mod ids;
pub mod merge;
pub mod pace;
pub mod poll;
pub mod provenance;
pub mod rank;
pub mod rotation;
mod str_enum;
pub mod text;
pub mod time;
pub mod tokens;
pub mod trust;
pub mod usage;
pub mod validate;

pub use classify::{OracleVerdict, OutgoingAction, OutgoingClass, OutgoingFacts, decide_outgoing};
pub use fingerprint::Fingerprint;
pub use ids::{AccountId, IdentityKey, ProviderId};
pub use pace::{Pace, ProjectionMethod, Sample};
pub use poll::{PollBudget, PollInputs, PollPlan};
pub use provenance::{ProvenanceVerdict, provenance};
pub use rotation::rotation_order;
pub use text::printable;
pub use trust::TrustInputs;
pub use usage::{Window, WindowKind};
