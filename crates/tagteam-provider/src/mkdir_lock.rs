use std::fs::{self, File};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use crate::cancel::Cancel;

#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error("{}", timeout_text(.0))]
    Timeout(PathBuf),
    #[error("the lock {0} was taken over while held")]
    Compromised(PathBuf),
    #[error("lock I/O failed: {0}")]
    Io(#[from] io::Error),
    /// The cancel token was set while waiting (§14.1). The wait took nothing.
    #[error("interrupted while waiting for the lock {path}")]
    Interrupted { path: PathBuf, signal: i32 },
}

/// A timeout's message. CC's config lock (`<global config>.lock`) may be one CC left behind
/// after a short command, which frees itself once stale (10 s, plus up to 1 s that its mtime
/// can lie in the future, §9.1), so its message says so.
fn timeout_text(path: &Path) -> String {
    let base = format!("timed out waiting for the lock {}", path.display());
    let config = path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(".json.lock"));
    if config {
        format!(
            "{base}; Claude Code may have left it behind after a short command, and it frees itself within about 11 s, so retry"
        )
    } else {
        base
    }
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

/// Sets the mtime through `dir`, the held directory fd (`futimens`), and reads back what the
/// filesystem stored through the same fd. Never by path (§9.1, L356).
fn touch(dir: &File, t: SystemTime) -> io::Result<SystemTime> {
    dir.set_modified(t)?;
    dir.metadata()?.modified()
}

/// How long `mkdir` may take to become an open fd before the directory opened can no longer be
/// trusted to be the one `mkdir` made: four fifths of the staleness. Within it the directory
/// cannot have looked stale to anyone, so nobody could have taken it over. The fifth held back
/// covers a taker whose filesystem keeps mtimes to the second, which can make the directory
/// look up to 1 s older: 2 s for CC's 10 s config lock, 12 s for its 60 s credential locks.
fn trusted_span(stale: Duration) -> Duration {
    stale - stale / 5
}

/// Removes the lock directory at `path` only while it is still the one this holder made: the
/// device and inode of its held fd, and, once touched, the mtime it set (§9.1). For a start that
/// fails after the directory was established as its own; the caller still holds the fd, so the
/// inode cannot have been reused. A directory that cannot be removed holds the lock until it
/// goes stale: logged at WARN with its cause (§14), as the caller reports its own failure.
fn remove_if_ours(path: &Path, id: DirId, set: Option<SystemTime>) {
    let ours = fs::symlink_metadata(path)
        .is_ok_and(|m| DirId::of(&m) == id && set.is_none_or(|t| m.modified().ok() == Some(t)));
    if ours {
        warn_if_left(path, fs::remove_dir(path));
    }
}

/// §14: a lock directory this holder made and could not remove is a contained error, logged
/// at WARN with its cause; Claude Code waits on it until it goes stale (§9.1). One already
/// gone was not left.
fn warn_if_left(path: &Path, removed: io::Result<()>) {
    match removed {
        Err(e) if e.kind() != io::ErrorKind::NotFound => tracing::warn!(
            "could not remove {}, which this process made, so it holds the lock until it goes stale: {e}",
            lock_role(path)
        ),
        _ => {}
    }
}

/// Which lock a directory is, by the name Claude Code gives each of its locks (§9.1): what a
/// log line names instead of the path. The path is under a Claude Code home, which the user may
/// have named after themselves (`CLAUDE_CONFIG_DIR`), and §14.2 keeps such names out of the log.
fn lock_role(path: &Path) -> &'static str {
    match path.file_name().and_then(|n| n.to_str()) {
        Some(".oauth_refresh.lock") => "Claude Code's refresh lock",
        Some(".storage-write" | ".storage-write.lock") => "Claude Code's storage-write lock",
        Some(".claude.json.lock") => "Claude Code's config lock",
        Some(name) if name.ends_with(".lock") => "Claude Code's credential lock",
        _ => "a lock directory",
    }
}

/// A directory's identity: the device and inode the lock path must still resolve to (§9.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DirId {
    dev: u64,
    ino: u64,
}

impl DirId {
    fn of(m: &fs::Metadata) -> Self {
        Self {
            dev: m.dev(),
            ino: m.ino(),
        }
    }
}

struct State {
    /// The directory this holder created, open since just after `mkdir`. Every touch goes
    /// through it, so a heartbeat never reaches a directory that replaced it; holding it also
    /// keeps its inode allocated, so no replacement can reuse the number.
    dir: File,
    /// `dir`'s device and inode, read once when the lock was taken.
    id: DirId,
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
    /// One attempt. `None` when the lock is held, or when this attempt stalled so long between
    /// `mkdir` and opening the directory that the directory may no longer be its own (`start`).
    pub fn try_acquire(spec: &MkdirLockSpec) -> Result<Option<Self>, LockError> {
        for _ in 0..2 {
            // Wall-clock time, as staleness is judged: a suspension counts (§9.1).
            let made_at = SystemTime::now();
            match fs::create_dir(&spec.path) {
                Ok(()) => return Self::start(spec, made_at),
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

    /// Waits up to `acquire_timeout`, polling every 250-500 ms, until the lock directory is
    /// absent or stale, and takes nothing (§9.1's pre-wait). Staleness is judged as
    /// `try_acquire` judges it, so a lock whose mtime lies in the future reads as age 0.
    /// `spec.cancel` is checked before every look (§14.1).
    pub fn wait_idle(spec: &MkdirLockSpec) -> Result<(), LockError> {
        let deadline = Instant::now() + spec.acquire_timeout;
        loop {
            check_cancel(&spec.cancel, &spec.path)?;
            match fs::metadata(&spec.path).and_then(|m| m.modified()) {
                Ok(mtime) => {
                    let age = SystemTime::now().duration_since(mtime).unwrap_or_default();
                    if age > spec.stale {
                        return Ok(());
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
                Err(e) => return Err(e.into()),
            }
            if Instant::now() >= deadline {
                return Err(LockError::Timeout(spec.path.clone()));
            }
            thread::sleep(Duration::from_millis(fastrand::u64(250..=500)));
        }
    }

    /// Takes the directory `mkdir` made at `made_at` as this holder's: opens it, records its
    /// identity, touches it and starts the heartbeat. `None`, touching and removing nothing,
    /// when the directory may have been replaced before it was opened (`trusted_span`): the
    /// attempt then counts as contention, and the staleness rule settles whose it is.
    fn start(spec: &MkdirLockSpec, made_at: SystemTime) -> Result<Option<Self>, LockError> {
        // Until the directory is open and known to be the one `mkdir` made, it may already be
        // another holder's: a failure here leaves the path alone.
        #[cfg(test)]
        seam::run(&spec.path, seam::Point::AfterMkdir)?;
        let dir = File::open(&spec.path)?;
        let id = DirId::of(&dir.metadata()?);
        // A clock set back reads as no time spent; it makes the directory look younger
        // to every taker too.
        let spent = SystemTime::now()
            .duration_since(made_at)
            .unwrap_or_default();
        if spent >= trusted_span(spec.stale) {
            return Ok(None);
        }
        // Ours from here. A failure removes the directory rather than block CC for the whole
        // staleness window, but only while the path still names it (`remove_if_ours`).
        let set =
            touch(&dir, SystemTime::now()).inspect_err(|_| remove_if_ours(&spec.path, id, None))?;
        let state = Arc::new(State {
            dir,
            id,
            last_set: Mutex::new(set),
            compromised: AtomicBool::new(false),
            stop: Mutex::new(false),
            wake: Condvar::new(),
        });
        let abandon = |e: io::Error| {
            remove_if_ours(&spec.path, id, Some(set));
            LockError::from(e)
        };
        #[cfg(test)]
        seam::run(&spec.path, seam::Point::BeforeHeartbeat).map_err(abandon)?;
        let (st, path, every) = (state.clone(), spec.path.clone(), spec.touch_every);
        // `Builder::spawn` (not `thread::spawn`) so a failure to spawn is an error we
        // can clean up after, not a panic.
        let heartbeat = thread::Builder::new()
            .spawn(move || {
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
                    #[cfg(test)]
                    let _ = seam::run(&path, seam::Point::BeatBeforeCheck);
                    if check_with(&path, &st, *last).is_ok() {
                        #[cfg(test)]
                        let _ = seam::run(&path, seam::Point::BeatBeforeTouch);
                        // Through the held fd: a directory that replaced ours between the check
                        // and this touch is never touched (L356).
                        match touch(&st.dir, SystemTime::now()) {
                            Ok(t) => *last = t,
                            Err(e) => {
                                // §14: the holder's next check reports the loss, not its cause.
                                tracing::warn!(
                                    "could not touch {}, which this process holds, so the lock counts as lost: {e}",
                                    lock_role(&path)
                                );
                                st.compromised.store(true, Ordering::SeqCst);
                            }
                        }
                    }
                }
            })
            .map_err(abandon)?;
        Ok(Some(Self {
            path: spec.path.clone(),
            state,
            heartbeat: Some(heartbeat),
        }))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_compromised(&self) -> bool {
        self.state.compromised.load(Ordering::SeqCst)
    }

    /// Synchronous ownership check (§9.1): the path must still name the directory this holder
    /// created (the device and inode of the held fd) and that directory must still carry the
    /// mtime this holder last set. A failure marks the guard compromised for good.
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
    // `symlink_metadata`: the path itself must be the directory, never a link to one.
    match fs::symlink_metadata(path) {
        Ok(m) if DirId::of(&m) == st.id && m.modified().ok() == Some(last) => Ok(()),
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
        // The check and the `rmdir` are two steps: a takeover between them needs this holder
        // to stall for the whole staleness window inside that gap, as with `proper-lockfile`.
        if self.check_owned().is_ok() {
            warn_if_left(&self.path, fs::remove_dir(&self.path));
        }
    }
}

/// Test-only seams in the lock's protocol: a test runs code at a named point, for one lock path
/// only, so tests running in parallel never meet each other's hooks.
#[cfg(test)]
mod seam {
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) enum Point {
        /// `mkdir` made the directory; it has not been opened yet.
        AfterMkdir,
        /// The directory is the holder's and touched; the heartbeat has not been started.
        BeforeHeartbeat,
        /// A heartbeat woke and holds the timestamp mutex; its ownership check has not run.
        BeatBeforeCheck,
        /// A heartbeat's ownership check passed; its touch has not run.
        BeatBeforeTouch,
    }

    type Hook = Arc<dyn Fn() -> io::Result<()> + Send + Sync>;

    static HOOKS: Mutex<Vec<(PathBuf, Point, Hook)>> = Mutex::new(Vec::new());

    /// Runs `hook` each time the lock at `path` passes `point`. A heartbeat hook runs with the
    /// timestamp mutex held, so it must never call `check_owned` on that lock.
    pub(super) fn set(
        path: &Path,
        point: Point,
        hook: impl Fn() -> io::Result<()> + Send + Sync + 'static,
    ) {
        HOOKS
            .lock()
            .unwrap()
            .push((path.to_path_buf(), point, Arc::new(hook)));
    }

    /// Runs the hooks set for `path` at `point`, in the order they were set. The first error
    /// is returned as the failure of the step at that point; the heartbeat's points ignore it.
    pub(super) fn run(path: &Path, point: Point) -> io::Result<()> {
        let hooks: Vec<Hook> = HOOKS
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, at, _)| p == path && *at == point)
            .map(|(_, _, hook)| hook.clone())
            .collect();
        for hook in hooks {
            hook()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::sync::mpsc;
    use std::time::SystemTime;

    use super::seam::Point;

    /// Sets a directory's mtime by path, as another process would.
    fn set_dir_mtime(path: &Path, t: SystemTime) -> io::Result<SystemTime> {
        File::open(path)?.set_modified(t)?;
        fs::metadata(path)?.modified()
    }

    /// Replaces the directory at `path` with a new one that carries `mtime`: another process's
    /// takeover. Carrying the holder's own mtime, it is what the old touch-by-path left after
    /// adopting a replacement (L356).
    fn replace_with_mtime(path: &Path, mtime: SystemTime) {
        fs::remove_dir(path).unwrap();
        fs::create_dir(path).unwrap();
        assert_eq!(set_dir_mtime(path, mtime).unwrap(), mtime);
    }

    fn mtime(path: &Path) -> SystemTime {
        fs::metadata(path).unwrap().modified().unwrap()
    }

    /// Runs `hook` the first time the lock at `path` passes `point`, then sends what it returns.
    fn once_at<T: Send + 'static>(
        path: &Path,
        point: Point,
        hook: impl Fn() -> T + Send + Sync + 'static,
    ) -> mpsc::Receiver<T> {
        let (tx, rx) = mpsc::channel();
        let fired = AtomicBool::new(false);
        seam::set(path, point, move || {
            if !fired.swap(true, Ordering::SeqCst) {
                let _ = tx.send(hook());
            }
            Ok(())
        });
        rx
    }

    /// Makes the step at `point` fail for the lock at `path`, after the hooks set before this.
    fn fail_at(path: &Path, point: Point) {
        seam::set(path, point, || Err(io::Error::other("injected failure")));
    }

    /// The `tracing` lines `f` logs on this thread, one per event, level first.
    fn logged(f: impl FnOnce()) -> Vec<String> {
        #[derive(Clone, Default)]
        struct Lines(Arc<Mutex<Vec<u8>>>);
        impl io::Write for Lines {
            fn write(&mut self, b: &[u8]) -> io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Lines {
            type Writer = Lines;
            fn make_writer(&'a self) -> Self::Writer {
                self.clone()
            }
        }
        let lines = Lines::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(lines.clone())
            .with_ansi(false)
            .without_time()
            .finish();
        tracing::subscriber::with_default(subscriber, f);
        let text = String::from_utf8(lines.0.lock().unwrap().clone()).unwrap();
        text.lines().map(str::to_owned).collect()
    }

    /// A directory made read-only, so no entry in it can be removed (as a non-root user), until
    /// this is dropped.
    struct ReadOnly(PathBuf);

    impl ReadOnly {
        fn new(dir: &Path) -> Self {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o500)).unwrap();
            Self(dir.to_path_buf())
        }
    }

    impl Drop for ReadOnly {
        fn drop(&mut self) {
            fs::set_permissions(&self.0, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }

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
    fn wait_idle_returns_at_once_for_an_absent_lock_and_takes_nothing() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 60_000, 100, 3_000);
        MkdirLock::wait_idle(&s).unwrap();
        assert!(!s.path.exists());
    }

    #[test]
    fn wait_idle_outlasts_a_lock_with_a_future_mtime_and_leaves_it() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 1_000, 5_000, 3_000);
        fs::create_dir(&s.path).unwrap();
        set_dir_mtime(&s.path, SystemTime::now() + Duration::from_secs(1)).unwrap();
        let start = Instant::now();
        MkdirLock::wait_idle(&s).unwrap();
        assert!(
            start.elapsed() >= Duration::from_millis(1_900),
            "returned after {:?}, before the lock went stale",
            start.elapsed()
        );
        assert!(s.path.is_dir(), "the pre-wait takes nothing over");
    }

    #[test]
    fn wait_idle_times_out_on_a_lock_that_stays_fresh() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 60_000, 600, 3_000);
        fs::create_dir(&s.path).unwrap();
        assert!(matches!(
            MkdirLock::wait_idle(&s),
            Err(LockError::Timeout(p)) if p == s.path
        ));
    }

    #[test]
    fn wait_idle_is_a_cancellation_point() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 60_000, 30_000, 3_000);
        fs::create_dir(&s.path).unwrap();
        s.cancel.request(15);
        assert_eq!(MkdirLock::wait_idle(&s).unwrap_err().signal(), Some(15));
    }

    #[test]
    fn a_config_lock_timeout_says_it_frees_itself_and_other_locks_do_not() {
        let config = LockError::Timeout(PathBuf::from("/h/.claude.json.lock")).to_string();
        assert_eq!(
            config,
            "timed out waiting for the lock /h/.claude.json.lock; Claude Code may have left it behind after a short command, and it frees itself within about 11 s, so retry"
        );
        assert_eq!(
            LockError::Timeout(PathBuf::from("/h/.oauth_refresh.lock")).to_string(),
            "timed out waiting for the lock /h/.oauth_refresh.lock"
        );
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

    /// L356, Review Focus 5: a holder suspended past the staleness window resumes to find its
    /// directory replaced, and the replacement carries the very mtime it last set. Only the
    /// device and inode tell them apart. Its check fails, so the write it protects is aborted,
    /// and it never removes the replacement.
    #[test]
    fn a_replacement_carrying_the_holders_mtime_is_still_not_its_own() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 60_000, 100, 3_600_000); // heartbeat parked: suspended
        let held = MkdirLock::acquire(&s).unwrap();
        let ours = mtime(&s.path);
        replace_with_mtime(&s.path, ours);

        assert!(matches!(held.check_owned(), Err(LockError::Compromised(_))));
        drop(held);
        assert!(
            s.path.is_dir(),
            "the resumed holder removed its replacement"
        );
        assert_eq!(mtime(&s.path), ours);
    }

    /// L356: a beat's own check refuses a directory that replaced the holder's before the beat,
    /// even one carrying the holder's mtime, so the beat never touches it.
    #[test]
    fn the_heartbeat_never_touches_a_directory_that_replaced_its_own() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 60_000, 100, 20);
        let path = s.path.clone();
        let swapped = once_at(&s.path, Point::BeatBeforeCheck, move || {
            let ours = mtime(&path);
            replace_with_mtime(&path, ours);
            ours
        });
        let held = MkdirLock::acquire(&s).unwrap();

        let ours = swapped.recv_timeout(Duration::from_secs(10)).unwrap();
        // The beat holds the timestamp mutex from its check through its touch, so this check
        // runs after the beat has finished.
        let checked = held.check_owned();
        assert_eq!(mtime(&s.path), ours, "the beat touched the replacement");
        assert!(matches!(checked, Err(LockError::Compromised(_))));
        assert!(held.is_compromised());
        drop(held);
        assert!(
            s.path.is_dir(),
            "a compromised guard leaves the directory alone"
        );
    }

    /// L356 itself: the beat's check passed, and the directory is replaced before its touch.
    /// The touch goes through the fd held since `mkdir`, so it reaches the holder's own
    /// (now unlinked) directory and never the replacement; the next check refuses.
    #[test]
    fn a_directory_replaced_between_the_beats_check_and_its_touch_is_never_touched() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 60_000, 100, 20);
        let path = s.path.clone();
        let swapped = once_at(&s.path, Point::BeatBeforeTouch, move || {
            let ours = mtime(&path);
            replace_with_mtime(&path, ours);
            ours
        });
        let held = MkdirLock::acquire(&s).unwrap();

        let ours = swapped.recv_timeout(Duration::from_secs(10)).unwrap();
        let checked = held.check_owned(); // after the beat's touch, as above
        assert_eq!(mtime(&s.path), ours, "the beat touched the replacement");
        assert!(matches!(checked, Err(LockError::Compromised(_))));
        drop(held);
        assert!(
            s.path.is_dir(),
            "a compromised guard leaves the directory alone"
        );
    }

    /// A holder suspended between its `mkdir` and opening the directory, long enough for the
    /// directory to look stale, may find another holder's directory there on resume. It never
    /// adopts, touches or removes it: the attempt counts as contention.
    #[test]
    fn a_stall_between_mkdir_and_open_never_adopts_the_directory_found_after_it() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 200, 0, 3_000); // stale after 200 ms; one attempt
        let path = s.path.clone();
        let theirs = once_at(&s.path, Point::AfterMkdir, move || {
            std::thread::sleep(Duration::from_millis(250)); // suspended past the staleness
            fs::remove_dir(&path).unwrap(); // CC took it over
            fs::create_dir(&path).unwrap();
            let m = fs::metadata(&path).unwrap();
            (m.ino(), m.modified().unwrap())
        });

        match MkdirLock::acquire(&s) {
            Err(LockError::Timeout(_)) => {}
            Err(e) => panic!("expected contention, got {e:?}"),
            Ok(_) => panic!("the holder adopted the directory that replaced its own"),
        }
        let (ino, modified) = theirs.try_recv().unwrap();
        let now = fs::metadata(&s.path).unwrap();
        assert_eq!(
            (now.ino(), now.modified().unwrap()),
            (ino, modified),
            "CC's directory is neither touched nor removed"
        );
    }

    /// The same stall with no takeover leaves the holder's own directory as a contended lock.
    /// It goes stale like any other and the retry takes it over (§9.1), within the deadline.
    #[test]
    fn a_stall_with_no_takeover_is_retried_and_the_lock_taken() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 200, 5_000, 3_000);
        let stalled = once_at(&s.path, Point::AfterMkdir, || {
            std::thread::sleep(Duration::from_millis(250));
        });

        let held = MkdirLock::acquire(&s).unwrap();
        stalled.try_recv().unwrap();
        assert!(held.check_owned().is_ok());
        drop(held);
        assert!(!s.path.exists());
    }

    /// A start whose open fails after `mkdir` never removes the path: by then it may name
    /// another holder's directory. Here CC removed the stale directory, the open found nothing,
    /// and CC made its own before the failure unwound.
    #[test]
    fn a_failed_open_after_mkdir_never_removes_the_directory_found_there() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 60_000, 0, 3_000);
        let path = s.path.clone();
        let theirs = once_at(&s.path, Point::AfterMkdir, move || {
            fs::remove_dir(&path).unwrap();
            fs::create_dir(&path).unwrap();
            let m = fs::metadata(&path).unwrap();
            (m.ino(), m.modified().unwrap())
        });
        fail_at(&s.path, Point::AfterMkdir); // the open fails

        assert!(matches!(MkdirLock::acquire(&s), Err(LockError::Io(_))));
        let (ino, modified) = theirs.try_recv().unwrap();
        assert!(s.path.is_dir(), "the failed start removed CC's directory");
        let now = fs::metadata(&s.path).unwrap();
        assert_eq!(
            (now.ino(), now.modified().unwrap()),
            (ino, modified),
            "CC's directory is neither touched nor removed"
        );
    }

    /// A start that fails once the directory is its own removes it, rather than leave CC
    /// blocked for the whole staleness window.
    #[test]
    fn a_start_that_fails_after_taking_its_directory_removes_it() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 60_000, 0, 3_000);
        fail_at(&s.path, Point::BeforeHeartbeat);

        assert!(matches!(MkdirLock::acquire(&s), Err(LockError::Io(_))));
        assert!(!s.path.exists());
    }

    /// The same failure after a takeover removes nothing: the cleanup is fenced by the held
    /// fd's device and inode and by the mtime the holder set.
    #[test]
    fn a_start_that_fails_after_a_takeover_leaves_the_replacement() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 60_000, 0, 3_000);
        let path = s.path.clone();
        let theirs = once_at(&s.path, Point::BeforeHeartbeat, move || {
            let ours = mtime(&path);
            replace_with_mtime(&path, ours);
            fs::metadata(&path).unwrap().ino()
        });
        fail_at(&s.path, Point::BeforeHeartbeat);

        assert!(matches!(MkdirLock::acquire(&s), Err(LockError::Io(_))));
        let ino = theirs.try_recv().unwrap();
        assert!(s.path.is_dir(), "the failed start removed the replacement");
        assert_eq!(fs::metadata(&s.path).unwrap().ino(), ino);
    }

    #[test]
    fn a_released_lock_directory_that_cannot_be_removed_is_logged_with_its_cause() {
        // §14: a contained error is logged, never discarded. The directory holds the lock until
        // it goes stale, so Claude Code waits on it; the line says why.
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 60_000, 100, 3_000);
        let lock = MkdirLock::acquire(&s).unwrap();
        let _restore = ReadOnly::new(d.path());
        let logs = logged(|| drop(lock));
        assert!(s.path.is_dir());
        let warnings: Vec<&String> = logs.iter().filter(|l| l.contains("WARN")).collect();
        assert_eq!(warnings.len(), 1, "{logs:?}");
        assert!(
            warnings[0].contains("could not remove Claude Code's credential lock")
                && warnings[0].contains("ermission denied"),
            "{}",
            warnings[0]
        );
        // The role, never the path: a Claude Code home may carry its user's name (§14.2).
        assert!(!warnings[0].contains(&d.path().display().to_string()));
        assert_eq!(
            lock_role(Path::new("/h/.oauth_refresh.lock")),
            "Claude Code's refresh lock"
        );
        assert_eq!(
            lock_role(Path::new("/h/.storage-write")),
            "Claude Code's storage-write lock"
        );
        assert_eq!(
            lock_role(Path::new("/h/.storage-write.lock")),
            "Claude Code's storage-write lock",
            "CC 2.1.292's spelling is no generic credential lock"
        );
    }

    #[test]
    fn a_failed_start_whose_directory_cannot_be_removed_is_logged_with_its_cause() {
        let d = tempfile::tempdir().unwrap();
        let s = spec(d.path(), 60_000, 0, 3_000);
        let parent = d.path().to_path_buf();
        let restore = once_at(&s.path, Point::BeforeHeartbeat, move || {
            ReadOnly::new(&parent)
        });
        fail_at(&s.path, Point::BeforeHeartbeat);
        let logs = logged(|| assert!(MkdirLock::acquire(&s).is_err()));
        drop(restore.try_recv().unwrap());
        assert!(s.path.is_dir(), "left until it goes stale");
        let warnings: Vec<&String> = logs.iter().filter(|l| l.contains("WARN")).collect();
        assert_eq!(warnings.len(), 1, "{logs:?}");
        assert!(
            warnings[0].contains("could not remove Claude Code's credential lock")
                && !warnings[0].contains(&d.path().display().to_string()),
            "{}",
            warnings[0]
        );
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
