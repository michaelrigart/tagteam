//! `cargo xtask compat` (§15.4): the interop checks that need the real `claude` and a real
//! test account, run against a `test-support` build of `tagteam` driven as a binary.

pub mod guard;
pub mod registry;
pub mod report;
pub mod version;
