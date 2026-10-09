//! Where compat keeps things.
//! - The compat store, `$XDG_STATE_HOME/tagteam-compat/` (§15.4), outlives runs: the vault's
//!   keychain file and its password, the XDG directories the test account's `tagteam` runs
//!   with (store, settings, log, session profiles), and `compat.lock`, which one run or
//!   `compat login` at a time holds (`RunLock`).
//! - A run's scratch directory, `$TMPDIR/tagteam-compat.<hex>/`, holds every Claude Code home
//!   the run makes: `live/` (the default home tagteam switches), `work/` (where `claude` runs),
//!   and `homes/<name>/`.
//! - The report goes to `<target>/compat/`.

use std::ffi::OsStr;
use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::fs::DirBuilderExt as _;
use std::path::{Path, PathBuf};

use tagteam_provider::FlockGuard;

use super::keychain::random_hex;
use super::sys::{HarnessError, harness};

#[derive(Debug, Clone)]
pub struct Layout {
    /// The workspace root.
    pub workspace: PathBuf,
    pub reports: PathBuf,
    pub state: PathBuf,
    pub scratch: PathBuf,
}

/// `$XDG_STATE_HOME/tagteam-compat`, or `~/.local/state/tagteam-compat` when that is unset or
/// not absolute.
pub fn state_dir(home: &Path, xdg_state_home: Option<&OsStr>) -> PathBuf {
    xdg_state_home
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".local/state"))
        .join("tagteam-compat")
}

/// `$CARGO_TARGET_DIR/compat`, or `<workspace>/target/compat`.
pub fn reports_dir(workspace: &Path, target_dir: Option<&OsStr>) -> PathBuf {
    target_dir
        .map(|t| workspace.join(t))
        .unwrap_or_else(|| workspace.join("target"))
        .join("compat")
}

/// A new directory 0700, never an existing one.
pub fn private_dir(path: &Path) -> io::Result<()> {
    DirBuilder::new().mode(0o700).create(path)
}

/// `$TMPDIR/tagteam-compat.<hex>`, new and 0700, by its canonical path.
pub fn make_scratch() -> Result<PathBuf, HarnessError> {
    let dir = std::env::temp_dir().join(format!("tagteam-compat.{}", &random_hex()?[..12]));
    private_dir(&dir)?;
    let dir = fs::canonicalize(dir)?;
    for sub in ["live", "work", "homes"] {
        private_dir(&dir.join(sub))?;
    }
    Ok(dir)
}

impl Layout {
    pub fn vault_keychain(&self) -> PathBuf {
        self.state.join("vault.keychain-db")
    }

    pub fn vault_password(&self) -> PathBuf {
        self.state.join("vault.password")
    }

    /// `XDG_CONFIG_HOME`, `XDG_DATA_HOME` and `XDG_STATE_HOME` for the test account's tagteam.
    pub fn xdg(&self) -> [(&'static str, PathBuf); 3] {
        [
            ("XDG_CONFIG_HOME", self.state.join("xdg/config")),
            ("XDG_DATA_HOME", self.state.join("xdg/data")),
            ("XDG_STATE_HOME", self.state.join("xdg/state")),
        ]
    }

    /// tagteam's data directory: `$XDG_DATA_HOME/tagteam`.
    pub fn data_dir(&self) -> PathBuf {
        self.state.join("xdg/data/tagteam")
    }

    /// An account's session profile (§12.2), as tagteam's `profile_path` names it.
    pub fn profile(&self, id: &str) -> PathBuf {
        self.data_dir().join("sessions").join(id)
    }

    pub fn live(&self) -> PathBuf {
        self.scratch.join("live")
    }

    pub fn work(&self) -> PathBuf {
        self.scratch.join("work")
    }

    pub fn homes(&self) -> PathBuf {
        self.scratch.join("homes")
    }
}

/// The compat store's run-wide lock: an exclusive `flock` on `compat.lock`, held by a run from
/// before its setup to after its teardown, and by `compat login`. Two runs would otherwise
/// activate one generation into two homes, where one's refresh consumes the other's token
/// (R9). It never waits: a second run refuses at once and touches nothing. The holder writes
/// its pid into the file, for the refusal to name.
#[derive(Debug)]
pub struct RunLock {
    _guard: FlockGuard,
}

impl RunLock {
    pub fn take(state: &Path) -> Result<Self, HarnessError> {
        let path = state.join("compat.lock");
        match FlockGuard::try_lock(&path) {
            Ok(Some(guard)) => {
                fs::write(&path, format!("{}\n", std::process::id()))?;
                Ok(Self { _guard: guard })
            }
            Ok(None) => {
                let holder = fs::read_to_string(&path)
                    .ok()
                    .and_then(|s| s.trim().parse::<u32>().ok())
                    .map_or_else(String::new, |pid| format!(" (pid {pid})"));
                Err(harness(format!(
                    "another compat run or compat login holds {}{holder}; this one touched nothing",
                    path.display()
                )))
            }
            Err(e) => Err(harness(format!("the compat lock {}: {e}", path.display()))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_store_follows_xdg_state_home_when_it_is_absolute() {
        let home = Path::new("/home/t");
        assert_eq!(
            state_dir(home, Some(OsStr::new("/x/state"))),
            PathBuf::from("/x/state/tagteam-compat")
        );
        assert_eq!(
            state_dir(home, Some(OsStr::new("rel"))),
            PathBuf::from("/home/t/.local/state/tagteam-compat")
        );
        assert_eq!(
            state_dir(home, None),
            PathBuf::from("/home/t/.local/state/tagteam-compat")
        );
    }

    #[test]
    fn the_report_goes_under_the_target_directory() {
        let ws = Path::new("/w");
        assert_eq!(reports_dir(ws, None), PathBuf::from("/w/target/compat"));
        assert_eq!(
            reports_dir(ws, Some(OsStr::new("/t"))),
            PathBuf::from("/t/compat")
        );
        assert_eq!(
            reports_dir(ws, Some(OsStr::new("out"))),
            PathBuf::from("/w/out/compat")
        );
    }

    #[test]
    fn a_scratch_directory_is_new_private_and_canonical() {
        use std::os::unix::fs::PermissionsExt as _;
        let a = make_scratch().unwrap();
        let b = make_scratch().unwrap();
        assert_ne!(a, b);
        assert_eq!(fs::canonicalize(&a).unwrap(), a);
        assert_eq!(
            fs::metadata(&a).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert!(a.join("live").is_dir() && a.join("work").is_dir() && a.join("homes").is_dir());
        fs::remove_dir_all(a).unwrap();
        fs::remove_dir_all(b).unwrap();
    }

    #[test]
    fn a_second_run_refuses_at_once_naming_the_holder() {
        // See `daemon`'s reservation test: a forked child would inherit the held lock.
        let _serial = crate::compat::sys::serial();
        let state = std::env::temp_dir().join(format!("xtask-lock-{}", std::process::id()));
        let _ = fs::remove_dir_all(&state);
        let held = RunLock::take(&state).unwrap();
        let refused = RunLock::take(&state).unwrap_err();
        assert!(
            refused.0.contains(&format!(
                "(pid {}); this one touched nothing",
                std::process::id()
            )),
            "{refused}"
        );
        drop(held);
        fs::remove_dir_all(&state).unwrap();
    }
}
