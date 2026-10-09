//! B.70: `cargo xtask compat` never touches a Claude Code Keychain item without a scratch hash
//! suffix. Every CC home it uses is a directory it made, exported as `CLAUDE_CONFIG_DIR`, so
//! every item CC or tagteam names from it carries the hash of that spelling (Appendix A.2). An
//! unsuffixed item is the user's own login. This module refuses every spelling, spawn
//! environment and service that could name one of the user's items.

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use tagteam_cc::{ItemKind, keychain_service};
use tagteam_provider::Env;

pub const CONFIG_DIR: &str = "CLAUDE_CONFIG_DIR";
pub const SECURE_STORAGE_DIR: &str = "CLAUDE_SECURESTORAGE_CONFIG_DIR";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// A spawn without `CLAUDE_CONFIG_DIR`: CC and tagteam would name the default items.
    NoConfigDir,
    /// A spawn with `CLAUDE_SECURESTORAGE_CONFIG_DIR` defined, which alone decides the names.
    SecureStorageDefined,
    /// Empty, relative, or with an empty, `.` or `..` component, or a trailing `/`.
    NotPlain(String),
    /// Neither under this run's scratch directory nor under the compat store, as spelled or
    /// once its links are resolved.
    Outside(String),
    /// One of the user's own CC homes, or inside one.
    UsersHome { spelling: String, home: PathBuf },
    /// A Claude Code service without the 8-hex-digit suffix.
    Unsuffixed(String),
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::NoConfigDir => write!(
                f,
                "a spawn without {CONFIG_DIR} would use the default Keychain items"
            ),
            Refusal::SecureStorageDefined => write!(
                f,
                "a spawn defines {SECURE_STORAGE_DIR}, which names the Keychain items alone"
            ),
            Refusal::NotPlain(s) => write!(f, "{s:?} is not a plain absolute path"),
            Refusal::Outside(s) => write!(
                f,
                "{s} is outside the scratch directory and the compat store"
            ),
            Refusal::UsersHome { spelling, home } => {
                write!(
                    f,
                    "{spelling} is the user's own Claude Code home {}, or inside it",
                    home.display()
                )
            }
            Refusal::Unsuffixed(s) => write!(f, "the Keychain service {s:?} has no hash suffix"),
        }
    }
}

/// `true` for `Claude Code…-<8 lowercase hex digits>`, the only CC services compat may name.
/// `Claude Code-credentials`, `Claude Code` and `Claude Code-device-keys` are the user's.
pub fn is_suffixed(service: &str) -> bool {
    service.starts_with("Claude Code")
        && service.rsplit_once('-').is_some_and(|(_, hash)| {
            hash.len() == 8 && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        })
}

/// The `Env` CC and tagteam resolve `spelling` with: `CLAUDE_CONFIG_DIR` set to it, secure
/// storage undefined, the real `HOME` (which `/usr/bin/security` needs to find the login
/// keychain), and a path guard that panics on any path inside the user's `~/.claude`.
/// `for_test` is the one constructor that reads none of this process's CC variables.
pub fn cc_env(spelling: &str, home: &Path, user: Option<&str>) -> Env {
    let mut env = Env::for_test(home).with_forbidden_root(home.join(".claude"));
    env.home = home.to_path_buf();
    env.user = user.map(str::to_owned);
    env.claude_config_dir = Some(spelling.into());
    env.claude_securestorage_config_dir = None;
    env
}

/// What compat may use, and what it must never touch.
#[derive(Debug, Clone)]
pub struct Roots {
    /// This run's scratch directory, canonical.
    pub scratch: PathBuf,
    /// The compat store, canonical: the test account's tagteam data, its profiles included.
    pub state: PathBuf,
    /// The user's own CC homes: `~/.claude`, and `CLAUDE_CONFIG_DIR` and
    /// `CLAUDE_SECURESTORAGE_CONFIG_DIR` when this process found them set and not empty.
    pub users: Vec<PathBuf>,
    pub home: PathBuf,
    pub user: Option<String>,
}

impl Roots {
    fn inside(&self, path: &Path) -> bool {
        path.starts_with(&self.scratch) || path.starts_with(&self.state)
    }

    fn check_path(&self, spelling: &str, path: &Path) -> Result<(), Refusal> {
        if !self.inside(path) {
            return Err(Refusal::Outside(spelling.to_owned()));
        }
        if let Some(home) = self.users.iter().find(|h| path.starts_with(h)) {
            return Err(Refusal::UsersHome {
                spelling: spelling.to_owned(),
                home: home.clone(),
            });
        }
        Ok(())
    }

    /// B.70 for one CC home: the two services CC and tagteam name from `spelling` (OAuth and
    /// managed key), once the spelling is shown to be compat's own.
    pub fn services(&self, spelling: &str) -> Result<[String; 2], Refusal> {
        // By string: `Path::components` drops a `.` and a trailing `/`, which CC hashes.
        let plain = spelling.strip_prefix('/').is_some_and(|rest| {
            rest.split('/')
                .all(|c| !c.is_empty() && c != "." && c != "..")
        });
        if !plain {
            return Err(Refusal::NotPlain(spelling.to_owned()));
        }
        let path = Path::new(spelling);
        self.check_path(spelling, path)?;
        if let Ok(real) = fs::canonicalize(path) {
            self.check_path(spelling, &real)?;
        }
        let env = cc_env(spelling, &self.home, self.user.as_deref());
        let services = [
            keychain_service(&env, ItemKind::OAuth),
            keychain_service(&env, ItemKind::ManagedKey),
        ];
        match services.iter().find(|s| !is_suffixed(s)) {
            Some(s) => Err(Refusal::Unsuffixed(s.clone())),
            None => Ok(services),
        }
    }

    /// Every process compat starts: `CLAUDE_CONFIG_DIR` set to a spelling `services` accepts,
    /// and `CLAUDE_SECURESTORAGE_CONFIG_DIR` not defined at all (`vars` is the whole
    /// environment; the last assignment of a name wins).
    pub fn check_env(&self, vars: &[(OsString, OsString)]) -> Result<(), Refusal> {
        let get = |name: &str| vars.iter().rev().find(|(n, _)| n == name).map(|(_, v)| v);
        if get(SECURE_STORAGE_DIR).is_some() {
            return Err(Refusal::SecureStorageDefined);
        }
        let dir = get(CONFIG_DIR).ok_or(Refusal::NoConfigDir)?;
        self.services(&dir.to_string_lossy()).map(|_| ())
    }

    /// A service compat itself reads, writes or deletes.
    pub fn check_service(&self, service: &str) -> Result<(), Refusal> {
        if is_suffixed(service) {
            Ok(())
        } else {
            Err(Refusal::Unsuffixed(service.to_owned()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots(scratch: &str) -> Roots {
        Roots {
            scratch: PathBuf::from(scratch),
            state: PathBuf::from("/home/tester/.local/state/tagteam-compat"),
            users: vec![PathBuf::from("/home/tester/.claude")],
            home: PathBuf::from("/home/tester"),
            user: Some("tester".into()),
        }
    }

    fn var(k: &str, v: &str) -> (OsString, OsString) {
        (k.into(), v.into())
    }

    #[test]
    fn a_scratch_spelling_names_two_suffixed_items() {
        // The R1 spike's spelling and hash (naming.rs pins the same pair).
        assert_eq!(
            roots("/tmp").services("/tmp/tagteam-r1-spike").unwrap(),
            [
                "Claude Code-credentials-ba6c431d".to_owned(),
                "Claude Code-ba6c431d".to_owned()
            ]
        );
        let r = roots("/tmp/tagteam-compat.0a1b");
        assert!(r.services("/tmp/tagteam-compat.0a1b/live").is_ok());
        assert!(
            r.services("/home/tester/.local/state/tagteam-compat/xdg/data/tagteam/sessions/x")
                .is_ok(),
            "a profile in the compat store"
        );
    }

    #[test]
    fn spellings_that_could_name_the_user_s_items_are_refused() {
        let r = roots("/tmp/tagteam-compat.0a1b");
        for bad in [
            "",
            "/",
            "live",
            "/tmp/tagteam-compat.0a1b/../x",
            "/tmp/tagteam-compat.0a1b/./x",
            "/tmp/tagteam-compat.0a1b//x",
            "/tmp/tagteam-compat.0a1b/x/",
        ] {
            assert_eq!(
                r.services(bad),
                Err(Refusal::NotPlain(bad.into())),
                "{bad:?}"
            );
        }
        for outside in ["/tmp/other", "/home/tester/.claude", "/home/tester"] {
            assert_eq!(
                r.services(outside),
                Err(Refusal::Outside(outside.into())),
                "{outside}"
            );
        }
        let mut r = roots("/home/tester");
        r.users = vec![PathBuf::from("/home/tester/.claude")];
        assert_eq!(
            r.services("/home/tester/.claude/sub"),
            Err(Refusal::UsersHome {
                spelling: "/home/tester/.claude/sub".into(),
                home: PathBuf::from("/home/tester/.claude"),
            })
        );
    }

    #[test]
    fn a_link_out_of_the_roots_is_refused() {
        let base = std::env::temp_dir().join(format!("xtask-guard-{}", std::process::id()));
        let scratch = base.join("scratch");
        fs::create_dir_all(&scratch).unwrap();
        let scratch = fs::canonicalize(scratch).unwrap();
        let outside = fs::canonicalize(&base).unwrap();
        std::os::unix::fs::symlink(&outside, scratch.join("link")).unwrap();
        let mut r = roots(scratch.to_str().unwrap());
        r.home = outside.clone();
        let link = scratch.join("link");
        let spelled = link.to_str().unwrap();
        assert_eq!(r.services(spelled), Err(Refusal::Outside(spelled.into())));
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn every_spawn_exports_a_scratch_config_dir_and_no_secure_storage_dir() {
        let r = roots("/tmp/s");
        assert_eq!(
            r.check_env(&[var("HOME", "/home/tester")]),
            Err(Refusal::NoConfigDir)
        );
        assert!(r.check_env(&[var(CONFIG_DIR, "/tmp/s/live")]).is_ok());
        assert_eq!(
            r.check_env(&[var(CONFIG_DIR, "/tmp/s/live"), var(SECURE_STORAGE_DIR, "")]),
            Err(Refusal::SecureStorageDefined),
            "defined but empty names the default items"
        );
        assert_eq!(
            r.check_env(&[var(CONFIG_DIR, "/tmp/s/live"), var(CONFIG_DIR, "")]),
            Err(Refusal::NotPlain(String::new())),
            "the last assignment wins"
        );
    }

    #[test]
    fn only_hash_suffixed_services_pass() {
        for ok in ["Claude Code-credentials-ba6c431d", "Claude Code-ba6c431d"] {
            assert!(is_suffixed(ok), "{ok}");
        }
        for bad in [
            "Claude Code-credentials",
            "Claude Code",
            "Claude Code-device-keys",
            "Claude Code-credentials-BA6C431D",
            "Claude Code-credentials-ba6c431",
            "tagteam",
        ] {
            assert!(!is_suffixed(bad), "{bad}");
            assert!(roots("/tmp").check_service(bad).is_err());
        }
    }
}
