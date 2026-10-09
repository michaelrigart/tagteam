//! `cargo xtask compat` (§15.4): the interop checks that need the real `claude` and a real
//! test account, run against a `test-support` build of `tagteam` driven as a binary.

pub mod capture;
pub mod checks;
pub mod ctx;
pub mod guard;
pub mod keychain;
pub mod layout;
pub mod registry;
pub mod report;
pub mod store;
pub mod sys;
pub mod version;
