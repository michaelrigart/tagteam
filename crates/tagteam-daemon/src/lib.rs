//! `tagteam daemon` (the daemon spec, D1–D16): the scheduler that the daemon and `tagteam auto`
//! both run on, the daemon's jobs, and the socket protocol (D3). Like the engine, it names no
//! provider's paths, keys or endpoints, and reaches providers through the engine only.

pub mod events;
pub mod protocol;
