//! The token report: the index of a provider's transcripts, the evidence that places their
//! sessions, and the report over them.

mod evidence;
mod reader;
mod refresh;
mod view;

use std::path::{Path, PathBuf};
use std::time::Duration;

use tagteam_core::ProviderId;
use tagteam_provider::{Cancel, Env, FlockGuard, LockError};

use crate::auto::lock_file_name;

pub use evidence::*;
pub use refresh::*;
pub use view::*;

/// `locks/tokens-<provider>.lock` (spec §5.1): held by one refresh of the provider at a time, a
/// leaf. A provider ID that is not a plain name (anything outside `[A-Za-z0-9._-]`, or one
/// starting with `.`) cannot name a path: its lock is named by the first 12 hex digits of the
/// ID's SHA-256, inside `locks/`, by the same `lock_file_name` as an engine lock.
pub fn tokens_lock_path(env: &Env, provider: &ProviderId) -> PathBuf {
    let name = lock_file_name(provider.as_str(), "tokens", "tokens");
    env.data_dir().join("locks").join(name)
}

/// One round of a wait for a tokens lock; the rounds repeat until it is free.
const WAIT_ROUND: Duration = Duration::from_secs(3_600);

/// Waits for a tokens lock until it is free, ended only by `cancel` (spec §5.1: a refresh with
/// `Wait::Wait`, and `purge`, wait cancellably). `FlockGuard::lock` polls every 100 ms and checks
/// `cancel` before each attempt; its timeout bounds one round, and the rounds repeat, so the wait
/// has no deadline of its own.
pub(crate) fn lock_until_free(path: &Path, cancel: &Cancel) -> Result<FlockGuard, LockError> {
    loop {
        match FlockGuard::lock(path, WAIT_ROUND, cancel) {
            Err(LockError::Timeout(_)) => {}
            other => return other,
        }
    }
}

#[cfg(test)]
mod tests {
    use tagteam_core::ProviderId;
    use tagteam_provider::Env;

    use super::*;

    #[test]
    fn the_tokens_lock_is_in_locks_and_a_stored_string_never_escapes_it() {
        let d = tempfile::tempdir().unwrap();
        let env = Env::for_test(d.path());
        let locks = env.data_dir().join("locks");
        assert_eq!(
            tokens_lock_path(&env, &ProviderId::new("claude-code")),
            locks.join("tokens-claude-code.lock")
        );
        let hostile = tokens_lock_path(&env, &ProviderId::new("/../../escape"));
        assert_eq!(hostile.parent(), Some(locks.as_path()));
        let name = hostile.file_name().unwrap().to_str().unwrap();
        assert!(
            name.starts_with("tokens-") && name.ends_with(".lock") && !name.contains("escape"),
            "{name}"
        );
        assert_eq!(name.len(), "tokens-".len() + 12 + ".lock".len(), "{name}");
    }

    #[test]
    fn a_wait_for_a_tokens_lock_never_takes_a_held_lock() {
        // Held elsewhere, the lock is never taken: only the cancel token ends the wait. Free, it
        // is taken at once, and held.
        let d = tempfile::tempdir().unwrap();
        let env = Env::for_test(d.path());
        let path = tokens_lock_path(&env, &ProviderId::new("claude-code"));
        let held = FlockGuard::try_lock(&path).unwrap().unwrap();
        let cancel = Cancel::new();
        let waited = std::thread::scope(|s| {
            let waiter = s.spawn(|| lock_until_free(&path, &cancel).map(drop));
            cancel.request(2);
            waiter.join().unwrap()
        });
        assert!(
            matches!(waited, Err(LockError::Interrupted { signal: 2, .. })),
            "{waited:?}"
        );
        drop(held);
        let taken = lock_until_free(&path, &Cancel::new()).unwrap();
        assert!(FlockGuard::try_lock(&path).unwrap().is_none(), "and held");
        drop(taken);
    }
}
