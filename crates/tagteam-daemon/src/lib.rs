//! `tagteam daemon` (the daemon spec, D1–D16): the scheduler that the daemon and `tagteam auto`
//! both run on, the daemon's jobs, and the socket protocol (D3). Like the engine, it names no
//! provider's paths, keys or endpoints, and reaches providers through the engine only.

pub mod auto;
pub mod backoff;
pub mod client;
pub mod daemon;
pub mod events;
pub mod notify;
pub mod protocol;
pub mod provider_job;
pub mod scheduler;
pub mod server;
pub mod service;
pub mod status;
#[cfg(test)]
mod testutil;
pub mod watch;
