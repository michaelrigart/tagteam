use crate::engine::Engine;
use crate::error::EngineError;

/// A named point in the switch transaction. With the `test-hooks` feature, the environment
/// variable `TAGTEAM_TEST_CRASH_AT=<name>` ends the process there as a kill would: no
/// destructor runs, so no lock guard and no rollback (the kill tests).
/// `TAGTEAM_TEST_PAUSE_AT=<name>` parks it there instead until the test lets it go (`pause_at`),
/// so a binary test can signal it at a known point (§14.1). `Engine::fail_at` injects an
/// error there (`"<name>"`) or a panic (`"panic:<name>"`) for the rollback tests;
/// `TAGTEAM_TEST_FAIL_AT` does the same (`<name>` or `panic:<name>`) for a process whose engine
/// a test cannot reach (the real binary).
/// Without the feature, a no-op.
#[cfg(feature = "test-hooks")]
pub(crate) fn point(engine: &Engine, name: &'static str) -> Result<(), EngineError> {
    if std::env::var("TAGTEAM_TEST_CRASH_AT").as_deref() == Ok(name) {
        std::process::exit(137);
    }
    pause_at(name);
    for (at, callback) in &*engine.on_point.lock().unwrap() {
        if *at == name {
            callback();
        }
    }
    let injected = *engine.fail_at.lock().unwrap();
    let from_env = std::env::var("TAGTEAM_TEST_FAIL_AT").ok();
    if injected == Some(name) || from_env.as_deref() == Some(name) {
        return Err(EngineError::InvalidInput(format!(
            "injected failure at {name}"
        )));
    }
    let panic_at = |n: &str| n.strip_prefix("panic:") == Some(name);
    if injected.is_some_and(panic_at) || from_env.as_deref().is_some_and(panic_at) {
        panic!("injected panic at {name}");
    }
    Ok(())
}

/// Parks the process at the point `name` when `TAGTEAM_TEST_PAUSE_AT=<name>`. It creates
/// `paused` in `TAGTEAM_TEST_PAUSE_DIR`, then waits for the test to create `resume` there, for at
/// most 30 s so that a broken test cannot leave it running. It never looks at the cancel token,
/// so a signal sent meanwhile meets the work after the point exactly as it would have met it
/// there. The engine's points park through it, and so do the CLI's own (`tagteam run`'s
/// `before-spawn`), so a binary test drives one protocol.
///
/// A point the process passes again finds `resume` there and goes on. With
/// `TAGTEAM_TEST_PAUSE_EACH` set too, each pass parks on files of its own instead: the `n`th
/// creates `paused-<n>` and waits for `resume-<n>`.
#[cfg(feature = "test-hooks")]
pub fn pause_at(name: &str) {
    use std::sync::atomic::{AtomicUsize, Ordering};

    static PASSES: AtomicUsize = AtomicUsize::new(0);
    if std::env::var("TAGTEAM_TEST_PAUSE_AT").as_deref() != Ok(name) {
        return;
    }
    let Some(dir) = std::env::var_os("TAGTEAM_TEST_PAUSE_DIR").map(std::path::PathBuf::from) else {
        return;
    };
    let (paused, resume) = if std::env::var_os("TAGTEAM_TEST_PAUSE_EACH").is_some() {
        let n = PASSES.fetch_add(1, Ordering::SeqCst) + 1;
        (format!("paused-{n}"), format!("resume-{n}"))
    } else {
        ("paused".to_owned(), "resume".to_owned())
    };
    let _ = std::fs::write(dir.join(paused), b"");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !dir.join(&resume).exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[cfg(not(feature = "test-hooks"))]
#[inline(always)]
pub(crate) fn point(_engine: &Engine, _name: &'static str) -> Result<(), EngineError> {
    Ok(())
}
