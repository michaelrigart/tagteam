use std::io::Write as _;
use std::os::unix::process::CommandExt as _;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::keychain::{
    Keychain, KeychainError, LockState, empty_service_err, empty_service_read, locked_err,
};
use crate::process::drain;
use crate::read::{Read, ReadError};

pub const SECURITY: &str = "/usr/bin/security";
/// The 4096-byte `security -i` line limit minus 64. An over-long line truncates silently and
/// leaves the old entry (Appendix A.3).
pub const LINE_LIMIT: usize = 4032;
const TIMEOUT: Duration = Duration::from_secs(5);
/// Appendix A.3: deleting by service deletes one item per call, at most this many times.
pub const DELETE_SERVICE_LIMIT: u32 = 10_000;

/// `stdout` of `find-generic-password -w` is a secret, so `Debug` shows lengths only.
#[derive(Clone)]
pub enum RunResult {
    Exited {
        code: i32,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
    /// The process did not finish in time: it hung (and was killed if it could be), or it exited but never
    /// closed its output pipes within the grace period.
    TimedOut,
    SpawnFailed(String),
}

impl std::fmt::Debug for RunResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunResult::Exited {
                code,
                stdout,
                stderr,
            } => f
                .debug_struct("Exited")
                .field("code", code)
                .field("stdout", &format_args!("<{} bytes>", stdout.len()))
                .field("stderr", &format_args!("<{} bytes>", stderr.len()))
                .finish(),
            RunResult::TimedOut => f.write_str("TimedOut"),
            RunResult::SpawnFailed(e) => f.debug_tuple("SpawnFailed").field(e).finish(),
        }
    }
}

pub trait Runner: Send + Sync {
    /// Runs without the terminal: stdin piped (`stdin` given) or null, stdout and stderr
    /// captured, killed after `timeout`. The child leads a process group of its own (§14.1),
    /// so a Ctrl-C at the terminal never reaches it and never cuts a Keychain write short.
    fn run(
        &self,
        program: &str,
        args: &[String],
        stdin: Option<&[u8]>,
        timeout: Duration,
    ) -> RunResult;
    /// Runs with the terminal attached: stdin and stderr inherited, the child's stdout sent to
    /// stderr (stdout is reserved for command output, §14). No timeout, because a person is
    /// answering. `Exited` carries no output. The child stays in tagteam's process group, the
    /// terminal's foreground one, so a Ctrl-C there reaches it too (§14.1).
    fn run_attached(&self, program: &str, args: &[String]) -> RunResult;
}

/// How long `run` waits for the output pipes to close once the child has exited. A grandchild
/// that inherited a pipe keeps it open after the child is gone; without a bound, `run` would
/// wait for that grandchild too (L343).
const DRAIN_GRACE: Duration = Duration::from_secs(2);

pub struct ProcessRunner;

/// What `wait_for` needs from a child process, so its failure paths are testable.
trait Waitable {
    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>>;
    fn kill(&mut self) -> std::io::Result<()>;
    fn wait(&mut self) -> std::io::Result<ExitStatus>;
}

impl Waitable for Child {
    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        Child::try_wait(self)
    }
    fn kill(&mut self) -> std::io::Result<()> {
        Child::kill(self)
    }
    fn wait(&mut self) -> std::io::Result<ExitStatus> {
        Child::wait(self)
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Waited {
    Exited(i32),
    TimedOut,
    Failed(String),
}

/// Polls until the child exits or `deadline` passes. A timeout and a failed poll both kill and
/// reap the child, so neither leaves a process running (L343), unless the kill itself fails.
fn wait_for(child: &mut dyn Waitable, deadline: Instant) -> Waited {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Waited::Exited(status.code().unwrap_or(-1)),
            Ok(None) if Instant::now() >= deadline => {
                reap(child);
                return Waited::TimedOut;
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(e) => {
                reap(child);
                return Waited::Failed(e.to_string());
            }
        }
    }
}

/// Kills the child and waits for it. A kill that fails leaves the child running, and `wait`
/// would block for as long as it does, so then it returns without waiting.
fn reap(child: &mut dyn Waitable) {
    if child.kill().is_ok() {
        let _ = child.wait();
    }
}

/// The drained bytes, or `None` when the pipe had not closed by `until`.
fn collect(rx: &mpsc::Receiver<Vec<u8>>, until: Instant) -> Option<Vec<u8>> {
    rx.recv_timeout(until.saturating_duration_since(Instant::now()))
        .ok()
}

impl ProcessRunner {
    fn run_bounded(
        program: &str,
        args: &[String],
        stdin: Option<&[u8]>,
        timeout: Duration,
        grace: Duration,
    ) -> RunResult {
        let mut child = match Command::new(program)
            .args(args)
            // §14.1: a group of its own, so a Ctrl-C at the terminal reaches tagteam alone and
            // never kills a Keychain write midway. Its stdin is a pipe or null and its output is
            // piped, so the background group never stops it for touching the terminal.
            .process_group(0)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => return RunResult::SpawnFailed(e.to_string()),
        };
        if let (Some(data), Some(mut pipe)) = (stdin, child.stdin.take()) {
            let data = data.to_vec();
            thread::spawn(move || {
                let _ = pipe.write_all(&data);
            });
        }
        let out = drain(child.stdout.take());
        let err = drain(child.stderr.take());
        match wait_for(&mut child, Instant::now() + timeout) {
            Waited::Exited(code) => {
                let until = Instant::now() + grace;
                match (collect(&out, until), collect(&err, until)) {
                    (Some(stdout), Some(stderr)) => RunResult::Exited {
                        code,
                        stdout,
                        stderr,
                    },
                    // Never hand back half an output as a whole one.
                    _ => RunResult::TimedOut,
                }
            }
            Waited::TimedOut => RunResult::TimedOut,
            Waited::Failed(e) => RunResult::SpawnFailed(e),
        }
    }
}

impl Runner for ProcessRunner {
    fn run(
        &self,
        program: &str,
        args: &[String],
        stdin: Option<&[u8]>,
        timeout: Duration,
    ) -> RunResult {
        Self::run_bounded(program, args, stdin, timeout, DRAIN_GRACE)
    }

    fn run_attached(&self, program: &str, args: &[String]) -> RunResult {
        match Command::new(program)
            .args(args)
            .stdin(Stdio::inherit())
            .stdout(std::io::stderr())
            .stderr(Stdio::inherit())
            .status()
        {
            Ok(status) => RunResult::Exited {
                code: status.code().unwrap_or(-1),
                stdout: vec![],
                stderr: vec![],
            },
            Err(e) => RunResult::SpawnFailed(e.to_string()),
        }
    }
}

/// Strips exactly one trailing `\n`. This no longer hex-decodes: `-w`'s rendering of a
/// genuinely non-printable secret and of a printable secret that happens to be all
/// lowercase hex digits of even length (e.g. a literal `"cafe"`) are byte-for-byte
/// identical, so that shape alone can never tell them apart. `SecurityCli::find` resolves
/// the ambiguity with a `-g` call before ever deciding to hex-decode (Appendix A.3).
pub fn decode_output(mut out: Vec<u8>) -> Vec<u8> {
    if out.last() == Some(&b'\n') {
        out.pop();
    }
    out
}

/// True when `bytes` could be `security`'s hex rendering of a non-printable secret, or
/// could just as well be a printable secret that happens to look like hex.
fn looks_like_hex(bytes: &[u8]) -> bool {
    !bytes.is_empty()
        && bytes.len() % 2 == 0
        && bytes
            .iter()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
}

/// A `-g` response that could not be trusted: missing, malformed, or disagreeing with
/// `-w`. Never built from `stderr` text, which is the channel `security` prints the
/// secret on.
const DISAMBIGUATION_MISMATCH: &str =
    "the -g disambiguation line was missing, malformed, or disagreed with -w";

/// Parses the `password: ...` line `-g` writes to stderr and cross-checks it against
/// `raw` (the `-w` bytes), returning the confirmed bytes: hex-decoded when `-g` says
/// binary, or `raw` unchanged when it says verbatim. `None` when the line is missing,
/// the hex digits are empty or odd in count, or the decoded/verbatim value disagrees
/// with `raw` — a mismatch is never trusted, e.g. because the item changed between the
/// two spawns.
fn confirm_password(stderr: &[u8], raw: &[u8]) -> Option<Vec<u8>> {
    let text = String::from_utf8_lossy(stderr);
    let rest = text.lines().find_map(|l| l.strip_prefix("password: "))?;
    let rest = rest.trim_end();
    if let Some(hex_part) = rest.strip_prefix("0x") {
        let digits: String = hex_part
            .chars()
            .take_while(char::is_ascii_hexdigit)
            .collect();
        if digits.is_empty() || digits.len() % 2 != 0 {
            return None;
        }
        let bytes = hex::decode(&digits).ok()?;
        let raw_str = std::str::from_utf8(raw).ok()?;
        // `-g` prints uppercase hex; `hex::encode` is always lowercase.
        return hex::encode(&bytes)
            .eq_ignore_ascii_case(raw_str)
            .then_some(bytes);
    }
    let body = rest.strip_prefix('"')?.strip_suffix('"')?;
    (body.as_bytes() == raw).then(|| raw.to_vec())
}

/// `disambiguate`'s own failures (a bad rc, a timeout, a spawn failure) never surface
/// `-g`'s stderr, unlike `describe()`/`unreadable()`: `-g`'s stderr is the channel
/// `security` prints the secret on. A timeout and a failure to run are different causes,
/// and say so (§14); a spawn failure's text is the operating system's, never `security`'s.
fn disambiguation_failed(r: &RunResult) -> ReadError {
    let detail = match r {
        RunResult::Exited { code, .. } => format!("rc {code}: the -g disambiguation call failed"),
        RunResult::TimedOut => "the -g disambiguation call did not finish in time".to_owned(),
        RunResult::SpawnFailed(e) => format!("the -g disambiguation call could not run: {e}"),
    };
    ReadError::new("keychain", detail)
}

pub struct SecurityCli {
    runner: Box<dyn Runner>,
    keychain_file: Option<PathBuf>,
}

impl SecurityCli {
    pub fn new() -> Self {
        Self {
            runner: Box::new(ProcessRunner),
            keychain_file: None,
        }
    }

    /// `keychain_file` targets a specific keychain (tests only); production searches the
    /// default list, as CC does.
    pub fn with_runner(runner: Box<dyn Runner>, keychain_file: Option<PathBuf>) -> Self {
        Self {
            runner,
            keychain_file,
        }
    }

    fn run(&self, mut args: Vec<String>, stdin: Option<&[u8]>) -> RunResult {
        if let Some(f) = &self.keychain_file {
            if stdin.is_none() {
                args.push(f.to_string_lossy().into_owned());
            }
        }
        self.runner.run(SECURITY, &args, stdin, TIMEOUT)
    }

    fn tail(&self) -> String {
        self.keychain_file
            .as_ref()
            .map(|f| format!(" \"{}\"", f.to_string_lossy()))
            .unwrap_or_default()
    }

    /// Resolves whether an item whose `-w` rendering looks like hex is actually binary
    /// (decode it) or a printable secret that happens to look like hex (keep `raw`), via
    /// one `find-generic-password -g` call on the same item, cross-checked against `raw`
    /// so a mismatch (e.g. the item changed between the two spawns) is never trusted
    /// (Appendix A.3).
    fn disambiguate(&self, service: &str, account: &str, raw: Vec<u8>) -> Read<Vec<u8>> {
        match self.run(
            s(&["find-generic-password", "-a", account, "-g", "-s", service]),
            None,
        ) {
            RunResult::Exited {
                code: 0, stderr, ..
            } => match confirm_password(&stderr, &raw) {
                Some(bytes) => Read::Present(bytes),
                None => Read::Unreadable(ReadError::new("keychain", DISAMBIGUATION_MISMATCH)),
            },
            other => Read::Unreadable(disambiguation_failed(&other)),
        }
    }
}

impl Default for SecurityCli {
    fn default() -> Self {
        Self::new()
    }
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| (*x).to_owned()).collect()
}

/// The `(rc, message)` pieces shared by every way a `RunResult` can fail. `rc` is `None`
/// unless the process actually exited, and `message` never repeats it.
fn describe(r: &RunResult) -> (Option<i32>, String) {
    match r {
        RunResult::Exited { code, stderr, .. } => (
            Some(*code),
            String::from_utf8_lossy(stderr).trim().to_owned(),
        ),
        RunResult::TimedOut => (
            None,
            "security did not finish in time (it hung or kept its output open)".to_owned(),
        ),
        RunResult::SpawnFailed(e) => (None, format!("could not run security: {e}")),
    }
}

fn unreadable(r: RunResult) -> ReadError {
    let (rc, message) = describe(&r);
    let detail = match rc {
        Some(code) => format!("rc {code}: {message}"),
        None => message,
    };
    ReadError::new("keychain", detail)
}

fn failed(r: RunResult) -> KeychainError {
    let (rc, detail) = describe(&r);
    KeychainError { rc, detail }
}

fn check_name(v: &str) -> Result<(), KeychainError> {
    if let Some(c) = v.chars().find(|&c| c == '"' || c == '\\' || c.is_control()) {
        return Err(KeychainError {
            rc: None,
            detail: format!("refusing an item name containing {c:?}: {v:?}"),
        });
    }
    Ok(())
}

impl Keychain for SecurityCli {
    fn find(&self, service: &str, account: &str) -> Read<Vec<u8>> {
        match self.run(
            s(&["find-generic-password", "-a", account, "-w", "-s", service]),
            None,
        ) {
            RunResult::Exited {
                code: 0, stdout, ..
            } => {
                let raw = decode_output(stdout);
                if looks_like_hex(&raw) {
                    self.disambiguate(service, account, raw)
                } else {
                    Read::Present(raw)
                }
            }
            RunResult::Exited { code: 44, .. } => Read::Absent,
            other => Read::Unreadable(unreadable(other)),
        }
    }

    fn exists(&self, service: &str, account: &str) -> Read<()> {
        match self.run(
            s(&["find-generic-password", "-a", account, "-s", service]),
            None,
        ) {
            RunResult::Exited { code: 0, .. } => Read::Present(()),
            RunResult::Exited { code: 44, .. } => Read::Absent,
            other => Read::Unreadable(unreadable(other)),
        }
    }

    fn upsert(&self, service: &str, account: &str, data: &[u8]) -> Result<(), KeychainError> {
        check_name(service)?;
        check_name(account)?;
        let hex = hex::encode(data);
        let line = format!(
            "add-generic-password -U -a \"{account}\" -s \"{service}\" -X \"{hex}\"{}\n",
            self.tail()
        );
        let result = if line.len() <= LINE_LIMIT {
            self.run(s(&["-i"]), Some(line.as_bytes()))
        } else {
            self.run(
                s(&[
                    "add-generic-password",
                    "-U",
                    "-a",
                    account,
                    "-s",
                    service,
                    "-X",
                    &hex,
                ]),
                None,
            )
        };
        match result {
            RunResult::Exited { code: 0, .. } => {}
            other => return Err(failed(other)),
        }
        match self.find(service, account) {
            Read::Present(v) if v == data => Ok(()),
            Read::Present(_) => Err(KeychainError {
                rc: None,
                detail: "the item did not read back as written".into(),
            }),
            Read::Absent => Err(KeychainError {
                rc: None,
                detail: "the item is missing after writing".into(),
            }),
            Read::Unreadable(e) => Err(KeychainError {
                rc: None,
                detail: e.to_string(),
            }),
        }
    }

    fn delete(&self, service: &str, account: &str) -> Result<(), KeychainError> {
        match self.run(
            s(&["delete-generic-password", "-a", account, "-s", service]),
            None,
        ) {
            RunResult::Exited { code: 0 | 44, .. } => Ok(()),
            other => Err(failed(other)),
        }
    }

    fn lock_state(&self) -> LockState {
        match self.run(s(&["show-keychain-info"]), None) {
            RunResult::Exited { code: 0, .. } => LockState::Unlocked,
            RunResult::Exited { code: 36, .. } => LockState::Locked,
            _ => LockState::Unknown,
        }
    }

    /// Appendix A.3's delete by service: `delete-generic-password -s <service>` without `-a`
    /// deletes one item of the service per call and returns rc 44 once none is left. It is
    /// repeated until then, at most `DELETE_SERVICE_LIMIT` times, and the attributes-only probe
    /// must then find none. A locked keychain (rc 36) stops it, named as such.
    fn delete_service(&self, service: &str) -> Result<u32, KeychainError> {
        if service.is_empty() {
            return Err(empty_service_err());
        }
        let mut deleted = 0;
        while deleted < DELETE_SERVICE_LIMIT {
            match self.run(s(&["delete-generic-password", "-s", service]), None) {
                RunResult::Exited { code: 0, .. } => deleted += 1,
                RunResult::Exited { code: 44, .. } => {
                    return match self.service_has_items(service) {
                        Read::Present(false) | Read::Absent => Ok(deleted),
                        Read::Present(true) => Err(KeychainError {
                            rc: None,
                            detail: format!(
                                "items of {service:?} are still present after deleting {deleted}"
                            ),
                        }),
                        Read::Unreadable(e) => Err(KeychainError {
                            rc: None,
                            detail: format!("could not verify that {service:?} is empty: {e}"),
                        }),
                    };
                }
                RunResult::Exited { code: 36, .. } => return Err(locked_err()),
                other => return Err(failed(other)),
            }
        }
        Err(KeychainError {
            rc: None,
            detail: format!(
                "{service:?}: stopped after {DELETE_SERVICE_LIMIT} deletions, the limit; whether any items remain was not checked"
            ),
        })
    }

    /// Appendix A.3: `find-generic-password -s <service>`, without `-a`, `-w` or `-g`: rc 0
    /// while any item of the service exists, rc 44 once none does. It never prompts.
    fn service_has_items(&self, service: &str) -> Read<bool> {
        if service.is_empty() {
            return empty_service_read();
        }
        match self.run(s(&["find-generic-password", "-s", service]), None) {
            RunResult::Exited { code: 0, .. } => Read::Present(true),
            RunResult::Exited { code: 44, .. } => Read::Present(false),
            other => Read::Unreadable(unreadable(other)),
        }
    }

    /// `unlock-keychain` with no `-p`: macOS reads the password from the terminal itself.
    fn unlock(&self) -> bool {
        let mut args = s(&["unlock-keychain"]);
        if let Some(f) = &self.keychain_file {
            args.push(f.to_string_lossy().into_owned());
        }
        matches!(
            self.runner.run_attached(SECURITY, &args),
            RunResult::Exited { code: 0, .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::fs;
    use std::io;
    use std::os::unix::process::ExitStatusExt;
    use std::sync::{Arc, Mutex};

    type Call = (String, Vec<String>, Option<Vec<u8>>);

    #[derive(Default, Clone)]
    struct Scripted {
        calls: Arc<Mutex<Vec<Call>>>,
        results: Arc<Mutex<VecDeque<RunResult>>>,
    }

    impl Scripted {
        fn then(self, r: RunResult) -> Self {
            self.results.lock().unwrap().push_back(r);
            self
        }
        fn calls(&self) -> Vec<Call> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Runner for Scripted {
        fn run(
            &self,
            program: &str,
            args: &[String],
            stdin: Option<&[u8]>,
            _t: Duration,
        ) -> RunResult {
            self.calls.lock().unwrap().push((
                program.into(),
                args.to_vec(),
                stdin.map(<[u8]>::to_vec),
            ));
            self.results
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected extra call")
        }
        fn run_attached(&self, program: &str, args: &[String]) -> RunResult {
            self.calls
                .lock()
                .unwrap()
                .push((format!("attached:{program}"), args.to_vec(), None));
            self.results
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected extra call")
        }
    }

    fn ok(stdout: &[u8]) -> RunResult {
        RunResult::Exited {
            code: 0,
            stdout: stdout.to_vec(),
            stderr: vec![],
        }
    }
    fn rc(code: i32) -> RunResult {
        RunResult::Exited {
            code,
            stdout: vec![],
            stderr: b"boom".to_vec(),
        }
    }
    fn cli(s: &Scripted, file: Option<&str>) -> SecurityCli {
        SecurityCli::with_runner(Box::new(s.clone()), file.map(PathBuf::from))
    }

    #[test]
    fn find_uses_the_absolute_binary_and_strips_one_newline() {
        let s = Scripted::default().then(ok(b"{\"a\":1}\n\n"));
        let r = cli(&s, None).find("svc", "acct");
        assert_eq!(r.present().unwrap(), b"{\"a\":1}\n");
        let (program, args, stdin) = &s.calls()[0];
        assert_eq!(program, "/usr/bin/security");
        assert_eq!(
            args,
            &["find-generic-password", "-a", "acct", "-w", "-s", "svc"]
        );
        assert!(stdin.is_none());
    }

    /// The argv of one call, with the keychain file a test targets appended, as `run` does.
    fn argv(args: &[&str], file: Option<&str>) -> Vec<String> {
        args.iter()
            .copied()
            .chain(file)
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn delete_service_deletes_until_rc_44_then_verifies_by_attributes_only() {
        for file in [None, Some("/tmp/t.keychain")] {
            let s = Scripted::default()
                .then(ok(b""))
                .then(ok(b""))
                .then(rc(44))
                .then(rc(44));
            assert_eq!(cli(&s, file).delete_service("tagteam").unwrap(), 2);
            let delete = argv(&["delete-generic-password", "-s", "tagteam"], file);
            let probe = argv(&["find-generic-password", "-s", "tagteam"], file);
            let calls: Vec<(String, Vec<String>, Option<Vec<u8>>)> = s.calls();
            assert_eq!(
                calls,
                vec![
                    (SECURITY.to_owned(), delete.clone(), None),
                    (SECURITY.to_owned(), delete.clone(), None),
                    (SECURITY.to_owned(), delete, None),
                    (SECURITY.to_owned(), probe, None),
                ],
                "{file:?}"
            );
        }
    }

    #[test]
    fn a_service_that_still_has_items_after_the_loop_is_an_error() {
        let s = Scripted::default().then(rc(44)).then(ok(b""));
        let err = cli(&s, None).delete_service("tagteam").unwrap_err();
        assert!(err.detail.contains("still present"), "{err}");
        let s = Scripted::default().then(rc(44)).then(RunResult::TimedOut);
        let err = cli(&s, None).delete_service("tagteam").unwrap_err();
        assert!(err.detail.contains("could not verify"), "{err}");
    }

    #[test]
    fn a_locked_keychain_stops_the_delete_by_service_at_once() {
        let s = Scripted::default().then(ok(b"")).then(rc(36));
        let err = cli(&s, None).delete_service("tagteam").unwrap_err();
        assert_eq!(err.rc, Some(36));
        assert!(err.to_string().contains("locked"), "{err}");
        assert_eq!(s.calls().len(), 2, "nothing after the lock");
    }

    #[test]
    fn any_other_failure_of_the_delete_loop_is_reported() {
        let s = Scripted::default().then(rc(51));
        assert_eq!(
            cli(&s, None).delete_service("tagteam").unwrap_err().rc,
            Some(51)
        );
        let s = Scripted::default().then(RunResult::TimedOut);
        assert_eq!(
            cli(&s, None).delete_service("tagteam").unwrap_err().rc,
            None
        );
    }

    #[test]
    fn delete_service_gives_up_after_ten_thousand_deletions() {
        let mut s = Scripted::default();
        for _ in 0..DELETE_SERVICE_LIMIT {
            s = s.then(ok(b""));
        }
        let err = cli(&s, None).delete_service("tagteam").unwrap_err();
        assert!(
            err.detail.contains(
                "stopped after 10000 deletions, the limit; whether any items remain was not checked"
            ),
            "{err}"
        );
        assert_eq!(
            s.calls().len(),
            DELETE_SERVICE_LIMIT as usize,
            "no call past the limit"
        );
    }

    #[test]
    fn an_empty_service_is_refused_before_any_call() {
        // `Scripted` panics on an unexpected call, and none is scripted.
        let s = Scripted::default();
        assert!(cli(&s, None).delete_service("").is_err());
        assert!(matches!(
            cli(&s, None).service_has_items(""),
            Read::Unreadable(_)
        ));
        assert!(s.calls().is_empty());
    }

    #[test]
    fn service_has_items_is_the_attributes_only_find_by_service() {
        let cases = [
            (ok(b""), Some(true)),
            (rc(44), Some(false)),
            (rc(36), None),
            (rc(1), None),
            (RunResult::TimedOut, None),
        ];
        for (result, want) in cases {
            let s = Scripted::default().then(result);
            let got = match cli(&s, None).service_has_items("tagteam") {
                Read::Present(b) => Some(b),
                Read::Absent => panic!("a probe is never Absent"),
                Read::Unreadable(_) => None,
            };
            assert_eq!(got, want);
            assert_eq!(
                s.calls()[0].1,
                ["find-generic-password", "-s", "tagteam"],
                "no -a, -w or -g"
            );
        }
    }

    #[test]
    fn decode_output_only_strips_one_trailing_newline() {
        // No hex-decoding here any more: `"cafe"` and the hex rendering of `[0xca, 0xfe]`
        // are the same bytes, so guessing from this text alone would corrupt one of them.
        assert_eq!(decode_output(b"cafe\n\n".to_vec()), b"cafe\n");
        assert_eq!(
            decode_output(b"sk-ant-api03-abc\n".to_vec()),
            b"sk-ant-api03-abc"
        );
        assert_eq!(decode_output(b"abc\n".to_vec()), b"abc");
    }

    fn og_hex(hex_upper: &str) -> RunResult {
        RunResult::Exited {
            code: 0,
            stdout: vec![],
            stderr: format!("password: 0x{hex_upper} \n").into_bytes(),
        }
    }
    fn og_verbatim(text: &str) -> RunResult {
        RunResult::Exited {
            code: 0,
            stdout: vec![],
            stderr: format!("password: \"{text}\"\n").into_bytes(),
        }
    }

    #[test]
    fn ambiguous_hex_output_is_confirmed_as_binary_via_dash_g() {
        let data = vec![0xde, 0xad, 0xbe, 0xef];
        let s = Scripted::default()
            .then(ok(format!("{}\n", hex::encode(&data)).as_bytes()))
            .then(og_hex("DEADBEEF"));
        assert_eq!(cli(&s, None).find("svc", "acct").present().unwrap(), data);
        let calls = s.calls();
        assert_eq!(
            calls[1].1,
            vec!["find-generic-password", "-a", "acct", "-g", "-s", "svc"]
        );
    }

    #[test]
    fn ambiguous_hex_output_is_confirmed_as_verbatim_via_dash_g() {
        let s = Scripted::default()
            .then(ok(b"cafe\n"))
            .then(og_verbatim("cafe"));
        assert_eq!(
            cli(&s, None).find("svc", "acct").present().unwrap(),
            b"cafe"
        );
    }

    #[test]
    fn a_failed_disambiguation_call_is_unreadable() {
        let s = Scripted::default().then(ok(b"cafe\n")).then(rc(1));
        assert!(matches!(
            cli(&s, None).find("svc", "acct"),
            Read::Unreadable(_)
        ));
    }

    #[test]
    fn a_dash_g_timeout_is_unreadable() {
        let s = Scripted::default()
            .then(ok(b"cafe\n"))
            .then(RunResult::TimedOut);
        assert!(matches!(
            cli(&s, None).find("svc", "acct"),
            Read::Unreadable(_)
        ));
    }

    #[test]
    fn a_dash_g_timeout_and_a_dash_g_that_cannot_run_are_told_apart() {
        // §14 (L349): a timeout and a failure to spawn are different causes.
        let detail = |r: RunResult| {
            let s = Scripted::default().then(ok(b"cafe\n")).then(r);
            match cli(&s, None).find("svc", "acct") {
                Read::Unreadable(e) => e.detail,
                other => panic!("expected Unreadable, got {other:?}"),
            }
        };
        assert_eq!(
            detail(RunResult::TimedOut),
            "the -g disambiguation call did not finish in time"
        );
        assert_eq!(
            detail(RunResult::SpawnFailed(
                "Resource temporarily unavailable (os error 35)".into()
            )),
            "the -g disambiguation call could not run: Resource temporarily unavailable (os error 35)"
        );
    }

    #[test]
    fn a_failed_dash_g_never_echoes_its_stderr() {
        let s = Scripted::default()
            .then(ok(b"cafe\n"))
            .then(RunResult::Exited {
                code: 1,
                stdout: vec![],
                stderr: b"password: \"sk-ant-secret\"\n".to_vec(),
            });
        let err = match cli(&s, None).find("svc", "acct") {
            Read::Unreadable(e) => e,
            other => panic!("expected Unreadable, got {other:?}"),
        };
        assert!(!err.detail.contains("sk-ant-secret"), "{}", err.detail);
    }

    #[test]
    fn a_combined_hex_and_quoted_dash_g_line_uses_only_the_hex_part() {
        let data = vec![0xde, 0xad, 0xbe, 0xef];
        let s = Scripted::default()
            .then(ok(format!("{}\n", hex::encode(&data)).as_bytes()))
            .then(RunResult::Exited {
                code: 0,
                stdout: vec![],
                stderr: b"password: 0xDEADBEEF  \"\\336\\255\\276\\357\"\n".to_vec(),
            });
        assert_eq!(cli(&s, None).find("svc", "acct").present().unwrap(), data);
    }

    #[test]
    fn a_dash_g_response_with_no_password_line_is_unreadable() {
        let s = Scripted::default()
            .then(ok(b"cafe\n"))
            .then(RunResult::Exited {
                code: 0,
                stdout: b"attributes only, no password line".to_vec(),
                stderr: vec![],
            });
        assert!(matches!(
            cli(&s, None).find("svc", "acct"),
            Read::Unreadable(_)
        ));
    }

    #[test]
    fn an_empty_or_odd_dash_g_hex_value_is_unreadable() {
        let empty = Scripted::default()
            .then(ok(b"cafe\n"))
            .then(RunResult::Exited {
                code: 0,
                stdout: vec![],
                stderr: b"password: 0x\n".to_vec(),
            });
        assert!(matches!(
            cli(&empty, None).find("svc", "acct"),
            Read::Unreadable(_)
        ));

        let odd = Scripted::default()
            .then(ok(b"cafe\n"))
            .then(RunResult::Exited {
                code: 0,
                stdout: vec![],
                stderr: b"password: 0xABC\n".to_vec(),
            });
        assert!(matches!(
            cli(&odd, None).find("svc", "acct"),
            Read::Unreadable(_)
        ));
    }

    #[test]
    fn a_disagreeing_dash_g_response_is_unreadable() {
        let s = Scripted::default()
            .then(ok(b"cafe\n"))
            .then(og_hex("DEADBEEF"));
        assert!(matches!(
            cli(&s, None).find("svc", "acct"),
            Read::Unreadable(_)
        ));
    }

    #[test]
    fn find_maps_return_codes() {
        let s = Scripted::default()
            .then(rc(44))
            .then(rc(36))
            .then(RunResult::TimedOut)
            .then(RunResult::SpawnFailed("x".into()))
            .then(rc(1));
        let k = cli(&s, None);
        assert!(matches!(k.find("s", "a"), Read::Absent));
        assert!(matches!(k.find("s", "a"), Read::Unreadable(e) if e.detail.contains("rc 36")));
        assert!(
            matches!(k.find("s", "a"), Read::Unreadable(e) if e.detail.contains("did not finish in time") && !e.detail.contains("gave up") && !e.detail.contains(" 5 s"))
        );
        assert!(matches!(k.find("s", "a"), Read::Unreadable(_)));
        assert!(matches!(k.find("s", "a"), Read::Unreadable(e) if e.detail.contains("rc 1")));
    }

    #[test]
    fn exists_never_asks_for_the_secret() {
        let s = Scripted::default().then(ok(b"attributes"));
        assert!(cli(&s, None).exists("svc", "acct").is_present());
        assert!(!s.calls()[0].1.contains(&"-w".to_string()));
    }

    #[test]
    fn a_test_keychain_file_is_the_last_argument() {
        let s = Scripted::default().then(rc(44));
        let _ = cli(&s, Some("/tmp/t.keychain")).find("svc", "acct");
        assert_eq!(s.calls()[0].1.last().unwrap(), "/tmp/t.keychain");
    }

    #[test]
    fn small_writes_go_through_interactive_mode_and_are_verified() {
        // Genuinely non-printable data: `security -w` renders this as hex, which is why
        // the read-back needs the extra `-g` disambiguation call.
        let data = [0xde, 0xad, 0xbe, 0xef];
        let s = Scripted::default()
            .then(ok(b""))
            .then(ok(b"deadbeef\n"))
            .then(og_hex("DEADBEEF"));
        cli(&s, None).upsert("svc", "acct", &data).unwrap();
        let calls = s.calls();
        assert_eq!(calls[0].1, vec!["-i".to_string()]);
        assert_eq!(
            calls[0].2.as_deref().unwrap(),
            b"add-generic-password -U -a \"acct\" -s \"svc\" -X \"deadbeef\"\n"
        );
        assert_eq!(calls[1].1[0], "find-generic-password");
        assert!(calls[2].1.contains(&"-g".to_string()));
    }

    #[test]
    fn long_writes_fall_back_to_argv() {
        let data = vec![b'x'; 2100]; // 4200 hex digits: over the 4032-byte line limit
        let s = Scripted::default().then(ok(b"")).then(ok(&data));
        cli(&s, None).upsert("svc", "acct", &data).unwrap();
        let (_, args, stdin) = &s.calls()[0];
        assert!(stdin.is_none());
        assert_eq!(
            &args[..7],
            [
                "add-generic-password",
                "-U",
                "-a",
                "acct",
                "-s",
                "svc",
                "-X"
            ]
        );
        assert_eq!(args[7], hex::encode(&data));
    }

    #[test]
    fn a_write_that_does_not_read_back_fails() {
        let s = Scripted::default()
            .then(ok(b""))
            .then(ok(b"something else"));
        assert!(cli(&s, None).upsert("svc", "acct", b"{}").is_err());
    }

    #[test]
    fn delete_treats_absent_as_success() {
        let s = Scripted::default().then(rc(44)).then(rc(36));
        let k = cli(&s, None);
        assert!(k.delete("svc", "acct").is_ok());
        assert_eq!(k.delete("svc", "acct").unwrap_err().rc, Some(36));
    }

    #[test]
    fn run_results_never_show_their_output() {
        let secret = b"sk-ant-ort01-SENTINEL".to_vec();
        let shown = format!("{:?}", ok(&secret));
        assert!(!shown.contains("SENTINEL"), "{shown}");
        assert!(!shown.contains(&format!("{secret:?}")), "{shown}");
    }

    #[test]
    fn quotes_in_names_are_refused_before_spawning() {
        let s = Scripted::default();
        assert!(cli(&s, None).upsert("s\"vc", "acct", b"{}").is_err());
        assert!(s.calls().is_empty());
    }

    #[test]
    fn control_characters_in_names_are_refused_before_spawning() {
        let s = Scripted::default();
        let err = cli(&s, None).upsert("svc\n", "acct", b"{}").unwrap_err();
        assert!(err.detail.contains("\\n"), "{}", err.detail); // names the offending character
        assert!(s.calls().is_empty());
    }

    #[test]
    fn the_lock_check_maps_show_keychain_info() {
        let s = Scripted::default()
            .then(ok(b""))
            .then(rc(36))
            .then(rc(128))
            .then(RunResult::TimedOut);
        let k = cli(&s, None);
        assert_eq!(k.lock_state(), LockState::Unlocked);
        assert_eq!(k.lock_state(), LockState::Locked);
        assert_eq!(k.lock_state(), LockState::Unknown); // any other rc: unknown
        assert_eq!(k.lock_state(), LockState::Unknown);
        assert_eq!(
            s.calls()[0],
            (
                "/usr/bin/security".to_string(),
                vec!["show-keychain-info".to_string()],
                None
            )
        );
    }

    #[test]
    fn unlock_attaches_the_terminal_and_never_passes_a_password() {
        let s = Scripted::default().then(ok(b"")).then(rc(1));
        let k = cli(&s, None);
        assert!(k.unlock());
        assert!(!k.unlock());
        assert_eq!(
            s.calls()[0],
            (
                "attached:/usr/bin/security".to_string(),
                vec!["unlock-keychain".to_string()],
                None
            )
        );
        let s = Scripted::default().then(ok(b""));
        assert!(cli(&s, Some("/tmp/t.keychain")).unlock());
        assert_eq!(
            s.calls()[0].1,
            vec!["unlock-keychain".to_string(), "/tmp/t.keychain".to_string()]
        );
    }

    /// A child whose `try_wait` answers are scripted, counting the kills and reaps.
    struct FakeChild {
        polls: VecDeque<io::Result<Option<ExitStatus>>>,
        kills: usize,
        waits: usize,
        kill_fails: bool,
    }

    impl FakeChild {
        fn new(polls: Vec<io::Result<Option<ExitStatus>>>) -> Self {
            Self {
                polls: polls.into(),
                kills: 0,
                waits: 0,
                kill_fails: false,
            }
        }

        fn unkillable(mut self) -> Self {
            self.kill_fails = true;
            self
        }
    }

    impl Waitable for FakeChild {
        fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
            self.polls.pop_front().expect("unexpected extra poll")
        }
        fn kill(&mut self) -> io::Result<()> {
            self.kills += 1;
            if self.kill_fails {
                return Err(io::Error::other("not permitted"));
            }
            Ok(())
        }
        fn wait(&mut self) -> io::Result<ExitStatus> {
            self.waits += 1;
            Ok(ExitStatus::from_raw(0))
        }
    }

    #[test]
    fn a_failed_try_wait_kills_and_reaps_the_child() {
        // L343: the child used to be left running, and never reaped.
        let mut child = FakeChild::new(vec![Err(io::Error::other("boom"))]);
        let waited = wait_for(&mut child, Instant::now() + Duration::from_secs(60));
        assert!(
            matches!(&waited, Waited::Failed(m) if m.contains("boom")),
            "{waited:?}"
        );
        assert_eq!((child.kills, child.waits), (1, 1));
    }

    #[test]
    fn a_child_past_its_deadline_is_killed_and_reaped() {
        let mut child = FakeChild::new(vec![Ok(None)]);
        assert_eq!(wait_for(&mut child, Instant::now()), Waited::TimedOut);
        assert_eq!((child.kills, child.waits), (1, 1));
    }

    #[test]
    fn a_child_that_cannot_be_killed_is_not_waited_for() {
        // `wait` on a child that is still running would block for as long as it runs.
        let mut past = FakeChild::new(vec![Ok(None)]).unkillable();
        assert_eq!(wait_for(&mut past, Instant::now()), Waited::TimedOut);
        assert_eq!((past.kills, past.waits), (1, 0));

        let mut failed = FakeChild::new(vec![Err(io::Error::other("boom"))]).unkillable();
        let waited = wait_for(&mut failed, Instant::now() + Duration::from_secs(60));
        assert!(
            matches!(&waited, Waited::Failed(m) if m.contains("boom")),
            "{waited:?}"
        );
        assert_eq!((failed.kills, failed.waits), (1, 0));
    }

    #[test]
    fn a_child_that_exits_in_time_is_left_alone() {
        let mut child = FakeChild::new(vec![Ok(None), Ok(Some(ExitStatus::from_raw(3 << 8)))]);
        let waited = wait_for(&mut child, Instant::now() + Duration::from_secs(60));
        assert_eq!(waited, Waited::Exited(3));
        assert_eq!((child.kills, child.waits), (0, 0));
    }

    #[test]
    fn a_real_process_reports_its_code_and_both_streams() {
        let r = ProcessRunner::run_bounded(
            "/bin/sh",
            &s(&["-c", "printf out; printf err >&2; exit 3"]),
            None,
            Duration::from_secs(5),
            Duration::from_secs(1),
        );
        match r {
            RunResult::Exited {
                code,
                stdout,
                stderr,
            } => {
                assert_eq!(code, 3);
                assert_eq!(stdout, b"out");
                assert_eq!(stderr, b"err");
            }
            other => panic!("expected Exited, got {other:?}"),
        }
    }

    #[test]
    fn a_grandchild_holding_the_pipes_cannot_hang_run() {
        // L343: `sleep` inherits the pipes and outlives the shell, so the readers never see
        // EOF. `run` used to join them and wait the full 8 s.
        let started = Instant::now();
        let r = ProcessRunner::run_bounded(
            "/bin/sh",
            &s(&["-c", "sleep 8 & echo done"]),
            None,
            Duration::from_secs(5),
            Duration::from_millis(200),
        );
        assert!(matches!(r, RunResult::TimedOut), "{r:?}");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
    }

    /// The pid of the shell `run` starts, and that pid's process group as the kernel reports it
    /// from outside while the shell is still running: the shell writes its pid to a file, then
    /// waits for a file the test creates once it has the group (capped at 10 s, so a test that
    /// fails first leaves no shell behind).
    fn pid_and_group(
        run: impl FnOnce(Vec<String>) -> RunResult + Send + 'static,
    ) -> (libc::pid_t, libc::pid_t) {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let go_file = dir.path().join("go");
        let script = format!(
            "echo $$ > '{}'; i=0; while [ ! -e '{}' ] && [ $i -lt 200 ]; do sleep 0.05; i=$((i+1)); done",
            pid_file.display(),
            go_file.display()
        );
        let shell = thread::spawn(move || run(s(&["-c", &script])));
        let deadline = Instant::now() + Duration::from_secs(5);
        let pid: libc::pid_t = loop {
            let written = fs::read_to_string(&pid_file).ok();
            if let Some(pid) = written.and_then(|t| t.trim().parse().ok()) {
                break pid;
            }
            assert!(Instant::now() < deadline, "the shell never wrote its pid");
            thread::sleep(Duration::from_millis(10));
        };
        // SAFETY: getpgid(2) only reads the process table, and `pid` is the shell, which is
        // still waiting for the go file and not yet reaped, so the pid names it and no other process.
        let group = unsafe { libc::getpgid(pid) };
        assert!(group > 0, "getpgid: {}", io::Error::last_os_error());
        fs::write(&go_file, b"").unwrap();
        let ran = shell.join().unwrap();
        assert!(matches!(ran, RunResult::Exited { code: 0, .. }), "{ran:?}");
        (pid, group)
    }

    /// This test process's group: the one a terminal's Ctrl-C would reach.
    fn own_group() -> libc::pid_t {
        // SAFETY: getpgrp(2) takes no arguments and cannot fail.
        unsafe { libc::getpgrp() }
    }

    #[test]
    fn a_bounded_child_leads_a_process_group_of_its_own() {
        // §14.1: a terminal's Ctrl-C reaches its foreground process group. A `security` write
        // in that group would die midway; in a group of its own, it never sees the signal.
        let _fork = crate::FORK_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (pid, group) = pid_and_group(|args| {
            ProcessRunner::run_bounded(
                "/bin/sh",
                &args,
                None,
                Duration::from_secs(5),
                Duration::from_secs(1),
            )
        });
        assert_eq!(group, pid, "the child leads its own group");
        assert_ne!(group, own_group(), "never the caller's group");
    }

    #[test]
    fn an_attached_child_stays_in_the_caller_s_process_group() {
        // §14.1: `security unlock-keychain` reads the password from the terminal, so it stays
        // in the terminal's foreground group, where a Ctrl-C reaches it along with tagteam.
        let _fork = crate::FORK_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (pid, group) = pid_and_group(|args| ProcessRunner.run_attached("/bin/sh", &args));
        assert_ne!(group, pid);
        assert_eq!(group, own_group());
    }
}
