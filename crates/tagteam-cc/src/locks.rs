use std::thread;
use std::time::{Duration, Instant};

use tagteam_provider::{Cancel, LiveLockSet, LockError, MkdirLock, MkdirLockSpec};

use crate::paths::CcPaths;

pub const CRED_STALE: Duration = Duration::from_secs(60);
pub const CONFIG_STALE: Duration = Duration::from_secs(10);
pub const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(9);

/// CC's credential locks (§9.1): the refresh lock, then the legacy lock. Fields drop in
/// declaration order, so the legacy lock is released first and the refresh lock last.
pub struct CcCredSet {
    legacy: MkdirLock,
    refresh: MkdirLock,
}

impl LiveLockSet for CcCredSet {
    fn check_owned(&self) -> Result<(), LockError> {
        self.refresh.check_owned()?;
        self.legacy.check_owned()
    }
}

/// CC's config lock (§9.1), the second stage of its live locks.
pub struct CcConfigSet {
    config: MkdirLock,
}

impl LiveLockSet for CcConfigSet {
    fn check_owned(&self) -> Result<(), LockError> {
        self.config.check_owned()
    }
}

/// The refresh lock, then the legacy lock. If the legacy lock is contended the refresh lock is
/// released and the pair retried, as CC does. tagteam never writes `.oauth_refresh.lock.owner`.
/// `cancel` is checked before every attempt (§14.1): a token set while the legacy lock is
/// contended ends the retries naming that lock, with the refresh lock already released.
pub fn acquire_credentials(
    paths: &CcPaths,
    timeout: Duration,
    cancel: &Cancel,
) -> Result<CcCredSet, LockError> {
    let deadline = Instant::now() + timeout;
    let remaining = || deadline.saturating_duration_since(Instant::now());
    loop {
        let refresh = MkdirLock::acquire(
            &MkdirLockSpec::new(paths.refresh_lock.clone(), CRED_STALE, remaining())
                .with_cancel(cancel),
        )?;
        let legacy_spec = MkdirLockSpec::new(paths.legacy_lock(), CRED_STALE, Duration::ZERO);
        match MkdirLock::try_acquire(&legacy_spec)? {
            Some(legacy) => return Ok(CcCredSet { legacy, refresh }),
            None => {
                drop(refresh);
                if Instant::now() >= deadline {
                    return Err(LockError::Timeout(legacy_spec.path));
                }
                thread::sleep(Duration::from_millis(fastrand::u64(250..=500)).min(remaining()));
                if let Some(signal) = cancel.requested() {
                    return Err(LockError::Interrupted {
                        path: legacy_spec.path,
                        signal,
                    });
                }
            }
        }
    }
}

/// The config lock alone, waited for under `cancel` (§14.1). A caller takes it only while
/// holding the credential locks (`CredLocks::with_config`, §4.3).
pub fn acquire_config(
    paths: &CcPaths,
    timeout: Duration,
    cancel: &Cancel,
) -> Result<CcConfigSet, LockError> {
    Ok(CcConfigSet {
        config: MkdirLock::acquire(
            &MkdirLockSpec::new(paths.config_lock.clone(), CONFIG_STALE, timeout)
                .with_cancel(cancel),
        )?,
    })
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
    fn the_credential_locks_never_touch_the_config_lock() {
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        let set = acquire_credentials(&p, ACQUIRE_TIMEOUT, &Cancel::new()).unwrap();
        assert!(p.refresh_lock.is_dir() && p.legacy_lock().is_dir());
        assert!(!p.config_lock.exists(), "only the config stage takes it");
        assert!(set.check_owned().is_ok());
        assert!(!p.config_home.join(".oauth_refresh.lock.owner").exists());
        drop(set);
        assert!(!p.refresh_lock.exists() && !p.legacy_lock().exists());
    }

    #[test]
    fn the_config_lock_is_taken_and_released_on_its_own() {
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        let set = acquire_config(&p, ACQUIRE_TIMEOUT, &Cancel::new()).unwrap();
        assert!(p.config_lock.is_dir());
        assert!(!p.refresh_lock.exists() && !p.legacy_lock().exists());
        assert!(set.check_owned().is_ok());
        drop(set);
        assert!(!p.config_lock.exists());
    }

    #[test]
    fn a_contended_legacy_lock_releases_the_refresh_lock_while_waiting() {
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        fs::create_dir(p.legacy_lock()).unwrap(); // CC holds it, freshly
        assert!(matches!(
            acquire_credentials(&p, Duration::from_millis(700), &Cancel::new()),
            Err(LockError::Timeout(_))
        ));
        assert!(
            !p.refresh_lock.exists(),
            "the refresh lock must not be held while waiting"
        );
        assert!(p.legacy_lock().is_dir(), "CC's lock is left alone");
    }

    /// Proves the release happens *during* the wait, not just after `acquire_credentials` gives
    /// up: while a background acquirer is genuinely still blocked on the (still-held) legacy
    /// lock, this thread must itself be able to actually acquire the refresh lock. That's true
    /// no matter how long the lock sits free between the background thread's retries — a
    /// microsecond or a second — unlike polling `exists()`, which can miss a window far
    /// narrower than its poll interval.
    ///
    /// The refresh lock being free proves nothing until the acquirer has actually tried it, so
    /// the test first waits for evidence that it has: taking the refresh lock and releasing it on
    /// the legacy contention (`mkdir`, then `rmdir`) changes the mtime of the directory holding
    /// it. Without that, this thread could take the free lock before the acquirer ever ran.
    #[test]
    fn the_refresh_lock_is_actually_free_during_a_legacy_wait_not_only_after_it() {
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        fs::create_dir(p.legacy_lock()).unwrap(); // CC holds it, freshly
        let lock_dir = p.refresh_lock.parent().unwrap().to_path_buf();
        let untouched = fs::metadata(&lock_dir).unwrap().modified().unwrap();
        let p2 = p.clone();
        // Generous on purpose: only this test's own deadline below is meant to be tight; this
        // just must outlast it plus the eventual release.
        let handle = thread::spawn(move || {
            acquire_credentials(&p2, Duration::from_secs(30), &Cancel::new())
        });

        let tried = Instant::now() + Duration::from_secs(10);
        while fs::metadata(&lock_dir).unwrap().modified().unwrap() == untouched {
            assert!(
                Instant::now() < tried,
                "the acquirer never reached the refresh lock"
            );
            thread::sleep(Duration::from_millis(5));
        }
        assert!(
            p.legacy_lock().is_dir(),
            "the legacy lock is still held: it is contended"
        );

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
    fn a_held_refresh_lock_times_out_without_touching_it() {
        // Review Focus 1 (M1): CC is mid-refresh.
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        fs::create_dir(&p.refresh_lock).unwrap();
        let start = std::time::Instant::now();
        assert!(matches!(
            acquire_credentials(&p, Duration::from_millis(500), &Cancel::new()),
            Err(LockError::Timeout(_))
        ));
        assert!(start.elapsed() < Duration::from_secs(2));
        assert!(p.refresh_lock.is_dir());
    }

    #[test]
    fn a_held_config_lock_times_out_without_touching_it() {
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        fs::create_dir(&p.config_lock).unwrap(); // something else holds it, freshly
        assert!(matches!(
            acquire_config(&p, Duration::from_millis(500), &Cancel::new()),
            Err(LockError::Timeout(_))
        ));
        assert!(
            p.config_lock.is_dir(),
            "the other holder's lock is left alone"
        );
    }

    /// Sets `cancel` to SIGINT from another thread 200 ms from now, as the CLI's handler would
    /// (§14.1), and returns the instant just before it did.
    fn interrupt_soon(cancel: &Cancel) -> thread::JoinHandle<Instant> {
        let cancel = cancel.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            let at = Instant::now();
            cancel.request(libc::SIGINT);
            at
        })
    }

    /// Runs `wait` while `interrupt_soon` sets the token; it must still be waiting then, and
    /// must end within one poll (at most 500 ms) of it. Returns the lock it names.
    fn interrupted<T>(wait: impl FnOnce(&Cancel) -> Result<T, LockError>) -> std::path::PathBuf {
        let cancel = Cancel::new();
        let setter = interrupt_soon(&cancel);
        let result = wait(&cancel);
        let ended = Instant::now();
        let set_at = setter.join().unwrap();
        assert!(ended >= set_at, "the wait ended before the token was set");
        assert!(
            ended - set_at < Duration::from_secs(1),
            "{:?}",
            ended - set_at
        );
        match result {
            Err(LockError::Interrupted { path, signal }) => {
                assert_eq!(signal, libc::SIGINT);
                path
            }
            Err(e) => panic!("expected an interrupted wait, got {e:?}"),
            Ok(_) => panic!("expected an interrupted wait, got the locks"),
        }
    }

    #[test]
    fn a_set_token_takes_neither_credential_lock() {
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        let cancel = Cancel::new();
        cancel.request(libc::SIGTERM);
        match acquire_credentials(&p, ACQUIRE_TIMEOUT, &cancel) {
            Err(LockError::Interrupted { path, signal }) => {
                assert_eq!((path, signal), (p.refresh_lock.clone(), libc::SIGTERM))
            }
            other => panic!("expected an interrupted wait, got {:?}", other.err()),
        }
        assert!(!p.refresh_lock.exists() && !p.legacy_lock().exists());
    }

    #[test]
    fn a_token_set_while_cc_holds_the_refresh_lock_ends_the_wait() {
        // Review Focus 1: CC is mid-refresh.
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        fs::create_dir(&p.refresh_lock).unwrap();
        let named = interrupted(|c| acquire_credentials(&p, Duration::from_secs(30), c));
        assert_eq!(named, p.refresh_lock);
        assert!(p.refresh_lock.is_dir(), "CC's lock is left alone");
        assert!(!p.legacy_lock().exists());
    }

    #[test]
    fn a_token_set_during_legacy_contention_ends_the_retries_naming_the_legacy_lock() {
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        fs::create_dir(p.legacy_lock()).unwrap(); // CC holds it, freshly
        let named = interrupted(|c| acquire_credentials(&p, Duration::from_secs(30), c));
        assert_eq!(named, p.legacy_lock());
        assert!(
            !p.refresh_lock.exists(),
            "the refresh lock each retry took is released"
        );
        assert!(p.legacy_lock().is_dir(), "CC's lock is left alone");
    }

    #[test]
    fn a_token_set_while_the_config_lock_is_held_ends_the_wait() {
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        fs::create_dir(&p.config_lock).unwrap(); // something else holds it, freshly
        let named = interrupted(|c| acquire_config(&p, Duration::from_secs(30), c));
        assert_eq!(named, p.config_lock);
        assert!(
            p.config_lock.is_dir(),
            "the other holder's lock is left alone"
        );
    }
}
