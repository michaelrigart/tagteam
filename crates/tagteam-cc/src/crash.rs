//! A crash point for the kill tests (§15.2) that sits inside a single opaque
//! `Provider::write_credential` call, between writing the target axis and clearing the other
//! one — a boundary the engine's own hooks (`tagteam_engine::hooks`) never see, since that
//! whole call is one atomic step to the engine. `tagteam-cc` cannot depend on `tagteam-engine`,
//! so this is a small local counterpart rather than a shared one.
#[cfg(feature = "test-hooks")]
pub(crate) fn point(name: &'static str) {
    if std::env::var("TAGTEAM_TEST_CRASH_AT").as_deref() == Ok(name) {
        std::process::exit(137);
    }
}

#[cfg(not(feature = "test-hooks"))]
#[inline(always)]
pub(crate) fn point(_name: &'static str) {}
