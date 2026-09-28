use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use tagteam_provider::Env;
use unicode_normalization::UnicodeNormalization;

pub(crate) fn nfc(v: &OsStr) -> String {
    v.to_string_lossy().nfc().collect()
}

fn set_dir(v: Option<&OsStr>) -> Option<PathBuf> {
    v.filter(|v| !v.is_empty()).map(PathBuf::from)
}

fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// Claude Code's path resolution (Appendix A.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CcPaths {
    pub config_home: PathBuf,
    pub global_config: PathBuf,
    pub secure_storage_dir: PathBuf,
    pub credentials_file: PathBuf,
    pub refresh_lock: PathBuf,
    pub config_lock: PathBuf,
}

impl CcPaths {
    pub fn resolve(env: &Env) -> Self {
        let config_dir = set_dir(env.claude_config_dir.as_deref());
        let config_home = config_dir
            .clone()
            .unwrap_or_else(|| env.home.join(".claude"));
        let legacy = config_home.join(".config.json");
        let global_config = if legacy.exists() {
            legacy
        } else {
            config_dir
                .unwrap_or_else(|| env.home.clone())
                .join(".claude.json")
        };
        let secure_storage_dir = match env.claude_securestorage_config_dir.as_deref() {
            Some(v) if v.is_empty() => env.home.join(".claude"),
            Some(v) => PathBuf::from(nfc(v)),
            None => config_home.clone(),
        };
        Self {
            credentials_file: env.guard(secure_storage_dir.join(".credentials.json")),
            refresh_lock: env.guard(secure_storage_dir.join(".oauth_refresh.lock")),
            config_lock: env.guard(with_suffix(&global_config, ".lock")),
            config_home: env.guard(config_home),
            global_config: env.guard(global_config),
            secure_storage_dir: env.guard(secure_storage_dir),
        }
    }

    /// `<realpath(secure-storage dir)>.lock`, or the unresolved path when realpath fails.
    pub fn legacy_lock(&self) -> PathBuf {
        let base = fs::canonicalize(&self.secure_storage_dir)
            .unwrap_or_else(|_| self.secure_storage_dir.clone());
        with_suffix(&base, ".lock")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn env(root: &Path) -> Env {
        Env::for_test(root)
    }

    #[test]
    fn defaults_follow_appendix_a1() {
        let d = tempfile::tempdir().unwrap();
        let h = d.path().join("home");
        let p = CcPaths::resolve(&env(d.path()));
        assert_eq!(p.config_home, h.join(".claude"));
        assert_eq!(p.global_config, h.join(".claude.json"));
        assert_eq!(p.secure_storage_dir, h.join(".claude"));
        assert_eq!(p.credentials_file, h.join(".claude/.credentials.json"));
        assert_eq!(p.refresh_lock, h.join(".claude/.oauth_refresh.lock"));
        assert_eq!(p.config_lock, h.join(".claude.json.lock"));
    }

    #[test]
    fn a_legacy_config_json_wins_when_present() {
        let d = tempfile::tempdir().unwrap();
        let claude = d.path().join("home/.claude");
        fs::create_dir_all(&claude).unwrap();
        fs::write(claude.join(".config.json"), "{}").unwrap();
        let p = CcPaths::resolve(&env(d.path()));
        assert_eq!(p.global_config, claude.join(".config.json"));
        assert_eq!(p.config_lock, claude.join(".config.json.lock"));
    }

    #[test]
    fn claude_config_dir_moves_home_and_config_but_empty_means_unset() {
        let d = tempfile::tempdir().unwrap();
        let mut e = env(d.path());
        e.claude_config_dir = Some("/p".into());
        let p = CcPaths::resolve(&e);
        assert_eq!(p.config_home, Path::new("/p"));
        assert_eq!(p.global_config, Path::new("/p/.claude.json"));
        assert_eq!(p.credentials_file, Path::new("/p/.credentials.json"));
        e.claude_config_dir = Some("".into());
        assert_eq!(
            CcPaths::resolve(&e).config_home,
            d.path().join("home/.claude")
        );
    }

    #[test]
    fn secure_storage_dir_anchors_credentials_and_locks() {
        let d = tempfile::tempdir().unwrap();
        let mut e = env(d.path());
        e.claude_config_dir = Some("/p".into());
        e.claude_securestorage_config_dir = Some("".into());
        let p = CcPaths::resolve(&e);
        assert_eq!(p.secure_storage_dir, d.path().join("home/.claude"));
        assert_eq!(
            p.credentials_file,
            d.path().join("home/.claude/.credentials.json")
        );
        e.claude_securestorage_config_dir = Some("/s".into());
        let p = CcPaths::resolve(&e);
        assert_eq!(p.refresh_lock, Path::new("/s/.oauth_refresh.lock"));
        assert_eq!(p.legacy_lock(), Path::new("/s.lock"));
    }

    #[test]
    fn the_legacy_lock_resolves_symlinks() {
        let d = tempfile::tempdir().unwrap();
        let real = d.path().join("real-claude");
        fs::create_dir_all(&real).unwrap();
        fs::create_dir_all(d.path().join("home")).unwrap();
        std::os::unix::fs::symlink(&real, d.path().join("home/.claude")).unwrap();
        let p = CcPaths::resolve(&env(d.path()));
        let mut expected = fs::canonicalize(&real).unwrap().into_os_string();
        expected.push(".lock");
        assert_eq!(p.legacy_lock(), PathBuf::from(expected));
    }
}
