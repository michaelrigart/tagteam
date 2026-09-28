use std::thread;
use std::time::{Duration, Instant};

use tagteam_provider::{LiveLockSet, LockError, MkdirLock, MkdirLockSpec};

use crate::paths::CcPaths;

pub const CRED_STALE: Duration = Duration::from_secs(60);
pub const CONFIG_STALE: Duration = Duration::from_secs(10);
pub const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(9);

/// CC's credential locks and config lock (§9.1). Fields drop in declaration order, so the
/// config lock is released first and the refresh lock last.
pub struct CcLockSet {
    config: MkdirLock,
    legacy: MkdirLock,
    refresh: MkdirLock,
}

impl LiveLockSet for CcLockSet {
    fn check_owned(&self) -> Result<(), LockError> {
        self.refresh.check_owned()?;
        self.legacy.check_owned()?;
        self.config.check_owned()
    }
}

pub fn acquire(paths: &CcPaths) -> Result<CcLockSet, LockError> {
    acquire_with(paths, ACQUIRE_TIMEOUT)
}

/// Refresh lock, then the legacy lock; if the legacy lock is contended the refresh lock is
/// released and the pair retried, as CC does. Then the config lock. tagteam never writes
/// `.oauth_refresh.lock.owner`.
pub fn acquire_with(paths: &CcPaths, timeout: Duration) -> Result<CcLockSet, LockError> {
    let deadline = Instant::now() + timeout;
    let remaining = || deadline.saturating_duration_since(Instant::now());
    loop {
        let refresh = MkdirLock::acquire(&MkdirLockSpec::new(
            paths.refresh_lock.clone(),
            CRED_STALE,
            remaining(),
        ))?;
        let legacy_spec = MkdirLockSpec::new(paths.legacy_lock(), CRED_STALE, Duration::ZERO);
        match MkdirLock::try_acquire(&legacy_spec)? {
            Some(legacy) => {
                let config = MkdirLock::acquire(&MkdirLockSpec::new(
                    paths.config_lock.clone(),
                    CONFIG_STALE,
                    remaining(),
                ))?;
                return Ok(CcLockSet {
                    config,
                    legacy,
                    refresh,
                });
            }
            None => {
                drop(refresh);
                if Instant::now() >= deadline {
                    return Err(LockError::Timeout(legacy_spec.path));
                }
                thread::sleep(Duration::from_millis(fastrand::u64(250..=500)).min(remaining()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tagteam_provider::Env;

    fn paths(root: &std::path::Path) -> CcPaths {
        let env = Env::for_test(root);
        fs::create_dir_all(env.home.join(".claude")).unwrap();
        CcPaths::resolve(&env)
    }

    #[test]
    fn takes_all_three_and_releases_them() {
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        let set = acquire(&p).unwrap();
        assert!(p.refresh_lock.is_dir() && p.legacy_lock().is_dir() && p.config_lock.is_dir());
        assert!(set.check_owned().is_ok());
        assert!(!p.config_home.join(".oauth_refresh.lock.owner").exists());
        drop(set);
        assert!(!p.refresh_lock.exists() && !p.legacy_lock().exists() && !p.config_lock.exists());
    }

    #[test]
    fn a_contended_legacy_lock_releases_the_refresh_lock_while_waiting() {
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        fs::create_dir(p.legacy_lock()).unwrap(); // CC holds it, freshly
        assert!(matches!(
            acquire_with(&p, Duration::from_millis(700)),
            Err(LockError::Timeout(_))
        ));
        assert!(
            !p.refresh_lock.exists(),
            "the refresh lock must not be held while waiting"
        );
        assert!(p.legacy_lock().is_dir(), "CC's lock is left alone");
    }

    /// Proves the release happens *during* the wait, not just after `acquire_with` gives up:
    /// while a background acquirer is genuinely still blocked on the (still-held) legacy lock,
    /// this thread must itself be able to actually acquire the refresh lock. That's true no
    /// matter how long the lock sits free between the background thread's retries — a
    /// microsecond or a second — unlike polling `exists()`, which can miss a window far
    /// narrower than its poll interval.
    #[test]
    fn the_refresh_lock_is_actually_free_during_a_legacy_wait_not_only_after_it() {
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        fs::create_dir(p.legacy_lock()).unwrap(); // CC holds it, freshly
        let p2 = p.clone();
        // Generous on purpose: only this test's own deadline below is meant to be tight; this
        // just must outlast it plus the eventual release.
        let handle = thread::spawn(move || acquire_with(&p2, Duration::from_secs(30)));

        let refresh_spec = MkdirLockSpec::new(p.refresh_lock.clone(), CRED_STALE, Duration::ZERO);
        let deadline = Instant::now() + Duration::from_secs(10);
        let observed = loop {
            if let Some(lock) = MkdirLock::try_acquire(&refresh_spec).unwrap() {
                break lock;
            }
            assert!(
                Instant::now() < deadline,
                "the refresh lock was never free while the acquirer waited on the legacy lock"
            );
        };
        assert!(
            !handle.is_finished(),
            "it should still be waiting on the legacy lock, not have given up or succeeded"
        );
        drop(observed);

        fs::remove_dir(p.legacy_lock()).unwrap(); // CC releases its lock
        let set = handle.join().unwrap().unwrap();
        assert!(p.legacy_lock().is_dir());
        drop(set);
        assert!(!p.refresh_lock.exists() && !p.legacy_lock().exists() && !p.config_lock.exists());
    }

    #[test]
    fn a_config_lock_timeout_releases_the_refresh_and_legacy_locks() {
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        fs::create_dir(&p.config_lock).unwrap(); // something else holds it, freshly
        assert!(matches!(
            acquire_with(&p, Duration::from_millis(500)),
            Err(LockError::Timeout(_))
        ));
        assert!(
            !p.refresh_lock.exists(),
            "the refresh lock must be released"
        );
        assert!(
            !p.legacy_lock().exists(),
            "the legacy lock must be released"
        );
        assert!(
            p.config_lock.is_dir(),
            "the other holder's config lock is left alone"
        );
    }

    #[test]
    fn a_held_refresh_lock_times_out_without_touching_it() {
        // Review Focus 1: CC is mid-refresh.
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        fs::create_dir(&p.refresh_lock).unwrap();
        let start = std::time::Instant::now();
        assert!(matches!(
            acquire_with(&p, Duration::from_millis(500)),
            Err(LockError::Timeout(_))
        ));
        assert!(start.elapsed() < Duration::from_secs(2));
        assert!(p.refresh_lock.is_dir());
    }
}
