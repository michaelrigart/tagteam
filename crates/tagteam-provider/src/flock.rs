use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use crate::env::Env;
use crate::mkdir_lock::LockError;

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

    pub fn lock(path: &Path, timeout: Duration) -> Result<Self, LockError> {
        let deadline = Instant::now() + timeout;
        loop {
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

/// tagteam's mutation lock (§9.1). Provider live locks can only be taken from one.
#[derive(Debug)]
pub struct MutationGuard {
    _lock: FlockGuard,
}

impl MutationGuard {
    pub const TIMEOUT: Duration = Duration::from_secs(10);
    pub const BOOTSTRAP_TIMEOUT: Duration = Duration::from_secs(30);

    pub fn acquire(env: &Env, timeout: Duration) -> Result<Self, LockError> {
        let path = env.data_dir().join(".mutation.lock");
        Ok(Self {
            _lock: FlockGuard::lock(&path, timeout)?,
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
            FlockGuard::lock(&p, Duration::from_millis(250)),
            Err(LockError::Timeout(_))
        ));
        drop(g);
        assert!(FlockGuard::try_lock(&p).unwrap().is_some());
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
}
