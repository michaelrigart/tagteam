use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use crate::cancel::Cancel;

/// §14.2: the suffixes of the log's rotations, newest first: `tagteam.log.1`, then `.2`. The
/// oldest is dropped at the next rotation.
pub const LOG_ROTATIONS: &[&str] = &[".1", ".2"];

/// Why the process's environment cannot place tagteam's files (§5). Each message says what to
/// do.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EnvError {
    #[error("HOME is not set; set it to the absolute path of your home directory")]
    HomeUnset,
    #[error("HOME is empty; set it to the absolute path of your home directory")]
    HomeEmpty,
    #[error(
        "HOME is {0:?}, which is not an absolute path; set it to the absolute path of your home directory"
    )]
    HomeRelative(String),
}

impl EnvError {
    /// Stable `error.type` for `--json` output (§5, §14).
    pub fn kind(&self) -> &'static str {
        "env"
    }
}

/// `HOME` as tagteam may use it (§5): set, non-empty and absolute. Every default path derives
/// from it, so a fallback would put state under `/` or the working directory.
pub fn home_from(value: Option<OsString>) -> Result<PathBuf, EnvError> {
    let value = value.ok_or(EnvError::HomeUnset)?;
    if value.is_empty() {
        return Err(EnvError::HomeEmpty);
    }
    let home = PathBuf::from(value);
    if !home.is_absolute() {
        return Err(EnvError::HomeRelative(home.to_string_lossy().into_owned()));
    }
    Ok(home)
}

/// Everything tagteam resolves paths from (§15.1). Tests build one with `for_test`.
#[derive(Debug, Clone)]
pub struct Env {
    pub home: PathBuf,
    pub user: Option<String>,
    pub xdg_config_home: Option<PathBuf>,
    pub xdg_data_home: Option<PathBuf>,
    pub xdg_state_home: Option<PathBuf>,
    /// The raw string, never canonicalized; `None` when unset.
    pub claude_config_dir: Option<OsString>,
    /// `None` when undefined; `Some("")` when defined but empty (Appendix A.1).
    pub claude_securestorage_config_dir: Option<OsString>,
    /// The process's cancel token (§14.1). Clones share it, so every engine and lock built
    /// from one Env sees the same signal; each `for_test` Env has a token of its own.
    pub cancel: Cancel,
    /// Provider-owned variables the registry asked for (§4.5 `session_dir_var`, `CLAUDECODE`),
    /// captured by `capture_vars`. Empty in `from_process` and `for_test`.
    pub vars: BTreeMap<String, OsString>,
    forbidden_root: Option<PathBuf>,
}

impl Env {
    /// The process's environment. Fails when `HOME` cannot place tagteam's files (§5).
    pub fn from_process() -> Result<Self, EnvError> {
        let abs = |k: &str| {
            std::env::var_os(k)
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
        };
        Ok(Self {
            home: home_from(std::env::var_os("HOME"))?,
            user: std::env::var("USER").ok(),
            xdg_config_home: abs("XDG_CONFIG_HOME"),
            xdg_data_home: abs("XDG_DATA_HOME"),
            xdg_state_home: abs("XDG_STATE_HOME"),
            claude_config_dir: std::env::var_os("CLAUDE_CONFIG_DIR"),
            claude_securestorage_config_dir: std::env::var_os("CLAUDE_SECURESTORAGE_CONFIG_DIR"),
            cancel: Cancel::new(),
            vars: BTreeMap::new(),
            forbidden_root: None,
        })
    }

    /// A fixture environment rooted at `root`, with the harness guard armed against the
    /// real HOME.
    ///
    /// Panics if the real `HOME` is unset: an unarmed guard would fail open and let a test
    /// write under the real HOME undetected.
    pub fn for_test(root: &std::path::Path) -> Self {
        let real_home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .expect("HOME must be set so Env::for_test can arm its real-HOME guard");
        Self {
            home: root.join("home"),
            user: Some("tester".into()),
            xdg_config_home: None,
            xdg_data_home: None,
            xdg_state_home: None,
            claude_config_dir: None,
            claude_securestorage_config_dir: None,
            cancel: Cancel::new(),
            vars: BTreeMap::new(),
            forbidden_root: Some(real_home),
        }
    }

    /// Reads each named variable from the process environment into `vars` (absent: not
    /// inserted, and any earlier value dropped).
    pub fn capture_vars(&mut self, names: &[&str]) {
        for name in names {
            match std::env::var_os(name) {
                Some(v) => {
                    self.vars.insert((*name).to_owned(), v);
                }
                None => {
                    self.vars.remove(*name);
                }
            }
        }
    }

    pub fn var(&self, name: &str) -> Option<&OsStr> {
        self.vars.get(name).map(OsString::as_os_str)
    }

    pub fn with_forbidden_root(mut self, root: PathBuf) -> Self {
        self.forbidden_root = Some(root);
        self
    }

    /// Panics in tests when a resolved path falls under the real HOME (§15.1).
    pub fn guard(&self, path: PathBuf) -> PathBuf {
        if let Some(root) = &self.forbidden_root {
            assert!(
                !path.starts_with(root),
                "test harness: resolved path {path:?} is under the real HOME"
            );
        }
        path
    }

    pub fn data_dir(&self) -> PathBuf {
        let base = self
            .xdg_data_home
            .clone()
            .unwrap_or_else(|| self.home.join(".local/share"));
        self.guard(base.join("tagteam"))
    }

    pub fn config_dir(&self) -> PathBuf {
        let base = self
            .xdg_config_home
            .clone()
            .unwrap_or_else(|| self.home.join(".config"));
        self.guard(base.join("tagteam"))
    }

    pub fn state_dir(&self) -> PathBuf {
        let base = self
            .xdg_state_home
            .clone()
            .unwrap_or_else(|| self.home.join(".local/state"));
        self.guard(base.join("tagteam"))
    }

    /// §5, §14.2: `$XDG_STATE_HOME/tagteam/tagteam.log`. Its rotations are this path with each
    /// of `LOG_ROTATIONS` appended, and its rotation lock `tagteam.log.lock` sits beside it.
    pub fn log_file(&self) -> PathBuf {
        self.state_dir().join("tagteam.log")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn xdg_defaults_live_under_home() {
        let env = Env::for_test(Path::new("/tmp/fixture"));
        assert_eq!(
            env.data_dir(),
            Path::new("/tmp/fixture/home/.local/share/tagteam")
        );
        assert_eq!(
            env.config_dir(),
            Path::new("/tmp/fixture/home/.config/tagteam")
        );
        assert_eq!(
            env.state_dir(),
            Path::new("/tmp/fixture/home/.local/state/tagteam")
        );
    }

    #[test]
    fn the_log_and_its_rotations_live_in_the_state_dir() {
        let mut env = Env::for_test(Path::new("/tmp/fixture"));
        assert_eq!(
            env.log_file(),
            Path::new("/tmp/fixture/home/.local/state/tagteam/tagteam.log")
        );
        assert_eq!(LOG_ROTATIONS, [".1", ".2"]);
        env.xdg_state_home = Some(PathBuf::from("/state"));
        assert_eq!(env.log_file(), Path::new("/state/tagteam/tagteam.log"));
    }

    #[test]
    fn absolute_xdg_overrides_win() {
        let mut env = Env::for_test(Path::new("/tmp/fixture"));
        env.xdg_data_home = Some(PathBuf::from("/data"));
        assert_eq!(env.data_dir(), Path::new("/data/tagteam"));
    }

    #[test]
    #[should_panic(expected = "under the real HOME")]
    fn the_harness_guard_trips() {
        let env = Env::for_test(Path::new("/tmp/fixture"))
            .with_forbidden_root(PathBuf::from("/tmp/fixture/home"));
        let _ = env.data_dir();
    }

    #[test]
    #[should_panic(expected = "under the real HOME")]
    fn for_test_arms_itself_against_the_real_home_by_default() {
        let real_home = PathBuf::from(std::env::var_os("HOME").unwrap());
        let env = Env::for_test(&real_home.join("tagteam-guard-probe"));
        let _ = env.data_dir();
    }

    #[test]
    fn clones_share_the_cancel_token_and_fixtures_never_do() {
        let env = Env::for_test(Path::new("/tmp/fixture"));
        let clone = env.clone();
        env.cancel.request(15);
        assert_eq!(clone.cancel.requested(), Some(15));
        let other = Env::for_test(Path::new("/tmp/fixture"));
        assert_eq!(other.cancel.requested(), None);
    }

    #[test]
    fn provider_variables_are_captured_only_when_asked_for() {
        const NEVER: &str = "TAGTEAM_TEST_NEVER_SET_7F3A";
        let mut env = Env::for_test(Path::new("/tmp/fixture"));
        assert!(env.vars.is_empty());
        assert_eq!(env.var("HOME"), None, "nothing is captured until asked");
        env.capture_vars(&["HOME", NEVER]);
        assert_eq!(env.var("HOME"), std::env::var_os("HOME").as_deref());
        assert_eq!(env.var(NEVER), None);
        assert!(
            !env.vars.contains_key(NEVER),
            "an unset variable is not inserted"
        );
        env.vars.insert(NEVER.into(), "stale".into());
        env.capture_vars(&[NEVER]);
        assert_eq!(
            env.var(NEVER),
            None,
            "a variable gone from the process is dropped"
        );
        assert!(Env::from_process().unwrap().vars.is_empty());
    }

    #[test]
    fn home_must_be_set_non_empty_and_absolute() {
        // §5: every default path derives from HOME, so nothing may fall back to `/` or the
        // working directory.
        assert_eq!(home_from(None), Err(EnvError::HomeUnset));
        assert_eq!(home_from(Some(OsString::new())), Err(EnvError::HomeEmpty));
        for relative in ["rel", "./home/u", "~/u", " /home/u"] {
            assert_eq!(
                home_from(Some(relative.into())),
                Err(EnvError::HomeRelative(relative.into())),
                "{relative:?}"
            );
        }
        assert_eq!(
            home_from(Some("/home/u".into())),
            Ok(PathBuf::from("/home/u"))
        );
        assert_eq!(
            home_from(Some("/home/u/".into())),
            Ok(PathBuf::from("/home/u/")),
            "a trailing slash is fine"
        );
        assert_eq!(home_from(Some("/".into())), Ok(PathBuf::from("/")));
    }

    #[test]
    fn each_refusal_says_what_to_do_and_is_kind_env() {
        let cases = [
            (
                EnvError::HomeUnset,
                "HOME is not set; set it to the absolute path of your home directory",
            ),
            (
                EnvError::HomeEmpty,
                "HOME is empty; set it to the absolute path of your home directory",
            ),
            (
                EnvError::HomeRelative("rel\x1b[2J".into()),
                "HOME is \"rel\\u{1b}[2J\", which is not an absolute path; set it to the absolute path of your home directory",
            ),
        ];
        for (e, message) in cases {
            assert_eq!(e.to_string(), message);
            assert_eq!(e.kind(), "env", "{e:?}");
        }
    }
}
