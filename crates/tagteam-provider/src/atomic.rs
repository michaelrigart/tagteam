use std::fs::{self, DirBuilder, File, OpenOptions, Permissions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub fn resolve_target(path: &Path) -> io::Result<PathBuf> {
    let mut p = path.to_path_buf();
    for _ in 0..40 {
        match fs::symlink_metadata(&p) {
            Ok(m) if m.file_type().is_symlink() => {
                let target = fs::read_link(&p)?;
                p = if target.is_absolute() {
                    target
                } else {
                    p.parent().unwrap_or(Path::new("/")).join(target)
                };
            }
            Ok(_) => return Ok(p),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(p),
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other(format!(
        "too many levels of symbolic links at {}",
        path.display()
    )))
}

/// Whether a write should match whatever mode the file it replaces already has, or force a
/// fixed mode regardless.
enum ModePolicy {
    /// The replaced file's own mode, or `new_mode` when creating it (CC's own files: tagteam
    /// must not narrow or widen a mode it doesn't own).
    Preserve(u32),
    /// Always this mode, even over a file an external actor has since widened (secrets: an
    /// externally widened mode must never survive a write).
    Force(u32),
}

/// The primitive every tagteam file write goes through (§9.5).
///
/// An error means the target was **not** replaced: everything that can fail happens before
/// the rename, and the directory fsync after it is best-effort. `before_publish` runs
/// immediately before the rename, so a lock holder can re-check ownership at the last moment
/// (§9.1); if it fails, nothing is published.
pub fn write_atomic_with<E: From<io::Error>>(
    path: &Path,
    bytes: &[u8],
    new_mode: u32,
    before_publish: impl Fn() -> Result<(), E>,
) -> Result<(), E> {
    write_atomic_mode_with(path, bytes, ModePolicy::Preserve(new_mode), before_publish)
}

/// Like `write_atomic_with`, but always applies `mode`, ignoring whatever mode the file it
/// replaces already has. For secrets — the vault, credential files — where a mode an external
/// actor has widened must never survive a write, unlike `write_atomic`'s CC-file-preserving
/// default.
pub fn write_atomic_private_with<E: From<io::Error>>(
    path: &Path,
    bytes: &[u8],
    mode: u32,
    before_publish: impl Fn() -> Result<(), E>,
) -> Result<(), E> {
    write_atomic_mode_with(path, bytes, ModePolicy::Force(mode), before_publish)
}

fn write_atomic_mode_with<E: From<io::Error>>(
    path: &Path,
    bytes: &[u8],
    policy: ModePolicy,
    before_publish: impl Fn() -> Result<(), E>,
) -> Result<(), E> {
    let target = resolve_target(path)?;
    let dir = target
        .parent()
        .ok_or_else(|| io::Error::other(format!("{} has no parent", target.display())))?;
    let name = target
        .file_name()
        .ok_or_else(|| io::Error::other(format!("{} has no file name", target.display())))?;
    let mode = match policy {
        ModePolicy::Force(mode) => mode,
        ModePolicy::Preserve(new_mode) => match fs::metadata(&target) {
            Ok(m) => m.permissions().mode() & 0o7777,
            Err(e) if e.kind() == io::ErrorKind::NotFound => new_mode,
            Err(e) => return Err(e.into()),
        },
    };
    let tmp = dir.join(format!(
        ".{}.tagteam-{}-{:08x}",
        name.to_string_lossy(),
        std::process::id(),
        fastrand::u32(..)
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&tmp)?;
    let prepared = (|| {
        // Before any byte is written, so the umask can never widen a secret file.
        file.set_permissions(Permissions::from_mode(mode))?;
        file.write_all(bytes)?;
        file.sync_all()
    })();
    let published = prepared
        .map_err(E::from)
        .and_then(|()| before_publish())
        .and_then(|()| fs::rename(&tmp, &target).map_err(E::from));
    if published.is_err() {
        let _ = fs::remove_file(&tmp);
        return published;
    }
    // Published: from here on nothing may report failure.
    let _ = File::open(dir).and_then(|d| d.sync_all());
    Ok(())
}

/// Removes the file a path resolves to and leaves any symlink in place, the mirror image of
/// how writes land in the link's target (§9.5). Absent is success.
pub fn remove_target(path: &Path) -> io::Result<()> {
    match fs::remove_file(resolve_target(path)?) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// `write_atomic_with` with no pre-publication check.
pub fn write_atomic(path: &Path, bytes: &[u8], new_mode: u32) -> io::Result<()> {
    write_atomic_with(path, bytes, new_mode, || Ok::<(), io::Error>(()))
}

/// `write_atomic_private_with` with no pre-publication check.
pub fn write_atomic_private(path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    write_atomic_private_with(path, bytes, mode, || Ok::<(), io::Error>(()))
}

pub fn ensure_private_dir(path: &Path) -> io::Result<()> {
    DirBuilder::new().recursive(true).mode(0o700).create(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn mode(p: &Path) -> u32 {
        fs::metadata(p).unwrap().permissions().mode() & 0o7777
    }

    #[test]
    fn new_files_get_the_requested_mode() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("creds.json");
        write_atomic(&p, b"{}", 0o600).unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"{}");
        assert_eq!(mode(&p), 0o600);
    }

    #[test]
    fn existing_mode_is_preserved() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.json");
        fs::write(&p, "old").unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();
        write_atomic(&p, b"new", 0o600).unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"new");
        assert_eq!(mode(&p), 0o644);
    }

    #[test]
    fn write_atomic_private_ignores_an_existing_widened_mode() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("secret.json");
        fs::write(&p, "old").unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();
        write_atomic_private(&p, b"new", 0o600).unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"new");
        assert_eq!(mode(&p), 0o600);
    }

    #[test]
    fn writes_land_in_the_symlink_target_and_the_link_survives() {
        let d = tempfile::tempdir().unwrap();
        let real = d.path().join("dotfiles/claude.json");
        fs::create_dir_all(real.parent().unwrap()).unwrap();
        fs::write(&real, "old").unwrap();
        let link = d.path().join(".claude.json");
        symlink("dotfiles/claude.json", &link).unwrap(); // relative target
        write_atomic(&link, b"new", 0o600).unwrap();
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read(&real).unwrap(), b"new");
    }

    #[test]
    fn a_dangling_link_creates_its_target() {
        let d = tempfile::tempdir().unwrap();
        let link = d.path().join("l");
        symlink(d.path().join("t"), &link).unwrap();
        write_atomic(&link, b"x", 0o600).unwrap();
        assert_eq!(fs::read(d.path().join("t")).unwrap(), b"x");
    }

    #[test]
    fn no_temp_files_are_left_behind() {
        let d = tempfile::tempdir().unwrap();
        write_atomic(&d.path().join("a"), b"1", 0o600).unwrap();
        write_atomic(&d.path().join("a"), b"2", 0o600).unwrap();
        let names: Vec<_> = fs::read_dir(d.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("a")]);
    }

    #[test]
    fn a_missing_directory_is_an_error() {
        let d = tempfile::tempdir().unwrap();
        assert!(write_atomic(&d.path().join("nope/a"), b"1", 0o600).is_err());
    }

    #[test]
    fn a_symlink_loop_is_an_error() {
        let d = tempfile::tempdir().unwrap();
        symlink(d.path().join("b"), d.path().join("a")).unwrap();
        symlink(d.path().join("a"), d.path().join("b")).unwrap();
        assert!(resolve_target(&d.path().join("a")).is_err());
    }

    #[test]
    fn removing_through_a_link_keeps_the_link() {
        let d = tempfile::tempdir().unwrap();
        let link = d.path().join("l");
        symlink(d.path().join("t"), &link).unwrap(); // dangling
        write_atomic(&link, b"x", 0o600).unwrap(); // creates the target
        remove_target(&link).unwrap();
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(!d.path().join("t").exists());
        remove_target(&link).unwrap(); // absent: still fine
    }

    #[test]
    fn a_failed_pre_publication_check_publishes_nothing() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.json");
        fs::write(&p, "old").unwrap();
        let r = write_atomic_with(&p, b"new", 0o600, || Err(io::Error::other("lock lost")));
        assert!(r.is_err());
        assert_eq!(fs::read(&p).unwrap(), b"old");
        assert_eq!(
            fs::read_dir(d.path()).unwrap().count(),
            1,
            "no temp file left"
        );
    }

    #[test]
    fn private_dirs_are_0700() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("x/y");
        ensure_private_dir(&p).unwrap();
        assert_eq!(mode(&p), 0o700);
        assert_eq!(mode(&d.path().join("x")), 0o700);
    }
}
