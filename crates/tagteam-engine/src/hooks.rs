use crate::engine::Engine;
use crate::error::EngineError;

/// A named point in the switch transaction. With the `test-hooks` feature, the environment
/// variable `TAGTEAM_TEST_CRASH_AT=<name>` ends the process there as a kill would: no
/// destructor runs, so no lock guard and no rollback (the kill tests).
/// `TAGTEAM_TEST_PAUSE_AT=<name>` parks it there instead until the test lets it go (`pause`),
/// so a binary test can signal it at a known point (§14.1). `Engine::fail_at` injects an
/// error there (`"<name>"`) or a panic (`"panic:<name>"`) for the rollback tests;
/// `TAGTEAM_TEST_FAIL_AT=<name>` injects the error for a process whose engine a test cannot
/// reach (the real binary).
/// Without the feature, a no-op.
#[cfg(feature = "test-hooks")]
pub(crate) fn point(engine: &Engine, name: &'static str) -> Result<(), EngineError> {
    if std::env::var("TAGTEAM_TEST_CRASH_AT").as_deref() == Ok(name) {
        std::process::exit(137);
    }
    if std::env::var("TAGTEAM_TEST_PAUSE_AT").as_deref() == Ok(name) {
        pause();
    }
    if let Some((at, callback)) = &*engine.on_point.lock().unwrap() {
        if *at == name {
            callback();
        }
    }
    let injected = *engine.fail_at.lock().unwrap();
    if injected == Some(name) || std::env::var("TAGTEAM_TEST_FAIL_AT").as_deref() == Ok(name) {
        return Err(EngineError::InvalidInput(format!(
            "injected failure at {name}"
        )));
    }
    if injected.and_then(|n| n.strip_prefix("panic:")) == Some(name) {
        panic!("injected panic at {name}");
    }
    Ok(())
}

/// Parks the process at a point. It creates `paused` in `TAGTEAM_TEST_PAUSE_DIR`, then waits
/// for the test to create `resume` there, for at most 30 s so that a broken test cannot leave
/// it running. It never looks at the cancel token, so a signal sent meanwhile meets the work
/// after the point exactly as it would have met it there.
#[cfg(feature = "test-hooks")]
fn pause() {
    let Some(dir) = std::env::var_os("TAGTEAM_TEST_PAUSE_DIR").map(std::path::PathBuf::from) else {
        return;
    };
    let _ = std::fs::write(dir.join("paused"), b"");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !dir.join("resume").exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[cfg(not(feature = "test-hooks"))]
#[inline(always)]
pub(crate) fn point(_engine: &Engine, _name: &'static str) -> Result<(), EngineError> {
    Ok(())
}
