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
