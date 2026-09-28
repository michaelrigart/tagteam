use crate::engine::Engine;
use crate::error::EngineError;

/// A named point in the switch transaction. With the `test-hooks` feature, the environment
/// variable `TAGTEAM_TEST_CRASH_AT=<name>` ends the process there as a kill would: no
/// destructor runs, so no lock guard and no rollback (the kill tests). `Engine::fail_at`
/// injects an error there (`"<name>"`) or a panic (`"panic:<name>"`) for the rollback tests.
/// Without the feature, a no-op.
#[cfg(feature = "test-hooks")]
pub(crate) fn point(engine: &Engine, name: &'static str) -> Result<(), EngineError> {
    if std::env::var("TAGTEAM_TEST_CRASH_AT").as_deref() == Ok(name) {
        std::process::exit(137);
    }
    if let Some((at, callback)) = &*engine.on_point.lock().unwrap() {
        if *at == name {
            callback();
        }
    }
    let injected = *engine.fail_at.lock().unwrap();
    if injected == Some(name) {
        return Err(EngineError::InvalidInput(format!(
            "injected failure at {name}"
        )));
    }
    if injected.and_then(|n| n.strip_prefix("panic:")) == Some(name) {
        panic!("injected panic at {name}");
    }
    Ok(())
}

#[cfg(not(feature = "test-hooks"))]
#[inline(always)]
pub(crate) fn point(_engine: &Engine, _name: &'static str) -> Result<(), EngineError> {
    Ok(())
}
