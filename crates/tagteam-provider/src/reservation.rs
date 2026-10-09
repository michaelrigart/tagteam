//! §12.5's launch reservation: a locked file in the profile saying that a launch, or the
//! `claude` it started, is alive. It is live while its file is locked, and the kernel holds the
//! lock for as long as tagteam or `claude`, which inherits the fd (Decision 3), lives. Others
//! only ever test it, non-blocking (`flock::probe_lock`, B.37); no pid is consulted.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

use crate::atomic::ensure_private_dir;
use crate::flock::{LockProbe, probe_lock};
use crate::profile::LAUNCH_DIR;

/// What marks a reservation still being created: its temporary name is
/// `.<pid>.lock.tagteam-<pid>-<rand>`, after the atomic writer's convention. It never ends in
/// `.lock`, so `profile::launch_reservations` never counts one.
const TEMP_MARK: &str = ".lock.tagteam-";

/// §12.5's launch reservation: `<profile>/.tagteam-launch/<pid>.lock`, created under a
/// temporary name, `flock`ed, then renamed into place, so it never appears unlocked.
#[derive(Debug)]
pub struct LaunchReservation {
    /// `O_CLOEXEC` in tagteam (std's default): only the session spawn passes it on.
    file: File,
    path: PathBuf,
}

impl LaunchReservation {
    /// Holds `MutationGuard` and the account lock: the caller's duty, not checked here. When a
    /// live `<pid>.lock` already holds the name (an orphaned `claude` of an earlier process with
    /// this pid), it fails with `io::ErrorKind::AlreadyExists`, naming the file, and leaves it.
    pub fn create(profile: &Path) -> io::Result<LaunchReservation> {
        Self::create_with(profile, &|_| {})
    }

    /// `create`, calling `before_rename` with the temporary file once it is locked and
    /// written: the moment the tests look at.
    fn create_with(profile: &Path, before_rename: &dyn Fn(&Path)) -> io::Result<LaunchReservation> {
        let dir = profile.join(LAUNCH_DIR);
        ensure_private_dir(&dir)?;
        let pid = std::process::id();
        let name = format!("{pid}.lock");
        let path = dir.join(&name);
        let (temp, file) = create_temp(&dir, &name, pid)?;
        let placed = lock(&file)
            .and_then(|()| (&file).write_all(&contents(pid)))
            .and_then(|()| {
                before_rename(&temp);
                never_over_a_live_one(&path)
            })
            .and_then(|()| fs::rename(&temp, &path));
        if let Err(e) = placed {
            // §14: the placement's error is the one returned; a temporary file it cannot
            // remove is logged with its cause, never its path (§14.2). `launch_reservations`
            // never counts one.
            match fs::remove_file(&temp) {
                Err(left) if left.kind() != io::ErrorKind::NotFound => tracing::warn!(
                    "could not remove a launch reservation's temporary file, left in its profile: {left}"
                ),
                _ => {}
            }
            return Err(e);
        }
        Ok(LaunchReservation { file, path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The locked descriptor, for `process::spawn_session` to pass to `claude`.
    pub fn fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }

    /// Unlinks the file (exit handling's last step). The lock goes when the last holder exits:
    /// this process's fd closes here, and a descendant of `claude` still holding the inherited
    /// one no longer matters, since liveness is judged by the path. A file already gone is not
    /// an error. Dropping a reservation never unlinks it.
    pub fn unlink(self) -> io::Result<()> {
        match fs::remove_file(&self.path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

/// A new temporary file in `dir`, 0600, created with `O_EXCL`, so no file that already exists
/// is ever reused as a reservation.
fn create_temp(dir: &Path, name: &str, pid: u32) -> io::Result<(PathBuf, File)> {
    loop {
        let temp = dir.join(format!(".{name}.tagteam-{pid}-{:08x}", fastrand::u32(..)));
        match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)
        {
            Ok(file) => return Ok((temp, file)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
}

/// Takes the reservation's lock. The file is new, so nothing else can hold it: a refusal is an
/// error, never a wait (B.37).
fn lock(file: &File) -> io::Result<()> {
    // SAFETY: `file` owns a valid descriptor for the duration of the call.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// `{pid, startedAt}`, for `doctor` and for a person reading the directory (§12.5). Liveness
/// never reads it.
fn contents(pid: u32) -> Vec<u8> {
    let started_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    let mut bytes = json!({"pid": pid, "startedAt": started_at})
        .to_string()
        .into_bytes();
    bytes.push(b'\n');
    bytes
}

/// The rename would replace whatever is at `path`. A dead file there (left by an earlier
/// process that had this pid) may go, but a live one is the reservation of an orphaned `claude`
/// of that process, and replacing it would hide a running session. The caller holds
/// `MutationGuard` and the account lock, the only way a reservation is created or removed, so
/// nothing can appear at `path` between this probe and the rename.
fn never_over_a_live_one(path: &Path) -> io::Result<()> {
    match probe_lock(path)? {
        LockProbe::Missing | LockProbe::Free => Ok(()),
        LockProbe::Held => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "{} is a live launch reservation of an earlier process with this pid",
                path.display()
            ),
        )),
    }
}

/// Removes every reservation in `profile` that `probe_lock` finds `Free` (dead). Returns their
/// paths, in name order. A temporary file left by a create that died before its rename is
/// removed too once it is free, and is not returned, since it was never a reservation. A profile
/// path that is not a directory has none, as `profile::launch_reservations` reads it. Under
/// `MutationGuard` and the account lock (§12.5).
pub fn remove_dead_reservations(profile: &Path) -> io::Result<Vec<PathBuf>> {
    let dir = profile.join(LAUNCH_DIR);
    let listing = match fs::read_dir(&dir) {
        Ok(l) => l,
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(vec![]);
        }
        Err(e) => return Err(e),
    };
    let mut paths = Vec::new();
    for entry in listing {
        paths.push(entry?.path());
    }
    paths.sort();
    let mut removed = Vec::new();
    for path in paths {
        // The same test as `profile::launch_reservations`.
        let reservation = path.extension().is_some_and(|x| x == "lock");
        let temp = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with('.') && n.contains(TEMP_MARK));
        if !(reservation || temp) || probe_lock(&path)? != LockProbe::Free {
            continue;
        }
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        }
        if reservation {
            removed.push(path);
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{MutexGuard, PoisonError};

    use crate::flock::FlockGuard;
    use crate::process::{SpawnSpec, spawn_session};
    use crate::profile::launch_reservations;
    use crate::read::Read;

    fn fork_guard() -> MutexGuard<'static, ()> {
        crate::FORK_GUARD
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn launch_dir(profile: &Path) -> PathBuf {
        profile.join(LAUNCH_DIR)
    }

    /// `sleep 30` as a session child.
    fn sleeper() -> SpawnSpec {
        SpawnSpec {
            program: "/bin/sleep".into(),
            args: vec!["30".into()],
            ..SpawnSpec::default()
        }
    }

    fn names_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_reservation_is_named_for_this_process_and_records_it() {
        let _fork = fork_guard();
        let d = tempfile::tempdir().unwrap();
        let r = LaunchReservation::create(d.path()).unwrap();
        let pid = std::process::id();
        assert_eq!(r.path(), launch_dir(d.path()).join(format!("{pid}.lock")));
        let v: serde_json::Value = serde_json::from_slice(&fs::read(r.path()).unwrap()).unwrap();
        assert_eq!(v["pid"], pid);
        assert!(
            v["startedAt"]
                .as_i64()
                .is_some_and(|ms| ms > 1_700_000_000_000),
            "{v}"
        );
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(r.path()), 0o600);
        assert_eq!(mode(&launch_dir(d.path())), 0o700);
        assert_eq!(
            names_in(&launch_dir(d.path())),
            [format!("{pid}.lock")],
            "no temporary file"
        );
    }

    #[test]
    fn a_reservation_is_never_visible_unlocked() {
        let _fork = fork_guard();
        let d = tempfile::tempdir().unwrap();
        let profile = d.path();
        let looked = Cell::new(false);
        let r = LaunchReservation::create_with(profile, &|temp| {
            // Before the rename: already locked, and no reservation to anyone who lists them.
            assert_eq!(probe_lock(temp).unwrap(), LockProbe::Held);
            assert!(
                temp.extension().is_none_or(|x| x != "lock"),
                "{}",
                temp.display()
            );
            assert!(matches!(launch_reservations(profile), Read::Present(v) if v.is_empty()));
            looked.set(true);
        })
        .unwrap();
        assert!(looked.get());
        let Read::Present(found) = launch_reservations(profile) else {
            panic!("the reservations list");
        };
        assert_eq!(found, vec![(r.path().to_path_buf(), LockProbe::Held)]);
    }

    #[test]
    fn a_reservation_is_live_while_its_holder_lives() {
        let _fork = fork_guard();
        let d = tempfile::tempdir().unwrap();
        let r = LaunchReservation::create(d.path()).unwrap();
        let path = r.path().to_path_buf();
        assert_eq!(probe_lock(&path).unwrap(), LockProbe::Held);
        assert!(
            remove_dead_reservations(d.path()).unwrap().is_empty(),
            "a held one is kept"
        );
        assert!(path.exists());
        drop(r);
        assert_eq!(
            probe_lock(&path).unwrap(),
            LockProbe::Free,
            "dead once its holder is"
        );
        assert!(path.exists(), "dropping never unlinks: exit handling does");
    }

    #[test]
    fn the_session_s_inherited_descriptor_keeps_it_live_after_tagteam_lets_go() {
        // Review Focus 1's mechanism: a `tagteam` killed while `claude` runs closes its own
        // descriptor, and `claude`'s inherited copy keeps the lock.
        let _fork = fork_guard();
        let d = tempfile::tempdir().unwrap();
        let r = LaunchReservation::create(d.path()).unwrap();
        let path = r.path().to_path_buf();
        let mut child = spawn_session(&sleeper(), Some(r.fd())).unwrap();
        drop(r);
        assert_eq!(probe_lock(&path).unwrap(), LockProbe::Held);
        assert!(remove_dead_reservations(d.path()).unwrap().is_empty());
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(probe_lock(&path).unwrap(), LockProbe::Free);
        assert_eq!(
            remove_dead_reservations(d.path()).unwrap(),
            vec![path.clone()]
        );
        assert!(!path.exists());
    }

    #[test]
    fn a_child_spawned_without_the_descriptor_never_holds_the_reservation() {
        // Decision 3: no other child (a `security` call, the validation spawn) keeps a launch
        // alive after tagteam.
        let _fork = fork_guard();
        let d = tempfile::tempdir().unwrap();
        let r = LaunchReservation::create(d.path()).unwrap();
        let path = r.path().to_path_buf();
        let mut child = spawn_session(&sleeper(), None).unwrap();
        drop(r);
        let probed = probe_lock(&path).unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(probed, LockProbe::Free);
    }

    #[test]
    fn dead_reservations_and_leftover_temporary_files_go_and_nothing_else() {
        let _fork = fork_guard();
        let d = tempfile::tempdir().unwrap();
        let dir = launch_dir(d.path());
        fs::create_dir_all(&dir).unwrap();
        // A launch whose parent and `claude` are both gone.
        fs::write(dir.join("100.lock"), b"{\"pid\":100}\n").unwrap();
        // A create that died before its rename.
        fs::write(dir.join(".7.lock.tagteam-7-0000abcd"), b"").unwrap();
        fs::write(dir.join("notes.txt"), b"").unwrap();
        let held = FlockGuard::try_lock(&dir.join("200.lock"))
            .unwrap()
            .unwrap();
        assert_eq!(
            remove_dead_reservations(d.path()).unwrap(),
            vec![dir.join("100.lock")]
        );
        assert_eq!(names_in(&dir), ["200.lock", "notes.txt"]);
        drop(held);
    }

    #[test]
    fn a_profile_without_reservations_has_none_to_remove_and_gains_no_directory() {
        let d = tempfile::tempdir().unwrap();
        assert!(remove_dead_reservations(d.path()).unwrap().is_empty());
        assert!(!launch_dir(d.path()).exists());
        // A profile path that is not a directory holds none either (T4-a), as
        // `launch_reservations` reads it.
        let file = d.path().join("not-a-directory");
        fs::write(&file, b"").unwrap();
        assert!(remove_dead_reservations(&file).unwrap().is_empty());
        assert!(file.is_file());
    }

    #[test]
    fn unlink_removes_the_file_and_tolerates_one_already_gone() {
        let _fork = fork_guard();
        let d = tempfile::tempdir().unwrap();
        let r = LaunchReservation::create(d.path()).unwrap();
        let path = r.path().to_path_buf();
        r.unlink().unwrap();
        assert!(!path.exists());
        assert!(matches!(launch_reservations(d.path()), Read::Present(v) if v.is_empty()));
        let r = LaunchReservation::create(d.path()).unwrap();
        fs::remove_file(r.path()).unwrap();
        r.unlink().unwrap();
    }

    #[test]
    fn a_dead_file_under_this_pid_is_replaced_and_a_live_one_refuses() {
        let _fork = fork_guard();
        let d = tempfile::tempdir().unwrap();
        let dir = launch_dir(d.path());
        fs::create_dir_all(&dir).unwrap();
        let mine = dir.join(format!("{}.lock", std::process::id()));
        fs::write(&mine, b"stale").unwrap();
        let r = LaunchReservation::create(d.path()).unwrap();
        assert_eq!(probe_lock(&mine).unwrap(), LockProbe::Held);
        assert_ne!(fs::read(&mine).unwrap(), b"stale");
        drop(r);
        // An earlier process with this pid whose orphaned `claude` still holds its reservation.
        let orphan = FlockGuard::try_lock(&mine).unwrap().unwrap();
        let before = fs::read(&mine).unwrap();
        let err = LaunchReservation::create(d.path()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&mine).unwrap(), before, "never replaced");
        assert_eq!(
            names_in(&dir),
            [format!("{}.lock", std::process::id())],
            "no temporary file is left"
        );
        drop(orphan);
    }
}
