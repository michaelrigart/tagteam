use std::fs::{self, File};
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use crate::cancel::Cancel;

#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error("timed out waiting for the lock {0}")]
    Timeout(PathBuf),
    #[error("the lock {0} was taken over while held")]
    Compromised(PathBuf),
    #[error("lock I/O failed: {0}")]
    Io(#[from] io::Error),
    /// The cancel token was set while waiting (§14.1). The wait took nothing.
    #[error("interrupted while waiting for the lock {path}")]
    Interrupted { path: PathBuf, signal: i32 },
}

impl LockError {
    /// The signal behind an interrupted wait; `None` for every other failure.
    pub fn signal(&self) -> Option<i32> {
        match self {
            LockError::Interrupted { signal, .. } => Some(*signal),
            _ => None,
        }
    }
}

/// Decision 3 (§14.1): every lock wait calls this before each attempt, the first included, so
/// a command whose token is set never takes a new lock.
pub(crate) fn check_cancel(cancel: &Cancel, path: &Path) -> Result<(), LockError> {
    match cancel.requested() {
        Some(signal) => Err(LockError::Interrupted {
            path: path.to_path_buf(),
            signal,
        }),
        None => Ok(()),
    }
}

#[derive(Debug, Clone)]
pub struct MkdirLockSpec {
    pub path: PathBuf,
    pub stale: Duration,
    pub acquire_timeout: Duration,
    pub touch_every: Duration,
    /// Checked before every attempt `MkdirLock::acquire` makes (§14.1). `new` gives a token
    /// nothing sets; `with_cancel` shares the caller's.
    pub cancel: Cancel,
}

impl MkdirLockSpec {
    pub fn new(path: PathBuf, stale: Duration, acquire_timeout: Duration) -> Self {
        Self {
            path,
            stale,
            acquire_timeout,
            touch_every: Duration::from_secs(3),
            cancel: Cancel::new(),
        }
    }

    /// The same lock, waited for under `cancel`.
    pub fn with_cancel(self, cancel: &Cancel) -> Self {
        Self {
            cancel: cancel.clone(),
            ..self
        }
    }
}

pub(crate) fn set_dir_mtime(path: &Path, t: SystemTime) -> io::Result<SystemTime> {
    File::open(path)?.set_modified(t)?;
    fs::metadata(path)?.modified()
}

struct State {
    last_set: Mutex<SystemTime>,
    compromised: AtomicBool,
    stop: Mutex<bool>,
    wake: Condvar,
}

/// CC's `proper-lockfile` protocol (§9.1): `mkdir` acquires, a stale mtime may be taken
/// over, a heartbeat touches the mtime, and ownership is re-checked before every protected
/// write and before release.
pub struct MkdirLock {
    path: PathBuf,
    state: Arc<State>,
    heartbeat: Option<JoinHandle<()>>,
}

impl MkdirLock {
    pub fn try_acquire(spec: &MkdirLockSpec) -> Result<Option<Self>, LockError> {
        for _ in 0..2 {
            match fs::create_dir(&spec.path) {
                Ok(()) => return Self::start(spec).map(Some),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    match fs::metadata(&spec.path).and_then(|m| m.modified()) {
                        Ok(mtime) => {
                            let age = SystemTime::now().duration_since(mtime).unwrap_or_default();
                            if age <= spec.stale {
                                return Ok(None);
                            }
                            // Stale: take it over. `NotFound` just means another taker won
                            // the race; any other failure (`ENOTEMPTY`, `EACCES`, ...) is a
                            // real fault and must not be swallowed into a spin-to-timeout.
                            match fs::remove_dir(&spec.path) {
                                Ok(()) => {}
                                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                                Err(e) => return Err(e.into()),
                            }
                        }
                        Err(e) if e.kind() == io::ErrorKind::NotFound => {} // vanished: retry
                        Err(e) => return Err(e.into()),
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    // The directory that holds the lock does not exist yet. Create only its
                    // immediate parent, non-recursively: this path can sit under CC-owned
                    // state (e.g. `CLAUDE_SECURESTORAGE_CONFIG_DIR`), and a typo'd env var
                    // must fail loudly instead of growing a whole directory chain there.
                    if let Some(parent) = spec.path.parent() {
                        match fs::DirBuilder::new().mode(0o700).create(parent) {
                            Ok(()) => {}
                            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                            Err(e) => return Err(e.into()),
                        }
                    }
                }
                Err(e) => return Err(e.into()),
            }
        }
        Ok(None)
    }

    /// Waits up to `acquire_timeout`, polling every 250–500 ms. `spec.cancel` is checked before
    /// every attempt (§14.1); `try_acquire`, one attempt and not a wait, never checks it.
    pub fn acquire(spec: &MkdirLockSpec) -> Result<Self, LockError> {
        let deadline = Instant::now() + spec.acquire_timeout;
        loop {
            check_cancel(&spec.cancel, &spec.path)?;
            if let Some(lock) = Self::try_acquire(spec)? {
                return Ok(lock);
            }
            if Instant::now() >= deadline {
                return Err(LockError::Timeout(spec.path.clone()));
            }
            thread::sleep(Duration::from_millis(fastrand::u64(250..=500)));
        }
    }

    fn start(spec: &MkdirLockSpec) -> Result<Self, LockError> {
        let result = (|| -> Result<Self, LockError> {
            let set = set_dir_mtime(&spec.path, SystemTime::now())?;
            let state = Arc::new(State {
                last_set: Mutex::new(set),
                compromised: AtomicBool::new(false),
                stop: Mutex::new(false),
                wake: Condvar::new(),
            });
            let (st, path, every) = (state.clone(), spec.path.clone(), spec.touch_every);
            // `Builder::spawn` (not `thread::spawn`) so a failure to spawn is an error we
            // can clean up after, not a panic.
            let heartbeat = thread::Builder::new().spawn(move || {
                let mut stop = st.stop.lock().unwrap();
                loop {
                    // The standard condvar predicate loop: `wait_timeout_while` checks `*stop`
                    // immediately, under the same lock `Drop` sets it under, before ever
                    // blocking. Without that check-before-wait, a `Drop` that sets `stop` and
                    // notifies before this thread reaches its first wait is lost entirely — the
                    // notify has nothing waiting to wake, and the plain `wait_timeout` used to
                    // block regardless for the full `every`, even though `*stop` was already
                    // true by the time it acquired the lock.
                    let (guard, _) = st.wake.wait_timeout_while(stop, every, |s| !*s).unwrap();
                    stop = guard;
                    if *stop {
                        return;
                    }
                    // Check and touch as one step under the mutex, so a concurrent check
                    // never compares a stale timestamp with the heartbeat's fresh one.
                    let mut last = st.last_set.lock().unwrap();
                    if check_with(&path, &st, *last).is_ok() {
                        match set_dir_mtime(&path, SystemTime::now()) {
                            Ok(t) => *last = t,
                            Err(_) => st.compromised.store(true, Ordering::SeqCst),
                        }
                    }
                }
            })?;
            Ok(Self {
                path: spec.path.clone(),
                state,
                heartbeat: Some(heartbeat),
            })
        })();
        if result.is_err() {
            // `mkdir` already succeeded but the lock never actually started: remove it
            // now, rather than blocking CC for the whole staleness window.
            let _ = fs::remove_dir(&spec.path);
        }
        result
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_compromised(&self) -> bool {
        self.state.compromised.load(Ordering::SeqCst)
    }

    /// Synchronous ownership check (§9.1): the directory must still carry the mtime this
    /// holder last set. A failure marks the guard compromised for good.
    pub fn check_owned(&self) -> Result<(), LockError> {
        check(&self.path, &self.state)
    }
}

fn check(path: &Path, st: &State) -> Result<(), LockError> {
    let last = st.last_set.lock().unwrap(); // held across the stat
    check_with(path, st, *last)
}

fn check_with(path: &Path, st: &State, last: SystemTime) -> Result<(), LockError> {
    if st.compromised.load(Ordering::SeqCst) {
        return Err(LockError::Compromised(path.to_path_buf()));
    }
    match fs::metadata(path).and_then(|m| m.modified()) {
        Ok(m) if m == last => Ok(()),
        _ => {
            st.compromised.store(true, Ordering::SeqCst);
            Err(LockError::Compromised(path.to_path_buf()))
        }
    }
}

impl Drop for MkdirLock {
    fn drop(&mut self) {
        *self.state.stop.lock().unwrap() = true;
        self.state.wake.notify_all();
        if let Some(h) = self.heartbeat.take() {
            let _ = h.join();
        }
        if self.check_owned().is_ok() {
            let _ = fs::remove_dir(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::SystemTime;

    fn spec(dir: &Path, stale_ms: u64, timeout_ms: u64, touch_ms: u64) -> MkdirLockSpec {
        MkdirLockSpec {
            path: dir.join("x.lock"),
            stale: Duration::from_millis(stale_ms),
            acquire_timeout: Duration::from_millis(timeout_ms),
            touch_every: Duration::from_millis(touch_ms),
            cancel: Cancel::new(),
        }
    }

    /// Sets `cancel` from another thread 200 ms from now, as a signal handler would, and
    /// returns the instant just before it did.
    fn interrupt_soon(cancel: &Cancel, signal: i32) -> std::thread::JoinHandle<Instant> {
        let cancel = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            let at = Instant::now();
            cancel.request(signal);
            at
        })
    }

    #[test]
    fn a_set_token_ends_the_wait_before_its_first_attempt() {
        let d = tempfile::tempdir().unwrap();
        let cancel = Cancel::new();
        cancel.request(2);
        let s = spec(d.path(), 60_000, 5_000, 3_000).with_cancel(&cancel);
        let start = Instant::now();
        match MkdirLock::acquire(&s) {
            Err(LockError::Interrupted { path, signal }) => {
                assert_eq!((path, signal), (s.path.clone(), 2))
            }
            Err(e) => panic!("expected an interrupted wait, got {e:?}"),
            Ok(_) => panic!("a set token took the lock"),
        }
        assert!(
            start.elapsed() < Duration::from_millis(100),
            "no poll was waited"
        );
        assert!(
            !s.path.exists(),
            "no attempt was made, so nothing was taken"
        );
    }

    #[test]
    fn an_unset_token_waits_and_acquires_as_before() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 60_000, 600, 3_000).with_cancel(&Cancel::new());
        let held = MkdirLock::acquire(&s).unwrap();
        assert!(matches!(MkdirLock::acquire(&s), Err(LockError::Timeout(_))));
        drop(held);
        assert!(MkdirLock::acquire(&s).is_ok());
    }

    #[test]
    fn a_token_set_while_another_holder_keeps_the_lock_ends_the_wait_within_one_poll() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 60_000, 30_000, 3_000);
        let holder = MkdirLock::acquire(&s).unwrap();
        let cancel = Cancel::new();
        let setter = interrupt_soon(&cancel, 15);
        let result = MkdirLock::acquire(&s.clone().with_cancel(&cancel));
        let ended = Instant::now();
        let set_at = setter.join().unwrap();
        assert!(
            matches!(&result, Err(LockError::Interrupted { path, signal: 15 }) if *path == s.path),
            "{:?}",
            result.as_ref().err()
        );
        assert!(ended >= set_at, "the wait ended before the token was set");
        // One jittered poll is at most 500 ms; the timeout is 30 s.
        assert!(
            ended - set_at < Duration::from_secs(1),
            "{:?}",
            ended - set_at
        );
        assert!(
            holder.check_owned().is_ok(),
            "the holder's lock is untouched"
        );
    }

    #[test]
    fn only_an_interrupted_wait_carries_a_signal() {
        let p = PathBuf::from("/x.lock");
        let e = LockError::Interrupted {
            path: p.clone(),
            signal: 1,
        };
        assert_eq!(e.signal(), Some(1));
        assert_eq!(
            e.to_string(),
            "interrupted while waiting for the lock /x.lock"
        );
        assert_eq!(LockError::Timeout(p.clone()).signal(), None);
        assert_eq!(LockError::Compromised(p).signal(), None);
        assert_eq!(LockError::Io(io::Error::other("x")).signal(), None);
    }

    #[test]
    fn acquire_creates_and_drop_removes() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 60_000, 100, 3_000);
        let l = MkdirLock::acquire(&s).unwrap();
        assert!(s.path.is_dir());
        drop(l);
        assert!(!s.path.exists());
    }

    /// A drop that races the heartbeat thread's very first wait must never miss the stop
    /// notification: if the notify happens before the thread reaches its first `wait_timeout`,
    /// the thread must still see `stop` already set instead of blocking for a whole
    /// `touch_every` regardless.
    #[test]
    fn dropping_a_freshly_acquired_lock_never_stalls_on_the_heartbeat() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 60_000, 100, 3_000);
        for i in 0..5 {
            let l = MkdirLock::acquire(&s).unwrap();
            let start = Instant::now();
            drop(l);
            let elapsed = start.elapsed();
            assert!(
                elapsed < Duration::from_millis(500),
                "drop {i} took {elapsed:?}: the heartbeat thread missed the stop notification \
                 and blocked for close to touch_every instead"
            );
        }
    }

    #[test]
    fn a_held_lock_times_out() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 60_000, 600, 3_000);
        let _held = MkdirLock::acquire(&s).unwrap();
        assert!(matches!(MkdirLock::acquire(&s), Err(LockError::Timeout(_))));
        assert!(MkdirLock::try_acquire(&s).unwrap().is_none());
    }

    #[test]
    fn a_stale_lock_is_taken_over() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 1_000, 100, 3_000);
        fs::create_dir(&s.path).unwrap();
        set_dir_mtime(&s.path, SystemTime::now() - Duration::from_secs(5)).unwrap();
        assert!(MkdirLock::acquire(&s).is_ok());
    }

    #[test]
    fn a_missing_immediate_parent_is_created_non_recursively_and_the_lock_is_taken() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(&d.path().join("sub"), 60_000, 100, 3_000);
        let l = MkdirLock::acquire(&s).unwrap();
        assert!(s.path.is_dir());
        let parent = s.path.parent().unwrap();
        assert_eq!(
            fs::metadata(parent).unwrap().permissions().mode() & 0o777,
            0o700
        );
        drop(l);
    }

    #[test]
    fn a_missing_grandparent_fails_and_creates_nothing() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(&d.path().join("a/b"), 60_000, 100, 3_000);
        assert!(matches!(MkdirLock::acquire(&s), Err(LockError::Io(_))));
        assert!(!d.path().join("a").exists(), "no directory was created");
    }

    #[test]
    fn a_non_removable_stale_lock_reports_io_promptly_not_timeout() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 1_000, 5_000, 3_000);
        fs::create_dir(&s.path).unwrap();
        fs::write(s.path.join("busy"), b"x").unwrap(); // non-empty: remove_dir fails
        set_dir_mtime(&s.path, SystemTime::now() - Duration::from_secs(5)).unwrap();
        let start = Instant::now();
        assert!(matches!(MkdirLock::acquire(&s), Err(LockError::Io(_))));
        assert!(
            start.elapsed() < Duration::from_millis(1_000),
            "must fail immediately on the real fault, not spin to the {:?} acquire timeout",
            s.acquire_timeout
        );
    }

    #[test]
    fn the_heartbeat_keeps_the_mtime_fresh() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 60_000, 100, 20);
        let l = MkdirLock::acquire(&s).unwrap();
        let before = fs::metadata(&s.path).unwrap().modified().unwrap();
        std::thread::sleep(Duration::from_millis(150));
        let after = fs::metadata(&s.path).unwrap().modified().unwrap();
        assert!(after > before);
        assert!(l.check_owned().is_ok());
    }

    #[test]
    fn checks_racing_the_heartbeat_never_see_a_false_takeover() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 60_000, 100, 1);
        let l = MkdirLock::acquire(&s).unwrap();
        for _ in 0..2_000 {
            l.check_owned().unwrap();
        }
        assert!(!l.is_compromised());
    }

    #[test]
    fn the_heartbeat_notices_an_external_takeover() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 60_000, 100, 20);
        let l = MkdirLock::acquire(&s).unwrap();
        // Another process replaced the lock: its mtime is no longer the one we set.
        set_dir_mtime(&s.path, SystemTime::now() - Duration::from_secs(30)).unwrap();
        std::thread::sleep(Duration::from_millis(150));
        assert!(l.is_compromised());
        drop(l);
        assert!(
            s.path.is_dir(),
            "a compromised guard must not remove the directory"
        );
    }

    #[test]
    fn a_suspended_holder_detects_the_takeover_and_leaves_the_new_lock() {
        let d = tempfile::tempdir().unwrap();
        // Heartbeat far in the future: models a holder that was suspended.
        let s = spec(d.path(), 200, 1_000, 3_600_000);
        let first = MkdirLock::acquire(&s).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let second = MkdirLock::acquire(&s).unwrap();
        assert!(matches!(
            first.check_owned(),
            Err(LockError::Compromised(_))
        ));
        drop(first);
        assert!(
            s.path.is_dir(),
            "the resumed holder removed its replacement's lock"
        );
        drop(second);
        assert!(!s.path.exists());
    }

    #[test]
    fn a_panic_releases_the_lock() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 60_000, 100, 3_000);
        let r = std::panic::catch_unwind(|| {
            let _l = MkdirLock::acquire(&s).unwrap();
            panic!("boom");
        });
        assert!(r.is_err());
        assert!(!s.path.exists());
    }
}
