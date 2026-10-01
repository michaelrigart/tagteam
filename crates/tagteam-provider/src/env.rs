use std::ffi::OsString;
use std::path::PathBuf;

use crate::cancel::Cancel;

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
    forbidden_root: Option<PathBuf>,
}

impl Env {
    pub fn from_process() -> Self {
        let abs = |k: &str| {
            std::env::var_os(k)
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
        };
        Self {
            home: std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| "/".into()),
            user: std::env::var("USER").ok(),
            xdg_config_home: abs("XDG_CONFIG_HOME"),
            xdg_data_home: abs("XDG_DATA_HOME"),
            xdg_state_home: abs("XDG_STATE_HOME"),
            claude_config_dir: std::env::var_os("CLAUDE_CONFIG_DIR"),
            claude_securestorage_config_dir: std::env::var_os("CLAUDE_SECURESTORAGE_CONFIG_DIR"),
            cancel: Cancel::new(),
            forbidden_root: None,
        }
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
            forbidden_root: Some(real_home),
        }
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

    /// `CLAUDE_CONFIG_DIR` under tagteam's `sessions/`: every command that changes accounts or
    /// the live login refuses there (§9.2, B.32).
    pub fn inside_run_shell(&self) -> bool {
        self.claude_config_dir
            .as_ref()
            .is_some_and(|d| PathBuf::from(d).starts_with(self.data_dir().join("sessions")))
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
    fn a_run_shell_is_detected_from_claude_config_dir() {
        let mut env = Env::for_test(Path::new("/tmp/fixture"));
        assert!(!env.inside_run_shell());
        env.claude_config_dir = Some("/tmp/fixture/home/.local/share/tagteam/sessions/0192".into());
        assert!(env.inside_run_shell());
    }
}
