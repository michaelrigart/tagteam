//! The checks of §15.4's `cargo xtask compat` list, one function each, in run order. Each needs
//! the real `claude` and the test account, so each is verified only by a live run, as M4b's
//! live acceptance is; the helpers below are unit-tested.

mod live;
mod opt_in;
mod profile;
mod standalone;

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, IsTerminal as _, Write as _};
use std::os::fd::RawFd;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde_json::Value;
use tagteam_provider::{Cancel, SessionRecord, parse_session_record};

use super::ctx::Ctx;
use super::registry::{Meta, OptIn, Phase};
use super::report::Outcome;
use super::sys::{HarnessError, cancel, interrupted, wait_until};

pub type CheckFn = fn(&mut Ctx) -> Result<Outcome, HarnessError>;

pub struct Check {
    pub meta: Meta,
    pub run: CheckFn,
}

const fn check(
    id: &'static str,
    title: &'static str,
    phase: Phase,
    macos_only: bool,
    opt_in: Option<OptIn>,
    run: CheckFn,
) -> Check {
    Check {
        meta: Meta {
            id,
            title,
            phase,
            macos_only,
            opt_in,
        },
        run,
    }
}

/// Every check, by phase, in the order a run makes them.
pub const CHECKS: &[Check] = &[
    check(
        "locked-file-probe",
        "the existence probe's answer on a locked keychain file (Appendix A.3)",
        Phase::Standalone,
        true,
        None,
        standalone::locked_file_probe,
    ),
    check(
        "auth-status",
        "`claude auth status --json`: every §12.3 outcome, the exported spelling, and a setup token's authMethod",
        Phase::Profile,
        false,
        None,
        profile::auth_status,
    ),
    check(
        "auth-status-read-only",
        "`claude auth status` writes nothing in the home it inspects (§13.6)",
        Phase::Profile,
        false,
        None,
        profile::auth_status_read_only,
    ),
    check(
        "profile-keychain-item",
        "the profile's item is named from the exported spelling, and CC moves a bootstrapped `.credentials.json` into it",
        Phase::Profile,
        true,
        None,
        profile::profile_keychain_item,
    ),
    check(
        "expires-at-integer",
        "CC writes `expiresAt` as an integer",
        Phase::Profile,
        false,
        None,
        profile::expires_at_integer,
    ),
    check(
        "shared-writes",
        "CC writes `settings.json` and appends `history.jsonl` through one link; what it does to `CLAUDE.md` and `keybindings.json`",
        Phase::Profile,
        false,
        None,
        profile::shared_writes,
    ),
    check(
        "session-records",
        "session records: written at start, removed on SIGINT, SIGTERM and SIGHUP, an `lstart` procStart; a `claude --bg` daemon's record",
        Phase::Profile,
        false,
        None,
        profile::session_records,
    ),
    check(
        "storage-write-lock",
        "CC waits for tagteam's storage-write lock, and tagteam for CC's",
        Phase::Profile,
        false,
        None,
        profile::storage_write_lock,
    ),
    check(
        "fresh-global-config",
        "CC accepts a global config tagteam created (§9.5), and its next write keeps every span as tagteam renders it",
        Phase::Live,
        false,
        None,
        live::fresh_global_config,
    ),
    check(
        "config-lock",
        "CC honours the global config's lock around its own writes (§9.1)",
        Phase::Live,
        false,
        None,
        live::config_lock,
    ),
    check(
        "refresh-lock-interop",
        "lock interop while CC refreshes, CC first and tagteam first (§7.5, §9.1)",
        Phase::Live,
        false,
        None,
        live::refresh_lock_interop,
    ),
    check(
        "hot-reload",
        "hot reload after a switch: by the file's mtime, and from the Keychain within 30 s",
        Phase::Live,
        false,
        None,
        live::hot_reload,
    ),
    check(
        "api-key-entry",
        "CC runs on an API key while the credential entry keeps only machine-shared keys (§9.4)",
        Phase::Live,
        false,
        None,
        live::api_key_entry,
    ),
    check(
        "managed-key-precedence",
        "the managed-key item or `primaryApiKey` first, and `primaryApiKey` on the next message (§9.4)",
        Phase::Live,
        true,
        None,
        live::managed_key_precedence,
    ),
    check(
        "ssh-keychain-read",
        "CC reads tagteam-written Keychain items silently over SSH (§17 R1)",
        Phase::Live,
        true,
        Some(OptIn::Ssh),
        opt_in::ssh_keychain_read,
    ),
    check(
        "locked-login-keychain",
        "the lock check on a locked login keychain in a GUI session (§17 O3)",
        Phase::Live,
        true,
        Some(OptIn::LockedKeychain),
        opt_in::locked_login_keychain,
    ),
];

pub fn metas() -> Vec<Meta> {
    CHECKS.iter().map(|c| c.meta).collect()
}

/// The session records in `dir` (`<home>/sessions`, Appendix A.7) that parse, with their paths.
pub fn records(dir: &Path) -> Vec<(PathBuf, SessionRecord)> {
    let Ok(listing) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(PathBuf, SessionRecord)> = listing
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| {
            let r = parse_session_record(&fs::read(&p).ok()?).ok()?;
            Some((p, r))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// The first record in `dir` that is not in `before`, waiting up to `timeout` for one.
pub fn new_record(
    dir: &Path,
    before: &[(PathBuf, SessionRecord)],
    timeout: Duration,
) -> Option<(PathBuf, SessionRecord)> {
    let mut found = None;
    wait_until(timeout, || {
        found = records(dir)
            .into_iter()
            .find(|(p, _)| !before.iter().any(|(q, _)| q == p));
        found.is_some()
    });
    found
}

/// `ps -o lstart=` for `pid`, as CC records it: `LC_ALL=C`, `TZ=UTC` (Appendix A.7).
pub fn lstart(pid: u32) -> Option<String> {
    let out = Command::new("ps")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .env_clear()
        .env("PATH", "/bin:/usr/bin")
        .env("LC_ALL", "C")
        .env("TZ", "UTC")
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (out.status.success() && !s.is_empty()).then_some(s)
}

/// Every entry under `root`, links not followed, by its path relative to `root`: its type,
/// size, mode, modification time and link target. Two snapshots differ wherever anything was
/// created, changed or removed.
pub fn snapshot(root: &Path) -> io::Result<BTreeMap<String, String>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) -> io::Result<()> {
        for entry in fs::read_dir(dir)? {
            let path = entry?.path();
            let m = fs::symlink_metadata(&path)?;
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .display()
                .to_string();
            let kind = if m.file_type().is_symlink() {
                format!("link to {}", fs::read_link(&path)?.display())
            } else if m.is_dir() {
                "dir".to_owned()
            } else {
                "file".to_owned()
            };
            out.insert(
                rel,
                format!(
                    "{kind}, {} bytes, mode {:o}, mtime {}.{:09}",
                    m.len(),
                    m.mode() & 0o7777,
                    m.mtime(),
                    m.mtime_nsec()
                ),
            );
            if m.is_dir() {
                walk(root, &path, out)?;
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    let m = fs::symlink_metadata(root)?;
    out.insert(
        ".".to_owned(),
        format!("dir, mtime {}.{:09}", m.mtime(), m.mtime_nsec()),
    );
    walk(root, root, &mut out)?;
    Ok(out)
}

/// What differs between two snapshots, one line per entry.
pub fn changes(before: &BTreeMap<String, String>, after: &BTreeMap<String, String>) -> Vec<String> {
    let mut out = Vec::new();
    for (path, was) in before {
        match after.get(path) {
            None => out.push(format!("removed {path}")),
            Some(now) if now != was => out.push(format!("changed {path}: {was} -> {now}")),
            Some(_) => {}
        }
    }
    for path in after.keys().filter(|p| !before.contains_key(*p)) {
        out.push(format!("created {path}"));
    }
    out
}

/// The keys of a JSON object, sorted.
pub fn keys(v: &Value) -> Vec<String> {
    let mut k: Vec<String> = v
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();
    k.sort();
    k
}

pub fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

/// How a yes-or-no question ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Answer {
    Yes,
    /// Anything but a yes, the end of input included.
    No,
    /// There was no terminal to ask on, or it could not be read.
    NoTerminal,
    /// The run's `Cancel` recorded this signal while the question waited.
    Interrupted(i32),
}

impl Answer {
    /// `Some(yes)` for an answer, `None` for no terminal, and the run's cancellation error for
    /// an interruption, so the caller's `?` unwinds through its cleanup (`with_login_locked`).
    pub fn decided(self) -> Result<Option<bool>, HarnessError> {
        match self {
            Self::Yes => Ok(Some(true)),
            Self::No => Ok(Some(false)),
            Self::NoTerminal => Ok(None),
            Self::Interrupted(n) => Err(interrupted(n)),
        }
    }
}

/// A yes-or-no question on the terminal, waiting for the answer as a cancellation point.
pub fn ask(question: &str) -> Answer {
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        return Answer::NoTerminal;
    }
    eprint!("{question} [y/N] ");
    let _ = io::stderr().flush();
    read_answer(libc::STDIN_FILENO, cancel())
}

/// How long one `poll(2)` waits before the token is looked at again.
const ASK_SLICE_MS: libc::c_int = 200;

/// One line from `fd`, read a byte at a time so nothing past it is taken from the terminal,
/// waiting in `ASK_SLICE_MS` slices and looking at `token` between them: a blocking read would
/// not notice a signal, which only sets the token (`catch_signals`).
fn read_answer(fd: RawFd, token: &Cancel) -> Answer {
    let mut line = Vec::new();
    loop {
        if let Some(n) = token.requested() {
            return Answer::Interrupted(n);
        }
        let mut fds = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `fds` is one valid, writable `pollfd` for the duration of the call.
        let ready = unsafe { libc::poll(&mut fds, 1, ASK_SLICE_MS) };
        if ready < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Answer::NoTerminal;
        }
        if ready == 0 {
            continue;
        }
        if fds.revents & libc::POLLNVAL != 0 {
            return Answer::NoTerminal;
        }
        let mut byte = 0u8;
        // SAFETY: `byte` is one valid, writable byte for the duration of the call.
        let n = unsafe { libc::read(fd, (&raw mut byte).cast(), 1) };
        match n {
            1 if byte == b'\n' => break,
            1 => line.push(byte),
            0 => break,
            _ if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted => {}
            _ => return Answer::NoTerminal,
        }
    }
    let line = String::from_utf8_lossy(&line);
    if matches!(line.trim(), "y" | "Y" | "yes") {
        Answer::Yes
    } else {
        Answer::No
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A pipe standing for stdin: (read end, write end).
    fn pipe() -> (RawFd, RawFd) {
        let mut fds = [0 as libc::c_int; 2];
        // SAFETY: `fds` holds the two descriptors `pipe(2)` writes.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        (fds[0], fds[1])
    }

    fn write_all(fd: RawFd, bytes: &[u8]) {
        // SAFETY: `bytes` is valid for its length for the duration of the call.
        let n = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
        assert_eq!(n, bytes.len() as isize);
    }

    fn close(fds: &[RawFd]) {
        for &fd in fds {
            // SAFETY: each descriptor was opened by `pipe` and is closed once.
            unsafe { libc::close(fd) };
        }
    }

    #[test]
    fn an_answer_is_read_up_to_its_line_and_no_further() {
        let (r, w) = pipe();
        write_all(w, b"y\nn\n");
        let token = Cancel::new();
        assert_eq!(read_answer(r, &token), Answer::Yes);
        assert_eq!(read_answer(r, &token), Answer::No, "the next line was left");
        write_all(w, b"yes\n");
        assert_eq!(read_answer(r, &token), Answer::Yes);
        write_all(w, b"maybe\n");
        assert_eq!(read_answer(r, &token), Answer::No);
        write_all(w, b"Y");
        close(&[w]);
        assert_eq!(
            read_answer(r, &token),
            Answer::Yes,
            "the end of input ends the line"
        );
        assert_eq!(read_answer(r, &token), Answer::No, "and nothing is a no");
        close(&[r]);
    }

    #[test]
    fn a_cancelled_token_ends_the_question_without_any_input() {
        let (r, w) = pipe();
        let token = Cancel::new();
        token.request(libc::SIGINT);
        assert_eq!(read_answer(r, &token), Answer::Interrupted(libc::SIGINT));
        // Input already waiting does not outrank the signal either.
        write_all(w, b"y\n");
        assert_eq!(read_answer(r, &token), Answer::Interrupted(libc::SIGINT));
        close(&[r, w]);
    }

    #[test]
    fn a_signal_arriving_while_the_question_waits_ends_it() {
        let (r, w) = pipe();
        let token = Cancel::new();
        let later = token.clone();
        let signaller = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            later.request(libc::SIGTERM);
        });
        assert_eq!(read_answer(r, &token), Answer::Interrupted(libc::SIGTERM));
        signaller.join().unwrap();
        close(&[r, w]);
    }

    #[test]
    fn a_descriptor_that_is_not_open_is_no_terminal() {
        let (r, w) = pipe();
        close(&[r, w]);
        assert_eq!(read_answer(r, &Cancel::new()), Answer::NoTerminal);
    }

    #[test]
    fn only_an_interruption_is_an_error_and_only_no_terminal_is_none() {
        assert_eq!(Answer::Yes.decided().unwrap(), Some(true));
        assert_eq!(Answer::No.decided().unwrap(), Some(false));
        assert_eq!(Answer::NoTerminal.decided().unwrap(), None);
        let err = Answer::Interrupted(libc::SIGINT).decided().unwrap_err();
        assert_eq!(err.to_string(), interrupted(libc::SIGINT).to_string());
    }

    #[test]
    fn every_check_is_listed_once_in_phase_order() {
        let ids: Vec<&str> = CHECKS.iter().map(|c| c.meta.id).collect();
        assert_eq!(
            ids,
            [
                "locked-file-probe",
                "auth-status",
                "auth-status-read-only",
                "profile-keychain-item",
                "expires-at-integer",
                "shared-writes",
                "session-records",
                "storage-write-lock",
                "fresh-global-config",
                "config-lock",
                "refresh-lock-interop",
                "hot-reload",
                "api-key-entry",
                "managed-key-precedence",
                "ssh-keychain-read",
                "locked-login-keychain",
            ]
        );
        assert!(
            CHECKS
                .windows(2)
                .all(|w| w[0].meta.phase <= w[1].meta.phase)
        );
        let opt_in: Vec<&str> = CHECKS
            .iter()
            .filter(|c| c.meta.opt_in.is_some())
            .map(|c| c.meta.id)
            .collect();
        assert_eq!(opt_in, ["ssh-keychain-read", "locked-login-keychain"]);
    }

    #[test]
    fn a_snapshot_sees_every_creation_change_and_removal() {
        let root = std::env::temp_dir().join(format!("xtask-snapshot-{}", std::process::id()));
        fs::create_dir_all(root.join("d")).unwrap();
        fs::write(root.join("d/f"), b"1").unwrap();
        fs::write(root.join("g"), b"1").unwrap();
        let before = snapshot(&root).unwrap();
        assert!(changes(&before, &snapshot(&root).unwrap()).is_empty());
        std::thread::sleep(Duration::from_millis(20));
        fs::write(root.join("d/f"), b"22").unwrap();
        fs::remove_file(root.join("g")).unwrap();
        std::os::unix::fs::symlink("d/f", root.join("l")).unwrap();
        let found = changes(&before, &snapshot(&root).unwrap());
        assert!(
            found.iter().any(|c| c.starts_with("changed d/f")),
            "{found:?}"
        );
        assert!(found.contains(&"removed g".to_owned()), "{found:?}");
        assert!(found.contains(&"created l".to_owned()), "{found:?}");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn keys_are_sorted_and_empty_for_a_non_object() {
        assert_eq!(keys(&json!({"b": 1, "a": 2})), ["a", "b"]);
        assert!(keys(&json!([1])).is_empty());
    }
}
