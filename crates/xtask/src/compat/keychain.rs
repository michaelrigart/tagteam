//! Keychain work the harness does itself, all through `/usr/bin/security` (Appendix A.3):
//! the compat vault's own keychain file, and the Claude Code items of its scratch homes, each
//! one named from a spelling the guard accepted (B.70).

use std::fs::{self, OpenOptions};
use std::io::{Read as _, Write as _};
use std::ops::Deref;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tagteam_cc::{ItemKind, keychain_account};
use tagteam_provider::keychain::Keychain as _;
use tagteam_provider::security::{ProcessRunner, SecurityCli};
use tagteam_provider::{LockState, Read};

use super::guard::{Roots, cc_env};
use super::sys::{HarnessError, harness};

pub const SECURITY: &str = "/usr/bin/security";
pub const LOGIN_KEYCHAIN: &str = "Library/Keychains/login.keychain-db";

/// `security <args>`; its exit code and standard output.
fn security(args: &[&str]) -> Result<(i32, String), HarnessError> {
    let out = Command::new(SECURITY)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| harness(format!("could not run security: {e}")))?;
    Ok((
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    ))
}

/// `security -i` with `line` on its standard input, so a password never reaches an argument
/// list that `ps` shows.
fn security_line(line: &str) -> Result<(), HarnessError> {
    let mut child = Command::new(SECURITY)
        .arg("-i")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| harness(format!("could not run security: {e}")))?;
    let mut stdin = child.stdin.take().expect("stdin is piped");
    stdin.write_all(format!("{line}\n").as_bytes())?;
    drop(stdin);
    let mut err = String::new();
    if let Some(mut e) = child.stderr.take() {
        e.read_to_string(&mut err)?;
    }
    let status = child.wait()?;
    // `security -i` exits 0 even when a command fails; the failure is on standard error.
    if status.success() && err.trim().is_empty() {
        Ok(())
    } else {
        let verb = line.split_whitespace().next().unwrap_or("");
        Err(harness(format!("security {verb} failed: {}", err.trim())))
    }
}

/// The paths `security list-keychains` prints, one quoted path per line.
pub fn parse_search_list(out: &str) -> Vec<String> {
    out.lines()
        .map(|l| l.trim().trim_matches('"').to_owned())
        .filter(|l| !l.is_empty())
        .collect()
}

fn search_list() -> Result<Vec<String>, HarnessError> {
    Ok(parse_search_list(
        &security(&["list-keychains", "-d", "user"])?.1,
    ))
}

/// 32 hex digits from `/dev/urandom`.
pub fn random_hex() -> Result<String, HarnessError> {
    let mut bytes = [0u8; 16];
    fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(hex::encode(bytes))
}

fn quoted(path: &Path) -> Result<String, HarnessError> {
    let s = path.to_string_lossy();
    if s.contains('"') || s.contains('\\') || s.contains('\n') {
        return Err(harness(format!("{s} cannot be quoted for security -i")));
    }
    Ok(format!("\"{s}\""))
}

/// The compat vault's keychain file (Decision 16): the test account's vault, which a
/// `test-support` `tagteam` reaches through `TAGTEAM_TEST_VAULT_KEYCHAIN`. Its password is a
/// generated one, kept beside it at 0600.
pub struct VaultKeychain {
    pub path: PathBuf,
    password_file: PathBuf,
}

impl VaultKeychain {
    /// Creates the file, never over an existing one. `create-keychain` may add it to the user's
    /// search list, where tagteam's own `-s tagteam` probe and purge would meet the compat
    /// vault; the list is put back as it was.
    pub fn create(path: &Path, password_file: &Path) -> Result<Unlocked<Self>, HarnessError> {
        let password = random_hex()?;
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(password_file)?;
        f.write_all(password.as_bytes())?;
        let before = search_list()?;
        security_line(&format!(
            "create-keychain -p \"{password}\" {}",
            quoted(path)?
        ))?;
        if !path.is_file() {
            return Err(harness(format!(
                "security create-keychain made no file at {}",
                path.display()
            )));
        }
        if search_list()? != before {
            let mut args = vec!["list-keychains", "-d", "user", "-s"];
            args.extend(before.iter().map(String::as_str));
            security(&args)?;
            if search_list()? != before {
                return Err(harness(
                    "could not restore the keychain search list after creating the compat vault",
                ));
            }
        }
        Self {
            path: path.to_path_buf(),
            password_file: password_file.to_path_buf(),
        }
        .unlock()
    }

    pub fn open(path: &Path, password_file: &Path) -> Result<Self, HarnessError> {
        if !path.is_file() || !password_file.is_file() {
            return Err(harness(format!(
                "no compat vault at {}: run `cargo xtask compat login` first",
                path.display()
            )));
        }
        Ok(Self {
            path: path.to_path_buf(),
            password_file: password_file.to_path_buf(),
        })
    }

    /// Unlocks it, and keeps it unlocked for the run: no lock after a timeout or on sleep. From
    /// the moment `unlock-keychain` succeeds it is held as `Unlocked`, which locks it again on
    /// every way out, the settings' own failure included.
    pub fn unlock(self) -> Result<Unlocked<Self>, HarnessError> {
        let password = fs::read_to_string(&self.password_file)?;
        security_line(&format!(
            "unlock-keychain -p \"{}\" {}",
            password.trim(),
            quoted(&self.path)?
        ))?;
        let path = self.path.to_string_lossy().into_owned();
        let unlocked = Unlocked(self);
        match security(&["set-keychain-settings", &path])? {
            (0, _) => Ok(unlocked),
            (rc, _) => Err(harness(format!("set-keychain-settings failed (rc {rc})"))),
        }
    }

    pub fn lock(&self) -> Result<(), HarnessError> {
        match security(&["lock-keychain", &self.path.to_string_lossy()])? {
            (0, _) => Ok(()),
            (rc, _) => Err(harness(format!("lock-keychain failed (rc {rc})"))),
        }
    }

    /// `/usr/bin/security` bound to this file, as tagteam's vault uses it.
    pub fn cli(&self) -> SecurityCli {
        SecurityCli::with_runner(Box::new(ProcessRunner), Some(self.path.clone()))
    }
}

/// What `Unlocked` locks again: the compat vault, or a test's double.
pub trait Relock {
    fn relock(&self) -> Result<(), HarnessError>;
}

impl Relock for VaultKeychain {
    fn relock(&self) -> Result<(), HarnessError> {
        self.lock()
    }
}

/// A keychain unlocked for the run, locked again when it goes: a return, a `?` or a panic,
/// whether or not teardown ran. A vault left unlocked with its auto-lock off would hold the
/// test account open until the next login. The normal path still locks it explicitly; a lock
/// of a locked keychain is harmless.
pub struct Unlocked<K: Relock>(K);

impl<K: Relock> Unlocked<K> {
    /// `keychain`, which has just been unlocked.
    pub fn new(keychain: K) -> Self {
        Self(keychain)
    }
}

impl<K: Relock> Deref for Unlocked<K> {
    type Target = K;

    fn deref(&self) -> &K {
        &self.0
    }
}

impl<K: Relock> Drop for Unlocked<K> {
    fn drop(&mut self) {
        let _ = self.0.relock();
    }
}

/// A throwaway keychain file in the scratch directory, for the locked-file probe.
pub struct ThrowawayKeychain(pub PathBuf);

impl ThrowawayKeychain {
    pub fn create(path: &Path) -> Result<Self, HarnessError> {
        let before = search_list()?;
        security_line(&format!(
            "create-keychain -p \"{}\" {}",
            random_hex()?,
            quoted(path)?
        ))?;
        if search_list()? != before {
            let mut args = vec!["list-keychains", "-d", "user", "-s"];
            args.extend(before.iter().map(String::as_str));
            security(&args)?;
        }
        Ok(Self(path.to_path_buf()))
    }

    pub fn cli(&self) -> SecurityCli {
        SecurityCli::with_runner(Box::new(ProcessRunner), Some(self.0.clone()))
    }

    pub fn lock(&self) -> Result<(), HarnessError> {
        match security(&["lock-keychain", &self.0.to_string_lossy()])? {
            (0, _) => Ok(()),
            (rc, _) => Err(harness(format!("lock-keychain failed (rc {rc})"))),
        }
    }
}

impl Drop for ThrowawayKeychain {
    fn drop(&mut self) {
        let _ = security(&["delete-keychain", &self.0.to_string_lossy()]);
    }
}

/// `YYYYMMDDhhmmssZ`, the `mdat` attribute `find-generic-password` prints, as epoch seconds.
pub fn parse_mdat(attributes: &str) -> Option<i64> {
    let line = attributes
        .lines()
        .find(|l| l.contains("\"mdat\"<timedate>="))?;
    let quoted = line.split('"').nth(3)?;
    let d = quoted.get(..14)?;
    if !d.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    tagteam_cc::usage::parse_iso8601(&format!(
        "{}-{}-{}T{}:{}:{}Z",
        &d[..4],
        &d[4..6],
        &d[6..8],
        &d[8..10],
        &d[10..12],
        &d[12..14]
    ))
}

/// One Claude Code item of a compat home, in the login keychain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CcItem {
    pub service: String,
    pub account: String,
}

impl CcItem {
    /// The item of `kind` CC names from `spelling`, once the guard has accepted the spelling.
    pub fn of(roots: &Roots, spelling: &str, kind: ItemKind) -> Result<Self, HarnessError> {
        let [oauth, managed] = roots.services(spelling)?;
        let service = match kind {
            ItemKind::OAuth => oauth,
            ItemKind::ManagedKey => managed,
        };
        roots.check_service(&service)?;
        let env = cc_env(spelling, &roots.home, roots.user.as_deref());
        Ok(Self {
            service,
            account: keychain_account(&env),
        })
    }

    fn cli() -> SecurityCli {
        SecurityCli::new()
    }

    pub fn exists(&self) -> Read<()> {
        Self::cli().exists(&self.service, &self.account)
    }

    pub fn read(&self) -> Read<Vec<u8>> {
        Self::cli().find(&self.service, &self.account)
    }

    pub fn write(&self, bytes: &[u8]) -> Result<(), HarnessError> {
        Self::cli()
            .upsert(&self.service, &self.account, bytes)
            .map_err(|e| harness(format!("writing {}: {e}", self.service)))
    }

    pub fn delete(&self) -> Result<(), HarnessError> {
        Self::cli()
            .delete(&self.service, &self.account)
            .map_err(|e| harness(format!("deleting {}: {e}", self.service)))
    }

    /// When the item was last written, to the second (its `mdat`), if it exists.
    pub fn modified(&self) -> Option<i64> {
        let (rc, out) = security(&[
            "find-generic-password",
            "-a",
            &self.account,
            "-s",
            &self.service,
        ])
        .ok()?;
        (rc == 0).then(|| parse_mdat(&out)).flatten()
    }
}

/// The login keychain's state by the lock check (Appendix A.3).
pub fn login_lock_state() -> LockState {
    SecurityCli::new().lock_state()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_search_list_is_one_quoted_path_per_line() {
        let out = "    \"/Users/t/Library/Keychains/login.keychain-db\"\n    \"/Library/Keychains/System.keychain\"\n";
        assert_eq!(
            parse_search_list(out),
            [
                "/Users/t/Library/Keychains/login.keychain-db",
                "/Library/Keychains/System.keychain"
            ]
        );
    }

    #[test]
    fn an_item_s_mdat_reads_as_epoch_seconds() {
        let attrs = "keychain: \"/x\"\nattributes:\n    \"mdat\"<timedate>=0x32303236313030323132303030305A00  \"20261002120000Z\\000\"\n";
        assert_eq!(parse_mdat(attrs), Some(1_790_942_400));
        assert_eq!(parse_mdat("attributes:\n"), None);
    }

    #[test]
    fn an_unlocked_vault_is_locked_again_on_every_way_out() {
        use std::cell::Cell;
        use std::rc::Rc;
        /// A vault double: the number of locks it saw.
        struct Double(Rc<Cell<u32>>);
        impl Relock for Double {
            fn relock(&self) -> Result<(), HarnessError> {
                self.0.set(self.0.get() + 1);
                Ok(())
            }
        }
        let locks = Rc::new(Cell::new(0));
        // An early `Err` after the unlock.
        let run = |locks: &Rc<Cell<u32>>| -> Result<(), HarnessError> {
            let _vault = Unlocked(Double(locks.clone()));
            Err(harness("claude is not on PATH"))?;
            Ok(())
        };
        assert!(run(&locks).is_err());
        assert_eq!(locks.get(), 1, "the `?` locked it");
        // A panic.
        let shared = locks.clone();
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _vault = Unlocked(Double(shared));
            panic!("a check panicked outside its catch_unwind");
        }));
        assert!(panicked.is_err());
        assert_eq!(locks.get(), 2, "the unwind locked it");
        // The normal path: teardown's own lock, then the guard's, which is harmless.
        {
            let vault = Unlocked(Double(locks.clone()));
            vault.relock().unwrap();
        }
        assert_eq!(locks.get(), 4);
    }

    #[test]
    fn random_hex_is_32_digits() {
        let h = random_hex().unwrap();
        assert_eq!(h.len(), 32);
        assert_ne!(h, random_hex().unwrap());
    }
}
