#![forbid(unsafe_code)]
#[cfg(test)]
mod compat;
pub mod config;
mod crash;
pub mod cswap;
mod doctor;
pub mod endpoints;
pub mod env;
pub mod live;
pub mod locks;
pub mod naming;
pub mod oauth;
pub mod paths;
pub mod prices;
pub mod provider;
pub mod records;
mod session;
pub mod shape;
pub mod transcripts;
pub mod usage;

pub const CLAUDE_CODE: &str = "claude-code";

pub use naming::{ItemKind, keychain_account, keychain_service};
pub use paths::CcPaths;
pub use provider::ClaudeCode;
