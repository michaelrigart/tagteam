//! `cargo xtask`: the workspace's development tasks (Decision 16). `compat` checks tagteam
//! against the real `claude` and a dedicated test account (§15.4). Nothing here ships.

#![forbid(unsafe_code)]

pub mod compat;
