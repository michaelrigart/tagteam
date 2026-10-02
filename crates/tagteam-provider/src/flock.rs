use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use crate::cancel::Cancel;
use crate::env::Env;
use crate::mkdir_lock::{LockError, check_cancel};

/// An exclusive `flock` on a file, released by the kernel when the holder exits. The fd is
/// `O_CLOEXEC` (std's default).
#[derive(Debug)]
pub struct FlockGuard {
    _file: File,
    path: PathBuf,
}

impl FlockGuard {
    pub fn try_lock(path: &Path) -> io::Result<Option<Self>> {
        if let Some(dir) = path.parent() {
            crate::atomic::ensure_private_dir(dir)?;
        }
        // A lock file is never truncated: a truncate here would race a concurrent
        // holder's read of it, and `create(true)` alone trips `suspicious_open_options`.
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path)?;
        // SAFETY: `file` owns a valid descriptor for the duration of the call.
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc == 0 {
            return Ok(Some(Self {
                _file: file,
                path: path.to_path_buf(),
            }));
        }
        let e = io::Error::last_os_error();
        if e.raw_os_error() == Some(libc::EWOULDBLOCK) {
            Ok(None)
        } else {
            Err(e)
        }
    }

    /// Waits up to `timeout`, polling every 100 ms. `cancel` is checked before every attempt,
    /// the first included (§14.1): a set token ends the wait with `LockError::Interrupted` and
    /// takes nothing.
    pub fn lock(path: &Path, timeout: Duration, cancel: &Cancel) -> Result<Self, LockError> {
        let deadline = Instant::now() + timeout;
        loop {
            check_cancel(cancel, path)?;
            if let Some(g) = Self::try_lock(path)? {
                return Ok(g);
            }
            if Instant::now() >= deadline {
                return Err(LockError::Timeout(path.to_path_buf()));
            }
            thread::sleep(Duration::from_millis(100));
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// What a non-blocking test of a lock file found (§12.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockProbe {
    /// No file at the path.
    Missing,
    /// The file exists and nothing holds an exclusive lock on it.
    Free,
    /// Another open file description holds an exclusive lock on it, as a launcher does.
    Held,
}

/// Tests `path` with a non-blocking `flock`, opening it read-only and never creating it. A
/// reservation is only ever tested, never waited on (§12.5), so a lock this takes is released
/// before it returns. The test is a shared lock: a launcher holds an exclusive one, which it
/// still sees, and two probes of one file never see each other as a holder.
pub fn probe_lock(path: &Path) -> io::Result<LockProbe> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(LockProbe::Missing),
        Err(e) => return Err(e),
    };
    // SAFETY: `file` owns a valid descriptor for the duration of the call.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) };
    if rc == 0 {
        // SAFETY: as above; this releases the lock the probe just took.
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
        return Ok(LockProbe::Free);
    }
    let e = io::Error::last_os_error();
    if e.raw_os_error() == Some(libc::EWOULDBLOCK) {
        Ok(LockProbe::Held)
    } else {
        Err(e)
    }
}

/// tagteam's mutation lock (§9.1). Provider live locks can only be taken from one.
#[derive(Debug)]
pub struct MutationGuard {
    _lock: FlockGuard,
}

impl MutationGuard {
    pub const TIMEOUT: Duration = Duration::from_secs(10);
    pub const BOOTSTRAP_TIMEOUT: Duration = Duration::from_secs(30);

    /// Waits up to `timeout`, on `env.cancel` (§14.1).
    pub fn acquire(env: &Env, timeout: Duration) -> Result<Self, LockError> {
        let path = env.data_dir().join(".mutation.lock");
        Ok(Self {
            _lock: FlockGuard::lock(&path, timeout, &env.cancel)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn a_second_lock_on_the_same_file_is_refused_until_release() {
        let _fork = crate::FORK_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("sub/x.lock");
        let g = FlockGuard::try_lock(&p).unwrap().unwrap();
        assert_eq!(
            fs::metadata(p.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert!(FlockGuard::try_lock(&p).unwrap().is_none());
        assert!(matches!(
            FlockGuard::lock(&p, Duration::from_millis(250), &Cancel::new()),
            Err(LockError::Timeout(_))
        ));
        drop(g);
        assert!(FlockGuard::try_lock(&p).unwrap().is_some());
    }

    #[test]
    fn a_probe_never_creates_a_missing_lock_file() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join(".tagteam-launch/42.lock");
        assert_eq!(probe_lock(&p).unwrap(), LockProbe::Missing);
        assert!(!p.parent().unwrap().exists(), "nor its directory");
        fs::create_dir(p.parent().unwrap()).unwrap();
        assert_eq!(probe_lock(&p).unwrap(), LockProbe::Missing);
        assert!(!p.exists());
    }

    #[test]
    fn a_probe_sees_a_held_lock_and_releases_a_free_one() {
        let _fork = crate::FORK_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("42.lock");
        let held = FlockGuard::try_lock(&p).unwrap().unwrap();
        assert_eq!(probe_lock(&p).unwrap(), LockProbe::Held);
        drop(held);
        fs::set_permissions(&p, fs::Permissions::from_mode(0o400)).unwrap();
        assert_eq!(
            probe_lock(&p).unwrap(),
            LockProbe::Free,
            "read-only is enough"
        );
        fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(
            FlockGuard::try_lock(&p).unwrap().is_some(),
            "the probe released what it took"
        );
    }

    #[test]
    fn two_simultaneous_probes_of_a_free_lock_both_see_it_free() {
        // One probe is between its `flock` and its unlock, as `probe_lock` is for an instant,
        // when the other runs: neither may take the other for a launcher (§12.5).
        let _fork = crate::FORK_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("42.lock");
        fs::write(&p, b"").unwrap();
        let first = File::open(&p).unwrap();
        // SAFETY: `first` owns a valid descriptor for the duration of the call.
        let rc = unsafe { libc::flock(first.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) };
        assert_eq!(rc, 0, "a free file takes a shared lock");
        assert_eq!(probe_lock(&p).unwrap(), LockProbe::Free);
        drop(first);
        let held = FlockGuard::try_lock(&p).unwrap().unwrap();
        assert_eq!(
            probe_lock(&p).unwrap(),
            LockProbe::Held,
            "a launcher's exclusive lock still shows"
        );
        drop(held);
    }

    #[test]
    fn a_probe_that_cannot_open_the_path_is_an_error() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("not-a-dir");
        fs::write(&file, b"").unwrap();
        assert!(probe_lock(&file.join("42.lock")).is_err());
    }

    #[test]
    fn the_mutation_guard_lives_in_the_data_dir() {
        let _fork = crate::FORK_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let d = tempfile::tempdir().unwrap();
        let env = Env::for_test(d.path());
        let g = MutationGuard::acquire(&env, Duration::from_millis(100)).unwrap();
        assert!(env.data_dir().join(".mutation.lock").exists());
        assert!(MutationGuard::acquire(&env, Duration::from_millis(150)).is_err());
        drop(g);
        assert!(MutationGuard::acquire(&env, Duration::from_millis(100)).is_ok());
    }

    #[test]
    fn a_set_token_ends_a_flock_wait_before_its_first_attempt() {
        let _fork = crate::FORK_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("sub/x.lock");
        let cancel = Cancel::new();
        cancel.request(2);
        match FlockGuard::lock(&p, Duration::from_secs(10), &cancel) {
            Err(LockError::Interrupted { path, signal }) => {
                assert_eq!((path, signal), (p.clone(), 2))
            }
            other => panic!("expected an interrupted wait, got {other:?}"),
        }
        assert!(
            !p.exists(),
            "no attempt was made, so nothing was created or locked"
        );
    }

    #[test]
    fn a_token_set_while_another_holder_keeps_the_flock_ends_the_wait_within_one_poll() {
        let _fork = crate::FORK_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("x.lock");
        let held = FlockGuard::try_lock(&p).unwrap().unwrap();
        let cancel = Cancel::new();
        let setter = {
            let cancel = cancel.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(200));
                let at = Instant::now();
                cancel.request(15);
                at
            })
        };
        let result = FlockGuard::lock(&p, Duration::from_secs(30), &cancel);
        let ended = Instant::now();
        let set_at = setter.join().unwrap();
        assert!(
            matches!(&result, Err(LockError::Interrupted { signal: 15, .. })),
            "{result:?}"
        );
        assert!(ended >= set_at, "the wait ended before the token was set");
        // One poll is 100 ms; the timeout is 30 s.
        assert!(
            ended - set_at < Duration::from_millis(500),
            "{:?}",
            ended - set_at
        );
        assert!(
            FlockGuard::try_lock(&p).unwrap().is_none(),
            "the holder keeps it"
        );
        drop(held);
    }

    #[test]
    fn the_mutation_guard_waits_on_the_envs_token() {
        let _fork = crate::FORK_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let d = tempfile::tempdir().unwrap();
        let env = Env::for_test(d.path());
        let held = MutationGuard::acquire(&env, Duration::from_millis(100)).unwrap();
        env.cancel.request(1);
        let start = Instant::now();
        let err = MutationGuard::acquire(&env, MutationGuard::TIMEOUT).unwrap_err();
        assert_eq!(err.signal(), Some(1), "{err}");
        assert!(
            start.elapsed() < Duration::from_millis(100),
            "checked before the first attempt"
        );
        drop(held);
        // The free lock is still refused under the set token, by every clone of the Env; an Env
        // with its own token takes it.
        assert!(MutationGuard::acquire(&env.clone(), Duration::ZERO).is_err());
        assert!(MutationGuard::acquire(&Env::for_test(d.path()), Duration::ZERO).is_ok());
    }
}
