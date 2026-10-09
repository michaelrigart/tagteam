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
use tagteam_provider::security::{ProcessRunner, Runner, SecurityCli};
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

/// The search list `security list-keychains -d user` answered with (`rc`, `out`), as a
/// snapshot to restore. A nonzero exit is a failed read, and an empty list cannot be told from
/// one (a user search list always holds a keychain), so neither is a snapshot: restoring either
/// would empty the user's list.
fn snapshot_from(rc: i32, out: &str) -> Result<Vec<String>, HarnessError> {
    if rc != 0 {
        return Err(harness(format!(
            "security list-keychains -d user exited {rc}, so the keychain search list cannot be snapshotted"
        )));
    }
    let list = parse_search_list(out);
    if list.is_empty() {
        return Err(harness(
            "security list-keychains -d user listed no keychain, so the keychain search list cannot be snapshotted",
        ));
    }
    Ok(list)
}

fn search_list() -> Result<Vec<String>, HarnessError> {
    let (rc, out) = security(&["list-keychains", "-d", "user"])?;
    snapshot_from(rc, &out)
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

/// The `security` operations keychain creation needs, so that its failure paths can be tested
/// without touching a real keychain.
trait SecurityTool {
    fn search_list(&self) -> Result<Vec<String>, HarnessError>;
    fn set_search_list(&self, list: &[String]) -> Result<(), HarnessError>;
    fn create_keychain(&self, password: &str, path: &Path) -> Result<(), HarnessError>;
    /// Best effort: the keychain, and its file, are gone afterwards or were never there.
    fn delete_keychain(&self, path: &Path);
}

struct RealSecurity;

impl SecurityTool for RealSecurity {
    fn search_list(&self) -> Result<Vec<String>, HarnessError> {
        search_list()
    }

    fn set_search_list(&self, list: &[String]) -> Result<(), HarnessError> {
        let mut args = vec!["list-keychains", "-d", "user", "-s"];
        args.extend(list.iter().map(String::as_str));
        match security(&args)? {
            (0, _) => Ok(()),
            (rc, _) => Err(harness(format!(
                "security list-keychains -d user -s exited {rc}"
            ))),
        }
    }

    fn create_keychain(&self, password: &str, path: &Path) -> Result<(), HarnessError> {
        security_line(&format!(
            "create-keychain -p \"{password}\" {}",
            quoted(path)?
        ))
    }

    fn delete_keychain(&self, path: &Path) {
        let _ = security(&["delete-keychain", &path.to_string_lossy()]);
        let _ = fs::remove_file(path);
    }
}

/// Puts the user's keychain search list back as `before` had it. `create-keychain` may add the
/// new file to it, where tagteam's own `-s tagteam` probe and purge would meet the compat
/// keychain.
fn restore_search_list(tool: &dyn SecurityTool, before: &[String]) -> Result<(), HarnessError> {
    if tool.search_list()? != before {
        tool.set_search_list(before)?;
        if tool.search_list()? != before {
            return Err(harness(
                "could not restore the keychain search list after creating a compat keychain",
            ));
        }
    }
    Ok(())
}

/// Formed before `create-keychain` runs: unless `disarm`ed, it puts the search list back,
/// deletes the keychain file and removes `also`, whatever way creation ended (an error, a `?`
/// or a panic).
struct CreationGuard<'a> {
    tool: &'a dyn SecurityTool,
    path: &'a Path,
    before: Vec<String>,
    also: Option<&'a Path>,
    armed: bool,
}

impl<'a> CreationGuard<'a> {
    fn new(
        tool: &'a dyn SecurityTool,
        path: &'a Path,
        also: Option<&'a Path>,
    ) -> Result<Self, HarnessError> {
        Ok(Self {
            tool,
            path,
            before: tool.search_list()?,
            also,
            armed: true,
        })
    }

    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for CreationGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = restore_search_list(self.tool, &self.before);
            self.tool.delete_keychain(self.path);
            if let Some(also) = self.also {
                let _ = fs::remove_file(also);
            }
        }
    }
}

/// The compat vault's keychain file (Decision 16): the test account's vault, which a
/// `test-support` `tagteam` reaches through `TAGTEAM_TEST_VAULT_KEYCHAIN`. Its password is a
/// generated one, kept beside it at 0600.
pub struct VaultKeychain {
    pub path: PathBuf,
    password_file: PathBuf,
}

impl VaultKeychain {
    /// Creates the file, never over an existing keychain or password file, and unlocks it.
    /// Whatever fails between `create-keychain` and the password being written leaves neither
    /// the keychain nor `vault.password` behind, and the search list as it was. A failed
    /// `unlock` comes after that, so it leaves both: a consistent vault, which the next
    /// `compat login` takes by the open path.
    pub fn create(path: &Path, password_file: &Path) -> Result<Unlocked<Self>, HarnessError> {
        Self::create_with(&RealSecurity, path, password_file)?.unlock()
    }

    fn create_with(
        tool: &dyn SecurityTool,
        path: &Path,
        password_file: &Path,
    ) -> Result<Self, HarnessError> {
        if password_file.exists() {
            return Err(harness(format!(
                "{} exists but the vault does not: delete it, or the compat store, and log in again",
                password_file.display()
            )));
        }
        // Before the guard: it deletes the keychain at `path`, which must be one this call made.
        if path.exists() {
            return Err(harness(format!(
                "{} exists but {} does not: delete it, or the compat store, and log in again",
                path.display(),
                password_file.display()
            )));
        }
        let password = random_hex()?;
        let guard = CreationGuard::new(tool, path, Some(password_file))?;
        tool.create_keychain(&password, path)?;
        if !path.is_file() {
            return Err(harness(format!(
                "security create-keychain made no file at {}",
                path.display()
            )));
        }
        restore_search_list(tool, &guard.before)?;
        // After the keychain exists: a failed create-keychain leaves no password behind.
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(password_file)?;
        f.write_all(password.as_bytes())?;
        guard.disarm();
        Ok(Self {
            path: path.to_path_buf(),
            password_file: password_file.to_path_buf(),
        })
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
    /// Whatever fails after `create-keychain` leaves no file and the search list as it was.
    pub fn create(path: &Path) -> Result<Self, HarnessError> {
        Self::create_with(&RealSecurity, path)
    }

    fn create_with(tool: &dyn SecurityTool, path: &Path) -> Result<Self, HarnessError> {
        let guard = CreationGuard::new(tool, path, None)?;
        tool.create_keychain(&random_hex()?, path)?;
        restore_search_list(tool, &guard.before)?;
        guard.disarm();
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

/// The state of the keychain file `file` by the lock check's own probe (Appendix A.3), asked
/// of that file: the file the caller also locks and unlocks, never whichever keychain is the
/// default.
pub fn login_lock_state(file: &Path) -> LockState {
    lock_state_with(Box::new(ProcessRunner), file)
}

fn lock_state_with(runner: Box<dyn Runner>, file: &Path) -> LockState {
    SecurityCli::with_runner(runner, Some(file.to_path_buf())).lock_state()
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
    fn a_search_list_snapshot_needs_a_zero_exit_and_a_keychain() {
        // Codex pre-merge slice 9: an unchecked failed read looked like an empty list, which
        // restoration would then write back, emptying the user's search list.
        let list = "    \"/Users/t/Library/Keychains/login.keychain-db\"\n";
        assert_eq!(
            snapshot_from(0, list).unwrap(),
            ["/Users/t/Library/Keychains/login.keychain-db"]
        );
        assert!(snapshot_from(0, "").is_err(), "empty is not a snapshot");
        assert!(snapshot_from(0, "  \n").is_err());
        assert!(snapshot_from(1, "").is_err(), "a failed read");
        assert!(
            snapshot_from(1, list).is_err(),
            "a failed read, whatever it printed"
        );
        assert!(snapshot_from(-1, list).is_err(), "no exit code (a signal)");
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

    use std::cell::RefCell;

    /// A `security` double: the search list it holds, and how creation goes wrong.
    #[derive(Default)]
    struct Fake {
        list: RefCell<Vec<String>>,
        /// `create-keychain` adds the file to the search list, as it may.
        adds_to_list: bool,
        /// `create-keychain` writes the file, then reports failure.
        fails: bool,
        /// `list-keychains -s` has no effect.
        cannot_restore: bool,
        deleted: RefCell<Vec<PathBuf>>,
    }

    impl SecurityTool for Fake {
        fn search_list(&self) -> Result<Vec<String>, HarnessError> {
            Ok(self.list.borrow().clone())
        }

        fn set_search_list(&self, list: &[String]) -> Result<(), HarnessError> {
            if !self.cannot_restore {
                *self.list.borrow_mut() = list.to_vec();
            }
            Ok(())
        }

        fn create_keychain(&self, _password: &str, path: &Path) -> Result<(), HarnessError> {
            fs::write(path, b"keychain")?;
            if self.adds_to_list {
                self.list.borrow_mut().push(path.display().to_string());
            }
            if self.fails {
                return Err(harness("security create-keychain failed: nope"));
            }
            Ok(())
        }

        fn delete_keychain(&self, path: &Path) {
            self.deleted.borrow_mut().push(path.to_path_buf());
            self.list
                .borrow_mut()
                .retain(|l| l != &path.display().to_string());
            let _ = fs::remove_file(path);
        }
    }

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("xtask-kc-{tag}-{}", random_hex().unwrap()));
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn fake(adds_to_list: bool, fails: bool, cannot_restore: bool) -> Fake {
        Fake {
            list: RefCell::new(vec!["/login.keychain-db".into()]),
            adds_to_list,
            fails,
            cannot_restore,
            ..Fake::default()
        }
    }

    #[test]
    fn a_vault_whose_creation_fails_leaves_no_keychain_no_password_and_the_list_as_it_was() {
        use std::os::unix::fs::PermissionsExt as _;
        let d = dir("vault");
        let (kc, pw) = (d.join("vault.keychain-db"), d.join("vault.password"));
        for (tool, why) in [
            (fake(true, true, false), "create-keychain fails"),
            (fake(true, false, true), "the list cannot be restored"),
        ] {
            let e = VaultKeychain::create_with(&tool, &kc, &pw).err();
            assert!(e.is_some(), "{why}");
            assert!(!kc.exists() && !pw.exists(), "{why}");
            // Deleting the keychain takes it out of the list, restored or not.
            assert_eq!(*tool.list.borrow(), ["/login.keychain-db"], "{why}");
            assert_eq!(tool.deleted.borrow().len(), 1, "{why}");
        }
        // Success: the password follows the keychain, 0600, and the list is restored.
        let tool = fake(true, false, false);
        let made = VaultKeychain::create_with(&tool, &kc, &pw).unwrap();
        assert_eq!(made.path, kc);
        assert_eq!(fs::read_to_string(&pw).unwrap().len(), 32);
        assert_eq!(
            fs::metadata(&pw).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(*tool.list.borrow(), ["/login.keychain-db"]);
        assert!(tool.deleted.borrow().is_empty());
        // An existing password file is never written over, and nothing is created.
        let tool = fake(false, false, false);
        fs::remove_file(&kc).unwrap();
        assert!(VaultKeychain::create_with(&tool, &kc, &pw).is_err());
        assert!(!kc.exists());
        assert_eq!(fs::read_to_string(&pw).unwrap().len(), 32, "kept");
        // An existing keychain file is refused and left alone: the guard never deletes one
        // this call did not create.
        fs::remove_file(&pw).unwrap();
        fs::write(&kc, b"someone else's keychain").unwrap();
        let tool = fake(true, true, false);
        let e = VaultKeychain::create_with(&tool, &kc, &pw).err().unwrap().0;
        assert!(e.contains("exists but"), "{e}");
        assert_eq!(fs::read(&kc).unwrap(), b"someone else's keychain");
        assert!(
            tool.deleted.borrow().is_empty(),
            "no delete-keychain issued"
        );
        assert!(!pw.exists());
        assert_eq!(*tool.list.borrow(), ["/login.keychain-db"]);
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_throwaway_keychain_whose_creation_fails_is_deleted_and_the_list_restored() {
        let d = dir("throwaway");
        let kc = d.join("probe.keychain-db");
        let tool = fake(true, true, false);
        assert!(ThrowawayKeychain::create_with(&tool, &kc).is_err());
        assert!(!kc.exists());
        assert_eq!(*tool.list.borrow(), ["/login.keychain-db"]);
        let tool = fake(true, false, true);
        assert!(ThrowawayKeychain::create_with(&tool, &kc).is_err());
        assert!(
            !kc.exists(),
            "a list that cannot be restored deletes the file"
        );
        let tool = fake(true, false, false);
        let made = ThrowawayKeychain::create_with(&tool, &kc).unwrap();
        assert_eq!(made.0, kc);
        assert_eq!(*tool.list.borrow(), ["/login.keychain-db"]);
        assert!(kc.exists());
        std::mem::forget(made); // its Drop would run the real `security delete-keychain`
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn the_lock_state_is_asked_of_the_file_that_is_locked() {
        use std::sync::{Arc, Mutex};
        use std::time::Duration;

        use tagteam_provider::security::RunResult;
        /// A runner that answers `code` and records the arguments it was given.
        struct Rec(Arc<Mutex<Vec<Vec<String>>>>, i32);
        impl Runner for Rec {
            fn run(
                &self,
                _program: &str,
                args: &[String],
                _stdin: Option<&[u8]>,
                _timeout: Duration,
            ) -> RunResult {
                self.0.lock().unwrap().push(args.to_vec());
                RunResult::Exited {
                    code: self.1,
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                }
            }

            fn run_attached(&self, _program: &str, _args: &[String]) -> RunResult {
                unreachable!("the probe never attaches the terminal")
            }
        }
        let file = Path::new("/Users/t/Library/Keychains/login.keychain-db");
        for (code, state) in [
            (0, LockState::Unlocked),
            (36, LockState::Locked),
            (1, LockState::Unknown),
        ] {
            let seen = Arc::new(Mutex::new(Vec::new()));
            let got = lock_state_with(Box::new(Rec(seen.clone(), code)), file);
            assert_eq!(got, state);
            assert_eq!(
                *seen.lock().unwrap(),
                [["show-keychain-info", &file.display().to_string()]]
            );
        }
    }
}
